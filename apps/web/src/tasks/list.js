import { taskResultDetails, formatTaskKey, isPlainObject } from "./results.js";
import {
  recordClientRuntimeError,
  recordClientRuntimeInfo,
  reportUiOperationFailure,
  reportUiOperationSuccess,
  setAccountAssistShortStatus,
  setTransientUiStatus,
  showConfirmDialog,
  showToast,
} from "../feedback.js";
import { escapeAttr, escapeHtml, statusText, writeClipboard } from "../format.js";
import {
  MINIMUM_NEW_PASSWORD_LENGTH,
  RUNTIME_LOG_COLLAPSED_KEY,
  endpoints,
  selectedAliasesInPickOrder,
  state,
  statusClearTimers,
} from "../state.js";
import {
  accountAssistStatusForMode,
  passwordToolConfig,
  setPasswordToolStatus,
  splitAliasText,
} from "../workspace.js";
import {
  taskActivePhases,
  taskInputActionForKind,
  taskListActions,
  taskTerminalPhases,
} from "../capabilities.js";
import { resolveDesktopFocusMainWindow, taskBrowserUrl } from "../bridge.js";
import { iconSvg } from "../icons.js";
import { clearAccountSelection, restoreActionButtonBusy } from "../accounts/list.js";
import { fetchJson } from "../http.js";
import { postJson, refresh, updateTasks } from "../sync.js";

export function renderDashboard(summary, taskSummary) {
  const taskText = taskDashboardText(taskSummary);
  const metrics = [
    { label: "账号", value: summary.account_count, tone: "count" },
    { label: "Pro 账号", value: summary.pro_account_count, tone: "count" },
    { label: "试用中", value: summary.trial_active_count, tone: "count" },
    { label: "Cookie 已过期", value: summary.cookie_expired_count, tone: "count" },
    { label: "缺少 Git 令牌", value: summary.git_token_missing_count, tone: "count" },
    {
      label: "扩展",
      value: state.serviceHealth === "unavailable" ? "待确认" : summary.extension_connected ? `${summary.extension_client_count || 0} 个已连接` : "离线",
      tone: "bridge",
    },
    { label: "浏览器账号", value: summary.browser_account_email || "-", tone: "identity" },
    { label: "任务", value: taskText, tone: "task" },
  ];

  document.getElementById("dashboard-grid").innerHTML = metrics
    .map(
      ({ label, value, tone }) => `
      <div class="metric metric-${escapeAttr(tone)}" title="${escapeHtml(value)}">
        <span class="metric-value ${tone === "count" ? "" : "metric-value-text"}">${escapeHtml(value)}</span>
        <span class="metric-label">${escapeHtml(label)}</span>
      </div>
    `,
    )
    .join("");
}

export function taskDashboardText(summary) {
  if (!summary) return "0";
  const total = Number(summary.total_count || 0);
  const active = Number(summary.active_count || 0);
  const waiting = Number(summary.waiting_for_user_count || 0);
  const failed = Number(summary.failed_count || 0);
  const recoverable = Number(summary.recoverable_failed_count || 0);
  const failureKindText = taskFailureKindSummary(summary.failed_by_kind);
  const waitingInputText = taskWaitingInputSummary(
    summary.waiting_for_input_by_kind,
    summary.unsupported_waiting_input_count,
  );
  const actionText = taskAvailableActionSummary(summary.available_actions_count);
  const parts = [`${active}/${total} 活跃`];
  if (waiting) parts.push(waitingInputText ? `${waiting} 个等待 (${waitingInputText})` : `${waiting} 个等待`);
  if (failed) {
    const detail = failureKindText || (recoverable ? `${recoverable} 个可恢复` : "");
    parts.push(detail ? `${failed} 个失败 (${detail})` : `${failed} 个失败`);
  }
  if (actionText) parts.push(`可操作 ${actionText}`);
  return parts.join(", ");
}

export function taskFailureKindSummary(failedByKind) {
  if (!isPlainObject(failedByKind)) return "";
  return Object.entries(failedByKind)
    .filter(([, count]) => Number(count || 0) > 0)
    .sort((a, b) => Number(b[1] || 0) - Number(a[1] || 0))
    .slice(0, 2)
    .map(([kind, count]) => `${formatTaskKey(kind)}: ${count}`)
    .join(", ");
}

export function taskWaitingInputSummary(waitingByKind, unsupportedCount) {
  const parts = taskKindCountEntries(waitingByKind, 2);
  const unsupported = Number(unsupportedCount || 0);
  if (unsupported > 0 && parts.length < 2) {
    parts.push(`不支持: ${unsupported}`);
  }
  return parts.join(", ");
}

export function taskAvailableActionSummary(actionsByKind) {
  return taskKindCountEntries(actionsByKind, 2).join(", ");
}

export function taskKindCountEntries(countsByKind, limit) {
  if (!isPlainObject(countsByKind)) return [];
  return Object.entries(countsByKind)
    .filter(([, count]) => Number(count || 0) > 0)
    .sort((a, b) => Number(b[1] || 0) - Number(a[1] || 0))
    .slice(0, limit)
    .map(([kind, count]) => `${formatTaskKey(kind)}: ${count}`);
}

export function renderTasks(tasks) {
  const taskItems = Array.isArray(tasks) ? tasks : [];
  const orderedTaskItems = orderRuntimeTasks(taskItems);
  const clientLogs = Array.isArray(state.clientRuntimeLogs) ? state.clientRuntimeLogs : [];
  const countParts = [];
  if (taskItems.length) countParts.push(`任务 ${taskItems.length}`);
  if (clientLogs.length) countParts.push(`诊断 ${clientLogs.length}`);
  document.getElementById("task-count").textContent = countParts.join(" · ") || "无任务";
  renderRuntimeLogSummary(state.taskSummary);
  const list = document.getElementById("tasks-list");
  const previousProgress = new Map(Array.from(list.querySelectorAll("[data-task-id]"), (block) => [
    block.dataset.taskId, block.querySelector(".runtime-progress > span")?.style.width,
  ]));
  const focusError = document.getElementById("focus-retryable-task");
  if (focusError) focusError.disabled = !taskItems.some(taskCanRetry);
  if (!taskItems.length && !clientLogs.length) {
    list.innerHTML = `
      <div class="runtime-line runtime-line-muted">
        <span class="runtime-time">--:--:--</span>
        <span class="runtime-level">idle</span>
        <span class="runtime-message">暂无活动任务，等待后台事件。</span>
      </div>
    `;
    return;
  }

  list.innerHTML = [
    clientLogs.length ? runtimeClientLogBlock(clientLogs) : "",
    ...orderedTaskItems.map((task) => runtimeTaskLogBlock(task)),
  ]
    .filter(Boolean)
    .join("");
  if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    list.querySelectorAll("[data-task-id]").forEach((block) => {
      const bar = block.querySelector(".runtime-progress > span");
      const previous = previousProgress.get(block.dataset.taskId);
      if (bar && previous && previous !== bar.style.width) {
        bar.animate([{ width: previous }, { width: bar.style.width }], { duration: 280, easing: "ease-out" });
      }
    });
  }
  scrollRuntimeLogToLatest(list);
}

export function focusLatestRetryableTask() {
  const task = orderRuntimeTasks(state.tasks).filter(taskCanRetry).at(-1);
  if (!task) return;
  state.runtimeLogStickToBottom = false;
  applyRuntimeLogCollapsed(false, { persist: true });
  const block = document.querySelector(`[data-task-id="${CSS.escape(task.task_id)}"]`);
  block?.querySelector(".runtime-line-primary")?.scrollIntoView({ block: "center", behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
  block?.querySelector("[data-task-retry]")?.focus({ preventScroll: true });
}

export function orderRuntimeTasks(tasks) {
  return tasks
    .map((task, index) => ({ task, index, timestamp: runtimeTaskTimestamp(task) }))
    .sort((left, right) => {
      const leftHasTime = Number.isFinite(left.timestamp);
      const rightHasTime = Number.isFinite(right.timestamp);
      if (leftHasTime && rightHasTime && left.timestamp !== right.timestamp) {
        return left.timestamp - right.timestamp;
      }
      if (leftHasTime !== rightHasTime) return leftHasTime ? 1 : -1;
      return left.index - right.index;
    })
    .map(({ task }) => task);
}

export function handleRuntimeLogScroll(event) {
  state.runtimeLogStickToBottom = runtimeLogIsNearBottom(event.currentTarget);
}

export function runtimeLogIsNearBottom(list) {
  if (!list) return true;
  return list.scrollHeight - list.scrollTop - list.clientHeight <= 24;
}

export function scrollRuntimeLogToLatest(list) {
  if (!list || state.runtimeLogCollapsed || !state.runtimeLogStickToBottom) return;
  list.scrollTop = list.scrollHeight;
}

export function runtimeClientLogBlock(logs) {
  const hasErrors = logs.some((log) => log.level === "error");
  const rows = logs
    .slice(-12)
    .map(
      (log) => `
    <div class="runtime-line${log.level === "error" ? " runtime-line-error" : ""}">
      <span class="runtime-time">${escapeHtml(new Date(log.createdAt).toLocaleTimeString())}</span>
      <span class="runtime-level">${escapeHtml(log.level)}</span>
      <span class="runtime-message"><strong>${escapeHtml(log.scope)}</strong> · ${escapeHtml(log.route)} · ${escapeHtml(log.message)}</span>
    </div>
  `,
    )
    .join("");
  return `
    <div class="runtime-task-block task-phase-${hasErrors ? "failed" : "completed"} runtime-client-log-block">
      <div class="runtime-line runtime-line-primary">
        <span class="runtime-time">${escapeHtml(new Date(logs[logs.length - 1].createdAt).toLocaleTimeString())}</span>
        <span class="runtime-level">界面</span>
        <span class="runtime-message"><strong>客户端日志</strong> 最近 ${Math.min(logs.length, 12)} 条</span>
      </div>
      ${rows}
    </div>
  `;
}

export function runtimeTaskLogBlock(task) {
  const phase = task.resolved_by ? "resolved" : task.phase || "unknown";
  const level = task.resolved_by ? "已恢复" : task.failure_kind || phase;
  const operation = task.operation_kind ? ` · ${formatTaskKey(task.operation_kind)}` : "";
  const locked =
    task.locked_aliases && task.locked_aliases.length
      ? ` · 锁定 ${task.locked_aliases.join(", ")}`
      : "";
  const actions = [
    taskCanRetry(task)
      ? `<button class="inline-action" type="button" data-task-retry="${escapeHtml(task.task_id)}">重试</button>`
      : "",
    taskCanCancel(task)
      ? `<button class="inline-action danger-action" type="button" data-task-cancel="${escapeHtml(task.task_id)}">取消</button>`
      : "",
  ]
    .filter(Boolean)
    .join("");
  const progress = task.progress
    ? `<div class="runtime-line runtime-line-muted"><span class="runtime-time"></span><span class="runtime-level">进度</span><span class="runtime-message">${escapeHtml(taskProgressText(task.progress))}</span></div><div class="runtime-progress" role="progressbar" aria-valuenow="${taskProgressPercent(task.progress)}" aria-valuemin="0" aria-valuemax="100"><span style="width:${taskProgressPercent(task.progress)}%"></span></div>`
    : "";
  const logs =
    task.logs && task.logs.length
      ? task.logs
          .slice(-40)
          .map(
            (log) => `
        <div class="runtime-line${log.level === "error" ? " runtime-line-error" : log.level === "warning" ? " runtime-line-warn" : ""}">
          <span class="runtime-time">#${escapeHtml(log.sequence)}</span>
          <span class="runtime-level">${escapeHtml(log.level || "log")}</span>
          <span class="runtime-message">${escapeHtml(log.message || "")}</span>
        </div>
      `,
          )
          .join("")
      : "";
  return `
    <div class="runtime-task-block task-phase-${escapeAttr(phase)}" data-task-id="${escapeAttr(task.task_id)}">
      <div class="runtime-line runtime-line-primary">
        <span class="runtime-time">${escapeHtml(runtimeTaskTime(task))}</span>
        <span class="runtime-level">${escapeHtml(statusText(level))}</span>
        <span class="runtime-message"><strong>${escapeHtml(task.name || task.task_id)}</strong>${escapeHtml(operation)}${escapeHtml(locked)} ${escapeHtml(task.message || "")}</span>
        ${actions ? `<span class="runtime-actions">${actions}</span>` : ""}
      </div>
      ${task.recovery_hint ? `<div class="runtime-line runtime-line-warn"><span class="runtime-time"></span><span class="runtime-level">hint</span><span class="runtime-message">${escapeHtml(task.recovery_hint)}</span></div>` : ""}
      ${task.error ? `<div class="runtime-line runtime-line-error"><span class="runtime-time"></span><span class="runtime-level">error</span><span class="runtime-message">${escapeHtml(task.error)}</span></div>` : ""}
      ${taskResultDetails(task) ? `<div class="runtime-result">${taskResultDetails(task)}</div>` : ""}
      ${logs}
      ${taskUserInputControls(task)}
      ${progress}
    </div>
  `;
}

export function runtimeTaskTime(task) {
  const timestamp = runtimeTaskTimestamp(task);
  return Number.isFinite(timestamp) ? new Date(timestamp).toLocaleTimeString() : "--:--:--";
}

export function runtimeTaskTimestamp(task) {
  const explicit = runtimeTimestampMilliseconds(
    task && (task.updated_at || task.finished_at || task.started_at || task.created_at),
  );
  if (Number.isFinite(explicit)) return explicit;

  const match = String((task && task.task_id) || "").match(/-(\d{13})$/);
  return match ? Number(match[1]) : Number.NaN;
}

export function runtimeTimestampMilliseconds(value) {
  if (value === null || value === undefined || value === "") return Number.NaN;
  const numeric = typeof value === "number" ? value : Number(value);
  if (Number.isFinite(numeric)) return numeric < 1e12 ? numeric * 1000 : numeric;
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : Number.NaN;
}

export function renderRuntimeLogSummary(summary = state.taskSummary) {
  const container = document.getElementById("runtime-log-summary");
  if (!container) return;
  const total = Number((summary && summary.total_count) || state.tasks.length || 0);
  const active = Number((summary && (summary.running_count ?? summary.active_count)) || 0);
  const waiting = Number(
    (summary && (summary.waiting_for_user_count || summary.waiting_for_input_count)) || 0,
  );
  const clientCount = state.clientRuntimeLogs.length;
  const failed = Number((summary && summary.failed_count) || 0);
  const recoverable = Number((summary && summary.recoverable_failed_count) || 0);
  const dock = document.querySelector(".runtime-log-dock");
  const stateChip = document.getElementById("runtime-log-state");
  const stateInfo = runtimeLogStateInfo({ total, active, waiting, failed, recoverable });
  if (dock) {
    dock.classList.remove("state-idle", "state-active", "state-waiting", "state-failed");
    dock.classList.add(`state-${stateInfo.key}`);
    dock.dataset.runtimeState = stateInfo.key;
  }
  if (stateChip) {
    stateChip.textContent = stateInfo.label;
    stateChip.className = `runtime-log-state state-${stateInfo.key}`;
    stateChip.title = stateInfo.detail;
  }
  container.innerHTML = `
    <span class="runtime-summary-text">
      运行 ${active} · 等待 ${waiting} · 失败 ${failed} · 诊断 ${clientCount}
    </span>
  `;
  container.title = `${stateInfo.detail}；任务总数：${total}`;
}

export function runtimeLogStateInfo({ total, active, waiting, failed, recoverable }) {
  if (failed > 0 || recoverable > 0) {
    return {
      key: "failed",
      label: recoverable > 0 ? "可恢复失败" : "存在失败",
      detail: `失败 ${failed} 个，可恢复 ${recoverable} 个`,
    };
  }
  if (waiting > 0) {
    return {
      key: "waiting",
      label: "等待输入",
      detail: `有 ${waiting} 个任务等待用户输入或确认`,
    };
  }
  if (active > 0) {
    return {
      key: "active",
      label: "运行中",
      detail: `有 ${active} 个任务正在运行`,
    };
  }
  if (total > 0) {
    return {
      key: "idle",
      label: "最近完成",
      detail: `当前无活动任务，保留 ${total} 个最近任务记录`,
    };
  }
  return {
    key: "idle",
    label: "空闲",
    detail: "暂无活动任务",
  };
}

export async function copyRuntimeLog() {
  const button = document.getElementById("copy-runtime-log");
  const originalText = button ? button.textContent : "复制日志";
  if (button) {
    button.disabled = true;
    button.textContent = "复制中";
  }
  recordClientRuntimeInfo({
    scope: "运行日志",
    route: "clipboard",
    message: "正在复制当前运行日志。",
  });
  showToast({ tone: "info", title: "正在复制运行日志", message: "" });
  const nextFrame = window.requestAnimationFrame || ((callback) => window.setTimeout(callback, 0));
  await new Promise((resolve) => nextFrame(resolve));
  const logText = document.getElementById("tasks-list")?.innerText.trim() || "暂无运行日志";
  try {
    await writeClipboard(logText);
    if (button) button.textContent = "已复制";
    recordClientRuntimeInfo({
      scope: "运行日志",
      route: "clipboard",
      message: "已复制当前运行日志。",
    });
    showToast({
      tone: "success",
      title: "运行日志已复制",
      message: "日志已复制到剪贴板。",
    });
  } catch (error) {
    reportUiOperationFailure({
      status: null,
      shortMessage: "",
      title: "复制运行日志失败",
      scope: "运行日志",
      route: "clipboard",
      error,
    });
  } finally {
    window.setTimeout(() => {
      if (button) {
        button.disabled = false;
        button.textContent = originalText || "复制日志";
      }
    }, 900);
  }
}

export function restoreRuntimeLogDockState() {
  let collapsed = false;
  try {
    collapsed = window.localStorage.getItem(RUNTIME_LOG_COLLAPSED_KEY) === "true";
  } catch (_error) {
    collapsed = false;
  }
  applyRuntimeLogCollapsed(collapsed);
}

export function toggleRuntimeLogDock() {
  applyRuntimeLogCollapsed(!state.runtimeLogCollapsed, { persist: true });
}

export function applyRuntimeLogCollapsed(collapsed, options = {}) {
  state.runtimeLogCollapsed = Boolean(collapsed);
  const dock = document.querySelector(".runtime-log-dock");
  const list = document.getElementById("tasks-list");
  const button = document.getElementById("runtime-log-toggle");
  document.body.classList.toggle("runtime-log-collapsed", state.runtimeLogCollapsed);
  if (dock) {
    dock.classList.toggle("collapsed", state.runtimeLogCollapsed);
    dock.setAttribute("aria-expanded", String(!state.runtimeLogCollapsed));
  }
  if (list) {
    list.hidden = state.runtimeLogCollapsed;
    if (!state.runtimeLogCollapsed) scrollRuntimeLogToLatest(list);
  }
  if (button) {
    button.textContent = state.runtimeLogCollapsed ? "展开" : "收起";
    button.setAttribute("aria-expanded", String(!state.runtimeLogCollapsed));
    button.title = state.runtimeLogCollapsed ? "展开底部运行日志" : "收起底部运行日志";
  }
  if (options.persist) {
    try {
      window.localStorage.setItem(RUNTIME_LOG_COLLAPSED_KEY, String(state.runtimeLogCollapsed));
    } catch (_error) {}
  }
}


export function taskCanRetry(task) {
  if (task?.resolved_by) return false;
  const capabilities = state.config && state.config.api_capabilities;
  const retryEndpointAvailable = !isPlainObject(capabilities) || capabilities.task_retry_endpoint !== false;
  if (Array.isArray(task && task.available_actions)) {
    return retryEndpointAvailable && taskHasAction(task, "retry");
  }
  const phase = String((task && task.phase) || "").toLowerCase();
  return retryEndpointAvailable && phase === "failed" && isPlainObject(task && task.retry_descriptor);
}

export function taskUserInputControls(task) {
  return [taskEmailCodeInput(task), taskRegistrationCredentialsInput(task), taskCaptchaInput(task)].join("");
}

export function taskCurrentlyWaitsForInput(task, inputKind) {
  return (
    String((task && task.phase) || "").toLowerCase() === "waiting_for_user" &&
    String((task && task.waiting_for_input) || "") === inputKind
  );
}

export function syncTaskInputWindowFocus(tasks) {
  const activeKeys = new Set();
  const emailAction = taskInputActionForKind("email_code");
  for (const task of Array.isArray(tasks) ? tasks : []) {
    if (
      !emailAction ||
      !taskCurrentlyWaitsForInput(task, "email_code") ||
      !taskHasAction(task, emailAction)
    ) {
      continue;
    }
    const key = `${task.task_id}:email_code`;
    activeKeys.add(key);
    if (state.focusedTaskInputKeys.has(key)) continue;
    state.focusedTaskInputKeys.add(key);
    const focusMainWindow = resolveDesktopFocusMainWindow();
    if (!focusMainWindow) continue;
    Promise.resolve(focusMainWindow()).catch((error) => {
      recordClientRuntimeError({
        scope: "前置验证码输入",
        route: "desktop://focus-main-window",
        error,
      });
    });
  }

  for (const key of Array.from(state.focusedTaskInputKeys)) {
    if (!activeKeys.has(key)) state.focusedTaskInputKeys.delete(key);
  }
}

export function taskEmailCodeInput(task) {
  const action = taskInputActionForKind("email_code");
  if (!taskCurrentlyWaitsForInput(task, "email_code") || !action || !taskHasAction(task, action)) {
    return "";
  }
  return `
    <form class="task-input-form" data-task-input-form="${escapeAttr(task.task_id)}" data-task-input-kind="email_code">
      <input type="text" inputmode="numeric" autocomplete="one-time-code" placeholder="邮箱验证码">
      <button class="inline-action" type="submit">提交验证码</button>
    </form>
  `;
}

export function taskRegistrationCredentialsInput(task) {
  if (task?.operation_kind === "account_registration") return "";
  const action = taskInputActionForKind("new_registration_credentials");
  if (
    !taskCurrentlyWaitsForInput(task, "new_registration_credentials") ||
    !action ||
    !taskHasAction(task, action)
  ) {
    return "";
  }
  return `
    <form class="task-input-form task-credential-form" data-task-input-form="${escapeAttr(task.task_id)}" data-task-input-kind="new_registration_credentials">
      <input name="alias" type="text" autocomplete="off" placeholder="新别名">
      <input name="email" type="email" autocomplete="off" placeholder="新邮箱">
      <input name="password" type="password" minlength="${MINIMUM_NEW_PASSWORD_LENGTH}" autocomplete="new-password" placeholder="新密码" required>
      <button class="inline-action" type="submit">提交新账号</button>
    </form>
  `;
}

export function taskCaptchaInput(task) {
  const action = taskInputActionForKind("captcha_completed");
  if (!taskCurrentlyWaitsForInput(task, "captcha_completed") || !action || !taskHasAction(task, action)) {
    return "";
  }
  return `
    <form class="task-input-form" data-task-input-form="${escapeAttr(task.task_id)}" data-task-input-kind="captcha_completed">
      ${taskBrowserUrl(state.config) ? `<a class="ui-btn" href="${escapeAttr(taskBrowserUrl(state.config))}" target="_blank" rel="noopener noreferrer">${iconSvg("monitor")}打开任务浏览器</a>` : ""}
      <button class="inline-action" type="submit">已完成人机验证</button>
    </form>
  `;
}

export function nonNegativeCount(value) {
  const count = Number(value);
  return Number.isFinite(count) && count >= 0 ? Math.floor(count) : 0;
}

export function reportCount(report, keys, fallback = 0) {
  for (const key of keys) {
    if (report && Object.prototype.hasOwnProperty.call(report, key)) {
      const value = report[key];
      if (value === null || value === undefined || value === "") continue;
      const count = Number(value);
      if (Number.isFinite(count) && count >= 0) return Math.floor(count);
    }
  }
  return nonNegativeCount(fallback);
}

export function accountActionReportCoverage(action, items, requestedAliases) {
  const coveredActions = new Set(["refresh-session", "refresh-git", "generate-git", "browser-login"]);
  if (!coveredActions.has(action) || !Array.isArray(requestedAliases) || !requestedAliases.length) {
    return { missingCount: 0, unexpectedCount: 0, duplicateCount: 0, mismatchCount: 0 };
  }
  const requested = new Set(requestedAliases.map((alias) => String(alias || "").trim()).filter(Boolean));
  const reported = Array.isArray(items) ? items.map((item) => String((item && item.alias) || "").trim()) : [];
  const reportedSet = new Set(reported.filter(Boolean));
  const missingCount = Array.from(requested).filter((alias) => !reportedSet.has(alias)).length;
  const unexpectedCount = reported.filter((alias) => !alias || !requested.has(alias)).length;
  const duplicateCount = reported.filter((alias, index) => alias && reported.indexOf(alias) !== index).length;
  return {
    missingCount,
    unexpectedCount,
    duplicateCount,
    mismatchCount: missingCount + unexpectedCount + duplicateCount,
  };
}

export function accountActionOutcome(action, report, requestedCount, requestedAliases = []) {
  const phase = String((report && report.phase) || "").toLowerCase();
  if (taskActivePhases().includes(phase)) {
    return {
      shortMessage: "任务已开始",
      titleSuffix: "已开始",
      tone: "info",
      detail: `任务已进入运行队列，目标 ${requestedCount} 个账号。`,
    };
  }

  if (action === "switch-plan") {
    const commandCount = reportCount(report, ["command_count"]);
    return {
      shortMessage: "换号计划已生成",
      titleSuffix: "完成",
      tone: "success",
      detail: `已生成 ${commandCount} 条换号命令。`,
    };
  }

  if (action === "switch-execute") {
    const executedCount = reportCount(report, ["executed_count"]);
    const migratedCount = reportCount(report && report.project_migration, ["migrated_count"]);
    const sync = report?.skills_sync;
    const syncFailed = Number(sync?.failed_count || 0) > 0;
    const syncDetail = (sync?.items || []).map((item) => item.message).filter(Boolean).join("；");
    return {
      shortMessage: syncFailed ? "换号完成，skills 未完全同步" : "换号执行完成",
      titleSuffix: "完成",
      tone: syncFailed ? "warning" : "success",
      toastMessage: sync ? syncDetail : undefined,
      detail:
        (report && report.project_migration
          ? `已执行 ${executedCount} 条换号命令，迁移 ${migratedCount} 个项目。`
          : `已执行 ${executedCount} 条换号命令。`) + syncDetail,
    };
  }

  const items = Array.isArray(report && report.items) ? report.items : [];
  const coverage = accountActionReportCoverage(action, items, requestedAliases);
  if (action === "browser-login") {
    const rawOpened = reportCount(
      report,
      ["opened_count"],
      items.filter((item) => item && item.opened).length,
    );
    const rawCancelled = reportCount(
      report,
      ["cancelled_count"],
      items.filter((item) => item && item.cancelled).length,
    );
    const opened = Math.min(Math.max(0, requestedCount - coverage.missingCount), rawOpened);
    const cancelled = Math.min(Math.max(0, requestedCount - opened), rawCancelled);
    // 未打开且未取消的目标都算失败；缺失结果已经包含在这个剩余集合内。
    const failed = Math.max(0, requestedCount - opened - cancelled);
    const hasFailure = failed > 0 || cancelled > 0 || coverage.mismatchCount > 0;
    const mismatch = coverage.mismatchCount ? `，结果集合异常 ${coverage.mismatchCount} 个` : "";
    return {
      shortMessage: hasFailure ? "部分浏览器未打开" : "浏览器已打开",
      titleSuffix: hasFailure ? "未完全完成" : "完成",
      tone: hasFailure ? "warning" : "success",
      detail: `已打开 ${opened} 个浏览器窗口，失败 ${failed} 个，取消 ${cancelled} 个${mismatch}。`,
    };
  }

  const refreshed = reportCount(
    report,
    ["refreshed_count", "updated_count", "changed_count"],
    items.filter((item) => item && (item.refreshed || item.updated || item.changed)).length,
  );
  const reportedFailed = reportCount(
    report,
    ["failed_count"],
    items.filter(
      (item) =>
        item &&
        !item.skipped &&
        (item.error || item.refreshed === false || item.updated === false || item.changed === false),
    ).length,
  );
  const rawSkipped = reportCount(
    report,
    ["skipped_count"],
    items.filter((item) => item && item.skipped).length,
  );
  const boundedRefreshed = Math.min(Math.max(0, requestedCount - coverage.missingCount), refreshed);
  const skipped = Math.min(Math.max(0, requestedCount - boundedRefreshed), rawSkipped);
  const remaining = Math.max(0, requestedCount - boundedRefreshed - skipped);
  const failed = Math.max(coverage.missingCount, Math.min(remaining, reportedFailed));
  const unaccounted = Math.max(0, remaining - failed);
  const boundedFailed = failed + unaccounted;
  const mismatch = coverage.mismatchCount ? `，结果集合异常 ${coverage.mismatchCount} 个` : "";
  const hasFailure = boundedFailed > 0 || coverage.mismatchCount > 0;
  return {
    shortMessage: hasFailure ? "部分操作未完成" : "操作完成",
    titleSuffix: hasFailure ? "未完全完成" : "完成",
    tone: hasFailure ? "warning" : "success",
    detail: `目标 ${requestedCount} 个账号，成功 ${boundedRefreshed} 个，跳过 ${skipped} 个，失败 ${boundedFailed} 个${mismatch}。`,
  };
}

export function accountActionTargetText(aliasList) {
  const targets = Array.isArray(aliasList) ? aliasList.filter(Boolean) : [];
  return targets.length <= 1 ? targets[0] || "目标账号" : targets.join(", ");
}

export function reportAccountActionTerminal({
  action,
  aliasList,
  button,
  busySnapshot,
  status,
  descriptor,
  task,
}) {
  restoreActionButtonBusy(button, busySnapshot);
  const phase = String((task && task.phase) || "").toLowerCase();
  const targetText = accountActionTargetText(aliasList);
  if (phase === "cancelled") {
    reportUiOperationSuccess({
      status,
      shortMessage: "操作已取消",
      title: `${descriptor.title}已取消`,
      scope: descriptor.scope,
      route: descriptor.endpoint,
      message: `${targetText}：任务已取消。`,
      tone: "info",
    });
    return;
  }

  const report = task && isPlainObject(task.result) ? task.result : null;
  if (!report) {
    reportUiOperationFailure({
      status,
      shortMessage: "任务未完成，请查看日志",
      title: `${descriptor.title}未完成`,
      scope: descriptor.scope,
      route: descriptor.endpoint,
      error: new Error((task && task.message) || "任务终态未返回结构化结果"),
    });
    return;
  }

  const outcome = accountActionOutcome(action, report, aliasList.length, aliasList);
  if (phase === "failed") {
    reportUiOperationFailure({
      status,
      shortMessage: outcome.shortMessage === "操作完成" ? "操作未完全完成" : outcome.shortMessage,
      title: `${descriptor.title}未完全完成`,
      scope: descriptor.scope,
      route: descriptor.endpoint,
      error: new Error(outcome.detail),
    });
    return;
  }
  reportUiOperationSuccess({
    status,
    shortMessage: outcome.shortMessage,
    title: `${descriptor.title}${outcome.titleSuffix}`,
    scope: descriptor.scope,
    route: descriptor.endpoint,
    message: `${targetText}：${outcome.detail}`,
    tone: outcome.tone,
    toastMessage: outcome.toastMessage,
  });
}

export function registerPendingAccountAction({
  taskId,
  task,
  action,
  aliasList,
  button,
  busySnapshot,
  status,
  descriptor,
}) {
  registerPendingAccountOperation(taskId, (terminalTask) => {
    reportAccountActionTerminal({
      action,
      aliasList,
      button,
      busySnapshot,
      status,
      descriptor,
      task: terminalTask,
    });
  });
  updateTasks([task], { incremental: true });
  setAccountAssistShortStatus(status, "任务已开始");
}

export function scheduleStatusClear(status, expectedText, delay = 4600) {
  if (!status || !expectedText || delay <= 0) return;
  const existingTimer = statusClearTimers.get(status);
  if (existingTimer) window.clearTimeout(existingTimer);
  const timer = window.setTimeout(() => {
    if (status.textContent === expectedText) status.textContent = "";
    if (statusClearTimers.get(status) === timer) statusClearTimers.delete(status);
  }, delay);
  statusClearTimers.set(status, timer);
}

export function isActiveTaskSnapshot(report) {
  const phase = String((report && report.phase) || "").toLowerCase();
  return taskActivePhases().includes(phase);
}

export function isTerminalTaskSnapshot(task) {
  const phase = String((task && task.phase) || "").toLowerCase();
  return taskTerminalPhases().includes(phase);
}

export function taskAwaitsInput(task, kind, itemId = "") {
  if (!isActiveTaskSnapshot(task) || task.cancel_requested) return false;
  if (itemId) return task.waiting_items?.some((item) => item.item_id === itemId && item.kind === kind) || false;
  return (task.phase === "waiting_for_user" && task.waiting_for_input === kind)
    || task.waiting_items?.some((item) => item.kind === kind) || false;
}

export function registerPendingAccountOperation(taskId, onTerminal) {
  const normalizedTaskId = String(taskId || "").trim();
  if (!normalizedTaskId || typeof onTerminal !== "function") return;

  const current = (Array.isArray(state.tasks) ? state.tasks : []).find(
    (task) => task && task.task_id === normalizedTaskId,
  );
  if (current && isTerminalTaskSnapshot(current)) {
    onTerminal(current);
    return;
  }
  state.pendingAccountOperations.set(normalizedTaskId, onTerminal);
}

export function registerPendingAccountIoOperation({
  taskId,
  task,
  submit,
  status,
  modeId,
  operationTitle,
  scope,
  route,
  cancelledShortMessage,
  missingShortMessage,
  resultFailureShortMessage,
  allowFailedResult = false,
  ownsForm = () => true,
  onResult,
}) {
  const restoreSubmit = () => {
    if (submit && ownsForm()) submit.disabled = false;
  };
  const reportStatus = () => ownsForm() ? accountAssistStatusForMode("io", modeId, status) : null;

  registerPendingAccountOperation(taskId, (terminalTask) => {
    const phase = String((terminalTask && terminalTask.phase) || "").toLowerCase();
    if (phase === "cancelled") {
      reportUiOperationSuccess({
        status: reportStatus(),
        shortMessage: cancelledShortMessage,
        title: `${operationTitle}已取消`,
        scope,
        route,
        message: `用户取消了${operationTitle}任务。`,
        tone: "info",
      });
      restoreSubmit();
      return;
    }

    const finalReport = terminalTask && isPlainObject(terminalTask.result) ? terminalTask.result : null;
    if (!finalReport || (phase === "failed" && !allowFailedResult)) {
      reportUiOperationFailure({
        status: reportStatus(),
        shortMessage: missingShortMessage,
        title: `${operationTitle}未完成`,
        scope,
        route,
        error: new Error(
          (terminalTask && (terminalTask.error || terminalTask.message)) ||
            `${operationTitle}任务未返回结构化结果`,
        ),
      });
      restoreSubmit();
      return;
    }

    Promise.resolve()
      .then(() => onResult(finalReport, phase))
      .catch((error) => {
        reportUiOperationFailure({
          status: reportStatus(),
          shortMessage: resultFailureShortMessage,
          title: `${operationTitle}结果处理失败`,
          scope,
          route,
          error,
        });
      })
      .finally(restoreSubmit);
  });
  updateTasks([task], { incremental: true });
  if (ownsForm()) setTransientUiStatus(status, "任务已开始", 0);
}

export function settlePendingAccountOperations(tasks, previousTasks = []) {
  if (!(state.pendingAccountOperations instanceof Map) || !state.pendingAccountOperations.size) return [];
  const snapshots = Array.isArray(tasks) ? tasks : [];
  const previousTaskList = Array.isArray(previousTasks) ? previousTasks : [];
  const previousById = new Map(
    previousTaskList.filter((task) => task && task.task_id).map((task) => [task.task_id, task]),
  );
  const synthesizedTerminalTasks = [];
  for (const [taskId, onTerminal] of Array.from(state.pendingAccountOperations.entries())) {
    const task = snapshots.find((item) => item && item.task_id === taskId);
    if (task && !isTerminalTaskSnapshot(task)) continue;
    const previousTask = previousById.get(taskId);
    if (!task && !previousTask) continue;
    const terminalTask = task || {
      task_id: taskId,
      operation_kind: previousTask && previousTask.operation_kind,
      phase: "failed",
      message: "任务记录已被清理，未收到终态结果",
      result: null,
    };
    if (!task) synthesizedTerminalTasks.push(terminalTask);
    state.pendingAccountOperations.delete(taskId);
    try {
      onTerminal(terminalTask);
    } catch (error) {
      recordClientRuntimeError({
        scope: "账号任务终态清理",
        route: "/tasks",
        error,
      });
    }
  }
  return synthesizedTerminalTasks;
}

export function clearPasswordToolInputs(tool) {
  const config = passwordToolConfig(tool);
  if (!config) return;
  const passwordInput = document.getElementById(config.passwordInputId);
  if (passwordInput) passwordInput.value = "";
  document.querySelectorAll(`[data-password-row="${tool}"]`).forEach((row) => {
    row.value = "";
  });
}

export function settlePasswordToolSubmission(tool, aliases, outcome) {
  if (!passwordToolSelectionMatches(tool, aliases)) return;
  const normalizedOutcome = outcome === "completed" || outcome === "cancelled" ? outcome : "failed";
  if (normalizedOutcome === "completed") {
    clearPasswordToolInputs(tool);
    clearAccountSelection();
  } else if (normalizedOutcome === "cancelled") {
    // 取消保留账号队列，清掉已输入密码，避免取消后残留敏感值。
    clearPasswordToolInputs(tool);
  }
  window.requestAnimationFrame(() => {
    document.querySelector(`[data-password-row="${tool}"]`)?.focus();
  });
}

export function settlePasswordToolTask(tool, task, submittedAliases, submit) {
  if (submit) submit.disabled = false;
  if (state.passwordSelectionTool !== tool) return;

  const phase = String((task && task.phase) || "").toLowerCase();
  const completed = phase === "completed";
  const cancelled = phase === "cancelled";
  const remote = tool === "overleaf-password";
  const actionLabel = remote ? "远端密码修改" : "本地密码更新";
  const shortStatus = completed
    ? `${actionLabel}完成`
    : cancelled
      ? `${actionLabel}已取消`
      : `${actionLabel}未完成，请查看运行日志`;
  setPasswordToolStatus(tool, shortStatus, completed ? 4600 : 7200);
  recordClientRuntimeInfo({
    scope: remote ? "修改 Overleaf 密码" : "更新本地密码",
    route: remote ? endpoints.overleafPassword : endpoints.localPassword,
    message: completed
      ? `${actionLabel}任务已完成。`
      : cancelled
        ? `${actionLabel}任务已取消。`
        : `${actionLabel}任务未完全完成，输入仍可重试。`,
  });
  settlePasswordToolSubmission(
    tool,
    submittedAliases,
    completed ? "completed" : cancelled ? "cancelled" : "failed",
  );
}

export function passwordToolSelectionMatches(tool, aliases) {
  if (state.passwordSelectionTool !== tool) return false;
  const expected = Array.isArray(aliases) ? aliases : splitAliasText(aliases);
  const current = selectedAliasesInPickOrder();
  return expected.length === current.length && expected.every((alias, index) => alias === current[index]);
}

export async function cancelTask(button) {
  const taskId = button.dataset.taskCancel;
  if (!taskId || button.disabled) return;

  const originalText = button.textContent;
  button.disabled = true;
  button.textContent = "取消中";

  try {
    const response = await postJson(endpoints.taskCancel, { task_id: taskId });
    await refresh();
    const task = (Array.isArray(state.tasks) ? state.tasks : []).find(
      (item) => item && item.task_id === taskId,
    );
    const terminal =
      String((task && task.phase) || (response && response.phase) || "").toLowerCase() === "cancelled";
    showToast({
      tone: terminal ? "success" : "info",
      title: terminal ? "任务已取消" : "已请求取消",
      message: terminal ? "任务资源已清理。" : "正在等待浏览器会话完成清理。",
    });
  } catch (error) {
    button.textContent = "失败";
    recordClientRuntimeError({ scope: "取消任务", route: endpoints.taskCancel, error });
    window.setTimeout(() => {
      button.disabled = false;
      button.textContent = originalText;
    }, 1600);
    showToast({ tone: "error", title: "取消任务失败", message: "详细原因已写入运行日志。" });
  }
}

export async function retryTask(button) {
  const taskId = button.dataset.taskRetry;
  if (!taskId || button.disabled) return;

  const task = (Array.isArray(state.tasks) ? state.tasks : []).find(
    (item) => item && item.task_id === taskId,
  );
  const descriptor = task && task.retry_descriptor;
  const requiresConfirmation = descriptor && descriptor.replay_safety === "requires_confirmation";
  if (requiresConfirmation) {
    const confirmed = await showConfirmDialog({
      eyebrow: "任务重试",
      title: "确认重试任务",
      message: "这个任务的重试可能会打开浏览器窗口、刷新 Cookie 或生成新的 Git 令牌。",
      points: [taskId ? `任务 ID：${taskId}` : "", "重试前请确认当前账号选择和浏览器状态符合预期。"],
      confirmText: "确认重试",
      tone: "warning",
    });
    if (!confirmed) {
      showToast({ tone: "info", title: "已取消重试", message: taskId });
      return;
    }
  }

  const originalText = button.textContent;
  button.disabled = true;
  button.textContent = "重试中";

  try {
    await postJson(endpoints.taskRetry, {
      task_id: taskId,
      confirm_replay: Boolean(requiresConfirmation),
    });
    await refresh();
    showToast({ tone: "success", title: "任务已提交重试", message: taskId });
  } catch (error) {
    button.textContent = "失败";
    recordClientRuntimeError({ scope: "重试任务", route: endpoints.taskRetry, error });
    window.setTimeout(() => {
      button.disabled = false;
      button.textContent = originalText;
    }, 1600);
    showToast({ tone: "error", title: "任务重试失败", message: "详细原因已写入运行日志。" });
  }
}

export async function clearTerminalTasks(event) {
  const button = event.currentTarget;
  if (!button || button.disabled) return;
  if (!taskListActions().includes("clear-terminal")) {
    button.title = "当前后端不支持清除已结束任务";
    return;
  }

  const originalText = button.textContent;
  button.disabled = true;
  button.textContent = "清理中";

  try {
    const report = await postJson(endpoints.clearTerminalTasks, {});
    const clientRemoved = state.clientRuntimeLogs.length;
    state.clientRuntimeLogs = [];
    const tasks = await fetchJson(endpoints.tasks);
    updateTasks(tasks, { updateDashboard: true });
    const taskRemoved = Number(report && report.removed_count) || 0;
    const removed = taskRemoved + clientRemoved;
    button.textContent = removed ? `已清理 ${removed}` : "无需清理";
    showToast({
      tone: "success",
      title: removed ? "已清理结束任务" : "没有可清理任务",
      message: removed
        ? `移除 ${taskRemoved} 个已结束任务和 ${clientRemoved} 条客户端诊断。`
        : "任务列表无需变更。",
    });
    window.setTimeout(() => {
      button.textContent = originalText;
      button.disabled = false;
    }, 1400);
  } catch (error) {
    button.textContent = "失败";
    recordClientRuntimeError({ scope: "清理已结束任务", route: endpoints.clearTerminalTasks, error });
    showToast({ tone: "error", title: "清理任务失败", message: "详细原因已写入运行日志。" });
    window.setTimeout(() => {
      button.textContent = originalText;
      button.disabled = false;
    }, 1600);
  }
}

export function taskCanCancel(task) {
  if (Array.isArray(task && task.available_actions)) {
    return taskHasAction(task, "cancel");
  }
  const phase = String(task.phase || "").toLowerCase();
  return !task.cancel_requested && taskActivePhases().includes(phase);
}

export function taskHasAction(task, action) {
  return Array.isArray(task && task.available_actions) && task.available_actions.includes(action);
}

export function makeTaskId(action, alias) {
  const safeAlias =
    String(alias || "account")
      .replace(/[^a-zA-Z0-9_-]+/g, "-")
      .replace(/^-+|-+$/g, "") || "account";
  return `${action}-${safeAlias}-${Date.now()}`;
}

export function taskProgressPercent(progress) {
  const current = Number(progress.current);
  const total = Number(progress.total);
  if (!Number.isFinite(current) || !Number.isFinite(total) || total <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((current / total) * 100)));
}

export function taskProgressText(progress) {
  const current = Number(progress.current);
  const total = Number(progress.total);
  if (!Number.isFinite(current) || !Number.isFinite(total) || total <= 0) return "";
  return `${Math.max(0, current)}/${total}`;
}

export async function submitTaskInput(event) {
  const form = event.target.closest("[data-task-input-form]");
  if (!form) return;
  event.preventDefault();

  const taskId = form.dataset.taskInputForm;
  const kind = form.dataset.taskInputKind;
  const button = form.querySelector("button");
  if (!taskId || !kind || !button || button.disabled) return;

  if (kind === "new_registration_credentials") {
    const passwordField = form.querySelector('[name="password"]');
    try {
      validateNewPasswordValue(fieldValue(form, "password"), passwordField);
    } catch (error) {
      showToast({ tone: "warning", title: "新密码不符合要求", message: error.message });
      return;
    }
  }

  const originalText = button.textContent;
  button.disabled = true;
  button.textContent = "提交中";

  try {
    const value = taskInputValue(form, kind);
    await postJson(endpoints.taskInput, {
      task_id: taskId,
      kind,
      value,
    });
    clearTaskInputForm(form);
    button.textContent = "已提交";
    await refresh();
  } catch (error) {
    button.textContent = "失败";
    recordClientRuntimeError({ scope: "提交任务输入", route: endpoints.taskInput, error });
    showToast({ tone: "error", title: "提交失败", message: "详细原因已写入运行日志。" });
  } finally {
    window.setTimeout(() => {
      button.disabled = false;
      button.textContent = originalText;
    }, 1600);
  }
}

export function taskInputValue(form, kind) {
  if (kind === "new_registration_credentials") {
    return JSON.stringify({
      alias: fieldValue(form, "alias"),
      email: fieldValue(form, "email"),
      password: fieldValue(form, "password"),
    });
  }
  if (kind === "new_browser_credentials") {
    return JSON.stringify({
      email: fieldValue(form, "email"),
      password: fieldValue(form, "password"),
    });
  }
  if (kind === "captcha_completed") {
    return "completed";
  }
  const input = form.querySelector("input");
  return input ? input.value : "";
}

export function clearTaskInputForm(form) {
  form.reset();
}

export function fieldValue(form, name) {
  const field = form.querySelector(`[name="${name}"]`);
  return field ? field.value : "";
}

export function validateNewPasswordValue(password, field = null) {
  if (Array.from(String(password || "").trim()).length >= MINIMUM_NEW_PASSWORD_LENGTH) return;
  const message = `新密码至少需要 ${MINIMUM_NEW_PASSWORD_LENGTH} 位`;
  if (field) {
    field.setCustomValidity(message);
    field.reportValidity();
    field.setCustomValidity("");
    field.focus();
  }
  throw new Error(message);
}
