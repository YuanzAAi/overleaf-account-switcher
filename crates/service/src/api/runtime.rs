use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeArtifactCandidate {
    name: &'static str,
    relative_path: &'static str,
    kind: &'static str,
    cleanup_hint: &'static str,
}

const RUNTIME_ARTIFACT_CANDIDATES: &[RuntimeArtifactCandidate] = &[
    RuntimeArtifactCandidate {
        name: "target",
        relative_path: "target",
        kind: "build_target",
        cleanup_hint: "Remove after final delivery or space warning",
    },
    RuntimeArtifactCandidate {
        name: "node_modules",
        relative_path: "node_modules",
        kind: "node_modules",
        cleanup_hint: "Remove if a Node toolchain was used unexpectedly",
    },
    RuntimeArtifactCandidate {
        name: ".next",
        relative_path: ".next",
        kind: "web_build",
        cleanup_hint: "Remove generated web build output",
    },
    RuntimeArtifactCandidate {
        name: "dist",
        relative_path: "dist",
        kind: "web_build",
        cleanup_hint: "Remove generated web build output",
    },
    RuntimeArtifactCandidate {
        name: "src-tauri target",
        relative_path: "src-tauri/target",
        kind: "tauri_build_target",
        cleanup_hint: "Remove generated Tauri build output",
    },
    RuntimeArtifactCandidate {
        name: "apps/web node_modules",
        relative_path: "apps/web/node_modules",
        kind: "web_node_modules",
        cleanup_hint: "Remove if a Node toolchain was used unexpectedly",
    },
    RuntimeArtifactCandidate {
        name: "apps/web .next",
        relative_path: "apps/web/.next",
        kind: "web_build",
        cleanup_hint: "Remove generated web build output",
    },
    RuntimeArtifactCandidate {
        name: "apps/web dist",
        relative_path: "apps/web/dist",
        kind: "web_build",
        cleanup_hint: "Remove generated web build output",
    },
    RuntimeArtifactCandidate {
        name: "apps/desktop target",
        relative_path: "apps/desktop/target",
        kind: "desktop_build_target",
        cleanup_hint: "Remove generated desktop build output",
    },
    RuntimeArtifactCandidate {
        name: "apps/desktop src-tauri target",
        relative_path: "apps/desktop/src-tauri/target",
        kind: "desktop_tauri_build_target",
        cleanup_hint: "Remove generated desktop Tauri build output",
    },
    RuntimeArtifactCandidate {
        name: ".pytest_cache",
        relative_path: ".pytest_cache",
        kind: "python_test_cache",
        cleanup_hint: "Remove unused Python test cache",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RuntimeTempProfileCleanupRequest {
    #[serde(default)]
    confirm_cleanup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RuntimeArtifactCleanupRequest {
    #[serde(default)]
    confirm_cleanup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeArtifactCleanupReport {
    pub removed_count: usize,
    pub failed_count: usize,
    pub removed_bytes: u64,
    pub items: Vec<RuntimeArtifactCleanupItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeArtifactCleanupItem {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    pub bytes: u64,
    pub status: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeTempProfileCleanupReport {
    pub removed_count: usize,
    pub failed_count: usize,
    pub skipped_user_window_count: usize,
    pub removed_bytes: u64,
    pub skipped_user_window_bytes: u64,
    pub items: Vec<RuntimeTempProfileCleanupItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeTempProfileCleanupPreview {
    pub candidate_count: usize,
    pub candidate_bytes: u64,
    pub skipped_user_window_count: usize,
    pub skipped_user_window_bytes: u64,
    pub items: Vec<RuntimeTempProfileCleanupItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeTempProfileCleanupItem {
    pub profile_name: String,
    pub status: &'static str,
    pub bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeArtifactStatus {
    pub workspace_dir: String,
    pub item_count: usize,
    pub total_bytes: u64,
    pub items: Vec<RuntimeArtifactItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeArtifactItem {
    pub name: String,
    pub path: String,
    pub kind: &'static str,
    pub bytes: u64,
    pub cleanup_hint: &'static str,
}

pub(super) fn preview_temp_profiles_response(state: &ApiState) -> ApiResponse {
    match preview_owned_temp_profiles(&state.config.tmp_dir) {
        Ok(preview) => json_response(200, &preview),
        Err(error) => io_error_response(error),
    }
}

pub(super) fn cleanup_temp_profiles_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: RuntimeTempProfileCleanupRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    if !request.confirm_cleanup {
        return json_response(
            400,
            &ApiErrorBody {
                error: "confirm_cleanup must be true".to_string(),
            },
        );
    }

    match cleanup_owned_temp_profiles(&state.config.tmp_dir) {
        Ok(report) => json_response(200, &report),
        Err(error) => io_error_response(error),
    }
}

pub(super) fn cleanup_runtime_artifacts_response(state: &ApiState, body: &str) -> ApiResponse {
    let request: RuntimeArtifactCleanupRequest = match parse_json_request(body) {
        Ok(request) => request,
        Err(response) => return response,
    };

    if !request.confirm_cleanup {
        return json_response(
            400,
            &ApiErrorBody {
                error: "confirm_cleanup must be true".to_string(),
            },
        );
    }

    match cleanup_runtime_artifacts_for_workspace(&state.config.workspace_dir) {
        Ok(report) => json_response(200, &report),
        Err(error) => io_error_response(error),
    }
}

pub(super) fn runtime_storage_status(state: &ApiState) -> RuntimeStorageStatus {
    let owned_profiles = temp_profile_inventory(&state.config.tmp_dir, |name| {
        is_owned_temp_profile_name(name)
    });
    let user_window_profiles = temp_profile_inventory(&state.config.tmp_dir, |name| {
        name.starts_with(USER_WINDOW_PROFILE_PREFIX)
    });
    let export_inventory = directory_file_inventory(&state.config.default_export_dir());

    RuntimeStorageStatus {
        data_dir_exists: path_is_dir(&state.config.data_dir),
        tmp_dir_exists: path_is_dir(&state.config.tmp_dir),
        export_dir_exists: path_is_dir(&state.config.default_export_dir()),
        export_file_count: export_inventory.count,
        export_total_bytes: export_inventory.bytes,
        accounts_file_bytes: file_size(&state.config.accounts_path()),
        cards_file_bytes: file_size(&state.config.cards_path()),
        addresses_file_bytes: file_size(&state.config.addresses_path()),
        owned_temp_profile_count: owned_profiles.count,
        owned_temp_profile_bytes: owned_profiles.bytes,
        user_window_profile_count: user_window_profiles.count,
        user_window_profile_bytes: user_window_profiles.bytes,
    }
}

pub(super) fn runtime_artifact_status(state: &ApiState) -> RuntimeArtifactStatus {
    runtime_artifact_status_for_workspace(&state.config.workspace_dir)
}

fn runtime_artifact_status_for_workspace(workspace_dir: &Path) -> RuntimeArtifactStatus {
    let mut items = Vec::new();
    for candidate in RUNTIME_ARTIFACT_CANDIDATES {
        let path = workspace_dir.join(candidate.relative_path);
        if path_is_dir(&path) {
            items.push(RuntimeArtifactItem {
                name: candidate.name.to_string(),
                path: path_string(&path),
                kind: candidate.kind,
                bytes: directory_size_bytes(&path),
                cleanup_hint: candidate.cleanup_hint,
            });
        }
    }

    for path in find_named_directories(workspace_dir, "__pycache__") {
        items.push(RuntimeArtifactItem {
            name: "__pycache__".to_string(),
            path: path_string(&path),
            kind: "python_bytecode_cache",
            bytes: directory_size_bytes(&path),
            cleanup_hint: "Remove unused Python bytecode cache",
        });
    }
    items.sort_by(|left, right| left.path.cmp(&right.path));

    RuntimeArtifactStatus {
        workspace_dir: path_string(workspace_dir),
        item_count: items.len(),
        total_bytes: items
            .iter()
            .fold(0u64, |total, item| total.saturating_add(item.bytes)),
        items,
    }
}

fn cleanup_runtime_artifacts_for_workspace(
    workspace_dir: &Path,
) -> io::Result<RuntimeArtifactCleanupReport> {
    let status = runtime_artifact_status_for_workspace(workspace_dir);
    let mut report = RuntimeArtifactCleanupReport {
        removed_count: 0,
        failed_count: 0,
        removed_bytes: 0,
        items: Vec::new(),
    };

    for item in status.items {
        let path = PathBuf::from(&item.path);
        if !runtime_artifact_path_is_safe_to_remove(workspace_dir, &path)? {
            report.failed_count += 1;
            report.items.push(RuntimeArtifactCleanupItem {
                name: item.name,
                path: item.path,
                kind: item.kind,
                bytes: item.bytes,
                status: "failed",
                error: Some("refuse to remove artifact outside workspace".to_string()),
            });
            continue;
        }

        match remove_dir_all_with_retry(
            &path,
            PROFILE_REMOVE_RETRY_ATTEMPTS,
            PROFILE_REMOVE_RETRY_DELAY_MS,
        ) {
            Ok(()) => {
                report.removed_count += 1;
                report.removed_bytes = report.removed_bytes.saturating_add(item.bytes);
                report.items.push(RuntimeArtifactCleanupItem {
                    name: item.name,
                    path: item.path,
                    kind: item.kind,
                    bytes: item.bytes,
                    status: "removed",
                    error: None,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.items.push(RuntimeArtifactCleanupItem {
                    name: item.name,
                    path: item.path,
                    kind: item.kind,
                    bytes: item.bytes,
                    status: "missing",
                    error: None,
                });
            }
            Err(error) => {
                report.failed_count += 1;
                report.items.push(RuntimeArtifactCleanupItem {
                    name: item.name,
                    path: item.path,
                    kind: item.kind,
                    bytes: item.bytes,
                    status: "failed",
                    error: Some(error.to_string()),
                });
            }
        }
    }

    Ok(report)
}

fn runtime_artifact_path_is_safe_to_remove(root: &Path, path: &Path) -> io::Result<bool> {
    let root = fs::canonicalize(root)?;
    let path = fs::canonicalize(path)?;
    Ok(path.starts_with(&root) && path != root)
}

fn find_named_directories(root: &Path, directory_name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name == directory_name {
                found.push(path);
                continue;
            }
            let is_protected_workspace_dir =
                path == root.join("data") || path == root.join("backups");
            if matches!(
                name.as_str(),
                "target" | "node_modules" | ".next" | "dist" | ".git"
            ) || is_protected_workspace_dir
            {
                continue;
            }
            stack.push(path);
        }
    }

    found
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileInventory {
    count: usize,
    bytes: u64,
}

fn directory_file_inventory(path: &Path) -> FileInventory {
    let mut inventory = FileInventory { count: 0, bytes: 0 };
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_file() {
                inventory.count += 1;
                inventory.bytes = inventory.bytes.saturating_add(metadata.len());
            } else if metadata.is_dir() {
                stack.push(path);
            }
        }
    }

    inventory
}

fn temp_profile_inventory(
    tmp_dir: &Path,
    is_target_profile: impl Fn(&str) -> bool,
) -> FileInventory {
    let mut inventory = FileInventory { count: 0, bytes: 0 };
    let Ok(entries) = fs::read_dir(tmp_dir) else {
        return inventory;
    };

    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_target_profile(&name) {
            continue;
        }
        let path = entry.path();
        if !path_is_dir(&path) {
            continue;
        }
        inventory.count += 1;
        inventory.bytes = inventory.bytes.saturating_add(directory_size_bytes(&path));
    }

    inventory
}

fn directory_size_bytes(path: &Path) -> u64 {
    directory_file_inventory(path).bytes
}

fn preview_owned_temp_profiles(tmp_dir: &Path) -> io::Result<RuntimeTempProfileCleanupPreview> {
    let mut preview = RuntimeTempProfileCleanupPreview {
        candidate_count: 0,
        candidate_bytes: 0,
        skipped_user_window_count: 0,
        skipped_user_window_bytes: 0,
        items: Vec::new(),
    };

    for (name, path) in sorted_temp_profile_dirs(tmp_dir)? {
        let bytes = directory_size_bytes(&path);
        if name.starts_with(USER_WINDOW_PROFILE_PREFIX) {
            preview.skipped_user_window_count += 1;
            preview.skipped_user_window_bytes =
                preview.skipped_user_window_bytes.saturating_add(bytes);
            preview.items.push(RuntimeTempProfileCleanupItem {
                profile_name: name,
                status: "skipped_user_window",
                bytes,
                error: None,
            });
            continue;
        }
        if !is_owned_temp_profile_name(&name) {
            continue;
        }

        preview.candidate_count += 1;
        preview.candidate_bytes = preview.candidate_bytes.saturating_add(bytes);
        preview.items.push(RuntimeTempProfileCleanupItem {
            profile_name: name,
            status: "candidate",
            bytes,
            error: None,
        });
    }

    Ok(preview)
}

fn cleanup_owned_temp_profiles(tmp_dir: &Path) -> io::Result<RuntimeTempProfileCleanupReport> {
    let mut report = RuntimeTempProfileCleanupReport {
        removed_count: 0,
        failed_count: 0,
        skipped_user_window_count: 0,
        removed_bytes: 0,
        skipped_user_window_bytes: 0,
        items: Vec::new(),
    };

    if !path_is_dir(tmp_dir) {
        return Ok(report);
    }

    for (name, path) in sorted_temp_profile_dirs(tmp_dir)? {
        if name.starts_with(USER_WINDOW_PROFILE_PREFIX) {
            report.skipped_user_window_count += 1;
            report.skipped_user_window_bytes = report
                .skipped_user_window_bytes
                .saturating_add(directory_size_bytes(&path));
            continue;
        }
        if !is_owned_temp_profile_name(&name) {
            continue;
        }

        let bytes = directory_size_bytes(&path);
        match cleanup_owned_chrome_profile(&path) {
            Ok(_) => {
                report.removed_count += 1;
                report.removed_bytes = report.removed_bytes.saturating_add(bytes);
                report.items.push(RuntimeTempProfileCleanupItem {
                    profile_name: name,
                    status: "removed",
                    bytes,
                    error: None,
                });
            }
            Err(error) => {
                report.failed_count += 1;
                report.items.push(RuntimeTempProfileCleanupItem {
                    profile_name: name,
                    status: "failed",
                    bytes,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    Ok(report)
}

fn remove_dir_all_with_retry(path: &Path, attempts: u32, delay_ms: u64) -> io::Result<()> {
    for attempt in 0..attempts.max(1) {
        if !path.exists() {
            return Ok(());
        }
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if attempt + 1 < attempts.max(1) => {
                let _ = error;
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn sorted_temp_profile_dirs(tmp_dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    if !path_is_dir(tmp_dir) {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(tmp_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            let name = entry.file_name().to_str()?.to_string();
            Some((name, entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
}

fn path_is_dir(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}
