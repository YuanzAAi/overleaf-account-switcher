use overleaf_browser::{cleanup_owned_chrome_profile, is_owned_temp_profile_name};
use overleaf_storage::{default_storage_root, DATA_DIR_ENV, TMP_DIR_ENV};
use std::{
    env, fs,
    io::{self, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

pub(super) const SERVICE_HOST: &str = "127.0.0.1";
pub(super) const SERVICE_PORT: u16 = 8765;
const SERVICE_HEALTH_PATH: &str = "/health";
const LEGACY_DESKTOP_STORAGE_DIR: &str = "OverleafAccountSwitcher";

include!(concat!(env!("OUT_DIR"), "/extension_assets.rs"));

pub(super) struct OwnedServiceProcess {
    child: Child,
    tmp_dir: PathBuf,
}

#[derive(Default)]
pub(super) struct ManagedService {
    process: Mutex<Option<OwnedServiceProcess>>,
}

impl ManagedService {
    pub(super) fn is_owned(&self) -> bool {
        self.process.lock().is_ok_and(|process| process.is_some())
    }

    pub(super) fn set(&self, process: OwnedServiceProcess) {
        if let Ok(mut owned) = self.process.lock() {
            *owned = Some(process);
        }
    }

    pub(super) fn stop(&self) {
        let Ok(mut owned) = self.process.lock() else {
            return;
        };
        let process = owned.take();
        drop(owned);

        if let Some(mut process) = process {
            let _ = process.child.kill();
            let _ = process.child.wait();
            match cleanup_owned_service_profiles(&process.tmp_dir) {
                Ok(report) if report.failed_count > 0 => eprintln!(
                    "owned Chrome cleanup completed with {} failure(s)",
                    report.failed_count
                ),
                Ok(_) => {}
                Err(error) => eprintln!(
                    "failed to inspect owned Chrome profiles under {}: {error}",
                    process.tmp_dir.display()
                ),
            }
        }
    }
}

impl Drop for ManagedService {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct OwnedProfileCleanupSummary {
    removed_count: usize,
    failed_count: usize,
    terminated_process_count: usize,
}

fn cleanup_owned_service_profiles(tmp_dir: &Path) -> io::Result<OwnedProfileCleanupSummary> {
    let mut summary = OwnedProfileCleanupSummary::default();
    if !tmp_dir.exists() {
        return Ok(summary);
    }

    for entry in fs::read_dir(tmp_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let profile_name = entry.file_name().to_string_lossy().to_string();
        if !is_owned_temp_profile_name(&profile_name) {
            continue;
        }
        match cleanup_owned_chrome_profile(entry.path()) {
            Ok(terminated_process_count) => {
                summary.removed_count += 1;
                summary.terminated_process_count += terminated_process_count;
            }
            Err(error) => {
                summary.failed_count += 1;
                eprintln!("failed to clean owned Chrome profile {profile_name}: {error}");
            }
        }
    }
    Ok(summary)
}

pub(super) fn ensure_local_service() -> Option<OwnedServiceProcess> {
    if service_ready(Duration::from_millis(400)) {
        return None;
    }

    let mut command = match service_command() {
        Ok(command) => command,
        Err(error) => {
            eprintln!("failed to locate service executable: {error}");
            return None;
        }
    };

    let workspace_dir = match workspace_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to prepare extension assets: {error}");
            return None;
        }
    };
    command
        .current_dir(&workspace_dir)
        .env("OVERLEAF_SWITCHER_WORKSPACE_DIR", &workspace_dir)
        .env("OVERLEAF_SWITCHER_SERVICE_HOST", SERVICE_HOST)
        .env("OVERLEAF_SWITCHER_SERVICE_PORT", SERVICE_PORT.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let tmp_dir = configure_desktop_storage_env(&mut command);
    configure_hidden_service_process(&mut command);

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("failed to start service: {error}");
            return None;
        }
    };

    if !service_ready(Duration::from_secs(12)) {
        eprintln!("overleaf-service-api was started but did not become healthy within 12s");
    }

    Some(OwnedServiceProcess { child, tmp_dir })
}

fn service_ready(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if health_request_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

pub(super) fn health_request_ok() -> bool {
    let Ok(mut stream) = TcpStream::connect((SERVICE_HOST, SERVICE_PORT)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let request = format!(
        "GET {SERVICE_HEALTH_PATH} HTTP/1.1\r\nHost: {SERVICE_HOST}:{SERVICE_PORT}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    response.starts_with("HTTP/1.1 200") && response.contains("\"status\":\"ok\"")
}

fn service_command() -> io::Result<Command> {
    if let Some(path) = env::var_os("OVERLEAF_SWITCHER_SERVICE_EXE").map(PathBuf::from) {
        if path.is_file() {
            return Ok(Command::new(path));
        }
    }
    // Keep the service in an owned child process, using the same executable and API code.
    let mut command = Command::new(env::current_exe()?);
    command.arg("--service");
    Ok(command)
}

fn workspace_dir() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("OVERLEAF_SWITCHER_WORKSPACE_DIR").map(PathBuf::from) {
        return Ok(path);
    }
    let executable_dir = current_exe_dir();
    if let Some(dir) = executable_dir
        .as_ref()
        .filter(|path| path.join("chrome_extension").is_dir())
    {
        return Ok(dir.clone());
    }
    if cfg!(debug_assertions) {
        if let Some(path) = compiled_workspace_root().filter(|path| path.exists()) {
            return Ok(path);
        }
    }
    // Chrome needs a persistent unpacked-extension path across application restarts.
    let workspace = default_storage_root().join("runtime");
    let directory = workspace.join("chrome_extension");
    fs::create_dir_all(&directory)?;
    for &(name, contents) in EXTENSION_ASSETS {
        let path = directory.join(name);
        if fs::read(&path).is_ok_and(|existing| existing == contents) {
            continue;
        }
        let mut file = tempfile::NamedTempFile::new_in(&directory)?;
        file.write_all(contents)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
    }
    Ok(workspace)
}

fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn compiled_workspace_root() -> Option<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn configure_desktop_storage_env(command: &mut Command) -> PathBuf {
    let configured_data_dir = env::var_os(DATA_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let configured_tmp_dir = env::var_os(TMP_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let data_dir_is_default = configured_data_dir.is_none();
    let tmp_dir_is_default = configured_tmp_dir.is_none();
    let mut data_dir = configured_data_dir.unwrap_or_else(overleaf_storage::data_dir);
    let mut tmp_dir = configured_tmp_dir.unwrap_or_else(overleaf_storage::tmp_dir);

    if data_dir_is_default && tmp_dir_is_default {
        if let Some(legacy_root) = legacy_desktop_storage_root() {
            if let Err(error) =
                migrate_legacy_desktop_storage(&legacy_root, &default_storage_root())
            {
                eprintln!(
                    "failed to migrate legacy desktop storage {}: {error}",
                    legacy_root.display()
                );
                let legacy_data_dir = legacy_root.join("data");
                if legacy_data_dir.exists() {
                    data_dir = legacy_data_dir;
                    tmp_dir = legacy_root.join("tmp");
                }
            }
        }
    }

    if data_dir_is_default {
        if let Err(error) = fs::create_dir_all(&data_dir) {
            eprintln!(
                "failed to create desktop data dir {}: {error}",
                data_dir.display()
            );
        }
        command.env(DATA_DIR_ENV, &data_dir);
    }
    if tmp_dir_is_default {
        if let Err(error) = fs::create_dir_all(&tmp_dir) {
            eprintln!(
                "failed to create desktop tmp dir {}: {error}",
                tmp_dir.display()
            );
        }
        command.env(TMP_DIR_ENV, &tmp_dir);
    }
    tmp_dir
}

#[cfg(windows)]
fn legacy_desktop_storage_root() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .map(|base| base.join(LEGACY_DESKTOP_STORAGE_DIR))
}

#[cfg(not(windows))]
fn legacy_desktop_storage_root() -> Option<PathBuf> {
    None
}

fn migrate_legacy_desktop_storage(legacy_root: &Path, target_root: &Path) -> io::Result<()> {
    if !legacy_root.exists() || legacy_root == target_root {
        return Ok(());
    }

    fs::create_dir_all(target_root)?;
    for entry in fs::read_dir(legacy_root)? {
        let entry = entry?;
        if entry.file_name() == "tmp" {
            continue;
        }
        copy_path_verified(&entry.path(), &target_root.join(entry.file_name()))?;
    }

    fs::remove_dir_all(legacy_root)
}

fn copy_path_verified(source: &Path, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("refusing to migrate symbolic link {}", source.display()),
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path_verified(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported legacy storage entry {}", source.display()),
        ));
    }

    if target.exists() {
        if files_equal(source, target)? {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("migration target conflicts with {}", target.display()),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, target)?;
    if files_equal(source, target)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("migration verification failed for {}", source.display()),
        ))
    }
}

fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    let left_metadata = fs::metadata(left)?;
    let right_metadata = fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    let mut left_file = fs::File::open(left)?;
    let mut right_file = fs::File::open(right)?;
    let mut left_buffer = [0_u8; 16 * 1024];
    let mut right_buffer = [0_u8; 16 * 1024];
    loop {
        let left_read = left_file.read(&mut left_buffer)?;
        let right_read = right_file.read(&mut right_buffer)?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

#[cfg(windows)]
fn configure_hidden_service_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_service_process(_command: &mut Command) {}
