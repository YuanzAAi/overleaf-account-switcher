use std::collections::BTreeMap;
use std::fmt;

use overleaf_core::{Project, ProjectAccessLevel, ProjectSource};
use serde_json::{json, Value};

use crate::{body_preview, format_cookie_header, header_map_from_strings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectApiRequest {
    pub method: HttpMethod,
    pub path: String,
    pub json_body: Option<Value>,
    pub requires_csrf: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectApiStatusKind {
    Success,
    RateLimited,
    Unauthorized,
    Forbidden,
    NotFound,
    ClientError,
    ServerError,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTokenSet {
    pub read_only: Option<String>,
    pub read_and_write: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub email: String,
    pub privileges: String,
    pub member_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectApiHttpResponse {
    pub status: u16,
    pub body: String,
}

impl ProjectApiHttpResponse {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectApiHttpError {
    pub status: u16,
    pub kind: ProjectApiStatusKind,
    pub body_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectApiTransportError {
    pub kind: ProjectApiTransportErrorKind,
    message: String,
}

impl ProjectApiTransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: ProjectApiTransportErrorKind::RequestFailed,
            message: message.into(),
        }
    }

    pub fn missing_csrf_token() -> Self {
        Self {
            kind: ProjectApiTransportErrorKind::MissingCsrfToken,
            message: "missing csrf token".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectApiTransportErrorKind {
    MissingCsrfToken,
    RequestFailed,
    InvalidHeader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectApiParseError {
    message: String,
}

impl ProjectApiParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProjectApiParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectApiParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectApiError {
    Http(ProjectApiHttpError),
    Transport(ProjectApiTransportError),
    Parse(ProjectApiParseError),
    MissingProjectId { operation: &'static str },
    MissingTokenSet,
}

impl fmt::Display for ProjectApiHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "project api returned HTTP {}", self.status)
    }
}

impl std::error::Error for ProjectApiHttpError {}

impl fmt::Display for ProjectApiTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectApiTransportError {}

impl fmt::Display for ProjectApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "{err}"),
            Self::Transport(err) => write!(f, "{err}"),
            Self::Parse(err) => write!(f, "{err}"),
            Self::MissingProjectId { operation } => {
                write!(f, "missing project id in {operation} response")
            }
            Self::MissingTokenSet => f.write_str("missing project sharing tokens"),
        }
    }
}

impl std::error::Error for ProjectApiError {}

impl From<ProjectApiHttpError> for ProjectApiError {
    fn from(value: ProjectApiHttpError) -> Self {
        Self::Http(value)
    }
}

impl From<ProjectApiTransportError> for ProjectApiError {
    fn from(value: ProjectApiTransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<ProjectApiParseError> for ProjectApiError {
    fn from(value: ProjectApiParseError) -> Self {
        Self::Parse(value)
    }
}

pub type ProjectApiResult<T> = Result<T, ProjectApiError>;

#[async_trait::async_trait]
pub trait ProjectApiTransport {
    async fn send(
        &self,
        request: &ProjectApiRequest,
    ) -> Result<ProjectApiHttpResponse, ProjectApiTransportError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedProjectApiRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub json_body: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ReqwestProjectApiTransport {
    client: reqwest::Client,
    cookies: BTreeMap<String, String>,
    csrf_token: Option<String>,
}

impl ReqwestProjectApiTransport {
    pub fn new(cookies: BTreeMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            cookies,
            csrf_token: None,
        }
    }

    pub fn with_csrf_token(mut self, csrf_token: impl Into<String>) -> Self {
        let token = csrf_token.into();
        self.csrf_token = (!token.trim().is_empty()).then_some(token);
        self
    }

    pub fn prepare_request(
        &self,
        request: &ProjectApiRequest,
    ) -> ProjectApiResult<PreparedProjectApiRequest> {
        let mut headers = BTreeMap::from([
            (
                "accept".to_string(),
                "application/json, text/plain, */*".to_string(),
            ),
            (
                "user-agent".to_string(),
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string(),
            ),
        ]);

        if !self.cookies.is_empty() {
            headers.insert("cookie".to_string(), format_cookie_header(&self.cookies));
        }

        if request.requires_csrf {
            let csrf_token = self
                .csrf_token
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ProjectApiError::Transport(ProjectApiTransportError::missing_csrf_token())
                })?;
            headers.insert("x-csrf-token".to_string(), csrf_token.to_string());
        }

        // 无请求体的写操作也要声明长度，否则 /leave 等端点会返回 411。
        if request.method != HttpMethod::Get && request.json_body.is_none() {
            headers.insert("content-length".to_string(), "0".to_string());
        }

        Ok(PreparedProjectApiRequest {
            method: request.method,
            url: format!("https://www.overleaf.com{}", request.path),
            headers,
            json_body: request.json_body.clone(),
        })
    }
}

#[async_trait::async_trait]
impl ProjectApiTransport for ReqwestProjectApiTransport {
    async fn send(
        &self,
        request: &ProjectApiRequest,
    ) -> Result<ProjectApiHttpResponse, ProjectApiTransportError> {
        let prepared = self.prepare_request(request).map_err(|err| match err {
            ProjectApiError::Transport(err) => err,
            other => ProjectApiTransportError::new(other.to_string()),
        })?;

        let method = match prepared.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };

        let mut builder = self.client.request(method, &prepared.url);
        let headers = header_map_from_strings(&prepared.headers).map_err(|message| {
            ProjectApiTransportError {
                kind: ProjectApiTransportErrorKind::InvalidHeader,
                message,
            }
        })?;
        builder = builder.headers(headers);
        if let Some(json_body) = prepared.json_body {
            builder = builder.json(&json_body);
        } else if prepared.method != HttpMethod::Get {
            builder = builder.body(Vec::new());
        }

        let response = builder
            .send()
            .await
            .map_err(|err| ProjectApiTransportError::new(err.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| ProjectApiTransportError::new(err.to_string()))?;

        Ok(ProjectApiHttpResponse::new(status, body))
    }
}

#[derive(Debug, Clone)]
pub struct ProjectApiClient<T> {
    transport: T,
}

impl<T> ProjectApiClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }
}

impl<T: ProjectApiTransport> ProjectApiClient<T> {
    pub async fn list_projects(&self) -> ProjectApiResult<Vec<Project>> {
        let response = self.send(projects_request()).await?;
        parse_projects_response(response.status, &response.body)
    }

    pub async fn enable_link_sharing(&self, project_id: &str) -> ProjectApiResult<ProjectTokenSet> {
        let response = self.send(enable_link_sharing_request(project_id)).await?;
        ensure_project_api_success(response.status, &response.body)?;
        self.get_project_tokens(project_id).await
    }

    pub async fn get_project_tokens(&self, project_id: &str) -> ProjectApiResult<ProjectTokenSet> {
        let response = self.send(project_tokens_request(project_id)).await?;
        parse_project_tokens_response(response.status, &response.body)
    }

    pub async fn join_project_via_token(
        &self,
        token: &str,
        is_read_only: bool,
    ) -> ProjectApiResult<Option<String>> {
        let token_page = self.send(token_page_request(token, is_read_only)).await?;
        let fallback_project_id =
            parse_token_page_project_id_response(token_page.status, &token_page.body)?;

        let grant = self.send(token_grant_request(token, is_read_only)).await?;
        parse_join_grant_response(grant.status, &grant.body, fallback_project_id.as_deref())
    }

    pub async fn clone_project(
        &self,
        project_id: &str,
        new_name: Option<&str>,
    ) -> ProjectApiResult<String> {
        let response = self
            .send(clone_project_request(project_id, new_name))
            .await?;
        parse_clone_project_response(response.status, &response.body)?
            .ok_or(ProjectApiError::MissingProjectId { operation: "clone" })
    }

    pub async fn delete_project(&self, project_id: &str) -> ProjectApiResult<()> {
        let response = self.send(delete_project_request(project_id)).await?;
        Ok(ensure_project_api_success(response.status, &response.body)?)
    }

    pub async fn leave_project(&self, project_id: &str) -> ProjectApiResult<()> {
        let response = self.send(leave_project_request(project_id)).await?;
        Ok(ensure_project_api_success(response.status, &response.body)?)
    }

    pub async fn restore_project(&self, project_id: &str) -> ProjectApiResult<()> {
        let response = self.send(restore_project_request(project_id)).await?;
        Ok(ensure_project_api_success(response.status, &response.body)?)
    }

    pub async fn invite_collaborator(
        &self,
        project_id: &str,
        email: &str,
        privileges: &str,
    ) -> ProjectApiResult<()> {
        let response = self
            .send(invite_collaborator_request(project_id, email, privileges))
            .await?;
        Ok(ensure_project_api_success(response.status, &response.body)?)
    }

    pub async fn get_project_members(
        &self,
        project_id: &str,
    ) -> ProjectApiResult<Vec<ProjectMember>> {
        let response = self.send(project_members_request(project_id)).await?;
        parse_project_members_response(response.status, &response.body)
    }

    async fn send(&self, request: ProjectApiRequest) -> ProjectApiResult<ProjectApiHttpResponse> {
        Ok(self.transport.send(&request).await?)
    }
}

pub fn projects_request() -> ProjectApiRequest {
    request(HttpMethod::Get, "/project", None, false)
}

pub fn enable_link_sharing_request(project_id: &str) -> ProjectApiRequest {
    project_admin_settings_request(project_id, "tokenBased")
}

pub fn disable_link_sharing_request(project_id: &str) -> ProjectApiRequest {
    project_admin_settings_request(project_id, "private")
}

pub fn project_tokens_request(project_id: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Get,
        format!("/project/{}/tokens", path_part(project_id)),
        None,
        false,
    )
}

pub fn token_page_request(token: &str, is_read_only: bool) -> ProjectApiRequest {
    let token = path_part(token);
    let path = if is_read_only {
        format!("/read/{token}")
    } else {
        format!("/{token}")
    };
    request(HttpMethod::Get, path, None, false)
}

pub fn token_grant_request(token: &str, is_read_only: bool) -> ProjectApiRequest {
    let token = path_part(token);
    let path = if is_read_only {
        format!("/read/{token}/grant")
    } else {
        format!("/{token}/grant")
    };
    request(
        HttpMethod::Post,
        path,
        Some(json!({"confirmedByUser": true})),
        true,
    )
}

pub fn clone_project_request(project_id: &str, new_name: Option<&str>) -> ProjectApiRequest {
    let mut body = json!({});
    if let Some(name) = non_empty(new_name) {
        body["projectName"] = Value::String(name.to_string());
    }

    request(
        HttpMethod::Post,
        format!("/Project/{}/clone", path_part(project_id)),
        Some(body),
        true,
    )
}

pub fn delete_project_request(project_id: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Delete,
        format!("/project/{}", path_part(project_id)),
        None,
        true,
    )
}

pub fn rename_project_request(project_id: &str, new_name: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Post,
        format!("/project/{}/rename", path_part(project_id)),
        Some(json!({"newProjectName": new_name})),
        true,
    )
}

pub fn leave_project_request(project_id: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Post,
        format!("/project/{}/leave", path_part(project_id)),
        None,
        true,
    )
}

pub fn restore_project_request(project_id: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Delete,
        format!("/project/{}/trash", path_part(project_id)),
        None,
        true,
    )
}

pub fn invite_collaborator_request(
    project_id: &str,
    email: &str,
    privileges: &str,
) -> ProjectApiRequest {
    request(
        HttpMethod::Post,
        format!("/project/{}/invite", path_part(project_id)),
        Some(json!({
            "email": email,
            "privileges": privileges,
        })),
        true,
    )
}

pub fn project_members_request(project_id: &str) -> ProjectApiRequest {
    request(
        HttpMethod::Get,
        format!("/project/{}/members", path_part(project_id)),
        None,
        false,
    )
}

pub fn classify_project_api_status(status: u16) -> ProjectApiStatusKind {
    match status {
        200..=204 => ProjectApiStatusKind::Success,
        401 => ProjectApiStatusKind::Unauthorized,
        403 => ProjectApiStatusKind::Forbidden,
        404 => ProjectApiStatusKind::NotFound,
        429 => ProjectApiStatusKind::RateLimited,
        400..=499 => ProjectApiStatusKind::ClientError,
        500..=599 => ProjectApiStatusKind::ServerError,
        _ => ProjectApiStatusKind::Unknown,
    }
}

pub fn is_project_api_success(status: u16) -> bool {
    classify_project_api_status(status) == ProjectApiStatusKind::Success
}

pub fn ensure_project_api_success(status: u16, body: &str) -> Result<(), ProjectApiHttpError> {
    let kind = classify_project_api_status(status);
    if kind == ProjectApiStatusKind::Success {
        Ok(())
    } else {
        Err(ProjectApiHttpError {
            status,
            kind,
            body_preview: body_preview(body),
        })
    }
}

pub fn parse_projects_response(status: u16, body: &str) -> ProjectApiResult<Vec<Project>> {
    ensure_project_api_success(status, body)?;
    let projects = parse_prefetched_projects(body)?;
    if !projects.is_empty() {
        return Ok(projects);
    }

    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(parse_projects_json(body)?);
    }

    Ok(projects)
}

pub fn parse_prefetched_projects(page_html: &str) -> Result<Vec<Project>, ProjectApiParseError> {
    let Some(raw) = crate::extract_meta_content(page_html, "ol-prefetchedProjectsBlob") else {
        return Ok(Vec::new());
    };
    parse_projects_json(&raw)
}

pub fn parse_projects_json(raw_json: &str) -> Result<Vec<Project>, ProjectApiParseError> {
    let value = parse_json(raw_json)?;
    let projects = value
        .get("projects")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| ProjectApiParseError::new("project list json must be an array or object"))?;

    let mut parsed = Vec::with_capacity(projects.len());
    for value in projects {
        if let Some(project) = parse_project_value(value) {
            parsed.push(project);
        }
    }

    Ok(parsed)
}

pub fn parse_project_tokens_json(
    raw_json: &str,
) -> Result<Option<ProjectTokenSet>, ProjectApiParseError> {
    let value = parse_json(raw_json)?;
    let tokens = ProjectTokenSet {
        read_only: string_field(&value, "readOnly"),
        read_and_write: string_field(&value, "readAndWrite"),
    };

    if tokens.read_only.is_some() || tokens.read_and_write.is_some() {
        Ok(Some(tokens))
    } else {
        Ok(None)
    }
}

pub fn parse_project_tokens_response(status: u16, body: &str) -> ProjectApiResult<ProjectTokenSet> {
    ensure_project_api_success(status, body)?;
    parse_project_tokens_json(body)?.ok_or(ProjectApiError::MissingTokenSet)
}

pub fn parse_project_id_from_token_page(
    page_html: &str,
) -> Result<Option<String>, ProjectApiParseError> {
    let Some(raw) = crate::extract_meta_content(page_html, "ol-project") else {
        return Ok(None);
    };
    let value = parse_json(&raw)?;
    Ok(string_field(&value, "_id").or_else(|| string_field(&value, "id")))
}

pub fn parse_token_page_project_id_response(
    status: u16,
    body: &str,
) -> ProjectApiResult<Option<String>> {
    ensure_project_api_success(status, body)?;
    Ok(parse_project_id_from_token_page(body)?)
}

pub fn parse_join_grant_project_id(
    raw_json: &str,
    fallback_project_id: Option<&str>,
) -> Result<Option<String>, ProjectApiParseError> {
    let value = parse_json(raw_json)?;
    Ok(string_field(&value, "projectId")
        .or_else(|| string_field(&value, "project_id"))
        .or_else(|| string_field(&value, "_id"))
        .or_else(|| fallback_project_id.map(str::to_string)))
}

pub fn parse_join_grant_response(
    status: u16,
    body: &str,
    fallback_project_id: Option<&str>,
) -> ProjectApiResult<Option<String>> {
    ensure_project_api_success(status, body)?;
    Ok(parse_join_grant_project_id(body, fallback_project_id)?)
}

pub fn parse_clone_project_id(raw_json: &str) -> Result<Option<String>, ProjectApiParseError> {
    let value = parse_json(raw_json)?;
    Ok(string_field(&value, "project_id").or_else(|| string_field(&value, "_id")))
}

pub fn parse_clone_project_response(status: u16, body: &str) -> ProjectApiResult<Option<String>> {
    ensure_project_api_success(status, body)?;
    Ok(parse_clone_project_id(body)?)
}

pub fn parse_project_members_json(
    raw_json: &str,
) -> Result<Vec<ProjectMember>, ProjectApiParseError> {
    let value = parse_json(raw_json)?;
    let Some(members) = value.get("members").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    Ok(members
        .iter()
        .filter_map(parse_member_value)
        .collect::<Vec<_>>())
}

pub fn parse_project_members_response(
    status: u16,
    body: &str,
) -> ProjectApiResult<Vec<ProjectMember>> {
    ensure_project_api_success(status, body)?;
    Ok(parse_project_members_json(body)?)
}

fn project_admin_settings_request(
    project_id: &str,
    public_access_level: &str,
) -> ProjectApiRequest {
    request(
        HttpMethod::Post,
        format!("/project/{}/settings/admin", path_part(project_id)),
        Some(json!({"publicAccessLevel": public_access_level})),
        true,
    )
}

fn request(
    method: HttpMethod,
    path: impl Into<String>,
    json_body: Option<Value>,
    requires_csrf: bool,
) -> ProjectApiRequest {
    ProjectApiRequest {
        method,
        path: path.into(),
        json_body,
        requires_csrf,
    }
}

fn path_part(input: &str) -> String {
    input.trim().trim_matches('/').to_string()
}

fn non_empty(input: Option<&str>) -> Option<&str> {
    input.map(str::trim).filter(|value| !value.is_empty())
}

fn parse_json(raw_json: &str) -> Result<Value, ProjectApiParseError> {
    serde_json::from_str(raw_json)
        .map_err(|err| ProjectApiParseError::new(format!("invalid json: {err}")))
}

fn parse_project_value(value: &Value) -> Option<Project> {
    let id = string_field(value, "id").or_else(|| string_field(value, "_id"))?;
    let name = string_field(value, "name").unwrap_or_else(|| id.clone());

    let mut project = Project::new(id, name);
    project.access_level = match string_field(value, "accessLevel")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "owner" => ProjectAccessLevel::Owner,
        "collaborator" | "readandwrite" | "readonly" | "read-only" => {
            ProjectAccessLevel::Collaborator
        }
        _ => ProjectAccessLevel::Unknown,
    };
    project.source = match string_field(value, "source")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "token" => ProjectSource::Token,
        "direct" => ProjectSource::Direct,
        _ => ProjectSource::Unknown,
    };
    project.owner_email = value
        .get("owner")
        .and_then(|owner| string_field(owner, "email"))
        .or_else(|| string_field(value, "ownerEmail"));
    project.last_updated =
        string_field(value, "lastUpdated").or_else(|| string_field(value, "last_updated"));
    project.trashed = value
        .get("trashed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Some(project)
}

fn parse_member_value(value: &Value) -> Option<ProjectMember> {
    let email = string_field(value, "email")?;
    Some(ProjectMember {
        email,
        privileges: string_field(value, "privileges").unwrap_or_else(|| "readAndWrite".to_string()),
        member_type: string_field(value, "type").unwrap_or_else(|| "member".to_string()),
    })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
