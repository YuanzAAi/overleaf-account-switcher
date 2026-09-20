use std::path::{Path, PathBuf};

use crate::{BrowserSessionPolicy, BrowserTaskKind, ClosePolicy};
use serde::{Deserialize, Serialize};

pub const DEFAULT_OVERLEAF_URL: &str = "https://www.overleaf.com/project";
pub const PROFILE_REMOVE_RETRY_ATTEMPTS: u32 = 10;
pub const PROFILE_REMOVE_RETRY_DELAY_MS: u64 = 300;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromeProxyMode {
    #[default]
    Auto,
    InheritSystem,
    Direct,
    Custom,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromeProxyPolicy {
    pub mode: ChromeProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeProxyAttempt {
    Primary,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeProxyRoute {
    InheritSystem,
    Direct,
    Custom(String),
}

impl ChromeProxyPolicy {
    pub fn primary_route(&self) -> ChromeProxyRoute {
        match self.mode {
            ChromeProxyMode::Auto | ChromeProxyMode::Direct => ChromeProxyRoute::Direct,
            ChromeProxyMode::InheritSystem => ChromeProxyRoute::InheritSystem,
            ChromeProxyMode::Custom => ChromeProxyRoute::Custom(
                self.server
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_default()
                    .to_string(),
            ),
        }
    }

    pub fn route_for_attempt(&self, attempt: ChromeProxyAttempt) -> ChromeProxyRoute {
        if attempt == ChromeProxyAttempt::Fallback && self.mode == ChromeProxyMode::Auto {
            ChromeProxyRoute::InheritSystem
        } else {
            self.primary_route()
        }
    }

    pub fn has_fallback(&self) -> bool {
        self.mode == ChromeProxyMode::Auto
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeLaunchOptions {
    pub chrome_executable: PathBuf,
    pub task_kind: BrowserTaskKind,
    pub debug_port: u16,
    pub user_data_dir: PathBuf,
    pub initial_url: Option<String>,
    pub extension_dir: Option<PathBuf>,
    pub proxy_route: ChromeProxyRoute,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeLaunchPlan {
    pub chrome_executable: PathBuf,
    pub args: Vec<String>,
    pub task_kind: BrowserTaskKind,
    pub close_policy: ClosePolicy,
    pub debug_port: u16,
    pub user_data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserSessionDescriptor {
    pub task_kind: BrowserTaskKind,
    pub close_policy: ClosePolicy,
    pub debug_port: u16,
    pub user_data_dir: PathBuf,
    pub process_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupTrigger {
    TaskFinished,
    UserWindowExited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserCleanupPlan {
    pub trigger: CleanupTrigger,
    pub close_policy: ClosePolicy,
    pub steps: Vec<BrowserCleanupStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCleanupStep {
    SendCdpBrowserClose {
        debug_port: u16,
    },
    WaitForProcessExit {
        process_id: Option<u32>,
    },
    KillProcessesByDebugPort {
        debug_port: u16,
    },
    KillProcessesByProfileDir {
        user_data_dir: PathBuf,
    },
    WaitForUserWindowExit {
        process_id: Option<u32>,
    },
    RemoveProfileDirWithRetry {
        user_data_dir: PathBuf,
        attempts: u32,
        delay_ms: u64,
    },
}

impl ChromeLaunchOptions {
    pub fn new(
        chrome_executable: impl Into<PathBuf>,
        task_kind: BrowserTaskKind,
        debug_port: u16,
        user_data_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            chrome_executable: chrome_executable.into(),
            task_kind,
            debug_port,
            user_data_dir: user_data_dir.into(),
            initial_url: Some(DEFAULT_OVERLEAF_URL.to_string()),
            extension_dir: None,
            proxy_route: ChromeProxyRoute::Direct,
        }
    }

    pub fn with_initial_url(mut self, initial_url: impl Into<String>) -> Self {
        self.initial_url = Some(initial_url.into());
        self
    }

    pub fn without_initial_url(mut self) -> Self {
        self.initial_url = None;
        self
    }

    pub fn with_extension_dir(mut self, extension_dir: impl Into<PathBuf>) -> Self {
        self.extension_dir = Some(extension_dir.into());
        self
    }

    pub fn with_proxy_route(mut self, proxy_route: ChromeProxyRoute) -> Self {
        self.proxy_route = proxy_route;
        self
    }
}

impl ChromeLaunchPlan {
    pub fn from_options(options: ChromeLaunchOptions) -> Self {
        let policy = BrowserSessionPolicy::for_task(options.task_kind);
        let mut args = vec![
            format!("--remote-debugging-port={}", options.debug_port),
            format!("--user-data-dir={}", path_arg(&options.user_data_dir)),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-popup-blocking".to_string(),
        ];

        match &options.proxy_route {
            ChromeProxyRoute::InheritSystem => {}
            ChromeProxyRoute::Direct => args.push("--no-proxy-server".to_string()),
            ChromeProxyRoute::Custom(server) => {
                args.push(format!("--proxy-server={server}"));
            }
        }

        if let Some(extension_dir) = options.extension_dir.as_deref() {
            args.push(format!("--load-extension={}", path_arg(extension_dir)));
        }
        if let Some(initial_url) = options.initial_url.as_deref() {
            args.push(initial_url.to_string());
        }

        Self {
            chrome_executable: options.chrome_executable,
            args,
            task_kind: options.task_kind,
            close_policy: policy.close_policy,
            debug_port: options.debug_port,
            user_data_dir: options.user_data_dir,
        }
    }

    pub fn session_descriptor(&self, process_id: Option<u32>) -> BrowserSessionDescriptor {
        BrowserSessionDescriptor {
            task_kind: self.task_kind,
            close_policy: self.close_policy,
            debug_port: self.debug_port,
            user_data_dir: self.user_data_dir.clone(),
            process_id,
        }
    }
}

impl BrowserCleanupPlan {
    pub fn for_session(session: &BrowserSessionDescriptor, trigger: CleanupTrigger) -> Self {
        let steps = match (session.close_policy, trigger) {
            (ClosePolicy::AutoCloseTemp, CleanupTrigger::TaskFinished) => vec![
                BrowserCleanupStep::SendCdpBrowserClose {
                    debug_port: session.debug_port,
                },
                BrowserCleanupStep::WaitForProcessExit {
                    process_id: session.process_id,
                },
                BrowserCleanupStep::KillProcessesByDebugPort {
                    debug_port: session.debug_port,
                },
                BrowserCleanupStep::KillProcessesByProfileDir {
                    user_data_dir: session.user_data_dir.clone(),
                },
                remove_profile_step(&session.user_data_dir),
            ],
            (ClosePolicy::KeepUserWindow, CleanupTrigger::TaskFinished) => Vec::new(),
            (ClosePolicy::KeepUserWindow, CleanupTrigger::UserWindowExited) => vec![
                BrowserCleanupStep::WaitForUserWindowExit {
                    process_id: session.process_id,
                },
                remove_profile_step(&session.user_data_dir),
            ],
            (ClosePolicy::AutoCloseTemp, CleanupTrigger::UserWindowExited) => {
                vec![remove_profile_step(&session.user_data_dir)]
            }
        };

        Self {
            trigger,
            close_policy: session.close_policy,
            steps,
        }
    }

    pub fn has_destructive_process_cleanup(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                step,
                BrowserCleanupStep::KillProcessesByDebugPort { .. }
                    | BrowserCleanupStep::KillProcessesByProfileDir { .. }
            )
        })
    }
}

fn remove_profile_step(user_data_dir: &Path) -> BrowserCleanupStep {
    BrowserCleanupStep::RemoveProfileDirWithRetry {
        user_data_dir: user_data_dir.to_path_buf(),
        attempts: PROFILE_REMOVE_RETRY_ATTEMPTS,
        delay_ms: PROFILE_REMOVE_RETRY_DELAY_MS,
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
