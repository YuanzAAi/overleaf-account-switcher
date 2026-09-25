use super::*;
use crate::projects::migration::reqwest_project_client;
use base64::Engine;
use overleaf_api::project_api::{ProjectApiClient, ProjectApiError, ReqwestProjectApiTransport};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    List,
    Copy,
    Rename,
    Archive,
    Unarchive,
    Trash,
    Restore,
    Delete,
    Zip,
    Pdf,
}

#[derive(Deserialize)]
struct Request {
    alias: Option<String>,
    action: Action,
    project_id: Option<String>,
    name: Option<String>,
    confirm_name: Option<String>,
    task_id: String,
}

fn error(status: u16, message: impl Into<String>) -> ApiResponse {
    json_response(
        status,
        &ApiErrorBody {
            error: message.into(),
        },
    )
}

pub(crate) fn handle(shared: &Arc<StdMutex<ApiState>>, method: &str, body: &str) -> ApiResponse {
    if method != "POST" {
        return error(405, "不支持的请求方式");
    }
    let request: Request = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.task_id.trim().is_empty() || request.task_id.len() > 200 {
        return error(400, "任务标识无效");
    }
    if !matches!(request.action, Action::List)
        && request
            .project_id
            .as_deref()
            .is_none_or(|id| id.len() != 24 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return error(400, "项目标识无效");
    }
    let (document, alias, backend, validator) = {
        let Ok(mut state) = shared.lock() else {
            return error(500, "服务状态暂不可用");
        };
        let document = match AccountStore::new(state.config.accounts_path()).load() {
            Ok(document) => document,
            Err(_) => return error(500, "无法读取账号"),
        };
        let Some(alias) = request
            .alias
            .as_deref()
            .or(document.current.as_deref())
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_owned)
        else {
            return error(409, "请先选择当前账号");
        };
        if !document.accounts.contains_key(&alias) {
            return error(404, "账号不存在");
        }
        let migration_active = state.tasks.list_snapshots().iter().any(|task| {
            task.operation_kind == Some(TaskOperationKind::AccountSwitchExecute)
                && !matches!(
                    task.phase,
                    ServiceTaskPhase::Completed
                        | ServiceTaskPhase::Failed
                        | ServiceTaskPhase::Cancelled
                )
                && task.retry_descriptor.as_ref().is_some_and(|retry| {
                    retry.payload.migrate_projects == Some(true)
                        && retry
                            .payload
                            .source_alias
                            .as_deref()
                            .or(document.current.as_deref())
                            == Some(alias.as_str())
                })
        });
        if migration_active {
            return error(409, "该账号正在迁移项目，请等待迁移结束");
        }
        if let Err(response) = start_alias_batch_task(
            &mut state,
            Some(&request.task_id),
            "管理账号项目",
            std::slice::from_ref(&alias),
        ) {
            return response;
        }
        (
            document,
            alias,
            state.secret_backend.clone(),
            state.session_identity_validator.clone(),
        )
    };
    // 网络操作不持有全局状态锁，不同账号的窗口可独立工作。
    let result = block_on_api(async {
        let record = &document.accounts[&alias];
        let cookies =
            crate::account_secrets::resolve_account_cookies(record, &alias, backend.as_ref())
                .map_err(|cause| match cause {
                    crate::account_secrets::AccountSecretStoreError::Missing { .. } => {
                        error(401, "账号需要重新登录")
                    }
                    _ => error(500, "无法读取账号 Cookie"),
                })?;
        validator
            .validate(&alias, record.email.as_deref(), &cookies)
            .await
            .map_err(|cause| {
                error(
                    if cause.requires_cookie_recovery() {
                        401
                    } else {
                        409
                    },
                    account_session_error_message(&cause),
                )
            })?;
        let client = reqwest_project_client(&document, &alias, backend.as_ref())
            .await
            .map_err(project_migration_error_response)?;
        operate(&client, &request, &alias, shared).await
    });
    let response = match result {
        Ok(result) => result,
        Err(response) => Err(response),
    };
    if let Ok(mut state) = shared.lock() {
        match &response {
            Ok(_) => complete_tracked_task(
                &mut state,
                Some(&request.task_id),
                "项目操作完成",
                &json!({"alias":alias, "completed":true}),
            ),
            Err(response) => {
                if response.status_code == 409
                    && state
                        .tasks
                        .snapshot(&request.task_id)
                        .is_ok_and(|task| task.cancel_requested)
                {
                    let _ = state.tasks.cancel_task(&request.task_id, "项目操作已取消");
                    return response.clone();
                }
                let message = serde_json::from_str::<serde_json::Value>(&response.body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "项目操作失败".into());
                fail_tracked_task(&mut state, Some(&request.task_id), message);
            }
        }
    }
    response.unwrap_or_else(|response| response)
}

fn api_error(cause: ProjectApiError) -> ApiResponse {
    match &cause {
        ProjectApiError::Http(http) => error(http.status, cause.to_string()),
        _ => error(502, cause.to_string()),
    }
}

async fn operate(
    client: &ProjectApiClient<ReqwestProjectApiTransport>,
    request: &Request,
    alias: &str,
    shared: &Arc<StdMutex<ApiState>>,
) -> Result<ApiResponse, ApiResponse> {
    let projects = client.list_projects().await.map_err(api_error)?;
    if browser_task_cancel_requested(shared, &request.task_id) {
        return Err(error(409, "项目操作已取消"));
    }
    if matches!(request.action, Action::List) {
        return Ok(json_response(
            200,
            &json!({"alias":alias, "projects":projects.iter().map(|p| json!({
            "id":p.id, "name":p.name, "owner":p.owner_email.as_ref().or(p.owner_name.as_ref()),
            "is_owner":p.is_owner(), "last_updated":p.last_updated,
            "archived":p.archived, "trashed":p.trashed,
        })).collect::<Vec<_>>()}),
        ));
    }
    let id = request.project_id.as_deref().unwrap_or_default();
    let project = projects
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| error(404, "项目已不在账号中，请刷新列表"))?;
    if matches!(request.action, Action::Rename | Action::Delete) && !project.is_owner() {
        return Err(error(403, "只有项目所有者可以执行此操作"));
    }
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let mut result = json!({"completed":true});
    match request.action {
        Action::List => unreachable!(),
        Action::Copy => {
            result["project_id"] = client
                .clone_project(id, name)
                .await
                .map_err(api_error)?
                .into();
        }
        Action::Rename => client
            .rename_project(id, name.ok_or_else(|| error(400, "项目名不能为空"))?)
            .await
            .map_err(api_error)?,
        Action::Archive | Action::Unarchive => client
            .set_archived(id, matches!(request.action, Action::Archive))
            .await
            .map_err(api_error)?,
        Action::Trash | Action::Restore => client
            .set_trashed(id, matches!(request.action, Action::Trash))
            .await
            .map_err(api_error)?,
        Action::Delete => {
            if !project.trashed || request.confirm_name.as_deref() != Some(&project.name) {
                return Err(error(409, "请在回收站确认项目名称后删除"));
            }
            client.delete_project(id).await.map_err(api_error)?;
        }
        Action::Zip | Action::Pdf => {
            let pdf = matches!(request.action, Action::Pdf);
            let path = if pdf {
                let compiled = client.compile_project(id).await.map_err(api_error)?;
                let url = compiled["outputFiles"]
                    .as_array()
                    .and_then(|files| files.iter().find(|file| file["path"] == "output.pdf"))
                    .and_then(|file| file["url"].as_str())
                    .ok_or_else(|| error(409, "编译未生成 PDF，请检查项目源码"))?;
                let mut url = reqwest::Url::parse("https://www.overleaf.com")
                    .unwrap()
                    .join(url)
                    .map_err(|_| error(502, "编译返回的下载地址无效"))?;
                if !url.query_pairs().any(|(key, _)| key == "clsiserverid") {
                    if let Some(server) = compiled["clsiServerId"].as_str() {
                        url.query_pairs_mut().append_pair("clsiserverid", server);
                    }
                }
                url.to_string()
            } else {
                format!("/project/{id}/download/zip")
            };
            let bytes = client
                .clone()
                .into_inner()
                .download(&path)
                .await
                .map_err(api_error)?;
            if (pdf && !bytes.starts_with(b"%PDF-")) || (!pdf && !bytes.starts_with(b"PK")) {
                return Err(error(502, "下载内容不是有效的项目文件"));
            }
            result = json!({"filename":format!("{}.{}",project.name,if pdf {"pdf"} else {"zip"}),
                "content_type":if pdf {"application/pdf"} else {"application/zip"},
                "data":base64::engine::general_purpose::STANDARD.encode(bytes)});
        }
    }
    Ok(json_response(200, &result))
}
