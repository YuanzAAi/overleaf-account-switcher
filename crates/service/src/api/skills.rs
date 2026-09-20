use super::*;
use std::{
    env,
    process::{Command, Stdio},
};

const INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/YuanzAAi/overleaf-skills/main/install.py";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillAgent {
    Codex,
    Claude,
}

impl SkillAgent {
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn home(self) -> io::Result<PathBuf> {
        let variable = match self {
            Self::Codex => "CODEX_HOME",
            Self::Claude => "CLAUDE_CONFIG_DIR",
        };
        if let Some(home) = env::var_os(variable).filter(|value| !value.is_empty()) {
            return user_path(PathBuf::from(home));
        }
        let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到用户目录"))?;
        Ok(PathBuf::from(home).join(format!(".{}", self.name())))
    }
}

fn user_path(path: PathBuf) -> io::Result<PathBuf> {
    let path = if let Ok(tail) = path.strip_prefix("~") {
        let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "未找到用户目录"))?;
        PathBuf::from(home).join(tail)
    } else {
        path
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillInstallation {
    pub agent: SkillAgent,
    pub path: Option<PathBuf>,
    pub installed: bool,
}

pub fn installations() -> Vec<SkillInstallation> {
    [SkillAgent::Codex, SkillAgent::Claude]
        .into_iter()
        .map(|agent| {
            let path = agent
                .home()
                .ok()
                .map(|home| home.join("skills/overleaf-skills"));
            let installed = path.as_ref().is_some_and(|path| {
                path.join("SKILL.md").is_file() && path.join("scripts/overleaf_skill.py").is_file()
            });
            SkillInstallation {
                agent,
                path,
                installed,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SkillAction {
    Install,
    Update,
    Uninstall,
}

#[derive(Deserialize)]
struct ManageRequest {
    agent: SkillAgent,
    action: SkillAction,
    task_id: String,
}

pub(super) fn manage_response(state: &mut ApiState, body: &str) -> ApiResponse {
    let request: ManageRequest = match serde_json::from_str(body) {
        Ok(request) => request,
        Err(_) => {
            return json_response(
                400,
                &ApiErrorBody {
                    error: "skills 操作参数不完整".into(),
                },
            )
        }
    };
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return json_response(
            400,
            &ApiErrorBody {
                error: "缺少任务编号".into(),
            },
        );
    }
    let title = match request.action {
        SkillAction::Install => "安装 Overleaf skills",
        SkillAction::Update => "更新 Overleaf skills",
        SkillAction::Uninstall => "卸载 Overleaf skills",
    };
    if let Err(response) = start_alias_batch_task(
        state,
        Some(&task_id),
        title,
        &[format!("skills:{}", request.agent.name())],
    ) {
        return response;
    }
    let id = task_id.clone();
    let job = ApiBackgroundJob::new("skills-manage", move |shared| {
        let result = run_installer(&shared, &id, request.agent, request.action);
        let cancelled = browser_task_cancel_requested(&shared, &id);
        if let Ok(mut state) = shared.lock() {
            if cancelled {
                let _ = cancel_task_with_registration_release(&mut state, &id, "skills 操作已取消");
                return;
            }
            match result {
                Ok(message) => complete_tracked_task(
                    &mut state,
                    Some(&id),
                    &message,
                    &json!({"agent": request.agent, "status": "completed"}),
                ),
                Err(message) => fail_tracked_task(&mut state, Some(&id), message),
            }
        }
    });
    if let Err(response) = state.enqueue_background_job(job) {
        fail_tracked_task(state, Some(&task_id), "无法启动 skills 操作");
        return response;
    }
    match state.tasks.snapshot(&task_id) {
        Ok(task) => json_response(202, &task),
        Err(error) => task_state_error_response(error),
    }
}

fn run_installer(
    shared: &Arc<StdMutex<ApiState>>,
    task_id: &str,
    agent: SkillAgent,
    action: SkillAction,
) -> Result<String, String> {
    let home = agent.home().map_err(|error| error.to_string())?;
    if matches!(action, SkillAction::Install)
        && home.join("skills/overleaf-skills/SKILL.md").is_file()
    {
        return Ok("skills 已安装".into());
    }
    let local = home.join("skills/overleaf-skills/install.py");
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let installer = if matches!(action, SkillAction::Uninstall) && local.is_file() {
        local
    } else {
        let runtime = build_api_runtime().map_err(|_| "无法启动下载".to_string())?;
        let data = runtime.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|error| error.to_string())?;
            let mut response = client
                .get(INSTALLER_URL)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| error.to_string())?;
            let mut data = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                if data.len() + chunk.len() > 1024 * 1024 {
                    return Err("安装程序超过大小限制".to_string());
                }
                data.extend_from_slice(&chunk);
            }
            Ok::<_, String>(data)
        })?;
        let path = directory.path().join("install.py");
        fs::write(&path, data).map_err(|error| error.to_string())?;
        path
    };
    let mut command = Command::new(if cfg!(windows) { "python" } else { "python3" });
    command.arg(installer).args(["--agent", agent.name()]);
    match action {
        SkillAction::Update => {
            command.arg("--update");
        }
        SkillAction::Uninstall => {
            command.arg("--uninstall");
        }
        SkillAction::Install => {}
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("无法启动 Python，请确认已安装 Python 3.10 或以上版本：{error}")
        })?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(180)
            || browser_task_cancel_requested(shared, task_id)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("skills 操作已取消或超时".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(match action {
        SkillAction::Install => "skills 已就绪",
        SkillAction::Update => "skills 已更新",
        SkillAction::Uninstall => "skills 已卸载，账号状态和缓存已保留",
    }
    .to_string())
}

fn write_state(path: &Path, session: &str, token: &str) -> io::Result<()> {
    // 同一份 JSON 原子替换，不能分别更新 Cookie 与 Git token。
    overleaf_storage::save_json_atomic(path, &json!({ "session": session, "git_token": token }))
}

pub(super) fn finish_switch(
    state: &mut ApiState,
    task_id: Option<&str>,
    report: &crate::AccountSwitchExecutionReport,
) -> serde_json::Value {
    state.runtime.browser_account_email = report.plan.email.clone();
    let mut result = serde_json::to_value(report).unwrap_or_default();
    let synchronize = task_id
        .and_then(|id| state.tasks.snapshot(id).ok())
        .and_then(|task| task.retry_descriptor)
        .and_then(|retry| retry.payload.sync_skills)
        .unwrap_or(false);
    if synchronize {
        let sync = sync_account(state, &report.plan.alias);
        if let Some(id) = task_id {
            for item in &sync["items"].as_array().cloned().unwrap_or_default() {
                let level = if item["success"] == true {
                    TaskLogLevel::Info
                } else {
                    TaskLogLevel::Warning
                };
                let _ = state.tasks.append_log(
                    id,
                    level,
                    item["message"].as_str().unwrap_or("skills 同步未完成"),
                );
            }
        }
        result["skills_sync"] = sync;
    }
    complete_tracked_task(state, task_id, "无感换号执行完成", &result);
    result
}

fn sync_account(state: &ApiState, alias: &str) -> serde_json::Value {
    let result = (|| {
        let document = load_accounts(state).map_err(|_| "无法读取账号库")?;
        let session = account_secret_for_copy_with_backend(
            &document,
            alias,
            AccountSecretField::Cookie,
            state.secret_backend.as_ref(),
        )
        .map_err(|_| "缺少可读取的 Cookie")?;
        let token = account_secret_for_copy_with_backend(
            &document,
            alias,
            AccountSecretField::GitToken,
            state.secret_backend.as_ref(),
        )
        .map_err(|_| "缺少可读取的 Git token")?;
        if session.trim().is_empty() || token.trim().is_empty() {
            return Err("Cookie 或 Git token 为空");
        }
        Ok((session, token))
    })();
    let mut items = Vec::new();
    match result {
        Ok((session, token)) => {
            for install in installations().into_iter().filter(|item| item.installed) {
                let saved = install.agent.home().and_then(|home| {
                    let directory = env::var_os("OVERLEAF_SKILL_STATE_DIR").filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join("overleaf-skills"));
                    let directory = user_path(directory)?;
                    write_state(&directory.join("state.json"), &session, &token)
                });
                let message = match &saved {
                    Ok(()) => format!("{} skills 账号已同步", install.agent.name()),
                    Err(error) => format!("{} skills 账号未同步：{error}", install.agent.name()),
                };
                items.push(json!({"agent": install.agent, "success": saved.is_ok(), "message": message}));
            }
            if items.is_empty() { items.push(json!({"success": false, "message": "未安装 Overleaf skills，账号未同步"})); }
        }
        Err(message) => items.push(json!({"success": false, "message": format!("skills 账号未同步：{message}；原有账号状态已保留")})),
    }
    json!({"failed_count": items.iter().filter(|item| item["success"] != true).count(), "items": items})
}
