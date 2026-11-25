use reqwest::{Client, StatusCode};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const REPOSITORY: &str = "YuanzAAi/overleaf-account-switcher";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub current_version: &'static str,
    pub latest_version: Option<String>,
    pub tag: Option<String>,
    pub available: bool,
    pub release_url: String,
    pub asset: Option<ReleaseAsset>,
    pub image: Option<String>,
}

pub fn release_url(tag: Option<&str>) -> String {
    match tag {
        Some(tag) => format!("https://github.com/{REPOSITORY}/releases/tag/{tag}"),
        None => format!("https://github.com/{REPOSITORY}/releases"),
    }
}

pub fn download_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}")
}

pub fn platform_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("overleaf-windows-x64.exe"),
        ("macos", "aarch64") => Some("overleaf-macos-arm64-portable.zip"),
        ("macos", "x86_64") => Some("overleaf-macos-x64-portable.zip"),
        _ => None,
    }
}

pub async fn check() -> Result<UpdateInfo, String> {
    let client = Client::builder()
        .user_agent(concat!(
            "overleaf-account-switcher/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())?;
    let release = fetch_latest_release(&client).await?;
    let mut info = UpdateInfo {
        current_version: env!("CARGO_PKG_VERSION"),
        latest_version: None,
        tag: None,
        available: false,
        release_url: release_url(None),
        asset: None,
        image: None,
    };
    let Some(release) = release else {
        return Ok(info);
    };
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| "发布版本号格式不正确".to_string())?;
    if release.draft || release.prerelease || !version.pre.is_empty() {
        return Err("发布信息不是稳定版本".to_string());
    }
    let current = Version::parse(info.current_version).map_err(|error| error.to_string())?;
    info.available = version > current;
    info.latest_version = Some(version.to_string());
    info.release_url = release_url(Some(&release.tag_name));
    info.asset = platform_asset().and_then(|name| {
        release.assets.into_iter().find(|asset| {
            asset.name == name
                && asset.size > 0
                && asset.browser_download_url == download_url(&release.tag_name, name)
        })
    });
    info.image = Some(format!(
        "ghcr.io/{}:{}",
        REPOSITORY.to_ascii_lowercase(),
        release.tag_name
    ));
    info.tag = Some(release.tag_name);
    Ok(info)
}

async fn fetch_latest_release(client: &Client) -> Result<Option<GithubRelease>, String> {
    let api_result = async {
        client
            .get(format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json::<GithubRelease>()
            .await
    }
    .await;
    match api_result {
        Ok(release) => Ok(Some(release)),
        Err(api_error) => {
            let response = client
                .head(format!("{}/latest", release_url(None)))
                .timeout(Duration::from_secs(8))
                .send()
                .await
                .map_err(|error| format!("无法读取 GitHub 发布版本: {api_error}; {error}"))?;
            if api_error.status() == Some(StatusCode::NOT_FOUND)
                && (response.status() == StatusCode::NOT_FOUND
                    || (response.status().is_success()
                        && response.url().as_str().trim_end_matches('/') == release_url(None)))
            {
                return Ok(None);
            }
            let response = response
                .error_for_status()
                .map_err(|error| format!("无法读取 GitHub 发布版本: {api_error}; {error}"))?;
            // 官方 latest 跳转可避开 API 限流；仅接受当前仓库的正式版本地址。
            let prefix = format!("{}/tag/", release_url(None));
            let tag = response
                .url()
                .as_str()
                .strip_prefix(&prefix)
                .ok_or("GitHub 未返回有效的发布版本地址")?;
            Ok(Some(GithubRelease {
                tag_name: tag.to_string(),
                draft: false,
                prerelease: false,
                assets: Vec::new(),
            }))
        }
    }
}

pub(super) fn response(method: &str) -> crate::ApiResponse {
    let result = if method == "GET" {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| runtime.block_on(check()))
    } else {
        Err("Method Not Allowed".to_string())
    };
    let (status_code, body) = match result {
        Ok(info) => (200, serde_json::json!(info)),
        Err(error) => (
            if method == "GET" { 503 } else { 405 },
            serde_json::json!({ "error": error }),
        ),
    };
    crate::ApiResponse {
        status_code,
        content_type: "application/json; charset=utf-8",
        body: body.to_string(),
    }
}
