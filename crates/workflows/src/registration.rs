use overleaf_core::{TaskEvent, TaskPhase, TaskStatus};

pub const REGISTERED_EMAIL_ERROR_TEXT: &str =
    "This email address is already associated with a different Overleaf account.";
pub const RECAPTCHA_WAIT_SECONDS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    InitBrowser,
    OpenSignup,
    FillCredentials,
    WaitCaptcha,
    DetectRegisteredEmail,
    WaitEmailCode,
    SubmitEmailCode,
    OpenSubscription,
    FetchAddress,
    FillAddress,
    SelectCard,
    FillPayment,
    SubmitPayment,
    WaitThankYou,
    OpenSubscriptionManagement,
    AcceptExtraTrial,
    ChangePlanToProAnnual,
    CancelSubscription,
    ReturnProject,
    ExtractCookie,
    ExtractGitToken,
    ParseTrialExpiry,
    SaveAccount,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationWorkflow {
    trial_days: u32,
    steps: Vec<RegistrationState>,
    index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationUserPrompt {
    Captcha { timeout_seconds: u64 },
    EmailCode,
    NewRegistrationCredentials { old_email: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationArtifact {
    Cookie,
    GitToken,
    TrialExpiry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationCompletionStatus {
    SaveAllowed,
    SubscriptionMissing,
    ArtifactsMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationCompletionInput {
    pub subscription_succeeded: bool,
    pub cookie_captured: bool,
    pub git_token_captured: bool,
    pub trial_expiry_captured: bool,
    pub git_token_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationCompletionDecision {
    pub can_save_account: bool,
    pub status: RegistrationCompletionStatus,
    pub missing_recoverable_artifacts: Vec<RegistrationArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationTaskEvent {
    Started {
        task_id: String,
        trial_days: u32,
    },
    StepChanged {
        task_id: String,
        state: RegistrationState,
    },
    NeedEmailCode {
        task_id: String,
        email: String,
    },
    NeedCaptcha {
        task_id: String,
        timeout_seconds: u64,
    },
    NeedNewRegistrationCredentials {
        task_id: String,
        old_email: String,
    },
    Completed {
        task_id: String,
        decision: RegistrationCompletionDecision,
    },
    Failed {
        task_id: String,
        error: String,
    },
    Cancelled {
        task_id: String,
    },
}

impl RegistrationWorkflow {
    pub fn new(trial_days: u32) -> Self {
        Self {
            trial_days,
            steps: registration_steps_for_trial(trial_days),
            index: 0,
        }
    }

    pub fn trial_days(&self) -> u32 {
        self.trial_days
    }

    pub fn current(&self) -> RegistrationState {
        self.steps
            .get(self.index)
            .copied()
            .unwrap_or(RegistrationState::Done)
    }

    pub fn current_step_number(&self) -> u32 {
        (self.index + 1).min(self.steps.len()) as u32
    }

    pub fn total_steps(&self) -> u32 {
        self.steps.len() as u32
    }

    pub fn steps(&self) -> &[RegistrationState] {
        &self.steps
    }

    pub fn advance(&mut self) -> RegistrationState {
        if self.index + 1 < self.steps.len() {
            self.index += 1;
        }
        self.current()
    }

    pub fn mark_failed(&mut self) {
        self.steps.truncate(self.index + 1);
        self.steps.push(RegistrationState::Failed);
        self.index = self.steps.len() - 1;
    }

    pub fn mark_cancelled(&mut self) {
        self.steps.truncate(self.index + 1);
        self.steps.push(RegistrationState::Cancelled);
        self.index = self.steps.len() - 1;
    }
}

impl RegistrationState {
    pub fn message(self) -> &'static str {
        match self {
            Self::InitBrowser => "启动临时浏览器",
            Self::OpenSignup => "打开注册页面",
            Self::FillCredentials => "填写邮箱和密码",
            Self::WaitCaptcha => "等待用户完成 reCAPTCHA",
            Self::DetectRegisteredEmail => "检测邮箱是否已注册",
            Self::WaitEmailCode => "等待邮箱验证码",
            Self::SubmitEmailCode => "提交邮箱验证码",
            Self::OpenSubscription => "打开订阅页面",
            Self::FetchAddress => "获取地址",
            Self::FillAddress => "填写地址",
            Self::SelectCard => "选择银行卡",
            Self::FillPayment => "填写支付信息",
            Self::SubmitPayment => "提交订阅支付",
            Self::WaitThankYou => "等待订阅成功页面",
            Self::OpenSubscriptionManagement => "打开订阅管理",
            Self::AcceptExtraTrial => "接受额外试用",
            Self::ChangePlanToProAnnual => "切换到 Pro annual",
            Self::CancelSubscription => "取消订阅",
            Self::ReturnProject => "返回项目页面",
            Self::ExtractCookie => "提取 Cookie",
            Self::ExtractGitToken => "提取 Git Integration token",
            Self::ParseTrialExpiry => "解析试用到期时间",
            Self::SaveAccount => "保存账号",
            Self::Done => "注册流程完成",
            Self::Failed => "注册流程失败",
            Self::Cancelled => "注册流程已取消",
        }
    }

    pub fn task_phase(self) -> TaskPhase {
        match self {
            Self::WaitCaptcha | Self::WaitEmailCode => TaskPhase::WaitingForUser,
            Self::Done => TaskPhase::Completed,
            Self::Failed => TaskPhase::Failed,
            Self::Cancelled => TaskPhase::Cancelled,
            _ => TaskPhase::Running,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn user_prompt(self) -> Option<RegistrationUserPrompt> {
        match self {
            Self::WaitCaptcha => Some(RegistrationUserPrompt::Captcha {
                timeout_seconds: RECAPTCHA_WAIT_SECONDS,
            }),
            Self::WaitEmailCode => Some(RegistrationUserPrompt::EmailCode),
            _ => None,
        }
    }

    pub fn to_task_status(self) -> TaskStatus {
        TaskStatus {
            phase: self.task_phase(),
            message: self.message().to_string(),
        }
    }
}

impl RegistrationTaskEvent {
    pub fn to_core_event(&self) -> TaskEvent {
        match self {
            Self::Started { trial_days, .. } => TaskEvent::Started {
                name: format!("自动注册账号（{trial_days} 天试用）"),
            },
            Self::StepChanged { state, .. } => TaskEvent::Progress(state.to_task_status()),
            Self::NeedEmailCode { email, .. } => TaskEvent::Progress(TaskStatus {
                phase: TaskPhase::WaitingForUser,
                message: format!("等待邮箱验证码: {email}"),
            }),
            Self::NeedCaptcha {
                timeout_seconds, ..
            } => TaskEvent::Progress(TaskStatus {
                phase: TaskPhase::WaitingForUser,
                message: format!("等待 reCAPTCHA，最长 {timeout_seconds} 秒"),
            }),
            Self::NeedNewRegistrationCredentials { old_email, .. } => {
                TaskEvent::Progress(TaskStatus {
                    phase: TaskPhase::WaitingForUser,
                    message: format!("邮箱已注册，请替换注册信息: {old_email}"),
                })
            }
            Self::Completed { decision, .. } => TaskEvent::Finished(TaskStatus {
                phase: TaskPhase::Completed,
                message: if decision.can_save_account {
                    "注册完成，账号可入库".to_string()
                } else if decision.status == RegistrationCompletionStatus::ArtifactsMissing {
                    "订阅流程已完成，但账号资料不完整，不能入库".to_string()
                } else {
                    "注册流程未完成，账号不能入库".to_string()
                },
            }),
            Self::Failed { error, .. } => TaskEvent::Finished(TaskStatus {
                phase: TaskPhase::Failed,
                message: error.clone(),
            }),
            Self::Cancelled { .. } => TaskEvent::Finished(TaskStatus {
                phase: TaskPhase::Cancelled,
                message: "注册流程已取消".to_string(),
            }),
        }
    }
}

pub fn registration_steps_for_trial(trial_days: u32) -> Vec<RegistrationState> {
    let mut steps = vec![
        RegistrationState::InitBrowser,
        RegistrationState::OpenSignup,
        RegistrationState::FillCredentials,
        RegistrationState::WaitCaptcha,
        RegistrationState::DetectRegisteredEmail,
        RegistrationState::WaitEmailCode,
        RegistrationState::SubmitEmailCode,
        RegistrationState::OpenSubscription,
        RegistrationState::FetchAddress,
        RegistrationState::FillAddress,
        RegistrationState::SelectCard,
        RegistrationState::FillPayment,
        RegistrationState::SubmitPayment,
        RegistrationState::WaitThankYou,
        RegistrationState::OpenSubscriptionManagement,
    ];
    if trial_days >= 21 {
        steps.push(RegistrationState::AcceptExtraTrial);
    }
    steps.extend([
        RegistrationState::ChangePlanToProAnnual,
        RegistrationState::CancelSubscription,
        RegistrationState::ParseTrialExpiry,
        RegistrationState::ReturnProject,
        RegistrationState::ExtractCookie,
        RegistrationState::ExtractGitToken,
        RegistrationState::SaveAccount,
        RegistrationState::Done,
    ]);
    steps
}

pub fn registered_email_error_seen(page_text: &str) -> bool {
    page_text.contains(REGISTERED_EMAIL_ERROR_TEXT)
}

pub fn registered_email_prompt(old_email: impl Into<String>) -> RegistrationUserPrompt {
    RegistrationUserPrompt::NewRegistrationCredentials {
        old_email: old_email.into(),
    }
}

pub fn decide_registration_completion(
    input: RegistrationCompletionInput,
) -> RegistrationCompletionDecision {
    let mut missing_recoverable_artifacts = Vec::new();
    if !input.cookie_captured {
        missing_recoverable_artifacts.push(RegistrationArtifact::Cookie);
    }
    if input.git_token_required && !input.git_token_captured {
        missing_recoverable_artifacts.push(RegistrationArtifact::GitToken);
    }
    if !input.trial_expiry_captured {
        missing_recoverable_artifacts.push(RegistrationArtifact::TrialExpiry);
    }

    let artifacts_complete = input.cookie_captured
        && input.trial_expiry_captured
        && (!input.git_token_required || input.git_token_captured);
    RegistrationCompletionDecision {
        can_save_account: input.subscription_succeeded && artifacts_complete,
        status: if !input.subscription_succeeded {
            RegistrationCompletionStatus::SubscriptionMissing
        } else if artifacts_complete {
            RegistrationCompletionStatus::SaveAllowed
        } else {
            RegistrationCompletionStatus::ArtifactsMissing
        },
        missing_recoverable_artifacts,
    }
}
