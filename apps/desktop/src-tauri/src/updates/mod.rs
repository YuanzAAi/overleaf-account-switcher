use overleaf_service::runtime::updates as releases;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Manager};

use crate::{service::ManagedService, DesktopLifecycle, MAIN_WINDOW_LABEL};

static INSTALLING: AtomicBool = AtomicBool::new(false);

struct UpdateGuard;

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

#[tauri::command]
pub(crate) async fn desktop_install_update(app: AppHandle, tag: String) -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("已有更新正在进行".into());
    }
    let _guard = UpdateGuard;
    let local_window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .and_then(|window| window.url().ok())
        .is_some_and(|url| crate::is_local_service_url(&url));
    if !local_window || !app.state::<ManagedService>().is_owned() {
        return Err("请退出共享服务后，重新打开桌面应用再更新".into());
    }
    if std::env::var_os("OVERLEAF_SWITCHER_SERVICE_EXE").is_some() {
        return Err("当前使用自定义服务程序，请分别更新桌面与服务".into());
    }
    let info = releases::check().await?;
    if !info.available || info.tag.as_deref() != Some(tag.as_str()) {
        return Err("发布版本已变化，请重新检查更新".into());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let target = installation_target(&executable)?;
    let parent = target.parent().ok_or("无法确定应用目录")?;
    let stage = tempfile::Builder::new()
        .prefix(".overleaf-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("应用目录不可写: {error}"))?;
    let client = Client::builder()
        .user_agent(concat!(
            "overleaf-account-switcher/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let archive = if cfg!(windows) {
        "overleaf-windows-x64-portable.zip"
    } else {
        releases::platform_asset().ok_or("当前平台不支持桌面更新")?
    };
    let sums_path = stage.path().join("SHA256SUMS");
    download(
        &client,
        &releases::download_url(&tag, "SHA256SUMS"),
        &sums_path,
        64 * 1024,
    )
    .await?;
    let sums = fs::read_to_string(&sums_path).map_err(|error| error.to_string())?;
    let expected = checksum(&sums, archive).ok_or("发布产物缺少有效 SHA-256 校验值")?;
    let actual = download(
        &client,
        &releases::download_url(&tag, archive),
        &stage.path().join("payload.zip"),
        512 * 1024 * 1024,
    )
    .await?;
    if !expected.eq_ignore_ascii_case(&actual) {
        return Err("更新包校验失败，原程序未修改".into());
    }
    let script_name = if cfg!(windows) {
        "apply.ps1"
    } else {
        "apply.sh"
    };
    let script = stage.path().join(script_name);
    let script_source = if cfg!(windows) {
        include_str!("windows.ps1")
    } else {
        include_str!("macos.sh")
    };
    fs::write(&script, script_source).map_err(|error| error.to_string())?;

    // Pause admissions only after a verified download, and before the owned service exits.
    if let Err(error) = service_update_gate(&client, "prepare").await {
        let _ = service_update_gate(&client, "cancel").await;
        return Err(error);
    }
    let mut command = helper_command(&script, &target, stage.path());
    if let Err(error) = command.spawn() {
        let _ = service_update_gate(&client, "cancel").await;
        return Err(format!("无法启动更新程序: {error}"));
    }
    // The helper now owns this directory and removes it after commit or rollback.
    let _ = stage.keep();
    app.state::<DesktopLifecycle>().request_exit();
    app.state::<ManagedService>().stop();
    app.exit(0);
    Ok(())
}

fn installation_target(executable: &Path) -> Result<PathBuf, String> {
    let executable = dunce::canonicalize(executable).map_err(|error| error.to_string())?;
    if cfg!(windows) {
        return Ok(executable);
    }
    if cfg!(target_os = "macos") {
        let bundle = executable.ancestors().nth(3).ok_or("未找到 macOS 应用包")?;
        if bundle
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            return Ok(bundle.to_path_buf());
        }
    }
    Err("请从正式桌面应用包执行更新".into())
}

fn helper_command(script: &Path, target: &Path, stage: &Path) -> Command {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .arg("-ParentProcessId")
            .arg(std::process::id().to_string())
            .arg("-Target")
            .arg(target)
            .arg("-Stage")
            .arg(stage)
            .creation_flags(0x08000000);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command
            .arg(script)
            .arg(std::process::id().to_string())
            .arg(target)
            .arg(stage);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

async fn service_update_gate(client: &Client, action: &str) -> Result<(), String> {
    let response = client
        .post(format!("http://127.0.0.1:8765/runtime/updates/{action}"))
        .timeout(Duration::from_secs(15))
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|error| format!("无法准备更新: {error}"))?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(body["error"]
            .as_str()
            .unwrap_or("服务未能进入更新状态")
            .to_string());
    }
    Ok(())
}

fn checksum<'a>(sums: &'a str, filename: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        let name = name.strip_prefix("./").unwrap_or(name);
        (name == filename
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            && fields.next().is_none())
        .then_some(digest)
    })
}

async fn download(client: &Client, url: &str, path: &Path, limit: u64) -> Result<String, String> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("下载失败: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err("更新产物超过大小限制".into());
    }
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        size += chunk.len() as u64;
        if size > limit {
            return Err("更新产物超过大小限制".into());
        }
        file.write_all(&chunk).map_err(|error| error.to_string())?;
        hasher.update(&chunk);
    }
    if size == 0 {
        return Err("下载内容为空".into());
    }
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}
