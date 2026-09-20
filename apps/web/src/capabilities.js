import { formatTaskKey } from "./tasks/results.js";
import { ACCOUNT_EXPORT_LAYOUT_MODES, state } from "./state.js";
import { escapeAttr, escapeHtml } from "./format.js";

import { extensionBridgeConnected, serviceHealthInfo } from "./settings.js";
import { syncAccountTaskLocks } from "./accounts/list.js";

export function accountRowActions(capabilities = null) {
  return capabilityChoices(
    "account_row_actions",
    ["refresh-session", "refresh-git", "generate-git", "browser-login", "switch-plan", "switch-execute"],
    capabilities,
  );
}

export function accountBulkActions(capabilities = null) {
  return capabilityChoices(
    "account_bulk_actions",
    ["refresh-session", "refresh-git", "generate-git", "browser-login"],
    capabilities,
  );
}

export function accountSwitchActions(capabilities = null) {
  return capabilityChoices("account_switch_actions", ["plan", "preview-projects", "execute"], capabilities);
}

export function accountRemovalActions(capabilities = null) {
  return capabilityChoices(
    "account_removal_actions",
    ["preview-remote-cleanup", "remove-local", "cleanup-remote-and-remove"],
    capabilities,
  );
}

export function accountCredentialActions(capabilities = null) {
  return capabilityChoices(
    "account_credential_actions",
    ["add-with-password", "refresh-cookie-with-password"],
    capabilities,
  );
}

export function accountPasswordActions(capabilities = null) {
  return capabilityChoices(
    "account_password_actions",
    ["update-local-password", "change-overleaf-password"],
    capabilities,
  );
}

export function accountSecretActions(capabilities = null) {
  return capabilityChoices("account_secret_actions", ["copy-account-secret"], capabilities);
}

export function accountImportSources(capabilities = null) {
  return capabilityChoices("account_import_sources", ["json_path", "json_text", "manual_cookie"], capabilities);
}

export function accountImportPostActions(capabilities = null) {
  return capabilityChoices(
    "account_import_post_actions",
    ["refresh_session_metadata", "fetch_git_token"],
    capabilities,
  );
}

export function accountExportModes(capabilities = null) {
  return capabilityChoices(
    "account_export_modes",
    ["single_file", "multiple_files", "browser_download_json"],
    capabilities,
  );
}

export function accountExportLayoutModes(capabilities = null) {
  const source = capabilities || (state.config && state.config.api_capabilities);
  if (source && Array.isArray(source.account_export_layout_modes)) {
    return source.account_export_layout_modes.filter((mode) => ACCOUNT_EXPORT_LAYOUT_MODES.includes(mode));
  }
  const modes = accountExportModes(source);
  const serverLayouts = modes.filter((mode) => ACCOUNT_EXPORT_LAYOUT_MODES.includes(mode));
  if (serverLayouts.length > 0) {
    return serverLayouts;
  }
  return modes.includes("browser_download_json") ? ACCOUNT_EXPORT_LAYOUT_MODES : [];
}

export function accountExportSafety(capabilities = null) {
  return capabilityChoices(
    "account_export_safety",
    ["main_config_overwrite_block", "existing_export_overwrite_confirmation", "sensitive_export_warning"],
    capabilities,
  );
}

export function browserProfileActions(capabilities = null) {
  return capabilityChoices(
    "browser_profile_actions",
    ["discover-profiles", "detect-current-account"],
    capabilities,
  );
}

export function runtimeDiagnosticActions(capabilities = null) {
  return capabilityChoices(
    "runtime_diagnostics",
    ["browser_profiles", "runtime_artifacts", "runtime_temp_profiles"],
    capabilities,
  );
}

export function browserProfileDiscoverySupported(capabilities = null) {
  return (
    browserProfileActions(capabilities).includes("discover-profiles") &&
    runtimeDiagnosticActions(capabilities).includes("browser_profiles")
  );
}

export function tempProfilePreviewSupported(capabilities = null) {
  return runtimeDiagnosticActions(capabilities).includes("runtime_temp_profiles");
}

export function runtimeCleanupActions(capabilities = null) {
  return capabilityChoices(
    "runtime_cleanup_actions",
    ["runtime_artifacts_cleanup", "runtime_temp_profiles_cleanup"],
    capabilities,
  );
}

export function secretStorageActions(capabilities = null) {
  return capabilityChoices("secret_storage_actions", ["status"], capabilities);
}

export function taskListActions(capabilities = null) {
  return capabilityChoices("task_list_actions", ["clear-terminal"], capabilities);
}

export function taskPhases(capabilities = null) {
  return capabilityChoices(
    "task_phases",
    ["pending", "running", "waiting_for_user", "completed", "failed", "cancelled"],
    capabilities,
  );
}

export function taskActivePhases(capabilities = null) {
  const defaults = ["pending", "running", "waiting_for_user"];
  const source = capabilities || (state.config && state.config.api_capabilities);
  return capabilityActionList(source, "task_active_phases", defaults, taskPhases(source));
}

export function taskTerminalPhases(capabilities = null) {
  const defaults = ["completed", "failed", "cancelled"];
  const source = capabilities || (state.config && state.config.api_capabilities);
  return capabilityActionList(source, "task_terminal_phases", defaults, taskPhases(source));
}

export function taskRecoverableFailureKinds(capabilities = null) {
  return capabilityChoices(
    "task_recoverable_failure_kinds",
    ["retryable", "needs_user_input", "needs_login", "page_changed"],
    capabilities,
  );
}

export function taskResultSectionKinds(capabilities = null) {
  return capabilityChoices(
    "task_result_sections",
    [
      "summary_chips",
      "import_decisions",
      "project_migration",
      "top_level_items",
      "nested_items",
      "post_action_errors",
    ],
    capabilities,
  );
}

export function taskResultSummarySourceKinds(capabilities = null) {
  return capabilityChoices(
    "task_result_summary_sources",
    ["root", "import", "session_refresh", "git_token_generation", "project_migration"],
    capabilities,
  );
}

export function taskResultSensitiveKeyRules(capabilities = null) {
  const defaults = [
    { matcher: "equals", value: "cookie" },
    { matcher: "ends_with", value: "_cookie" },
    { matcher: "contains", value: "password" },
    { matcher: "equals", value: "session" },
    { matcher: "ends_with", value: "_session" },
    { matcher: "equals", value: "secret" },
    { matcher: "ends_with", value: "_secret" },
    { matcher: "equals", value: "token" },
    { matcher: "ends_with", value: "_token" },
    { matcher: "equals", value: "cvc" },
    { matcher: "equals", value: "number" },
    { matcher: "equals", value: "card_number" },
    { matcher: "equals", value: "number_or_suffix" },
  ];
  const source = capabilities || (state.config && state.config.api_capabilities);
  const raw =
    source && Array.isArray(source.task_result_sensitive_key_rules)
      ? source.task_result_sensitive_key_rules
      : defaults;
  const allowedMatchers = new Set(["equals", "ends_with", "contains"]);
  return raw.filter(
    (rule) =>
      rule && allowedMatchers.has(rule.matcher) && typeof rule.value === "string" && rule.value.length > 0,
  );
}

export function taskResultSensitiveValueMarkers(capabilities = null) {
  return capabilityChoices(
    "task_result_sensitive_value_markers",
    [
      "git_token_prefix",
      "overleaf_session_cookie_assignment",
      "encoded_overleaf_session_prefix",
      "raw_overleaf_session_prefix",
    ],
    capabilities,
  );
}

export function taskUserInputKinds(capabilities = null) {
  return capabilityChoices(
    "task_user_input_kinds",
    ["email_code", "new_registration_credentials", "new_browser_credentials", "captcha_completed"],
    capabilities,
  );
}

export function taskAvailableActionKinds(capabilities = null) {
  return capabilityChoices(
    "task_available_actions",
    [
      "cancel",
      "retry",
      "submit_email_code",
      "submit_new_registration_credentials",
      "submit_new_browser_credentials",
      "mark_captcha_completed",
    ],
    capabilities,
  );
}

export function taskInputActionSpecs(capabilities = null) {
  const defaults = [
    { action: "submit_email_code", input_kind: "email_code" },
    { action: "submit_new_registration_credentials", input_kind: "new_registration_credentials" },
    { action: "submit_new_browser_credentials", input_kind: "new_browser_credentials" },
    { action: "mark_captcha_completed", input_kind: "captcha_completed" },
  ];
  const source = capabilities || (state.config && state.config.api_capabilities);
  const raw = source && Array.isArray(source.task_input_action_map) ? source.task_input_action_map : defaults;
  const allowedActions = new Set(taskAvailableActionKinds(source));
  const allowedInputs = new Set(taskUserInputKinds(source));
  return raw.filter(
    (spec) =>
      spec &&
      typeof spec.action === "string" &&
      typeof spec.input_kind === "string" &&
      allowedActions.has(spec.action) &&
      allowedInputs.has(spec.input_kind),
  );
}

export function taskInputActionForKind(inputKind, capabilities = null) {
  const spec = taskInputActionSpecs(capabilities).find((item) => item.input_kind === inputKind);
  return spec ? spec.action : "";
}

export function taskEventsSupported(capabilities = null) {
  const source = capabilities || (state.config && state.config.api_capabilities);
  return !source || source.task_events_sse !== false;
}

export function taskSinceCursorSupported(capabilities = null) {
  const source = capabilities || (state.config && state.config.api_capabilities);
  return !source || source.task_since_cursor !== false;
}

export function taskSummarySupported(capabilities = null) {
  const source = capabilities || (state.config && state.config.api_capabilities);
  return !source || source.task_summary !== false;
}

export function registrationActions(capabilities = null) {
  return capabilityChoices("registration_actions", ["start", "auto-fetch-git-token"], capabilities);
}

export function capabilityActionList(capabilities, field, defaults, allowed) {
  if (capabilities && Array.isArray(capabilities[field])) {
    return capabilities[field].filter((action) => allowed.includes(action));
  }
  return defaults;
}

export function cardMutationActions(capabilities = null) {
  return capabilityChoices("card_mutation_actions", ["used", "failed", "remove"], capabilities);
}

export function cardCollectionActions(capabilities = null) {
  return capabilityChoices("card_collection_actions", ["add", "export"], capabilities);
}

export function cardActionButtons(card) {
  const key = escapeAttr(card.id || card.last_four || "");
  return cardMutationActions()
    .map((action) => {
      const dangerClass = action === "remove" ? " ui-btn-danger danger-action" : "";
      const label = cardMutationActionLabel(action);
      const text = { used: "已用", failed: "失败" }[action] || label;
      return `<button class="inline-action ui-btn ui-btn-sm${dangerClass}" type="button" title="${escapeAttr(label)}" aria-label="${escapeAttr(label)}" data-card-action="${escapeAttr(action)}" data-card-key="${key}" ${state.deletingCards ? "disabled" : ""}>${escapeHtml(text)}</button>`;
    })
    .join("");
}

export function cardMutationActionLabel(action) {
  const labels = {
    used: "标记已用",
    failed: "标记失败",
    remove: "删除",
  };
  return labels[action] || formatTaskKey(action);
}

export function addressSelectionActions(capabilities = null) {
  return capabilityChoices("address_selection_actions", ["select", "fallback"], capabilities);
}

export function addressCollectionActions(capabilities = null) {
  return capabilityChoices("address_collection_actions", ["fetch"], capabilities);
}

export function addressActionButtons(address) {
  if (!addressSelectionActions().includes("select")) return "";
  return `<button class="inline-action ui-btn ui-btn-sm" type="button" data-address-action="select" data-address-index="${escapeAttr(address.index)}">选择</button>`;
}

export function renderRegistrationCapabilityOptions(capabilities) {
  if (!capabilities) return;
  const actions = registrationActions(capabilities);

  const trialDays = Array.isArray(capabilities.registration_trial_days)
    ? capabilities.registration_trial_days
        .map((value) => Number(value))
        .filter((value) => Number.isInteger(value) && value > 0)
    : [];
  renderSelectOptions("registration-trial-days", trialDays, (value) => `${value} 天`);

  const cardStrategies = Array.isArray(capabilities.registration_card_selection_strategies)
    ? capabilities.registration_card_selection_strategies.filter(
        (value) => typeof value === "string" && value.trim(),
      )
    : [];
  renderSelectOptions("registration-card-strategy", cardStrategies, registrationCardStrategyLabel);
  setButtonCapability("registration-submit", actions.includes("start"));
  setCheckboxCapability("registration-git-token", actions.includes("auto-fetch-git-token"));
}

export function renderAccountCapabilityControls(capabilities) {
  const actions = accountBulkActions(capabilities);
  document.querySelectorAll("[data-account-bulk-action]").forEach((button) => {
    const action = button.dataset.accountBulkAction;
    if (!action || action === "clear-selection") return;
    const supported = actions.includes(action);
    const needsBridge = accountBulkActionNeedsBridge(action);
    const bridgeUnavailable = needsBridge && !extensionBridgeConnected();
    button.hidden = !supported;
    button.disabled = !supported || bridgeUnavailable;
    button.classList.toggle("bridge-required", bridgeUnavailable);
    button.title = bridgeUnavailable
      ? "打开浏览器需要 Chrome 扩展桥。账号查看、导入导出和本地维护仍可继续。"
      : accountBulkActionTitle(action);
  });
  syncAccountTaskLocks();
}

export function accountBulkActionNeedsBridge(action) {
  return action === "browser-login";
}

export function accountBulkActionTitle(action) {
  return (
    {
      "refresh-session": "批量刷新 Cookie",
      "refresh-git": "批量刷新 Git 令牌状态",
      "generate-git": "批量获取 Git 令牌",
      "browser-login": "为选中账号打开浏览器登录窗口",
    }[action] || ""
  );
}

export function renderAccountSwitchCapabilityControls(capabilities) {
  const actions = accountSwitchActions(capabilities);
  setButtonCapability("switch-submit", actions.includes("plan"));
  setButtonCapability("switch-project-preview", actions.includes("preview-projects"));
  setButtonCapability("switch-execute", actions.includes("execute"));
  const executeButton = document.getElementById("switch-execute");
  if (executeButton && actions.includes("execute") && !extensionBridgeConnected()) {
    executeButton.hidden = false;
    executeButton.disabled = true;
    executeButton.title = "Chrome 扩展桥连接后可用。";
    const status = document.getElementById("switch-status");
    if (status && !status.textContent) {
      status.textContent = "执行换号需要 Chrome 扩展桥。";
    }
  } else if (executeButton) {
    executeButton.title = "";
  }
}

export function renderAccountRemovalCapabilityControls(capabilities) {
  const actions = accountRemovalActions(capabilities);
  const canRemoveLocal = actions.includes("remove-local");
  const canCleanupRemote = actions.includes("cleanup-remote-and-remove");
  const cleanupInput = document.getElementById("remove-cleanup");
  const confirmInput = document.getElementById("remove-confirm");
  const toolbarDelete = document.getElementById("account-toolbar-delete");

  setButtonCapability("remove-preview", actions.includes("preview-remote-cleanup"));
  setButtonCapability("remove-submit", canRemoveLocal || canCleanupRemote);
  if (toolbarDelete) {
    toolbarDelete.disabled = !(canRemoveLocal || canCleanupRemote);
    toolbarDelete.title = toolbarDelete.disabled ? "当前后端不支持删除账号" : "删除选中账号";
  }

  if (cleanupInput) {
    cleanupInput.disabled = !canCleanupRemote;
    if (!canCleanupRemote) cleanupInput.checked = false;
    if (!canRemoveLocal && canCleanupRemote) cleanupInput.checked = true;
  }
  if (confirmInput) {
    confirmInput.disabled = !canRemoveLocal;
    if (!canRemoveLocal) confirmInput.checked = false;
  }
}

export function renderAccountCredentialCapabilityControls(capabilities) {
  const actions = accountCredentialActions(capabilities);
  setButtonCapability("credential-add-submit", actions.includes("add-with-password"));
  setButtonCapability("credential-refresh-submit", actions.includes("refresh-cookie-with-password"));
}

export function renderAccountPasswordCapabilityControls(capabilities) {
  const actions = accountPasswordActions(capabilities);
  setButtonCapability("password-submit", actions.includes("update-local-password"));
  setButtonCapability("overleaf-password-submit", actions.includes("change-overleaf-password"));
  document.querySelectorAll('[data-password-batch-open="passwords"]').forEach((button) => {
    button.hidden = !actions.includes("update-local-password");
    button.disabled = !actions.includes("update-local-password");
  });
  document.querySelectorAll('[data-password-batch-open="overleaf-password"]').forEach((button) => {
    button.hidden = !actions.includes("change-overleaf-password");
    button.disabled = !actions.includes("change-overleaf-password");
  });
}

export function renderAccountIoCapabilityControls(capabilities) {
  const sources = accountImportSources(capabilities);
  const postActions = accountImportPostActions(capabilities);
  const exportModes = accountExportModes(capabilities);
  const exportLayoutModes = accountExportLayoutModes(capabilities);
  const exportSafety = accountExportSafety(capabilities);
  const jsonImportSupported = sources.includes("json_path") || sources.includes("json_text");
  const manualCookieSupported = sources.includes("manual_cookie");
  const serverExportSupported = exportModes.some((mode) => ACCOUNT_EXPORT_LAYOUT_MODES.includes(mode));
  const browserDownloadSupported = exportModes.includes("browser_download_json");

  setInputCapability("import-web-files", sources.includes("json_text"));
  setButtonCapability("import-choose-file", jsonImportSupported);
  setButtonCapability("import-submit", jsonImportSupported);
  if (state.importingFiles) document.getElementById("import-submit").disabled = true;
  setInputCapability("manual-cookie-entries", manualCookieSupported);
  setButtonCapability("manual-cookie-submit", manualCookieSupported);
  setCheckboxCapability("import-refresh", postActions.includes("refresh_session_metadata"));
  setCheckboxCapability("import-git-token", postActions.includes("fetch_git_token"));
  setCheckboxCapability("manual-cookie-refresh", postActions.includes("refresh_session_metadata"));
  setCheckboxCapability("manual-cookie-git-token", postActions.includes("fetch_git_token"));
  renderSelectOptions("export-mode", exportLayoutModes, exportModeLabel, { clearWhenEmpty: true });
  setInputCapability("export-mode", exportLayoutModes.length > 0);
  setInputCapability("export-output-dir", serverExportSupported);
  setButtonCapability("export-choose-dir", serverExportSupported);
  setButtonCapability("export-submit", serverExportSupported);
  setButtonCapability("export-download", browserDownloadSupported && exportLayoutModes.length > 0);
  setButtonCapability("account-toolbar-export", serverExportSupported || browserDownloadSupported);
  setCheckboxCapability(
    "export-confirm-overwrite",
    exportSafety.includes("existing_export_overwrite_confirmation"),
  );
  setElementCapability("export-sensitive-warning", exportSafety.includes("sensitive_export_warning"));
}

export function renderBrowserProfileCapabilityControls(capabilities) {
  const actions = browserProfileActions(capabilities);
  setButtonCapability("detect-browser-account", actions.includes("detect-current-account"));
  const button = document.getElementById("detect-browser-account");
  if (button && actions.includes("detect-current-account") && !extensionBridgeConnected()) {
    button.hidden = false;
    button.disabled = true;
    button.title = "Chrome 扩展桥连接后可用。";
    const status = document.getElementById("browser-account-status");
    if (status && !status.textContent) {
      status.textContent = "需要扩展桥";
    }
  } else if (button) {
    button.title = "";
  }
}

export function renderRuntimeCapabilityControls(capabilities) {
  const diagnostics = runtimeDiagnosticActions(capabilities);
  const cleanup = runtimeCleanupActions(capabilities);
  setButtonCapability("scan-artifacts", diagnostics.includes("runtime_artifacts"));
  setButtonCapability("cleanup-artifacts", cleanup.includes("runtime_artifacts_cleanup"));
  setButtonCapability("cleanup-temp-profiles", cleanup.includes("runtime_temp_profiles_cleanup"));
}

export function serviceApiRoutes(capabilities = null) {
  const source = capabilities || (state.config && state.config.api_capabilities);
  if (!source || !Array.isArray(source.service_api_routes)) {
    return [];
  }
  return source.service_api_routes
    .map((route) => ({
      path: typeof route.path === "string" ? route.path.trim() : "",
      methods: Array.isArray(route.methods)
        ? route.methods.filter((method) => typeof method === "string" && method.trim())
        : [],
    }))
    .filter((route) => route.path.startsWith("/") && route.methods.length > 0);
}

export function renderServiceApiRoutes(capabilities) {
  const list = document.getElementById("service-api-routes-list");
  if (!list) return;
  const routes = serviceApiRoutes(capabilities);
  if (!routes.length) {
    list.innerHTML = `<div class="empty">本地服务 API 路由目录不可用。</div>`;
    return;
  }

  const service = serviceHealthInfo();
  list.innerHTML = `<div class="config-row"><strong>服务接口</strong><span class="pill ${service.tone === "ready" ? "ready" : "pending"}">${service.label}</span></div>`;
}

export function renderTaskListCapabilityControls(capabilities) {
  setButtonCapability("clear-terminal-tasks", taskListActions(capabilities).includes("clear-terminal"));
}

export function renderCardCapabilityControls(capabilities = null) {
  setButtonCapability("card-submit", cardCollectionActions(capabilities).includes("add"));
  syncCardSelectionControls(capabilities);
}

export function syncCardSelectionControls(capabilities = null) {
  setButtonCapability("card-export", cardCollectionActions(capabilities).includes("export"));
  setButtonCapability("card-delete", cardMutationActions(capabilities).includes("remove"));
  for (const id of ["card-export", "card-delete"]) {
    document.getElementById(id).disabled ||= state.deletingCards || state.selectedCardIds.size === 0;
  }
}

export function setButtonCapability(buttonId, supported) {
  const button = document.getElementById(buttonId);
  if (!button) return;
  button.hidden = !supported;
  button.disabled = !supported;
}

export function setInputCapability(inputId, supported) {
  const input = document.getElementById(inputId);
  if (!input) return;
  input.disabled = !supported;
  if (!supported) {
    input.value = "";
  }
}

export function setCheckboxCapability(inputId, supported) {
  const input = document.getElementById(inputId);
  if (!input) return;
  input.disabled = !supported;
  if (!supported) {
    input.checked = false;
  }
}

export function setElementCapability(elementId, supported) {
  const element = document.getElementById(elementId);
  if (!element) return;
  element.hidden = !supported;
}

export function renderAddressCapabilityControls(capabilities) {
  const fallbackButton = document.getElementById("address-fallback");
  const selectionActions = addressSelectionActions(capabilities);
  setButtonCapability("address-submit", addressCollectionActions(capabilities).includes("fetch"));
  if (fallbackButton) {
    fallbackButton.disabled = !selectionActions.includes("fallback");
  }
}

export function renderSelectOptions(selectId, values, labelForValue, options = {}) {
  const select = document.getElementById(selectId);
  if (!select) return;
  if (!Array.isArray(values) || values.length === 0) {
    if (options.clearWhenEmpty) {
      select.innerHTML = "";
      select.value = "";
    }
    return;
  }

  const current = select.value;
  select.innerHTML = values
    .map((value) => {
      const text = typeof labelForValue === "function" ? labelForValue(value) : String(value);
      return `<option value="${escapeHtml(value)}">${escapeHtml(text)}</option>`;
    })
    .join("");
  if (values.map(String).includes(current)) {
    select.value = current;
  }
}

export function registrationCardStrategyLabel(strategy) {
  const labels = {
    new_then_used_then_failed: "新卡优先，其次已用/失败卡",
    new_only: "只使用新卡",
  };
  return labels[strategy] || formatTaskKey(strategy);
}

export function exportModeLabel(mode) {
  const labels = {
    single_file: "单个 accounts.json",
    multiple_files: "每个账号一个文件",
  };
  return labels[mode] || formatTaskKey(mode);
}

function capabilityChoices(field, defaults, capabilities) {
  return capabilityActionList(capabilities || state.config?.api_capabilities, field, defaults, defaults);
}
