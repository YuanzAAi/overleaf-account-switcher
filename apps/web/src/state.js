export const endpoints = {
  health: "/health",
  config: "/config",
  skills: "/runtime/skills",
  updates: "/runtime/updates",
  updateDefaultExportDir: "/config/default-export-dir",
  updateBrowserProxy: "/config/browser-proxy",
  dashboard: "/dashboard",
  accounts: "/accounts",
  cards: "/cards",
  addresses: "/addresses",
  currentAddress: "/addresses/current",
  tasks: "/tasks",
  tasksSummary: "/tasks/summary",
  tasksSince: (sequence) => `/tasks?since=${encodeURIComponent(sequence)}`,
  browserProfiles: "/browser/profiles",
  accountSecret: (alias, field) =>
    `/secrets/account?alias=${encodeURIComponent(alias)}&field=${encodeURIComponent(field)}`,
  importAccounts: "/accounts/import",
  importCookieAccounts: "/accounts/import/cookie",
  exportAccounts: "/accounts/export",
  exportAccountsJson: "/accounts/export/json",
  registerAccount: "/accounts/register",
  addCredentials: "/accounts/credentials",
  refreshCredentials: "/accounts/credentials/refresh",
  refreshSubscriptions: "/accounts/session/refresh",
  trialEligibility: "/accounts/trial-eligibility",
  gitTokenRefresh: "/accounts/git-token/refresh",
  gitTokenGenerate: "/accounts/git-token/generate",
  browserLogin: "/accounts/browser-login",
  switchPlan: "/accounts/switch/plan",
  switchProjectPreview: "/accounts/switch/projects/preview",
  switchExecute: "/accounts/switch/execute",
  addCard: "/cards",
  cardStatus: "/cards/status",
  removeCard: "/cards/remove",
  exportCards: "/cards/export",
  fetchAddress: "/addresses/fetch",
  addressSelect: "/addresses/select",
  addressFallback: "/addresses/fallback",
  localPassword: "/accounts/password",
  overleafPassword: "/accounts/overleaf-password",
  removeAccountPreview: "/accounts/remove/projects/preview",
  removeAccount: "/accounts/remove",
  taskCancel: "/tasks/cancel",
  taskRetry: "/tasks/retry",
  taskInput: "/tasks/input",
  clearTerminalTasks: "/tasks/clear-terminal",
  taskEvents: "/tasks/events",
  browserCurrentAccount: "/browser/current-account",
  runtimeArtifacts: "/runtime/artifacts",
  cleanupRuntimeArtifacts: "/runtime/artifacts/cleanup",
  tempProfilesPreview: "/runtime/temp-profiles/preview",
  cleanupTempProfiles: "/runtime/temp-profiles/cleanup",
};

export const MINIMUM_NEW_PASSWORD_LENGTH = 8;

export const accountMenuCloseTimers = new WeakMap();

export const statusClearTimers = new WeakMap();

export const state = {
  loading: false,
  config: null,
  serviceHealth: "unknown",
  accounts: [],
  cards: [],
  importFiles: [],
  importingFiles: false,
  selectedCardIds: new Set(),
  deletingCards: false,
  dashboard: null,
  browserProfiles: null,
  tempProfilesPreview: null,
  runtimeArtifacts: null,
  tasks: [],
  taskSummary: null,
  taskSequence: 0,
  taskFallbackInFlight: false,
  taskFallbackTimer: null,
  accountStateRefreshTimer: null,
  focusedTaskInputKeys: new Set(),
  pendingAccountOperations: new Map(),
  browserCredentialRecoveryTaskId: "",
  browserCredentialRecoveryItemId: "",
  browserCredentialRecoveryAlias: "",
  browserCredentialRecoveryMode: "",
  browserCredentialRecoveryAssistTaskId: "",
  registrationEligibleAliases: [],
  registrationEligibilityLoading: false,
  registrationEligibilityRequestVersion: 0,
  registrationTaskId: "",
  registrationTaskStatusOwner: "",
  registrationTaskStatusMessage: "",
  registrationCredentialRecoveryTaskId: "",
  registrationCredentialRecoveryKind: "",
  selectedAliases: new Set(),
  selectedAddress: null,
  addressPrefetchTimer: null,
  taskEvents: null,
  accountView: "cards",
  expandedCardBins: new Set(),
  accountSearch: "",
  accountFilter: "all",
  accountSort: "import-order",
  activePage: "accounts",
  onboardingDismissedForSession: false,
  selectedProfileName: "",
  browserCurrentAccount: null,
  browserCurrentAccountError: "",
  activeAccountTool: "",
  activeAccountToolFocusId: "",
  passwordSelectionTool: "",
  runtimeLogCollapsed: false,
  runtimeLogStickToBottom: true,
  confirmDialogResolver: null,
  accountDeleteAliases: [],
  clientRuntimeLogs: [],
  nextClientRuntimeLogId: 1,
  toasts: [],
  toastTimers: new Map(),
  nextToastId: 1,
};

export const ACCOUNT_EXPORT_LAYOUT_MODES = ["single_file", "multiple_files"];

export const ONBOARDING_STORAGE_KEY = "overleaf-switcher.extension-guide.dismissed";

export const PROFILE_STORAGE_KEY = "overleaf-switcher.chrome-profile.selected";

export const RUNTIME_LOG_COLLAPSED_KEY = "overleaf-switcher.runtime-log.collapsed";

export const UI_THEME_STORAGE_KEY = "overleaf-switcher.ui-theme";

export const CLIENT_RUNTIME_LOG_LIMIT = 50;

export const ADDRESS_HISTORY_DISPLAY_LIMIT = 1;

export const accountStateRefreshOperations = new Set([
  "account_import",
  "account_manual_cookie_import",
  "account_credentials_add",
  "account_credentials_refresh",
  "account_registration",
  "account_git_token_refresh",
  "account_git_token_generate",
  "browser_login",
  "account_remove",
  "local_password_update",
  "overleaf_password_change",
  "account_switch_execute",
]);

export const terminalTaskPhases = new Set(["completed", "failed", "cancelled"]);

export function selectedAliasesInPickOrder() {
  return Array.from(state.selectedAliases);
}

export function selectedAliasesText() {
  return selectedAliasesInPickOrder().join(",");
}

export const accountActionByOperationKind = Object.freeze({
  account_credentials_refresh: "refresh-session",
  account_git_token_refresh: "refresh-git",
  account_git_token_generate: "generate-git",
  browser_login: "browser-login",
  account_switch_plan: "switch-plan",
  account_switch_execute: "switch-execute",
});
