import { resolveTauriWriteClipboardText } from "./bridge.js";

export async function writeClipboard(value) {
  const desktopClipboard = resolveTauriWriteClipboardText();
  if (desktopClipboard) {
    await desktopClipboard(value);
    return;
  }

  if (navigator.clipboard && window.isSecureContext) {
    await navigator.clipboard.writeText(value);
    return;
  }

  const field = document.createElement("textarea");
  field.value = value;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.left = "-9999px";
  document.body.appendChild(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();
  if (!copied) {
    throw new Error("剪贴板不可用");
  }
}

export function downloadTextFile(filename, text, type) {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.style.display = "none";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

export function splitAliasInput(value) {
  return String(value || "")
    .split(/[,\uFF0C]/)
    .map((alias) => alias.trim())
    .filter(Boolean);
}

export function safeDownloadName(value) {
  return (
    String(value || "account")
      .replace(/[^A-Za-z0-9_.-]+/g, "_")
      .replace(/^[._]+|[._]+$/g, "") || "account"
  );
}

export function gitTokenStatus(account) {
  return secretStatus(account.git_token);
}

export function secretStatus(secret) {
  if (!secret || !secret.present) return pill("missing");
  const status = secret.status || "unknown";
  const expiry = secret.expiry ? `<br><span class="muted">${formatUnix(secret.expiry)}</span>` : "";
  return `${pill(status)}${expiry}`;
}

export function subscriptionLabelText(value) {
  const raw = String(value || "").trim();
  if (!raw) return "";
  const key = raw.toLowerCase().replaceAll(" ", "_").replaceAll("-", "_");
  const labels = {
    needs_refresh: "需要刷新",
    trial_active: "试用有效",
    trial_expired: "试用已过期",
    pro_annual: "Pro annual",
    pro_monthly: "Pro monthly",
    professional_annual: "Pro annual",
    professional_monthly: "Pro monthly",
    standard: "Standard",
    standard_annual: "Standard annual",
    standard_monthly: "Standard monthly",
    free: "Free",
    unknown: "未知",
  };
  return labels[key] || raw;
}

export function pill(value) {
  const normalized = String(value || "unknown")
    .toLowerCase()
    .replaceAll(" ", "_")
    .replaceAll("-", "_");
  return `<span class="pill ${escapeAttr(normalized)}">${escapeHtml(statusText(value || "unknown"))}</span>`;
}

export function statusText(value) {
  const key = String(value || "unknown")
    .toLowerCase()
    .replaceAll(" ", "_")
    .replaceAll("-", "_");
  const labels = {
    ok: "正常",
    valid: "有效",
    eligible: "可试用",
    active_trial: "试用中",
    ineligible: "不可试用",
    available: "可用",
    existing_subscription: "已有订阅",
    present: "已保存",
    completed: "已完成",
    pro: "Pro",
    trial: "试用",
    free: "Free",
    unknown: "未知",
    running: "运行中",
    pending: "等待中",
    waiting: "等待中",
    waiting_for_user: "等待用户",
    cancel_requested: "正在取消",
    expiring_soon: "即将过期",
    expired: "已过期",
    missing: "缺失",
    failed: "失败",
    stale: "需刷新",
    candidate: "候选",
    skipped: "已跳过",
    imported: "已导入",
    duplicate: "重复",
    duplicate_email: "重复邮箱",
    alias_conflict: "别名冲突",
    git_token: "Git 令牌",
    needs_refresh: "需要刷新",
    trial_active: "试用有效",
    pro_annual: "Pro annual",
    pro_monthly: "Pro monthly",
  };
  return labels[key] || String(value || "unknown");
}

export function formatUnix(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) return "-";
  return new Date(number * 1000).toLocaleString();
}

export function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

export function escapeAttr(value) {
  return escapeHtml(value);
}
