import { isPlainObject } from "./tasks/results.js";
import { fetchJson, fetchOptionalJson, request } from "./http.js";
import {
  recordClientRuntimeError,
  reportUiOperationFailure,
  setAccountAssistShortStatus,
  showToast,
} from "./feedback.js";
import { accountStateRefreshOperations, endpoints, state, terminalTaskPhases } from "./state.js";
import {
  renderAccounts,
  renderAccountsError,
  renderAccountsLoading,
  syncAccountTaskLocks,
} from "./accounts/list.js";
import {
  extensionBridgeConnected,
  renderBrowserProfiles,
  renderConfig,
  renderError,
  renderExtensionStatus,
  renderOnboarding,
  renderRuntimeHealthStrip,
  renderTempProfilePreview,
  renderTopProfile,
  serviceHealthInfo,
} from "./settings.js";
import {
  browserProfileActions,
  browserProfileDiscoverySupported,
  renderServiceApiRoutes,
  taskActivePhases,
  taskEventsSupported,
  taskRecoverableFailureKinds,
  taskSinceCursorSupported,
  taskSummarySupported,
  taskTerminalPhases,
  tempProfilePreviewSupported,
} from "./capabilities.js";
import {
  addressSummaryFromSelection,
  renderAddresses,
  renderCards,
  syncCurrentAddressDisplay,
} from "./resources.js";
import {
  isActiveTaskSnapshot,
  makeTaskId,
  registerPendingAccountOperation,
  renderDashboard,
  renderRuntimeLogSummary,
  renderTasks,
  settlePendingAccountOperations,
  syncTaskInputWindowFocus,
  taskHasAction,
} from "./tasks/list.js";
import {
  closeAccountWorkbench,
  currentAccountAssistMode,
  openAccountTool,
  setActivePage,
} from "./workspace.js";
import { setRegistrationStatus, syncRegistrationSource } from "./registration.js";
import { statusText } from "./format.js";

export async function refresh({ detectBrowser = true, refreshSubscriptions = false } = {}) {
  if (state.loading) return;
  cancelScheduledAccountStateRefresh();
  state.loading = true;
  setRefreshEnabled(false);
  if (!state.accounts.length) {
    renderAccountsLoading();
  }

  try {
    const health = await checkServiceHealth();
    if (!health || health.status !== "ok") throw new Error("本地服务未响应，请稍后刷新");
    const config = state.config;
    ensureTaskEventConnection(config && config.api_capabilities);
    renderExtensionStatus(config);
    renderOnboarding(config);
    if (detectBrowser) await detectCurrentBrowserAccount({ silent: true });
    const [
      dashboard,
      accounts,
      cards,
      addresses,
      currentAddress,
      tasks,
      taskSummary,
      browserProfiles,
      tempProfilesPreview,
    ] = await Promise.all([
      fetchJson(endpoints.dashboard),
      fetchJson(endpoints.accounts),
      fetchJson(endpoints.cards),
      fetchJson(endpoints.addresses),
      fetchOptionalJson(endpoints.currentAddress),
      fetchJson(endpoints.tasks),
      taskSummarySupported(config && config.api_capabilities)
        ? fetchOptionalJson(endpoints.tasksSummary)
        : Promise.resolve(null),
      browserProfileDiscoverySupported(config && config.api_capabilities)
        ? fetchOptionalJson(endpoints.browserProfiles)
        : Promise.resolve(null),
      tempProfilePreviewSupported(config && config.api_capabilities)
        ? fetchOptionalJson(endpoints.tempProfilesPreview)
        : Promise.resolve(null),
    ]);

    state.dashboard = dashboard;
    state.browserProfiles = browserProfiles;
    state.tempProfilesPreview = tempProfilesPreview;
    state.accounts = Array.isArray(accounts) ? accounts : [];
    state.selectedAddress = addressSummaryFromSelection(currentAddress);
    renderConfig(config);
    renderTopProfile(browserProfiles);
    renderBrowserProfiles(browserProfiles);
    renderTempProfilePreview(tempProfilesPreview);
    updateTasks(tasks, { updateDashboard: false, suppressAccountRefresh: true });
    state.taskSummary = taskSummary || taskSummaryFromSnapshots(state.tasks);
    renderDashboard(dashboard, state.taskSummary);
    renderRuntimeLogSummary(state.taskSummary);
    renderAccounts(state.accounts);
    renderCards(cards);
    renderAddresses(addresses);
    syncCurrentAddressDisplay(currentAddress);
    document.getElementById("last-updated").textContent = new Date().toLocaleString();
    if (refreshSubscriptions) {
      const aliases = state.accounts.filter((account) => account.cookie?.present).map((account) => account.alias).join(",");
      if (aliases) {
        await postTaskResult(endpoints.refreshSubscriptions, { aliases, task_id: makeTaskId("subscription-refresh", aliases) });
        [state.accounts, state.dashboard] = await Promise.all([
          fetchJson(endpoints.accounts), fetchJson(endpoints.dashboard),
        ]);
        renderAccounts(state.accounts);
        renderDashboard(state.dashboard, state.taskSummary);
      }
    }
  } catch (error) {
    renderError(error);
    renderAccountsError(error);
    recordClientRuntimeError({
      scope: "工作台刷新",
      route: error.route || "refresh",
      error,
    });
    showToast({
      tone: "error",
      title: "刷新失败",
      message: "详细原因已写入运行日志。",
    });
  } finally {
    state.loading = false;
    setRefreshEnabled(true);
  }
}

export function ensureTaskEventConnection(capabilities = null) {
  const status = document.getElementById("task-stream-status");
  if (!taskEventsSupported(capabilities)) {
    if (state.taskEvents) {
      state.taskEvents.close();
      state.taskEvents = null;
    }
    status.textContent = "实时流不可用";
    scheduleTaskFallbackSync();
    return;
  }
  if (!state.taskEvents) {
    connectTaskEvents();
  }
}

export function connectTaskEvents() {
  const status = document.getElementById("task-stream-status");
  if (!taskEventsSupported() || !("EventSource" in window)) {
    status.textContent = "实时流不可用";
    fetchTaskUpdates().catch((error) => {
      status.textContent = "轮询失败，请查看运行日志";
      recordClientRuntimeError({ scope: "任务轮询", route: endpoints.tasks, error });
    });
    return;
  }

  if (state.taskEvents) {
    state.taskEvents.close();
  }

  const events = new EventSource(endpoints.taskEvents);
  state.taskEvents = events;

  events.addEventListener("open", () => {
    cancelScheduledTaskFallbackSync();
    status.textContent = "live";
  });

  events.addEventListener("tasks", (event) => {
    try {
      const tasks = JSON.parse(event.data);
      updateTasks(tasks);
      updateTaskSequenceFromEventId(event.lastEventId);
      status.textContent = event.lastEventId ? `live #${event.lastEventId}` : "live";
    } catch (error) {
      status.textContent = "实时流解析失败";
      recordClientRuntimeError({
        scope: "任务实时流解析",
        route: endpoints.taskEvents,
        error,
      });
    }
  });

  events.addEventListener("error", () => {
    status.textContent = "reconnecting";
    scheduleTaskFallbackSync();
  });
}

export function updateTasks(tasks, options = {}) {
  const snapshots = Array.isArray(tasks) ? tasks : [];
  const previousTasks = state.tasks;
  state.tasks = options.incremental ? mergeTaskSnapshots(state.tasks, snapshots) : snapshots;
  state.taskSummary = taskSummaryFromSnapshots(state.tasks);
  state.taskSequence = Math.max(
    state.taskSequence,
    latestTaskSequence(state.tasks),
    latestTaskSequence(snapshots),
  );
  syncRegistrationTaskStatus(state.tasks);
  syncBrowserCredentialRecoveryTask(state.tasks);
  renderTasks(state.tasks);
  syncTaskInputWindowFocus(state.tasks);
  syncAccountTaskLocks();
  const synthesizedTerminalTasks = settlePendingAccountOperations(state.tasks, previousTasks);
  if (options.updateDashboard !== false && state.dashboard) {
    renderDashboard(state.dashboard, state.taskSummary);
  }
  if (!options.suppressAccountRefresh) {
    scheduleAccountStateRefresh(previousTasks, state.tasks, synthesizedTerminalTasks);
  }
}

export function taskChangesAccountState(task) {
  return Boolean(task && accountStateRefreshOperations.has(String(task.operation_kind || "")));
}

export function scheduleAccountStateRefresh(previousTasks, nextTasks, additionalTerminalTasks = []) {
  const previousById = new Map(
    (Array.isArray(previousTasks) ? previousTasks : [])
      .filter((task) => task && task.task_id)
      .map((task) => [task.task_id, String(task.phase || "").toLowerCase()]),
  );
  const terminalCandidates = [
    ...(Array.isArray(nextTasks) ? nextTasks : []),
    ...(Array.isArray(additionalTerminalTasks) ? additionalTerminalTasks : []),
  ];
  const reachedTerminalState = terminalCandidates.some((task) => {
    if (!taskChangesAccountState(task)) return false;
    const phase = String(task.phase || "").toLowerCase();
    if (!terminalTaskPhases.has(phase)) return false;
    return !terminalTaskPhases.has(previousById.get(task.task_id));
  });
  if (!reachedTerminalState || state.accountStateRefreshTimer) return;

  const run = async () => {
    state.accountStateRefreshTimer = null;
    if (state.loading) {
      state.accountStateRefreshTimer = window.setTimeout(run, 250);
      return;
    }
    try {
      await refresh();
    } catch {
      // 刷新已报告错误，保留任务流连接。
    }
  };
  state.accountStateRefreshTimer = window.setTimeout(run, 180);
}

export function cancelScheduledAccountStateRefresh() {
  if (!state.accountStateRefreshTimer) return;
  window.clearTimeout(state.accountStateRefreshTimer);
  state.accountStateRefreshTimer = null;
}

export function registrationTaskStatusInfo(task) {
  const phase = String((task && task.phase) || "").toLowerCase();
  const waitingItems = Array.isArray(task && task.waiting_items) ? task.waiting_items : [];
  const waitingKind = String(
    (task && task.waiting_for_input) || (waitingItems.find((item) => item && item.kind) || {}).kind || "",
  );
  if (phase === "waiting_for_user") {
    const message =
      {
        captcha_completed: "等待完成 reCAPTCHA",
        email_code: "等待邮箱验证码",
        new_registration_credentials: "请重新填写注册信息",
        new_browser_credentials: "请重新输入账号密码",
      }[waitingKind] || "等待用户操作";
    return { message, tone: "warning", clearAfter: 0, terminal: false };
  }
  if (phase === "completed") {
    return { message: "试用任务完成", tone: "success", clearAfter: 4600, terminal: true };
  }
    if (phase === "failed") {
    return { message: "试用任务失败，请查看运行日志", tone: "error", clearAfter: 7200, terminal: true };
  }
  if (phase === "cancelled") {
    return { message: "试用任务已取消", tone: "warning", clearAfter: 4600, terminal: true };
  }
  return { message: "试用任务进行中", tone: "info", clearAfter: 0, terminal: false };
}

export function syncRegistrationTaskStatus(tasks) {
  const taskList = (Array.isArray(tasks) ? tasks : []).filter(
    (task) => task && task.operation_kind === "account_registration",
  );
  const trackedTaskId = String(state.registrationTaskId || "");
  let task = trackedTaskId ? taskList.find((item) => item.task_id === trackedTaskId) || null : null;
  if (!task) {
    task = taskList
      .filter((item) => taskActivePhases().includes(String(item.phase || "").toLowerCase()))
      .reduce(
        (latest, item) =>
          !latest || Number(item.last_sequence || 0) >= Number(latest.last_sequence || 0) ? item : latest,
        null,
      );
    if (task) {
      if (trackedTaskId && trackedTaskId !== String(task.task_id || "")) {
        clearRegistrationTaskStatusIfOwned(trackedTaskId);
      }
      state.registrationTaskId = String(task.task_id || "");
    } else if (trackedTaskId) {
      clearRegistrationTaskStatusIfOwned(trackedTaskId);
      state.registrationTaskId = "";
      return;
    } else {
      return;
    }
  }

  const info = registrationTaskStatusInfo(task);
  const visibleMessage = setRegistrationStatus(info.message, info.tone, info.clearAfter);
  if (info.terminal) {
    state.registrationTaskId = "";
    state.registrationTaskStatusOwner = "";
    state.registrationTaskStatusMessage = "";
  } else {
    state.registrationTaskStatusOwner = String(task.task_id || "");
    state.registrationTaskStatusMessage = visibleMessage;
  }
}

export function clearRegistrationTaskStatusIfOwned(taskId) {
  if (!taskId || state.registrationTaskStatusOwner !== taskId) return;
  const status = document.getElementById("registration-status");
  if (
    status &&
    (!state.registrationTaskStatusMessage || status.textContent === state.registrationTaskStatusMessage)
  ) {
    setRegistrationStatus("", "info");
  }
  state.registrationTaskStatusOwner = "";
  state.registrationTaskStatusMessage = "";
}

export function clearBrowserCredentialRecoveryState({ closeOwnedAssist = false } = {}) {
  const taskId = state.browserCredentialRecoveryTaskId;
  const ownsAssist = Boolean(taskId) && state.browserCredentialRecoveryAssistTaskId === taskId;
  state.browserCredentialRecoveryTaskId = "";
  state.browserCredentialRecoveryItemId = "";
  state.browserCredentialRecoveryAlias = "";
  state.browserCredentialRecoveryMode = "";
  state.browserCredentialRecoveryAssistTaskId = "";
  if (!taskId) return;

  ["credential-refresh-passwords", "credential-add-passwords"].forEach((id) => {
    const passwordInput = document.getElementById(id);
    if (passwordInput) passwordInput.value = "";
  });
  const status = document.getElementById("credential-status");
  if (status) status.textContent = "";
  const mode = currentAccountAssistMode();
  if (
    closeOwnedAssist &&
    ownsAssist &&
    state.activeAccountTool === "credentials" &&
    mode &&
    ["credential-add", "credential-refresh"].includes(mode.id)
  ) {
    closeAccountWorkbench();
  }
}

export function browserCredentialRecoveryModeForTask(task) {
  if (String((task && task.operation_kind) || "") === "account_credentials_add") {
    return {
      id: "credential-add",
      focusId: "credential-add-passwords",
    };
  }
  return {
    id: "credential-refresh",
    focusId: "credential-refresh-passwords",
  };
}

export function syncBrowserCredentialRecoveryTask(tasks) {
  const taskList = Array.isArray(tasks) ? tasks : [];
  const registrationTask =
    taskList.find(
      (task) =>
        task &&
        task.operation_kind === "account_registration" &&
        ((task.phase === "waiting_for_user" &&
          ["new_registration_credentials", "new_browser_credentials"].includes(task.waiting_for_input)) ||
          (Array.isArray(task.waiting_items) &&
            task.waiting_items.some(
              (item) =>
                item && ["new_registration_credentials", "new_browser_credentials"].includes(item.kind),
            ))),
    ) || null;
  syncRegistrationCredentialRecoveryTask(registrationTask);
  const waitingTask =
    taskList.find(
      (task) =>
        task &&
        task !== registrationTask &&
        ((task.phase === "waiting_for_user" && task.waiting_for_input === "new_browser_credentials") ||
          (Array.isArray(task.waiting_items) &&
            task.waiting_items.some((item) => item && item.kind === "new_browser_credentials"))),
    ) || null;
  if (!waitingTask) {
    clearBrowserCredentialRecoveryState({ closeOwnedAssist: true });
    return;
  }

  const waitingItems = Array.isArray(waitingTask.waiting_items)
    ? waitingTask.waiting_items.filter((item) => item && item.kind === "new_browser_credentials")
    : [];
  const activeItem =
    waitingItems.find((item) => item.item_id === state.browserCredentialRecoveryItemId) ||
    waitingItems[0] ||
    null;
  const itemId = activeItem ? String(activeItem.item_id || "") : "";
  const alias = activeItem
    ? String(activeItem.alias || "")
    : Array.isArray(waitingTask.locked_aliases)
      ? waitingTask.locked_aliases[0] || ""
      : "";
  const recoveryMode = browserCredentialRecoveryModeForTask(waitingTask);
  const unchanged =
    state.browserCredentialRecoveryTaskId === waitingTask.task_id &&
    state.browserCredentialRecoveryItemId === itemId &&
    state.browserCredentialRecoveryMode === recoveryMode.id;
  const message =
    waitingItems.length > 1
      ? `${waitingItems.length} 个账号等待输入，当前：${alias}`
      : alias
        ? `${alias} 需要重新输入密码`
        : "请重新输入密码后继续原任务";

  if (unchanged) {
    const status = document.getElementById("credential-status");
    setAccountAssistShortStatus(status, message);
    return;
  }

  state.browserCredentialRecoveryTaskId = waitingTask.task_id;
  state.browserCredentialRecoveryItemId = itemId;
  state.browserCredentialRecoveryAlias = alias;
  state.browserCredentialRecoveryMode = recoveryMode.id;
  openAccountTool("credentials", {
    aliases: recoveryMode.id === "credential-refresh" ? alias : "",
    focusId: recoveryMode.focusId,
    recoveryTaskId: waitingTask.task_id,
  });
  if (recoveryMode.id === "credential-add") {
    const aliasInput = document.getElementById("credential-add-aliases");
    if (aliasInput && !aliasInput.value.trim()) aliasInput.value = alias;
  }
  const passwordInput = document.getElementById(
    recoveryMode.id === "credential-add" ? "credential-add-passwords" : "credential-refresh-passwords",
  );
  if (passwordInput) passwordInput.value = "";
  const status = document.getElementById("credential-status");
  setAccountAssistShortStatus(status, message);
}

export function syncRegistrationCredentialRecoveryTask(task) {
  if (!task) {
    clearRegistrationCredentialRecoveryState();
    return;
  }

  const waitingItems = Array.isArray(task.waiting_items) ? task.waiting_items : [];
  const kind = ["new_registration_credentials", "new_browser_credentials"].includes(task.waiting_for_input)
    ? task.waiting_for_input
    : String(
        waitingItems.find(
          (item) => item && ["new_registration_credentials", "new_browser_credentials"].includes(item.kind),
        )?.kind || "",
      );
  if (!kind) {
    clearRegistrationCredentialRecoveryState();
    return;
  }
  if (
    state.registrationCredentialRecoveryTaskId &&
    (state.registrationCredentialRecoveryTaskId !== task.task_id ||
      state.registrationCredentialRecoveryKind !== kind)
  ) {
    clearRegistrationCredentialRecoveryState();
  }
  const unchanged =
    state.registrationCredentialRecoveryTaskId === task.task_id &&
    state.registrationCredentialRecoveryKind === kind;
  state.registrationCredentialRecoveryTaskId = task.task_id;
  state.registrationCredentialRecoveryKind = kind;
  const message = kind === "new_browser_credentials" ? "请重新输入账号密码" : "请重新填写注册信息";
  if (unchanged) {
    setRegistrationStatus(message, "warning");
    return;
  }

  setActivePage("registration", { preserveScroll: true });
  const source = document.getElementById("registration-source");
  const method = document.getElementById("registration-existing-method");
  if (source) source.value = kind === "new_browser_credentials" ? "existing" : "new";
  if (method && kind === "new_browser_credentials") method.value = "credentials";
  syncRegistrationSource();
  const aliasInput = document.getElementById("registration-alias");
  const lockedAlias = Array.isArray(task.locked_aliases) ? String(task.locked_aliases[0] || "") : "";
  if (aliasInput && !aliasInput.value && lockedAlias) aliasInput.value = lockedAlias;
  const emailInput = document.getElementById("registration-email");
  const passwordInput = document.getElementById("registration-password");
  if (kind === "new_registration_credentials" && emailInput) {
    emailInput.value = "";
    emailInput.focus();
  }
  if (passwordInput) passwordInput.value = "";
  if (kind === "new_browser_credentials" && passwordInput) {
    passwordInput.focus();
  }
  setRegistrationStatus(message, "warning");
}

export function clearRegistrationCredentialRecoveryState() {
  const taskId = state.registrationCredentialRecoveryTaskId;
  const kind = state.registrationCredentialRecoveryKind;
  state.registrationCredentialRecoveryTaskId = "";
  state.registrationCredentialRecoveryKind = "";
  if (!taskId) return;

  const passwordInput = document.getElementById("registration-password");
  const cookieInput = document.getElementById("registration-session-cookie");
  if (passwordInput) passwordInput.value = "";
  if (cookieInput) cookieInput.value = "";

  const status = document.getElementById("registration-status");
  const expectedMessage = kind === "new_browser_credentials" ? "请重新输入账号密码" : "请重新填写注册信息";
  if (status && status.textContent === expectedMessage) {
    setRegistrationStatus("", "info");
  }
}

export function taskSnapshotSequence(task) {
  const sequence = Number(task && task.last_sequence);
  return Number.isFinite(sequence) && sequence >= 0 ? sequence : null;
}

export function taskSnapshotCanReplace(current, incoming) {
  if (!current) return true;
  const currentSequence = taskSnapshotSequence(current);
  const incomingSequence = taskSnapshotSequence(incoming);
  if (currentSequence !== null && incomingSequence !== null) {
    if (incomingSequence < currentSequence) return false;
    if (incomingSequence > currentSequence) return true;
  }

  const currentPhase = String(current.phase || "").toLowerCase();
  const incomingPhase = String((incoming && incoming.phase) || "").toLowerCase();
  return !terminalTaskPhases.has(currentPhase) || terminalTaskPhases.has(incomingPhase);
}

export function mergeTaskSnapshots(existing, changed) {
  const byId = new Map();
  for (const task of Array.isArray(existing) ? existing : []) {
    if (task && task.task_id) {
      byId.set(task.task_id, task);
    }
  }
  for (const task of Array.isArray(changed) ? changed : []) {
    if (task && task.task_id && taskSnapshotCanReplace(byId.get(task.task_id), task)) {
      byId.set(task.task_id, task);
    }
  }
  return Array.from(byId.values());
}

export function latestTaskSequence(tasks) {
  return (Array.isArray(tasks) ? tasks : []).reduce((latest, task) => {
    const sequence = Number(task && task.last_sequence);
    return Number.isFinite(sequence) ? Math.max(latest, sequence) : latest;
  }, 0);
}

export function taskSummaryFromSnapshots(tasks) {
  const activePhases = new Set(taskActivePhases());
  const terminalPhases = new Set(taskTerminalPhases());
  const recoverableFailureKinds = new Set(taskRecoverableFailureKinds());
  const summary = {
    total_count: 0,
    running_count: 0,
    waiting_for_user_count: 0,
    failed_count: 0,
    active_count: 0,
    clearable_count: 0,
    waiting_for_input_count: 0,
    unsupported_waiting_input_count: 0,
    recoverable_failed_count: 0,
    waiting_for_input_by_kind: {},
    failed_by_kind: {},
    available_actions_count: {},
  };
  for (const task of Array.isArray(tasks) ? tasks : []) {
    const phase = String((task && task.phase) || "").toLowerCase();
    const waitingItems = Array.isArray(task && task.waiting_items) ? task.waiting_items : [];
    summary.total_count += 1;
    for (const item of waitingItems) {
      const kind = String((item && item.kind) || "");
      if (!kind) continue;
      summary.waiting_for_input_count += 1;
      summary.waiting_for_input_by_kind[kind] = (summary.waiting_for_input_by_kind[kind] || 0) + 1;
    }
    if (Array.isArray(task && task.available_actions)) {
      for (const action of task.available_actions) {
        summary.available_actions_count[action] = (summary.available_actions_count[action] || 0) + 1;
      }
    }
    if (phase === "running") {
      summary.running_count += 1;
      if (activePhases.has(phase)) summary.active_count += 1;
    } else if (phase === "pending") {
      if (activePhases.has(phase)) summary.active_count += 1;
    } else if (phase === "waiting_for_user") {
      summary.waiting_for_user_count += 1;
      if (activePhases.has(phase)) summary.active_count += 1;
      if (task && task.waiting_for_input) {
        summary.waiting_for_input_count += 1;
        summary.waiting_for_input_by_kind[task.waiting_for_input] =
          (summary.waiting_for_input_by_kind[task.waiting_for_input] || 0) + 1;
      } else if (waitingItems.length === 0) {
        summary.unsupported_waiting_input_count += 1;
      }
    } else if (phase === "failed") {
      if (task.resolved_by) { summary.clearable_count += 1; continue; }
      summary.failed_count += 1;
      if (terminalPhases.has(phase)) summary.clearable_count += 1;
      const hasBackendAction = Array.isArray(task && task.available_actions);
      if (
        hasBackendAction
          ? taskHasAction(task, "retry")
          : task && recoverableFailureKinds.has(task.failure_kind)
      ) {
        summary.recoverable_failed_count += 1;
      }
      if (task && task.failure_kind) {
        summary.failed_by_kind[task.failure_kind] = (summary.failed_by_kind[task.failure_kind] || 0) + 1;
      }
    } else if (terminalPhases.has(phase)) {
      summary.clearable_count += 1;
    }
  }
  return summary;
}

export function updateTaskSequenceFromEventId(eventId) {
  const sequence = Number(eventId);
  if (Number.isFinite(sequence)) {
    state.taskSequence = Math.max(state.taskSequence, sequence);
  }
}

export function scheduleTaskFallbackSync() {
  if (state.taskFallbackTimer || state.taskFallbackInFlight) {
    return;
  }
  state.taskFallbackTimer = window.setTimeout(() => {
    state.taskFallbackTimer = null;
    fetchTaskUpdates().catch((error) => {
      const status = document.getElementById("task-stream-status");
      status.textContent = "轮询失败，请查看运行日志";
      recordClientRuntimeError({ scope: "任务轮询", route: endpoints.tasks, error });
    });
  }, 1000);
}

export function cancelScheduledTaskFallbackSync() {
  if (!state.taskFallbackTimer) return;
  window.clearTimeout(state.taskFallbackTimer);
  state.taskFallbackTimer = null;
}

export function taskEventStreamIsHealthy() {
  if (!state.taskEvents || typeof EventSource === "undefined") return false;
  return state.taskEvents.readyState === EventSource.OPEN;
}

export async function fetchTaskUpdates() {
  if (state.taskFallbackInFlight) {
    return;
  }
  state.taskFallbackInFlight = true;
  try {
    const incremental = state.taskSequence > 0 && taskSinceCursorSupported();
    const tasks = await fetchJson(incremental ? endpoints.tasksSince(state.taskSequence) : endpoints.tasks);
    updateTasks(tasks, { incremental });
    const status = document.getElementById("task-stream-status");
    status.textContent = state.taskSequence ? `polled #${state.taskSequence}` : "polled";
  } finally {
    state.taskFallbackInFlight = false;
    if (!taskEventStreamIsHealthy()) {
      scheduleTaskFallbackSync();
    }
  }
}

export async function syncTaskAfterRequest(taskId, response) {
  const normalizedTaskId = String(taskId || "").trim();
  if (!normalizedTaskId) return;

  if (isPlainObject(response) && response.task_id && response.phase) {
    updateTasks([response], { incremental: true });
    return;
  }

  try {
    const cursor = state.taskSequence;
    const incremental = cursor > 0 && taskSinceCursorSupported();
    const tasks = await fetchJson(incremental ? endpoints.tasksSince(cursor) : endpoints.tasks);
    updateTasks(tasks, { incremental });
    if (!state.tasks.some((task) => task && task.task_id === normalizedTaskId)) {
      const fullTasks = await fetchJson(endpoints.tasks);
      updateTasks(fullTasks, { incremental: false });
    }
  } catch (error) {
    recordClientRuntimeError({
      scope: "任务日志同步",
      route: endpoints.tasks,
      error,
    });
  }
}

export async function postJson(path, payload) {
  const taskId = payload && typeof payload.task_id === "string" ? payload.task_id : "";
  let data;
  try {
    data = await request(path, { method: "POST", payload });
  } catch (error) {
    if (error.status !== undefined) await syncTaskAfterRequest(taskId, error.payload);
    throw error;
  }
  await syncTaskAfterRequest(taskId, data);
  return data;
}

export async function postTaskResult(path, payload) {
  const response = await postJson(path, payload);
  if (!isActiveTaskSnapshot(response)) return response;
  return new Promise((resolve, reject) => {
    registerPendingAccountOperation(response.task_id, (task) => {
      if (task.result) resolve(task.result);
      else reject(new Error(task.error || "任务未完成，请查看运行日志"));
    });
  });
}

export function setRefreshEnabled(enabled) {
  const button = document.getElementById("refresh-button");
  button.disabled = !enabled;
  const label = enabled ? "刷新" : "刷新中";
  button.innerHTML = `
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="23 4 23 10 17 10"></polyline><polyline points="1 20 1 14 7 14"></polyline><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"></path></svg>
    <span>${label}</span>
  `;
}

let healthRequest = null;

export function checkServiceHealth() {
  if (healthRequest) return healthRequest;
  // 日志流有独立连接，不能代替普通 HTTP 请求的响应检测。
  healthRequest = (async () => {
    try {
      const [health, config] = await Promise.all([
        fetchJson(endpoints.health, 5000), fetchJson(endpoints.config, 5000),
      ]);
      state.config = config;
      renderStatus(health);
      return health;
    } catch (error) {
      if (state.serviceHealth !== "unavailable") {
        recordClientRuntimeError({ scope: "服务连接检测", route: error.route || endpoints.health, error });
      }
      renderStatus({ status: "unavailable" });
      return null;
    } finally {
      healthRequest = null;
    }
  })();
  return healthRequest;
}

export function startServiceHealthMonitor() {
  const poll = async () => {
    await checkServiceHealth();
    window.setTimeout(poll, 5000);
  };
  window.setTimeout(poll, 5000);
}

export function renderStatus(health) {
  const value = health && health.status ? health.status : "unknown";
  const serviceStatus = document.getElementById("service-status");
  const normalized = String(value || "unknown").toLowerCase();
  const previous = state.serviceHealth;
  state.serviceHealth = normalized === "ok" ? "ok" : normalized === "unknown" ? "unknown" : "unavailable";
  const toneClass = normalized === "ok" ? "ui-tag-green" : normalized === "unknown" ? "" : "ui-tag-red";
  const ledClass = normalized === "ok" ? "on" : normalized === "unknown" ? "warn" : "off";
  if (serviceStatus) {
    serviceStatus.className = `ui-tag ${toneClass}`.trim();
    const label = state.serviceHealth === "unavailable" ? "服务无响应" : `服务${statusText(value)}`;
    serviceStatus.innerHTML = `<span class="status-led ${ledClass}" aria-hidden="true"></span>${label}`;
  }
  const service = serviceHealthInfo();
  document.querySelectorAll("[data-service-health]").forEach((element) => { element.textContent = service.label; });
  renderRuntimeHealthStrip(state.config);
  renderServiceApiRoutes(state.config?.api_capabilities);
  renderExtensionStatus();
  if (state.dashboard && previous !== state.serviceHealth) renderDashboard(state.dashboard, state.taskSummary);
}

export async function detectCurrentBrowserAccount({ status = null, refreshAfter = false, silent = false } = {}) {
  if (!browserProfileActions().includes("detect-current-account")) {
    const message = "当前后端不支持检测浏览器账号";
    state.browserCurrentAccount = null;
    state.browserCurrentAccountError = message;
    if (status) status.textContent = message;
    renderTopProfile(state.browserProfiles);
    return null;
  }
  if (!extensionBridgeConnected()) {
    const message = "需要 Chrome 扩展桥";
    state.browserCurrentAccount = null;
    state.browserCurrentAccountError = message;
    if (status) status.textContent = message;
    renderTopProfile(state.browserProfiles);
    return null;
  }

  if (status) status.textContent = "检测中";

  try {
    const report = await postJson(endpoints.browserCurrentAccount, {
      ...(silent ? {} : { task_id: makeTaskId("browser-current", "profile") }),
    });
    state.browserCurrentAccount = report;
    state.accounts = state.accounts.map((account) => ({ ...account, is_current: account.alias === report.saved_alias }));
    renderAccounts(state.accounts);
    state.browserCurrentAccountError = "";
    if (status) {
      status.textContent = report.saved_alias
        ? `${report.saved_alias} (${report.email || "-"})`
        : `未保存账号 ${report.email || "account"}`;
    }
    renderTopProfile(state.browserProfiles);
    if (refreshAfter) {
      await refresh({ detectBrowser: false });
    }
    return report;
  } catch (error) {
    state.browserCurrentAccount = null;
    state.browserCurrentAccountError = "检测失败，请查看运行日志";
    state.accounts = state.accounts.map((account) => ({ ...account, is_current: false }));
    renderAccounts(state.accounts);
    if (!silent) reportUiOperationFailure({
      status,
      shortMessage: state.browserCurrentAccountError,
      title: "浏览器账号检测失败",
      scope: "浏览器账号检测",
      route: endpoints.browserCurrentAccount,
      error,
    });
    renderTopProfile(state.browserProfiles);
    return null;
  }
}

export async function detectBrowserAccount() {
  const button = document.getElementById("detect-browser-account");
  const status = document.getElementById("browser-account-status");

  if (button) button.disabled = true;
  try {
    await detectCurrentBrowserAccount({ status, refreshAfter: true });
  } finally {
    if (button) button.disabled = false;
  }
}
