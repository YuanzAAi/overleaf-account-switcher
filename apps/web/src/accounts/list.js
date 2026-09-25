import {
  recordClientRuntimeInfo,
  setAccountAssistShortStatus,
  showToast,
} from "../feedback.js";
import { accountActionByOperationKind, endpoints, selectedAliasesInPickOrder, selectedAliasesText, state } from "../state.js";
export { selectedAliasesInPickOrder as selectedAliasesArray, selectedAliasesText } from "../state.js";
import {
  escapeAttr,
  escapeHtml,
  formatUnix,
  gitTokenStatus,
  pill,
  secretStatus,
  subscriptionLabelText,
} from "../format.js";
import { refresh } from "../sync.js";
import { trialUsageMeta } from "./trial.js";
import {
  accountLookupByAlias,
  accountToolbarMigratesProjects,
  isPasswordTool,
  renderAccountWorkbenchHeader,
  renderPasswordSelectionAssist,
  renderPasswordTargetList,
  replaceSelectedAliases,
  selectionBoundAliasInputIds,
  syncSelectionBoundAliasInputs,
} from "../workspace.js";
import { extensionBridgeConnected } from "../settings.js";
import { iconSvg } from "../icons.js";
import {
  accountBulkActionNeedsBridge,
  accountBulkActionTitle,
  accountBulkActions,
  accountRowActions,
  accountSecretActions,
  taskActivePhases,
} from "../capabilities.js";
import {
  scheduleStatusClear,
} from "../tasks/list.js";
import { performAccountBulkAction } from "./actions.js";

export function renderAccounts(accounts) {
  document.getElementById("account-count").textContent = `${accounts.length} 个账号`;
  const body = document.getElementById("accounts-body");
  const grid = document.getElementById("account-card-grid");
  if (!grid) return;
  pruneSelectedAliases(accounts);
  const visibleAccounts = sortAccountsForDisplay(filterAccounts(accounts));
  syncAccountFilterControls();
  renderAccountViewMode();
  renderAccountSelectionStatus(visibleAccounts);
  if (!accounts.length) {
    grid.innerHTML = accountEmptyStateMarkup(
      "还没有 Overleaf 账号",
      "从账号密码登录、Cookie 登录或 JSON 文件导入开始。账号管理会集中显示 Cookie、Git 令牌、密码和订阅状态。",
    );
    if (body) body.innerHTML = `<tr><td colspan="8"><div class="empty">没有账号。</div></td></tr>`;
    return;
  }
  if (!visibleAccounts.length) {
    grid.innerHTML = accountEmptyStateMarkup(
      "没有匹配的账号",
      "换一个关键词、调整筛选规则，或清空搜索后查看全部账号。",
      { searchEmpty: true, query: state.accountSearch },
    );
    if (body)
      body.innerHTML = `<tr><td colspan="8"><div class="empty">没有匹配当前搜索和筛选的账号。</div></td></tr>`;
    return;
  }

  grid.innerHTML = visibleAccounts.map((account) => accountCardMarkup(account)).join("");

  if (body) {
    body.innerHTML = visibleAccounts
      .map(
        (account) => `
        <tr>
          <td><input class="ui-checkbox" type="checkbox" data-account-select="${escapeHtml(account.alias)}" ${state.selectedAliases.has(account.alias) ? "checked" : ""} aria-label="选择 ${escapeHtml(account.alias)}"></td>
          <td><strong>${escapeHtml(account.alias)}</strong>${account.is_current ? ' <span class="pill present">当前</span>' : ""}</td>
          <td>${escapeHtml(account.email || "-")}</td>
          <td>${pill(account.subscription_status || "unknown")}<br><span class="muted">${escapeHtml(subscriptionLabelText(account.subscription_label))}</span></td>
          <td>${secretStatus(account.cookie)}</td>
          <td>${gitTokenStatus(account)}</td>
          <td>${account.password && account.password.present ? pill("present") : pill("missing")}</td>
          <td class="actions">${accountActionButtons(account)}</td>
        </tr>
      `,
      )
      .join("");
  }
  renderAccountSelectionStatus(visibleAccounts);
  syncAccountTaskLocks();
}

export function accountEmptyStateMarkup(title, detail, options = {}) {
  const searchTerm = String(options.query || "").trim();
  const actions = options.searchEmpty
    ? `
      <button class="account-empty-action primary" type="button" data-account-empty-clear-search>查看全部账号</button>
      <button class="account-empty-action" type="button" data-account-tool-open="credentials" data-account-tool-focus="credential-add-aliases">新增账号</button>
    `
    : `
      <button class="account-empty-action primary" type="button" data-account-tool-open="credentials" data-account-tool-focus="credential-add-aliases">账号密码登录</button>
      <button class="account-empty-action" type="button" data-account-tool-open="io" data-account-tool-focus="manual-cookie-entries">Cookie 登录</button>
      <button class="account-empty-action" type="button" data-account-tool-open="io" data-account-tool-focus="import-choose-file">文件导入</button>
      <button class="account-empty-action subtle" type="button" data-open-onboarding>安装引导</button>
    `;
  const steps = options.searchEmpty
    ? `
      <div class="account-empty-steps search">
        <span><strong>搜索词</strong>${escapeHtml(searchTerm || "空")}</span>
        <span><strong>范围</strong>别名 / 邮箱 / 状态</span>
      </div>
    `
    : `
      <div class="account-empty-steps">
        <span><strong>1</strong>添加账号密码或 Cookie</span>
        <span><strong>2</strong>刷新 Cookie / Git 令牌</span>
        <span><strong>3</strong>用卡片执行无感换号</span>
      </div>
    `;
  return `
    <div class="account-empty-card ${options.searchEmpty ? "search-empty" : "no-accounts"}">
      <div class="account-empty-visual" aria-hidden="true">
        <span></span><span></span><span></span>
      </div>
      <div class="account-empty-copy">
        <span class="eyebrow">账号管理</span>
        <strong>${escapeHtml(title)}</strong>
        <p>${escapeHtml(detail)}</p>
        ${steps}
      </div>
      <div class="account-empty-actions">${actions}</div>
    </div>
  `;
}

export function sortAccountsForDisplay(accounts) {
  const decorated = accounts.map((account, index) => ({ account, index }));
  const fallback = (a, b) => a.index - b.index;
  const sortMode = state.accountSort || "import-order";
  if (sortMode === "import-order") {
    decorated.sort(
      (a, b) => accountImportOrderValue(b.account) - accountImportOrderValue(a.account) || fallback(a, b),
    );
    return decorated.map((item) => item.account);
  }
  decorated.sort((a, b) => {
    if (sortMode === "current" && Boolean(a.account.is_current) !== Boolean(b.account.is_current)) {
      return a.account.is_current ? -1 : 1;
    }
    if (sortMode === "alias") {
      return (
        String(a.account.alias || "").localeCompare(String(b.account.alias || ""), "zh-CN", {
          sensitivity: "base",
        }) || fallback(a, b)
      );
    }
    if (sortMode === "trial-remaining-asc" || sortMode === "trial-remaining-desc") {
      const diff = accountRemainingDays(a.account) - accountRemainingDays(b.account);
      return (sortMode === "trial-remaining-desc" ? -diff : diff) || fallback(a, b);
    }
    if (sortMode === "cookie-status") {
      return secretSortRank(a.account.cookie) - secretSortRank(b.account.cookie) || fallback(a, b);
    }
    if (sortMode === "git-status") {
      return secretSortRank(a.account.git_token) - secretSortRank(b.account.git_token) || fallback(a, b);
    }
    return fallback(a, b);
  });
  return decorated.map((item) => item.account);
}

export function renderAccountsLoading() {
  const grid = document.getElementById("account-card-grid");
  const body = document.getElementById("accounts-body");
  if (!grid) return;
  grid.innerHTML = [1, 2, 3].map((_, index) => accountSkeletonCardMarkup(index)).join("");
  if (body) body.innerHTML = `<tr><td colspan="8"><div class="empty">正在加载账号...</div></td></tr>`;
}

export function accountSkeletonCardMarkup(index) {
  return `
    <article class="account-card skeleton-card" aria-hidden="true">
      <div class="account-card-status-strip"></div>
      <div class="skeleton-line skeleton-badge ${index === 0 ? "wide" : ""}"></div>
      <div class="skeleton-head">
        <div class="skeleton-avatar"></div>
        <div class="skeleton-title">
          <span class="skeleton-line title"></span>
          <span class="skeleton-line subtitle"></span>
        </div>
      </div>
      <div class="skeleton-summary">
        <span class="skeleton-line"></span>
        <span class="skeleton-line"></span>
        <span class="skeleton-line"></span>
      </div>
      <div class="skeleton-meter"></div>
      <div class="skeleton-facts">
        <span></span><span></span><span></span><span></span>
      </div>
    </article>
  `;
}

export function renderAccountsError(error) {
  const grid = document.getElementById("account-card-grid");
  const body = document.getElementById("accounts-body");
  if (!grid) return;
  const message = "账号数据暂不可用，请查看运行日志";
  grid.innerHTML = `
    <div class="account-error-card">
      <div class="account-error-icon" aria-hidden="true"></div>
      <div class="account-empty-copy">
        <span class="eyebrow">账号管理</span>
        <strong>账号列表加载失败</strong>
        <p>${escapeHtml(message)}</p>
      </div>
      <div class="account-empty-actions">
        <button class="account-empty-action primary" type="button" id="account-error-refresh">重新加载</button>
        <button class="account-empty-action subtle" type="button" data-open-onboarding>检查扩展引导</button>
      </div>
    </div>
  `;
  if (body) body.innerHTML = `<tr><td colspan="8"><div class="error">${escapeHtml(message)}</div></td></tr>`;
  const retry = document.getElementById("account-error-refresh");
  if (retry) retry.addEventListener("click", refresh);
}

export function filterAccounts(accounts) {
  const query = state.accountSearch.trim().toLowerCase();
  return accounts.filter((account) => {
    if (!accountMatchesFilter(account, state.accountFilter || "all")) {
      return false;
    }
    if (!query) return true;
    const values = [
      account.alias,
      account.email,
      account.subscription_status,
      account.subscription_label,
      account.is_current ? "当前" : "",
      account.cookie && account.cookie.status,
      account.git_token && account.git_token.status,
    ];
    return values.some((value) =>
      String(value || "")
        .toLowerCase()
        .includes(query),
    );
  });
}

export function handleAccountFilterChange(event) {
  state.accountFilter = event.target.value || "all";
  renderAccounts(state.accounts);
}

export function handleAccountSortChange(event) {
  state.accountSort = event.target.value || "import-order";
  renderAccounts(state.accounts);
}

export function syncAccountFilterControls() {
  const filter = document.getElementById("account-filter");
  const sort = document.getElementById("account-sort");
  const searchInput = document.getElementById("account-search");
  const searchActive =
    Boolean(state.accountSearch && state.accountSearch.trim()) || document.activeElement === searchInput;
  if (filter && filter.value !== state.accountFilter) filter.value = state.accountFilter || "all";
  if (sort && sort.value !== state.accountSort) sort.value = state.accountSort || "import-order";
  filter?.closest(".compact-select")?.classList.toggle("active", (state.accountFilter || "all") !== "all");
  sort
    ?.closest(".compact-select")
    ?.classList.toggle("active", (state.accountSort || "import-order") !== "import-order");
  syncCompactSelectMenu(filter, state.accountFilter || "all");
  syncCompactSelectMenu(sort, state.accountSort || "import-order");
  searchInput?.closest(".compact-search-field")?.classList.toggle("active", searchActive);
}

export function syncCompactSelectMenu(select, value) {
  const menu = select && select.closest ? select.closest(".compact-select") : null;
  if (!menu) return;
  menu.querySelectorAll("[data-compact-select-value]").forEach((button) => {
    const active = button.dataset.compactSelectValue === value;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

export function accountMatchesFilter(account, filter) {
  switch (filter) {
    case "active":
      return !accountTrialExpired(account);
    case "expired":
      return accountTrialExpired(account);
    case "trial-7":
      return Number(account && account.trial_days) === 7;
    case "trial-21":
      return Number(account && account.trial_days) === 21;
    case "cookie-invalid":
      return secretNeedsAttention(account && account.cookie);
    case "git-invalid":
      return secretNeedsAttention(account && account.git_token);
    case "git-missing":
      return secretMissing(account && account.git_token);
    case "password-missing":
      return accountPasswordMissing(account);
    case "password-error":
      return accountPasswordHasError(account);
    default:
      return true;
  }
}

export function accountsMatchingStatus(filter) {
  return (Array.isArray(state.accounts) ? state.accounts : [])
    .filter((account) => accountMatchesFilter(account, filter))
    .map((account) => account.alias)
    .filter(Boolean);
}

export function accountTrialExpired(account) {
  const usage = trialUsageMeta(account);
  if (usage) return usage.expired;
  return String((account && account.subscription_status) || "")
    .toLowerCase()
    .includes("expired");
}

export function accountRemainingDays(account) {
  const usage = trialUsageMeta(account);
  if (!usage) return Number.MAX_SAFE_INTEGER;
  return Number.isFinite(usage.remainingDays) ? usage.remainingDays : Number.MAX_SAFE_INTEGER;
}

export function accountImportOrderValue(account) {
  const created = Number(account && account.created_at);
  return Number.isFinite(created) && created > 0 ? created : 0;
}

export function secretNeedsAttention(secret) {
  if (!secret || !secret.present) return true;
  const status = String(secret.status || "").toLowerCase();
  return ["missing", "expired", "failed", "invalid", "stale", "needs_refresh", "needs-refresh"].includes(
    status,
  );
}

export function secretMissing(secret) {
  if (!secret || !secret.present) return true;
  const status = String(secret.status || "").toLowerCase();
  return ["missing", "not_found", "not-found", "empty", "none"].includes(status);
}

export function secretSortRank(secret) {
  if (!secret || !secret.present) return 0;
  const status = String(secret.status || "").toLowerCase();
  if (["expired", "failed", "invalid", "missing"].includes(status)) return 1;
  if (["stale", "needs_refresh", "needs-refresh", "expiring_soon"].includes(status)) return 2;
  return 3;
}

export function accountPasswordHasError(account) {
  const password = account && account.password;
  const status = [
    password && password.status,
    password && password.last_error,
    password && password.error,
    account && account.password_status,
    account && account.password_error,
  ]
    .map((value) => String(value || "").toLowerCase())
    .join(" ");
  return /\b(wrong|invalid|incorrect|failed|error|auth|login|rejected|bad)\b/.test(status);
}

export function accountPasswordMissing(account) {
  return !(account && account.password && account.password.present);
}

export function accountCardMarkup(account) {
  const selected = state.selectedAliases.has(account.alias);
  const selectedOrder =
    selected && isPasswordTool(state.passwordSelectionTool)
      ? selectedAliasesInPickOrder().indexOf(account.alias) + 1
      : 0;
  const subscriptionText = ["unknown", "needs_refresh"].includes(account.subscription_status)
    ? "-"
    : subscriptionLabelText(account.subscription_label || account.subscription_status) || "-";
  const trialUsage = trialUsageMarkup(account);
  const healthClass = accountHealthClass(account);
  const health = accountHealthMeta(account, healthClass);
  const bridgeConnected = extensionBridgeConnected();
  const bridgeUnavailable = !bridgeConnected && !account.is_current;
  const liveLabel = account.is_current
    ? "运行中"
    : bridgeUnavailable
      ? "需扩展"
      : selected
        ? "已选择"
        : "待换号";
  const bridgeNote = bridgeUnavailable
    ? `<div class="account-card-bridge-note"><strong>扩展桥未连接</strong><span>无感换号和浏览器登录暂不可用，账号查看、导入导出和本地维护仍可继续。</span></div>`
    : "";
  const alias = escapeHtml(account.alias || "-");
  const email = escapeHtml(account.email || "-");
  const cardClasses = [
    "account-card",
    "ui-account-card",
    `account-health-${healthClass}`,
    bridgeUnavailable ? "bridge-unavailable" : "",
    account.is_current ? "current" : "",
    selected ? "card-selected" : "",
  ]
    .filter(Boolean)
    .join(" ");
  return `
    <article class="${cardClasses}" data-account-alias="${escapeAttr(account.alias)}">
      ${selectedOrder ? `<span class="account-selection-order-badge">第 ${selectedOrder} 个</span>` : ""}
      <div class="card-header">
        <div class="card-header-left">
          <input class="ui-checkbox account-card-check" type="checkbox" data-account-select="${escapeAttr(account.alias)}" ${selected ? "checked" : ""} aria-label="选择 ${escapeAttr(account.alias)}">
          <span class="avatar-circle ${account.is_current ? "avatar-green" : healthClass === "attention" ? "avatar-orange" : "avatar-blue"}" aria-hidden="true">${escapeHtml(accountInitial(account))}</span>
          <span class="card-title-group">
            <strong class="card-alias" title="${alias}">${alias}</strong>
            <span class="card-email">
              <span title="${email}">${email}</span>
              ${account.email ? `<button class="copy-btn" type="button" data-copy-email="${escapeAttr(account.email)}" title="复制邮箱" aria-label="复制 ${escapeAttr(account.alias)} 的邮箱">${iconSvg("copy")}</button>` : ""}
            </span>
          </span>
        </div>
        ${accountStatusTag(account, liveLabel)}
      </div>
      <div class="card-body">
        <div class="tags-row">
          ${accountSubscriptionBadge(account, subscriptionText)}
          <span class="ui-tag">${escapeHtml(health.label)}</span>
        </div>
        ${trialUsage || trialUsagePlaceholderMarkup()}
        <div class="token-info">
          ${accountSecretTokenItem("Cookie", account.cookie, secretLedClass(account.cookie), "cookie", account.alias)}
          ${accountSecretTokenItem("Git", account.git_token, secretLedClass(account.git_token), "git_token", account.alias)}
          ${accountSecretTokenItem("密码", account.password, account.password && account.password.present ? "on" : "off", "password", account.alias)}
        </div>
        ${bridgeNote}
      </div>
      <div class="card-footer">
        ${accountCardPrimaryAction(account)}
        <div class="card-footer-icons">${accountCardUtilityButtons(account)}</div>
      </div>
    </article>
  `;
}

export function accountSubscriptionBadge(account, label) {
  const labelKey = String((account && account.subscription_label) || "")
    .trim()
    .toLowerCase()
    .replaceAll(" ", "_")
    .replaceAll("-", "_");
  const statusKey = String((account && account.subscription_status) || "")
    .trim()
    .toLowerCase()
    .replaceAll(" ", "_")
    .replaceAll("-", "_");
  let tier = "unknown";
  if (["unknown", "needs_refresh"].includes(statusKey)) {
    tier = "unknown";
  } else if (labelKey === "trial_active") {
    tier = "trial";
  } else if (labelKey === "trial_expired") {
    tier = "expired";
  } else if (["needs_refresh", "unknown"].includes(labelKey)) {
    tier = "unknown";
  } else if (labelKey.includes("pro") || labelKey.includes("professional")) {
    tier = "pro";
  } else if (labelKey === "free") {
    tier = "free";
  } else if (["trial", "trial_active"].includes(statusKey)) {
    tier = "trial";
  } else if (statusKey.includes("expired")) {
    tier = "expired";
  } else if (statusKey === "pro" || statusKey.includes("professional")) {
    tier = "pro";
  } else if (statusKey === "free") {
    tier = "free";
  }
  const text = String(label || "-");
  return `<span class="account-plan-badge account-plan-badge-${tier}" data-subscription-tier="${tier}" title="订阅套餐：${escapeAttr(text)}" aria-label="订阅套餐：${escapeAttr(text)}">${escapeHtml(text)}</span>`;
}

export function accountSecretTokenItem(label, secret, ledClass, field, alias) {
  const present = Boolean(secret && secret.present);
  const hint = present ? secret.masked_hint || "已保存" : "未保存";
  const canCopy = present && accountSecretActions().includes("copy-account-secret");
  const copyButton = canCopy
    ? `<button class="copy-btn copy-btn-compact" type="button" data-copy-account-secret="true" data-secret-alias="${escapeAttr(alias)}" data-secret-field="${escapeAttr(field)}" aria-label="复制 ${escapeAttr(alias)} 的${escapeAttr(label)}">${iconSvg("copy")}</button>`
    : "";
  return `<span class="token-item secret-token-item"><span class="status-led ${ledClass}" aria-hidden="true"></span><span class="secret-token-label">${escapeHtml(label)}</span><span class="secret-token-mask">${escapeHtml(hint)}</span>${copyButton}</span>`;
}

export function accountStatusTag(account, liveLabel) {
  return `<span class="ui-tag account-state${account.is_current ? " account-state-current" : ""}">${escapeHtml(liveLabel)}</span>`;
}

export function secretLedClass(secret) {
  const status = String((secret && secret.status) || "").toLowerCase();
  if (secret && secret.present && !["expired", "failed", "missing"].includes(status)) {
    return ["stale", "needs_refresh", "expiring_soon"].includes(status) ? "warn" : "on";
  }
  return status === "stale" || status === "needs_refresh" || status === "expiring_soon" ? "warn" : "off";
}

export function trialUsageMarkup(account) {
  const usage = trialUsageMeta(account);
  if (!usage) return "";
  const status = usage.expired ? "已过期" : `剩余 ${usage.remainingDays} 天`;
  const expiryText = formatUnix(usage.expiry);
  const heading = usage.totalDays ? "试用有效期" : "订阅有效期";
  const progress = usage.totalDays ? `${usage.usedDays} / ${usage.totalDays} 天` : status;
  return `
    <div class="trial-meter ${usage.expired ? "expired" : "active"}" title="${heading}，${status}，到期 ${escapeAttr(expiryText)}">
      <div class="trial-meter-head">
        <span>${heading}</span>
        <strong>${progress}</strong>
      </div>
      ${usage.percent === null ? "" : `<span class="trial-meter-track" aria-hidden="true">
        <span style="width:${usage.percent.toFixed(1)}%"></span>
      </span>`}
      <span class="trial-meter-foot">
        <span>到期 ${escapeHtml(expiryText)}</span>
        ${usage.totalDays ? `<em>${escapeHtml(status)}</em>` : ""}
      </span>
    </div>
  `;
}

export function trialUsagePlaceholderMarkup() {
  return `
    <div class="trial-meter disabled" title="没有试用进度记录">
      <div class="trial-meter-head">
        <span>无使用进度</span>
      </div>
      <span class="trial-meter-track" aria-hidden="true"><span></span></span>
      <span class="trial-meter-foot"><span>长期计划兜底</span></span>
    </div>
  `;
}

export function accountHealthClass(account) {
  if (account.is_current) return "live";
  const statuses = [
    account.cookie && account.cookie.status,
    account.git_token && account.git_token.status,
    account.subscription_status,
  ].map((value) => String(value || "").toLowerCase());
  if (statuses.some((value) => ["failed", "expired", "missing"].includes(value))) return "attention";
  if (statuses.some((value) => ["stale", "needs_refresh", "expiring_soon"].includes(value))) return "warn";
  if ((account.cookie && account.cookie.present) || (account.git_token && account.git_token.present))
    return "ready";
  return "idle";
}

export function accountHealthMeta(account, healthClass = accountHealthClass(account)) {
  if (healthClass === "live") {
    return { label: "运行中", detail: "当前浏览器会话使用此账号" };
  }
  const missing = [];
  if (!(account.cookie && account.cookie.present)) missing.push("Cookie");
  if (!(account.git_token && account.git_token.present)) missing.push("Git");
  if (!(account.password && account.password.present)) missing.push("密码");
  if (healthClass === "attention") {
    return {
      label: "需处理",
      detail: missing.length ? `缺少 ${missing.join(" / ")}` : "存在过期或失败状态",
    };
  }
  if (healthClass === "warn") {
    return { label: "待刷新", detail: "状态接近过期或需要重新检测" };
  }
  if (healthClass === "ready") {
    return { label: "可用", detail: "核心凭据已保存，可进行常规维护" };
  }
  return { label: "未完成", detail: missing.length ? `待补 ${missing.join(" / ")}` : "等待补充账号状态" };
}

export function accountInitial(account) {
  const text = (account && (account.alias || account.email)) || "?";
  const trimmed = String(text).trim();
  return trimmed ? trimmed[0].toUpperCase() : "?";
}

export function handleAccountSearch(event) {
  state.accountSearch = event.target.value || "";
  renderAccounts(state.accounts);
}

export function setAccountView(view) {
  state.accountView = "cards";
  renderAccountViewMode();
}

export function renderAccountViewMode() {
  const cardsButton = document.getElementById("account-view-cards");
  const tableButton = document.getElementById("account-view-table");
  const grid = document.getElementById("account-card-grid");
  const table = document.getElementById("accounts-table-panel");
  state.accountView = "cards";
  if (cardsButton) cardsButton.setAttribute("aria-pressed", "true");
  if (tableButton) tableButton.setAttribute("aria-pressed", "false");
  if (grid) grid.hidden = false;
  if (table) table.hidden = true;
}

export function accountActionLabel(action) {
  return (
    {
      "refresh-session": "Cookie",
      "refresh-git": "Git",
      "generate-git": "令牌",
      "projects": "项目",
      "switch-plan": "计划",
      "switch-execute": "换号",
    }[action] || action
  );
}

export function accountActionButtons(account) {
  return accountRowActions()
    .map((action) => {
      const needsBridge = action === "switch-execute";
      const unavailable = needsBridge && !extensionBridgeConnected();
      const title = unavailable ? "Chrome 扩展桥连接后可用。" : "";
      return `<button class="inline-action" type="button" data-account-action="${escapeAttr(action)}" data-account-alias="${escapeHtml(account.alias)}" ${unavailable ? "disabled" : ""} title="${escapeHtml(title)}">${escapeHtml(accountActionLabel(action))}</button>`;
    })
    .join("");
}

export function accountCardPrimaryAction(account) {
  if (account.is_current) {
    return `
      <button
        class="ui-btn account-primary-switch active"
        type="button"
        disabled
        data-power-state="running"
        aria-label="当前正在使用 ${escapeAttr(account.alias)}"
        title="当前运行账号"
      >${iconSvg("pause")}运行中</button>
    `;
  }
  const unavailable = !extensionBridgeConnected();
  const migratesProjects = accountToolbarMigratesProjects();
  const migrationText = migratesProjects ? "带项目迁移" : "不迁移项目";
  const title = unavailable ? "Chrome 扩展桥连接后可用。" : `换到 ${account.alias}（${migrationText}）`;
  return `
    <button
      class="ui-btn ui-btn-primary account-primary-switch ${unavailable ? "unavailable" : "ready"}"
      type="button"
      data-account-action="switch-execute"
      data-account-alias="${escapeHtml(account.alias)}"
      data-power-state="${unavailable ? "unavailable" : "ready"}"
      data-migrate-projects="${migratesProjects ? "true" : "false"}"
      ${unavailable ? "disabled" : ""}
      aria-label="换到 ${escapeAttr(account.alias)}，${migrationText}"
      title="${escapeHtml(title)}"
    >${iconSvg("play")}无感换号</button>
  `;
}

export function accountCardUtilityButtons(account) {
  const alias = escapeHtml(account.alias);
  const toolButtons = [
    accountMiniToolButton({
      icon: "local-password",
      title: "更新本地保存密码",
      tool: "passwords",
      focusId: "password-values",
      alias,
    }),
    accountMiniToolButton({
      icon: "remote-password",
      title: "修改 Overleaf 远端密码",
      tool: "overleaf-password",
      focusId: "overleaf-password-values",
      alias,
      danger: true,
    }),
  ];
  const actionButtons = accountRowActions()
    .filter((action) => action !== "switch-execute" && action !== "switch-plan")
    .map((action) => {
      const needsBridge = action === "browser-login";
      const unavailable = needsBridge && !extensionBridgeConnected();
      const title = unavailable ? "Chrome 扩展桥连接后可用。" : accountActionTitle(action);
      return `<button class="icon-btn account-mini-action" type="button" data-account-action="${escapeAttr(action)}" data-account-alias="${alias}" ${unavailable ? "disabled" : ""} title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}">${accountActionIcon(action)}<span class="visually-hidden">${escapeHtml(accountActionLabel(action))}</span></button>`;
    });
  const deleteButton = `<button class="icon-btn account-mini-action danger" type="button" data-account-delete="${alias}" title="删除账号" aria-label="删除 ${alias}">${iconSvg("trash")}<span class="visually-hidden">删除账号</span></button>`;
  return [...toolButtons, ...actionButtons, deleteButton].join("");
}

export function accountMiniToolButton(options) {
  const dangerClass = options.danger ? " danger" : "";
  return `<button class="icon-btn account-mini-action${dangerClass}" type="button" data-account-tool-open="${escapeAttr(options.tool)}" data-account-tool-focus="${escapeAttr(options.focusId)}" data-tool-alias="${options.alias}" title="${escapeHtml(options.title)}" aria-label="${escapeHtml(options.title)}">${toolIconSvg(options.icon)}<span class="visually-hidden">${escapeHtml(options.title)}</span></button>`;
}

export function accountActionTitle(action) {
  return (
    {
      "refresh-session": "刷新 Cookie",
      "refresh-git": "刷新 Git 令牌状态",
      "generate-git": "生成 Git 令牌",
      "projects": "项目管理",
    }[action] || ""
  );
}

export function toolIconSvg(icon) {
  return (
    {
      cookie: iconSvg("cookie"),
      "local-password": iconSvg("edit"),
      "remote-password": iconSvg("settings"),
    }[icon] || iconSvg("circle")
  );
}

export function accountActionIcon(action) {
  return (
    {
      "refresh-session": iconSvg("refresh"),
      "refresh-git": iconSvg("git"),
      "generate-git": iconSvg("key"),
      "projects": iconSvg("folder"),
    }[action] || iconSvg("circle")
  );
}

export function handleAccountSelectionChange(event) {
  const checkbox = event.target;
  if (!(checkbox instanceof HTMLInputElement) || checkbox.type !== "checkbox") {
    return;
  }

  if (checkbox.id === "accounts-select-all") {
    const aliases = allSelectableAliases();
    aliases.forEach((alias) => {
      if (checkbox.checked) {
        state.selectedAliases.add(alias);
      } else {
        state.selectedAliases.delete(alias);
      }
    });
    syncAccountSelectionInputs();
    renderAccountSelectionStatus();
    return;
  }

  const alias = checkbox.dataset.accountSelect;
  if (!alias) return;
  if (checkbox.checked) {
    state.selectedAliases.add(alias);
  } else {
    state.selectedAliases.delete(alias);
  }
  syncAccountSelectionInputs();
  renderAccountSelectionStatus();
}

export function pruneSelectedAliases(accounts) {
  const available = new Set(accounts.map((account) => account.alias));
  const preserveUnknownAliases = selectionBoundAliasInputIds().length > 0;
  for (const alias of Array.from(state.selectedAliases)) {
    if (!available.has(alias) && !preserveUnknownAliases) {
      state.selectedAliases.delete(alias);
    }
  }
}

export function currentSelectableAliases(accounts = null) {
  if (Array.isArray(accounts)) {
    return accounts.map((account) => account.alias).filter(Boolean);
  }
  return Array.from(
    new Set(
      Array.from(document.querySelectorAll("[data-account-select]"))
        .map((input) => input.dataset.accountSelect)
        .filter(Boolean),
    ),
  );
}

export function allSelectableAliases() {
  const accounts = Array.isArray(state.accounts) ? state.accounts : [];
  const aliases = accounts.map((account) => account.alias).filter(Boolean);
  return aliases.length ? aliases : currentSelectableAliases();
}

export function renderAccountSelectionStatus(accounts = null, options = {}) {
  const selected = state.selectedAliases.size;
  const selectableAliases = allSelectableAliases();
  const accountCount = selectableAliases.length;
  const knownSelectedCount = selectableAliases.filter((alias) => state.selectedAliases.has(alias)).length;
  const status = document.getElementById("account-selection-status");
  if (status) {
    status.textContent = `已选 ${selected} 个`;
    status.className = selected ? "account-selection-status has-selection" : "account-selection-status";
  }
  const deck = document.querySelector(".account-command-deck");
  if (deck) {
    deck.classList.toggle("has-selection", selected > 0);
    deck.dataset.selectedCount = String(selected);
  }
  const bulkBar = document.querySelector(".account-bulk-bar");
  if (bulkBar) {
    bulkBar.classList.toggle("has-selection", selected > 0);
    bulkBar.dataset.selectedCount = String(selected);
  }
  syncAccountTaskLocks();
  syncAccountSelectionInputs();
  syncSelectionBoundAliasInputs(options.skipAliasInputId || "");
  renderSelectedAccountPreview();
  renderPasswordSelectionAssist();
  const passwordTool = state.passwordSelectionTool || state.activeAccountTool;
  if (isPasswordTool(passwordTool)) {
    renderPasswordTargetList(passwordTool);
    renderAccountWorkbenchHeader();
  }

  const selectAll = document.getElementById("accounts-select-all");
  if (!selectAll) return;
  selectAll.checked = accountCount > 0 && knownSelectedCount === accountCount;
  selectAll.indeterminate = knownSelectedCount > 0 && knownSelectedCount < accountCount;
  selectAll.disabled = accountCount === 0;
  selectAll.setAttribute("aria-label", accountCount ? `选择全部 ${accountCount} 个账号` : "没有可选账号");
  selectAll.title = accountCount ? `选择全部 ${accountCount} 个账号` : "没有可选账号";
}

export function renderSelectedAccountPreview() {
  const preview = document.getElementById("account-selected-preview");
  if (!preview) return;
  preview.hidden = true;
  preview.innerHTML = "";
}

export function syncAccountSelectionInputs() {
  const lookup = accountLookupByAlias();
  const orderedAliases = selectedAliasesInPickOrder();
  const showOrder = isPasswordTool(state.passwordSelectionTool);
  document.querySelectorAll("[data-account-select]").forEach((input) => {
    const alias = input.dataset.accountSelect;
    const checked = state.selectedAliases.has(alias);
    input.checked = checked;
    const card = input.closest(".account-card");
    if (!card) return;
    card.classList.toggle("card-selected", checked);
    const order = checked && showOrder ? orderedAliases.indexOf(alias) + 1 : 0;
    let badge = card.querySelector(".account-selection-order-badge");
    if (order > 0) {
      if (!badge) {
        badge = document.createElement("span");
        badge.className = "account-selection-order-badge";
        card.prepend(badge);
      }
      badge.textContent = `第 ${order} 个`;
    } else if (badge) {
      badge.remove();
    }
    const account = lookup.get(alias);
    const liveTag = card.querySelector(".card-header > .ui-tag");
    if (account && liveTag && !account.is_current) {
      liveTag.textContent = !extensionBridgeConnected() ? "需扩展" : checked ? "已选择" : "待换号";
    }
  });
}

export function aliasesFromInputOrSelection(inputId, options = {}) {
  const explicit = document.getElementById(inputId).value.trim();
  if (explicit) return explicit;

  const selected = selectedAliasesArray();
  if (!selected.length) return "";
  if (options.single && selected.length !== 1) {
    throw new Error("请选择一个账号，或手动输入一个别名");
  }
  return selected.join(",");
}

export function clearAccountSelection() {
  replaceSelectedAliases([]);
  document.querySelectorAll("[data-account-select]").forEach((input) => {
    input.checked = false;
  });
  renderAccountSelectionStatus();
}

export async function handleAccountBulkAction(button) {
  const action = button.dataset.accountBulkAction;
  const status = document.getElementById("account-action-status");
  if (!action || button.disabled) return;
  if (action === "clear-selection") {
    clearAccountSelection();
    const shortMessage = setAccountAssistShortStatus(status, "已清除选择");
    scheduleStatusClear(status, shortMessage);
    showToast({ tone: "info", title: "已清除选择", message: "账号批量队列已清空。" });
    return;
  }
  if (!accountBulkActions().includes(action)) {
    const shortMessage = setAccountAssistShortStatus(status, "操作不可用");
    scheduleStatusClear(status, shortMessage, 7200);
    showToast({ tone: "warning", title: "操作不可用", message: status.textContent });
    return;
  }
  if (accountBulkActionNeedsBridge(action) && !extensionBridgeConnected()) {
    const shortMessage = "需要扩展桥";
    setAccountAssistShortStatus(status, shortMessage);
    recordClientRuntimeInfo({
      scope: "批量账号操作",
      route: endpoints.browserLogin,
      message: "打开浏览器登录需要先连接 Chrome 扩展桥；其他账号操作不受影响。",
    });
    showToast({ tone: "warning", title: shortMessage, message: "请先在教程页连接 Chrome 扩展桥。" });
    scheduleStatusClear(status, shortMessage);
    return;
  }

  const aliases = selectedAliasesText();
  if (!aliases) {
    const shortMessage = setAccountAssistShortStatus(status, "请至少选择一个账号");
    scheduleStatusClear(status, shortMessage);
    showToast({ tone: "warning", title: "没有选中账号", message: "请先在账号卡片里勾选目标账号。" });
    return;
  }

  await performAccountBulkAction(action, aliases, button);
}

export function selectAccountsByStatus(filter) {
  const aliases = accountsMatchingStatus(filter);
  setSelectedAliases(aliases);
  const status = document.getElementById("account-action-status");
  const text = aliases.length ? `已按状态选中 ${aliases.length} 个账号` : "没有匹配这个状态的账号";
  if (status) {
    const shortMessage = setAccountAssistShortStatus(status, text);
    scheduleStatusClear(status, shortMessage);
  }
  showToast({
    tone: aliases.length ? "success" : "info",
    title: aliases.length ? "已按状态选中" : "没有匹配账号",
    message: text,
  });
}

export function setSelectedAliases(aliases) {
  replaceSelectedAliases(aliases);
  syncAccountSelectionInputs();
  renderAccountSelectionStatus();
}

export function beginActionButtonBusy(button, label = "处理中") {
  const snapshot = {
    disabled: button.disabled,
    html: button.innerHTML,
    ariaBusy: button.getAttribute("aria-busy"),
  };
  const hasIconContent = Boolean(button.querySelector("svg"));
  button.disabled = true;
  button.setAttribute("aria-busy", "true");
  if (button.classList.contains("account-power-button") || hasIconContent) {
    button.classList.add("busy");
  } else {
    button.textContent = label;
  }
  return snapshot;
}

export function restoreActionButtonBusy(button, snapshot) {
  button.disabled = Boolean(snapshot && snapshot.disabled);
  button.classList.remove("busy");
  if (snapshot && snapshot.ariaBusy === null) {
    button.removeAttribute("aria-busy");
  } else if (snapshot && snapshot.ariaBusy) {
    button.setAttribute("aria-busy", snapshot.ariaBusy);
  } else {
    button.removeAttribute("aria-busy");
  }
  if (snapshot && typeof snapshot.html === "string") {
    button.innerHTML = snapshot.html;
  }
}

export function accountActionDescriptor(action, bulk = false) {
  const descriptor = {
    "refresh-session": { endpoint: endpoints.refreshCredentials, title: "刷新 Cookie" },
    "refresh-git": { endpoint: endpoints.gitTokenRefresh, title: "刷新 Git 令牌状态" },
    "generate-git": { endpoint: endpoints.gitTokenGenerate, title: "获取 Git 令牌" },
    "browser-login": { endpoint: endpoints.browserLogin, title: "打开浏览器登录" },
    "projects": { endpoint: endpoints.projects, title: "项目管理" },
    "switch-plan": { endpoint: endpoints.switchPlan, title: "生成换号计划" },
    "switch-execute": { endpoint: endpoints.switchExecute, title: "执行无感换号" },
  }[action];
  if (!descriptor) return null;
  const bulkTitle = bulk ? accountBulkActionTitle(action) : "";
  return {
    ...descriptor,
    title: bulkTitle || descriptor.title,
    scope: bulkTitle || descriptor.title,
  };
}

export function accountActionForOperationKind(operationKind) {
  return accountActionByOperationKind[String(operationKind || "").toLowerCase()] || "";
}

export function activeAccountTaskSnapshots() {
  const activePhases = new Set(taskActivePhases());
  return (Array.isArray(state.tasks) ? state.tasks : []).filter(
    (task) =>
      task &&
      activePhases.has(String(task.phase || "").toLowerCase()) &&
      Array.isArray(task.locked_aliases) &&
      task.locked_aliases.length > 0,
  );
}

export function taskLocksAlias(task, alias) {
  const normalizedAlias = String(alias || "").trim();
  return (
    Boolean(normalizedAlias) &&
    Array.isArray(task && task.locked_aliases) &&
    task.locked_aliases.some((item) => String(item || "").trim() === normalizedAlias)
  );
}

export function activeAccountTaskForAlias(alias, activeTasks = activeAccountTaskSnapshots()) {
  return activeTasks.find((task) => taskLocksAlias(task, alias)) || null;
}

export function setButtonTaskBusy(button, busy) {
  button.classList.toggle("busy", Boolean(busy));
  if (busy) {
    button.setAttribute("aria-busy", "true");
  } else {
    button.removeAttribute("aria-busy");
  }
}

export function syncAccountTaskLocks() {
  const activeTasks = activeAccountTaskSnapshots();
  document.querySelectorAll("[data-account-action][data-account-alias]").forEach((button) => {
    const action = button.dataset.accountAction || "";
    const alias = button.dataset.accountAlias || "";
    const task = activeAccountTaskForAlias(alias, activeTasks);
    const bridgeUnavailable =
      ["browser-login", "switch-execute"].includes(action) && !extensionBridgeConnected();
    button.disabled = bridgeUnavailable || Boolean(task);
    setButtonTaskBusy(button, Boolean(task) && accountActionForOperationKind(task.operation_kind) === action);
  });

  document.querySelectorAll("[data-account-delete]").forEach((button) => {
    button.disabled = Boolean(activeAccountTaskForAlias(button.dataset.accountDelete, activeTasks));
  });
  document.querySelectorAll("[data-tool-alias]").forEach((button) => {
    button.disabled = Boolean(activeAccountTaskForAlias(button.dataset.toolAlias, activeTasks));
  });

  const selectedAliases = selectedAliasesInPickOrder();
  const selectedTasks = activeTasks.filter((task) =>
    selectedAliases.some((alias) => taskLocksAlias(task, alias)),
  );
  document.querySelectorAll("[data-account-bulk-action]").forEach((button) => {
    const action = button.dataset.accountBulkAction || "";
    if (!action || action === "clear-selection") return;
    const supported = accountBulkActions().includes(action);
    const bridgeUnavailable = accountBulkActionNeedsBridge(action) && !extensionBridgeConnected();
    const matchingTask = selectedTasks.find(
      (task) => accountActionForOperationKind(task.operation_kind) === action,
    );
    button.disabled = !supported || bridgeUnavailable || selectedTasks.length > 0;
    setButtonTaskBusy(button, Boolean(matchingTask));
  });
}
