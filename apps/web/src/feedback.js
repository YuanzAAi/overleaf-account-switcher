import { CLIENT_RUNTIME_LOG_LIMIT, state, statusClearTimers } from "./state.js";
import { escapeAttr, escapeHtml } from "./format.js";
import {
  renderTasks,
  scheduleStatusClear,
} from "./tasks/list.js";
import { closeAccountWorkbench } from "./workspace.js";

export function showToast(options = {}) {
  const id = `toast-${state.nextToastId++}`;
  const tone = options.tone || "info";
  const title = normalizeVisibleUiText(options.title || "状态更新", {
    maxLength: 56,
    diagnosticFallback: "状态更新",
  });
  const message = normalizeVisibleUiText(options.message || "", {
    maxLength: 140,
    diagnosticFallback: "详细信息已写入运行日志。",
  });
  const duration = Number.isFinite(options.duration) ? options.duration : tone === "error" ? 5000 : 2200;

  state.toasts = [{ id, tone, title, message }, ...state.toasts].slice(0, 3);
  renderToasts();

  if (duration > 0) {
    const timer = window.setTimeout(() => dismissToast(id), duration);
    state.toastTimers.set(id, timer);
  }
  return id;
}

export function reportUiOperationFailure({
  status,
  title,
  scope,
  route,
  error,
}) {
  setAccountAssistShortStatus(status, "");
  recordClientRuntimeError({ scope, route, error });
  showToast({
    tone: "error",
    title,
    message: "详细原因已写入运行日志。",
  });
}

export function reportUiOperationStart({ status, shortMessage, scope, route, message }) {
  setAccountAssistShortStatus(status, shortMessage);
  recordClientRuntimeInfo({ scope, route, message });
}

export function reportUiOperationSuccess({
  status,
  shortMessage,
  title,
  scope,
  route,
  message,
  closeAssist = false,
  tone = "success",
  toastMessage,
  clearAfter = 4600,
}) {
  const visibleMessage = setAccountAssistShortStatus(status, shortMessage);
  if (status) {
    if (!closeAssist) scheduleStatusClear(status, visibleMessage, clearAfter);
  }
  recordClientRuntimeInfo({ scope, route, message });
  showToast({
    tone,
    title,
    message: toastMessage || "详细结果已写入运行日志。",
  });
  if (closeAssist) closeAccountWorkbench();
}

export function setAccountAssistShortStatus(status, message) {
  const visibleMessage = normalizeVisibleUiText(message, {
    maxLength: 48,
    diagnosticFallback: "详细信息已写入运行日志",
  });
  if (status) {
    const existingTimer = statusClearTimers.get(status);
    if (existingTimer) {
      window.clearTimeout(existingTimer);
      statusClearTimers.delete(status);
    }
    status.textContent = visibleMessage;
  }
  return visibleMessage;
}

export function setTransientUiStatus(status, message, clearAfter = 4600) {
  const visibleMessage = setAccountAssistShortStatus(status, message);
  if (status && visibleMessage && clearAfter > 0) {
    scheduleStatusClear(status, visibleMessage, clearAfter);
  }
  return visibleMessage;
}

export function clearTransientUiStatus(status) {
  if (!status) return;
  setAccountAssistShortStatus(status, "");
}

export function clearAccountAssistTransientStatuses() {
  [
    "credential-status",
    "io-status",
    "password-update-status",
    "overleaf-password-status",
    "password-selection-live-status",
  ].forEach((id) => {
    clearTransientUiStatus(document.getElementById(id));
  });
}

export function recordClientRuntimeError({ scope, route, error }) {
  return recordClientRuntimeLog({
    level: "error",
    scope,
    route,
    message: sanitizeClientRuntimeMessage(error),
  });
}

export function recordClientRuntimeInfo({ scope, route, message }) {
  return recordClientRuntimeLog({
    level: "info",
    scope,
    route,
    message: sanitizeClientRuntimeMessage(message),
  });
}

export function recordClientRuntimeLog({ level, scope, route, message }) {
  const entry = {
    id: `client-log-${state.nextClientRuntimeLogId++}`,
    createdAt: Date.now(),
    level: level || "info",
    scope: normalizeVisibleUiText(scope || "界面操作", { maxLength: 72 }) || "界面操作",
    route: sanitizeClientRuntimeRoute(route),
    message: sanitizeClientRuntimeMessage(message || "-"),
  };
  state.clientRuntimeLogs = [...state.clientRuntimeLogs, entry].slice(-CLIENT_RUNTIME_LOG_LIMIT);
  renderTasks(state.tasks);
  return entry;
}

export function sanitizeClientRuntimeRoute(route) {
  return redactDiagnosticPaths(String(route || "-"))
    .replace(/(overleaf_session2\s*=\s*)[^;,\s]+/gi, "$1[hidden]")
    .slice(0, 160);
}

export function sanitizeClientRuntimeMessage(error) {
  const raw = error && error.message ? error.message : String(error || "未知错误");
  return redactDiagnosticPaths(raw)
    .replace(/(overleaf_session2\s*=\s*)[^;,\s]+/gi, "$1[hidden]")
    .replace(/\bolp_[a-z0-9._-]+\b/gi, "[hidden]")
    .replace(/\b(password|cookie|token|secret|cvc|card_number)\b(\s*[:=]\s*)([^,;\s]+)/gi, "$1$2[hidden]")
    .replace(/\b\d{12,19}\b/g, (value) => `${value.slice(0, 6)}******${value.slice(-4)}`)
    .slice(0, 360);
}

export function normalizeVisibleUiText(value, options = {}) {
  const raw = value && value.message ? value.message : String(value || "");
  const normalized = raw.replace(/\s+/g, " ").trim();
  if (!normalized) return "";

  const sanitized = sanitizeClientRuntimeMessage(normalized).replace(/\s+/g, " ").trim();
  const diagnosticFallback = String(options.diagnosticFallback || "").trim();
  const visible =
    diagnosticFallback && visibleUiTextContainsDiagnostics(normalized, sanitized)
      ? diagnosticFallback
      : sanitized;
  const maxLength = Number.isFinite(options.maxLength) ? Math.max(8, options.maxLength) : 72;
  return visible.length > maxLength ? `${visible.slice(0, maxLength - 3)}...` : visible;
}

export function visibleUiTextContainsDiagnostics(raw, sanitized) {
  return (
    sanitized.includes("[path hidden]") ||
    sanitized.includes("[hidden]") ||
    /(^|[\s[(])\/[a-z][a-z0-9._-]*(?:\/[a-z0-9._?=&-]+)*/i.test(raw) ||
    /\b(?:GET|POST|PUT|PATCH|DELETE)\s+\/\S+/i.test(raw) ||
    /\bHTTP(?:\/\d(?:\.\d)?)?\s*\d{3}\b/i.test(raw) ||
    /\b(?:status|status_code)\s*[:=]\s*\d{3}\b/i.test(raw)
  );
}

export function redactDiagnosticPaths(value) {
  return String(value || "")
    .replace(/\b[A-Za-z]:\\[^,\r\n;)}\]]+/g, "[path hidden]")
    .replace(/(^|[\s(])\\\\[^,\r\n;)}\]]+/g, "$1[path hidden]")
    .replace(/(^|[\s(])\/(?:[^\/\s,;)}\]]+\/){2,}[^,\s;)}\]]+/g, "$1[path hidden]");
}

export function dismissToast(id) {
  if (!id) return;
  const timer = state.toastTimers.get(id);
  if (timer) window.clearTimeout(timer);
  state.toastTimers.delete(id);
  state.toasts = state.toasts.filter((toast) => toast.id !== id);
  const card = document.querySelector(`[data-toast-id="${id}"]`);
  if (!card || card.dataset.closing) return;
  card.dataset.closing = "true";
  const duration = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 180;
  card.animate(
    [{ opacity: 1, transform: "translateY(0)" }, { opacity: 0, transform: "translateY(-6px)" }],
    { duration, easing: "ease-in", fill: "forwards" },
  ).finished.then(() => card.remove());
}

export function renderToasts() {
  const region = document.getElementById("toast-region");
  if (!region) return;
  const activeIds = new Set(state.toasts.map((toast) => toast.id));
  for (const card of region.children) {
    if (!activeIds.has(card.dataset.toastId)) dismissToast(card.dataset.toastId);
  }
  // 保留已有节点，避免新通知让其他通知重新播放入场动画。
  for (const toast of [...state.toasts].reverse()) {
    if (region.querySelector(`[data-toast-id="${toast.id}"]`)) continue;
    region.insertAdjacentHTML("afterbegin", `
    <article class="toast-card ${escapeAttr(toast.tone)}" data-toast-id="${escapeAttr(toast.id)}">
      <span class="toast-icon status-led" aria-hidden="true"></span>
      <div class="toast-copy">
        <strong>${escapeHtml(toast.title)}</strong>
        ${toast.message ? `<span>${escapeHtml(toast.message)}</span>` : ""}
      </div>
      <button class="toast-close icon-btn" type="button" data-toast-close="${escapeAttr(toast.id)}" aria-label="关闭通知" title="关闭通知">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M18 6 6 18"></path>
          <path d="m6 6 12 12"></path>
        </svg>
      </button>
    </article>
  `);
  }
}

export function handleToastClick(event) {
  const closeButton = event.target.closest("[data-toast-close]");
  if (!closeButton) return;
  dismissToast(closeButton.dataset.toastClose || "");
}

export function showConfirmDialog(options = {}) {
  const modal = document.getElementById("confirm-modal");
  if (!modal) {
    return Promise.resolve(false);
  }

  if (state.confirmDialogResolver) {
    closeConfirmDialog(false);
  }

  const eyebrow = document.getElementById("confirm-eyebrow");
  const title = document.getElementById("confirm-title");
  const message = document.getElementById("confirm-message");
  const points = document.getElementById("confirm-points");
  const cancel = document.getElementById("confirm-cancel");
  const submit = document.getElementById("confirm-submit");
  const tone = options.tone || "warning";

  if (eyebrow) eyebrow.textContent = options.eyebrow || "需要确认";
  if (title) title.textContent = options.title || "确认操作";
  if (message) message.textContent = options.message || "此操作需要你确认后继续。";
  if (cancel) cancel.textContent = options.cancelText || "取消";
  if (submit) submit.textContent = options.confirmText || "确认继续";
  if (points) {
    const items = Array.isArray(options.points) ? options.points.filter(Boolean) : [];
    points.hidden = items.length === 0;
    points.innerHTML = items.map((item) => `<li>${escapeHtml(item)}</li>`).join("");
  }

  modal.dataset.tone = tone;
  modal.hidden = false;
  window.setTimeout(() => {
    if (submit) submit.focus();
  }, 0);

  return new Promise((resolve) => {
    state.confirmDialogResolver = resolve;
  });
}

export function closeConfirmDialog(confirmed) {
  const modal = document.getElementById("confirm-modal");
  if (modal) modal.hidden = true;
  const resolver = state.confirmDialogResolver;
  state.confirmDialogResolver = null;
  if (resolver) resolver(Boolean(confirmed));
}
