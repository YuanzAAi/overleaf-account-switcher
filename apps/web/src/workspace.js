import {
  showToast,
  reportUiOperationFailure,
  reportUiOperationStart,
  reportUiOperationSuccess,
  setAccountAssistShortStatus,
  setTransientUiStatus,
  clearAccountAssistTransientStatuses,
  recordClientRuntimeError,
  closeConfirmDialog,
} from "./feedback.js";
import {
  MINIMUM_NEW_PASSWORD_LENGTH,
  UI_THEME_STORAGE_KEY,
  accountMenuCloseTimers,
  endpoints,
  selectedAliasesInPickOrder,
  selectedAliasesText as selectedAliasText,
  state,
} from "./state.js";
export { selectedAliasesInPickOrder, selectedAliasesText as selectedAliasText } from "./state.js";
import {
  cancelTask,
  clearPasswordToolInputs,
  makeTaskId,
  retryTask,
  scheduleStatusClear,
  isActiveTaskSnapshot,
} from "./tasks/list.js";
import { clearRegistrationAccountInputs } from "./registration.js";
import { escapeAttr, escapeHtml, subscriptionLabelText } from "./format.js";
import { postTaskResult, refresh } from "./sync.js";
import { accountRemovalActions } from "./capabilities.js";
import {
  accountInitial,
  clearAccountSelection,
  handleAccountBulkAction,
  renderAccountSelectionStatus,
  renderAccounts,
  selectAccountsByStatus,
  selectedAliasesText,
  setSelectedAliases,
  syncAccountFilterControls,
  syncAccountSelectionInputs,
} from "./accounts/list.js";
import { showOnboarding } from "./settings.js";
import { copyAccountEmail, copyAccountSecret, handleAccountAction } from "./accounts/actions.js";
import { handleAddressAction, handleCardAction, toggleCardBin } from "./resources.js";

export function refreshAccountWorkbench() {
  const button = document.getElementById("account-toolbar-refresh");
  if (!button || button.disabled) return;
  button.disabled = true;
  button.classList.add("spinning");
  const status = document.getElementById("account-action-status");
  reportUiOperationStart({
    status,
    shortMessage: "正在刷新",
    scope: "刷新账号工作台",
    route: "/dashboard",
    message: "正在重新读取账号、任务和扩展状态。",
  });
  refresh({ refreshSubscriptions: true })
    .then(() => {
      reportUiOperationSuccess({
        status,
        shortMessage: "刷新完成",
        title: "账号工作台已刷新",
        scope: "刷新账号工作台",
        route: "/dashboard",
        message: "账号、任务和扩展状态已重新读取。",
      });
    })
    .catch((error) => {
      reportUiOperationFailure({
        status,
        shortMessage: "刷新失败，请查看运行日志",
        title: "刷新账号工作台失败",
        scope: "刷新账号工作台",
        route: "/dashboard",
        error,
      });
    })
    .finally(() => {
      button.disabled = false;
      button.classList.remove("spinning");
    });
}

export function openAccountDeleteFlow(aliases = "") {
  const selected = aliasesForAccountTool(aliases);
  const status = document.getElementById("account-action-status");
  if (!selected.length) {
    setTransientUiStatus(status, "请先选择要删除的账号");
    showToast({
      tone: "warning",
      title: "没有选中账号",
      message: "请先勾选账号，或在单张卡片上点击删除图标。",
    });
    return;
  }
  state.accountDeleteAliases = selected;
  renderAccountDeleteModal(selected);
  const modal = document.getElementById("account-delete-modal");
  if (modal) modal.hidden = false;
}

export function closeAccountDeleteModal() {
  state.accountDeleteAliases = [];
  const modal = document.getElementById("account-delete-modal");
  if (modal) modal.hidden = true;
}

export function handleAccountDeleteBackdropClick(event) {
  if (event.target && event.target.id === "account-delete-modal") {
    closeAccountDeleteModal();
  }
}

export function renderAccountDeleteModal(aliases) {
  const list = document.getElementById("account-delete-list");
  if (!list) return;
  const lookup = accountLookupByAlias();
  list.innerHTML = aliases
    .map((alias, index) => {
      const account = lookup.get(alias);
      const email = account && account.email ? account.email : "未读取邮箱";
      const subscription = account
        ? subscriptionLabelText(account.subscription_label) || account.subscription_status || "未知订阅"
        : "账号未加载";
      const cookie = account && account.cookie && account.cookie.present ? "Cookie 已保存" : "Cookie 缺失";
      return `
      <label class="account-delete-row">
        <span class="account-delete-index">${index + 1}</span>
        <span class="account-delete-meta">
          <strong>${escapeHtml(alias)}</strong>
          <span>${escapeHtml(email)}</span>
          <em>${escapeHtml(subscription)} · ${escapeHtml(cookie)}</em>
        </span>
        <span class="account-delete-cleanup">
          <input type="checkbox" data-delete-cleanup="${escapeHtml(alias)}">
          清理项目
        </span>
      </label>
    `;
    })
    .join("");
}

export async function submitAccountDeleteModal() {
  const aliases = state.accountDeleteAliases.slice();
  const button = document.getElementById("account-delete-submit");
  const status = document.getElementById("remove-status") || document.getElementById("account-action-status");
  if (!aliases.length || !button || button.disabled) return;
  const actions = accountRemovalActions();
  if (!actions.includes("remove-local")) {
    setTransientUiStatus(status, "操作不可用");
    recordClientRuntimeError({
      scope: "删除账号",
      route: endpoints.removeAccount,
      error: new Error("当前后端不支持本地删除"),
    });
    showToast({ tone: "warning", title: "删除不可用", message: "当前后端不支持本地删除账号。" });
    return;
  }
  const cleanupAliases = Array.from(document.querySelectorAll("[data-delete-cleanup]:checked"))
    .map((input) => input.dataset.deleteCleanup)
    .filter(Boolean);
  if (cleanupAliases.length && !actions.includes("cleanup-remote-and-remove")) {
    setTransientUiStatus(status, "清理项目删除不可用");
    recordClientRuntimeError({
      scope: "删除账号",
      route: endpoints.removeAccount,
      error: new Error("当前后端不支持清理项目后删除"),
    });
    showToast({ tone: "warning", title: "清理项目不可用", message: "请取消清理项目选项后再删除。" });
    return;
  }

  button.disabled = true;
  button.textContent = "删除中";
  reportUiOperationStart({
    status,
    shortMessage: "正在删除",
    scope: "删除账号",
    route: endpoints.removeAccount,
    message: `已提交 ${aliases.length} 个账号的删除任务。`,
  });

  try {
    const cleanupSet = new Set(cleanupAliases);
    const localOnlyAliases = aliases.filter((alias) => !cleanupSet.has(alias));
    const reports = [];
    if (localOnlyAliases.length) {
      reports.push(
        await postTaskResult(endpoints.removeAccount, {
          aliases: localOnlyAliases.join(","),
          cleanup_remote_projects: false,
          confirm_local_only: true,
          task_id: makeTaskId("remove-account", localOnlyAliases.join(",")),
        }),
      );
    }
    if (cleanupAliases.length) {
      reports.push(
        await postTaskResult(endpoints.removeAccount, {
          aliases: cleanupAliases.join(","),
          cleanup_remote_projects: true,
          confirm_local_only: true,
          task_id: makeTaskId("clean-remove", cleanupAliases.join(",")),
        }),
      );
    }
    const removed = reports.reduce((sum, report) => sum + Number(report.removed_count || 0), 0);
    const failed = reports.reduce((sum, report) => sum + Number(report.failed_count || 0), 0);
    const incomplete = reports.reduce((sum, report) => sum + Number(report.incomplete_count || 0), 0);
    const hasIncomplete = failed > 0 || incomplete > 0;
    reportUiOperationSuccess({
      status,
      shortMessage: hasIncomplete ? "删除未完全完成" : "删除完成",
      title: "账号删除完成",
      scope: "删除账号",
      route: endpoints.removeAccount,
      message: `已删除 ${removed} 个，未完成 ${incomplete} 个，失败 ${failed} 个。`,
      tone: hasIncomplete ? "warning" : "success",
    });
    closeAccountDeleteModal();
    clearAccountSelection();
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "删除失败，请查看运行日志",
      title: "账号删除失败",
      scope: "删除账号",
      route: endpoints.removeAccount,
      error,
    });
  } finally {
    button.disabled = false;
    button.textContent = "确认删除";
  }
}

export function handleConfirmBackdropClick(event) {
  if (event.target && event.target.id === "confirm-modal") {
    closeConfirmDialog(false);
  }
}

export function handleConfirmDialogKeydown(event) {
  const deleteModal = document.getElementById("account-delete-modal");
  if (deleteModal && !deleteModal.hidden && event.key === "Escape") {
    event.preventDefault();
    closeAccountDeleteModal();
    return;
  }
  const modal = document.getElementById("confirm-modal");
  if (!modal || modal.hidden || !state.confirmDialogResolver) return;
  if (event.key === "Escape") {
    event.preventDefault();
    closeConfirmDialog(false);
  }
}

export function setupAccountWorkbench() {
  const workbench = document.getElementById("account-workbench");
  if (!workbench) return;
  document.querySelectorAll("[data-account-tool]").forEach((section) => {
    section.classList.remove("band");
    section.classList.add("account-tool-card");
    section.hidden = true;
    workbench.appendChild(section);
  });
}

export function setupPageNavigation() {
  document.querySelectorAll("[data-page-target]").forEach((button) => {
    button.addEventListener("click", () => setActivePage(button.dataset.pageTarget || "accounts"));
  });
  setActivePage(state.activePage || "accounts", { preserveScroll: true });
}

export function setupSidebarToggle() {
  const button = document.getElementById("sidebar-toggle");
  if (!button) return;
  button.addEventListener("click", toggleSidebarCompact);
  button.addEventListener("pointerenter", () => button.classList.add("is-hovered"));
  button.addEventListener("pointerleave", () => button.classList.remove("is-hovered"));
  button.addEventListener("blur", () => button.classList.remove("is-hovered"));
}

export function setupAccountMenus() {
  document.querySelectorAll(".account-add-menu, .account-status-menu, .compact-select").forEach((menu) => {
    const summary = menu.querySelector("summary");
    if (!summary) return;
    const openMenu = () => {
      cancelAccountMenuClose(menu);
      closeAccountMenus(menu);
      menu.open = true;
    };
    const scheduleClose = () => scheduleAccountMenuClose(menu);
    menu.addEventListener("mouseenter", openMenu);
    menu.addEventListener("mouseleave", scheduleClose);
    menu.addEventListener("focusin", openMenu);
    menu.addEventListener("focusout", scheduleClose);
    summary.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      openMenu();
    });
  });
  document.addEventListener("click", (event) => {
    if (!event.target.closest(".account-add-menu, .account-status-menu, .compact-select")) {
      closeAccountMenus();
    }
  });
}

export function closeAccountMenus(exceptMenu = null) {
  document.querySelectorAll(".account-add-menu, .account-status-menu, .compact-select").forEach((menu) => {
    if (menu === exceptMenu) return;
    cancelAccountMenuClose(menu);
    menu.open = false;
  });
}

export function closeAccountMenuForElement(element) {
  const menu =
    element && element.closest
      ? element.closest(".account-add-menu, .account-status-menu, .compact-select")
      : null;
  if (menu) {
    cancelAccountMenuClose(menu);
    menu.open = false;
  }
}

export function scheduleAccountMenuClose(menu) {
  cancelAccountMenuClose(menu);
  const timer = window.setTimeout(() => {
    const panel = menu.querySelector(
      ".account-add-menu-panel, .account-status-menu-panel, .compact-select-panel",
    );
    const focusInPanel = panel ? panel.contains(document.activeElement) : false;
    if (!menu.matches(":hover") && !focusInPanel) {
      menu.open = false;
    }
    accountMenuCloseTimers.delete(menu);
  }, 120);
  accountMenuCloseTimers.set(menu, timer);
}

export function cancelAccountMenuClose(menu) {
  const timer = accountMenuCloseTimers.get(menu);
  if (timer) {
    window.clearTimeout(timer);
    accountMenuCloseTimers.delete(menu);
  }
}

export function setupUiTheme() {
  let savedTheme = "light";
  try {
    savedTheme = window.localStorage.getItem(UI_THEME_STORAGE_KEY) || "light";
  } catch (_) {
    savedTheme = "light";
  }
  applyUiTheme(savedTheme === "dark" ? "dark" : "light", { animate: false });
}

export function applyUiTheme(theme, options = {}) {
  const nextTheme = theme === "dark" ? "dark" : "light";
  const button = document.getElementById("theme-toggle");
  const update = () => {
    document.body.dataset.theme = nextTheme;
    if (!button) return;
    const isDark = nextTheme === "dark";
    button.setAttribute("aria-label", isDark ? "切换为日间模式" : "切换为夜间模式");
    button.title = isDark ? "切换为日间模式" : "切换为夜间模式";
  };
  if (!options.animate) {
    update();
    return;
  }
  if (button) {
    const bounds = button.getBoundingClientRect();
    document.documentElement.style.setProperty("--theme-x", `${((bounds.left + bounds.width / 2) / innerWidth) * 100}%`);
    document.documentElement.style.setProperty("--theme-y", `${((bounds.top + bounds.height / 2) / innerHeight) * 100}%`);
  }
  document.documentElement.classList.remove("theme-target-dark", "theme-target-light");
  document.documentElement.classList.add("theme-transitioning", `theme-target-${nextTheme}`);
  update();
  const finish = () => document.documentElement.classList.remove("theme-transitioning", `theme-target-${nextTheme}`);
  window.setTimeout(finish, 680);
}

export function toggleUiTheme() {
  const nextTheme = document.body.dataset.theme === "dark" ? "light" : "dark";
  applyUiTheme(nextTheme, { animate: true });
  try {
    window.localStorage.setItem(UI_THEME_STORAGE_KEY, nextTheme);
  } catch (_) {
    // 受限 WebView 可能禁用本地存储。
  }
}

export function toggleSidebarCompact() {
  const compact = !document.body.classList.contains("sidebar-collapsed");
  document.body.classList.toggle("sidebar-collapsed", compact);
  const button = document.getElementById("sidebar-toggle");
  if (button) {
    button.setAttribute("aria-label", compact ? "展开侧边栏" : "收起侧边栏");
    button.title = compact ? "展开侧边栏" : "收起侧边栏";
  }
}

export function setActivePage(page, options = {}) {
  const nextPage = ["dashboard", "accounts", "registration", "resources", "settings"].includes(page)
    ? page
    : "accounts";
  if (nextPage === "registration" && !options.preserveScroll
    && !state.tasks.some((task) => task.operation_kind === "account_registration" && isActiveTaskSnapshot(task))) {
    clearRegistrationAccountInputs();
  }
  state.activePage = nextPage;
  const nav = document.querySelector(".sidebar-nav");
  const navIndex = ["dashboard", "accounts", "registration", "resources", "settings"].indexOf(nextPage);
  nav?.style.setProperty("--active-nav-index", String(Math.max(0, navIndex)));
  document.querySelectorAll("[data-page-target]").forEach((button) => {
    const active = button.dataset.pageTarget === nextPage;
    button.classList.toggle("active", active);
    button.setAttribute("aria-current", active ? "page" : "false");
  });
  document.querySelectorAll("[data-app-page]").forEach((panel) => {
    const active = panel.dataset.appPage === nextPage;
    panel.hidden = !active;
    panel.classList.toggle("active", active);
  });
  const content = document.querySelector(".app-content");
  if (content && !options.preserveScroll) {
    content.scrollTo({ top: 0, behavior: "auto" });
  }
}

export function handleAccountToolOpen(event) {
  const button = event.currentTarget;
  const tool = button && button.dataset.accountToolOpen;
  const focusId = button && button.dataset.accountToolFocus;
  if (!tool) return;
  const aliases = focusId === "export-aliases" ? selectedAliasesText() : "";
  openAccountTool(tool, { focusId, aliases });
  closeAccountMenuForElement(button);
}

export function handleAccountCardToolClick(event) {
  const target = event.target instanceof Element ? event.target : null;
  if (!target) return;
  const button = target.closest("[data-account-tool-open]");
  if (!button) return;
  event.preventDefault();
  const tool = button.dataset.accountToolOpen;
  const focusId = button.dataset.accountToolFocus;
  const alias = button.dataset.toolAlias || "";
  if (alias) setSelectedAliases([alias]);
  if (isPasswordTool(tool)) {
    openPasswordBatchTool(tool);
    return;
  }
  if (alias) clearPasswordSelectionMode();
  openAccountTool(tool, { focusId, aliases: alias });
}

export function handleAccountCardSelectionClick(event) {
  const target = event.target instanceof Element ? event.target : null;
  if (!target || accountCardClickHitsActionControl(target)) return;
  const card = target.closest(".account-card[data-account-alias]");
  if (!card) return;
  toggleAccountCardSelection(card.dataset.accountAlias || "");
}

export function accountCardClickHitsActionControl(target) {
  return Boolean(
    target.closest(
      [
        "button",
        "a",
        "input",
        "select",
        "textarea",
        "summary",
        "[role='button']",
        "[contenteditable='true']",
        "[data-account-action]",
        "[data-account-tool-open]",
        "[data-account-delete]",
        "[data-copy-email]",
        "[data-copy-account-secret]",
      ].join(","),
    ),
  );
}

export function toggleAccountCardSelection(alias) {
  if (!alias) return;
  if (state.selectedAliases.has(alias)) {
    state.selectedAliases.delete(alias);
  } else {
    state.selectedAliases.add(alias);
  }
  syncAccountSelectionInputs();
  renderAccountSelectionStatus();
}

export function openAccountTool(tool, options = {}) {
  setActivePage("accounts", { preserveScroll: true });
  const escapedTool =
    typeof CSS !== "undefined" && typeof CSS.escape === "function"
      ? CSS.escape(tool)
      : String(tool).replace(/"/g, '\\"');
  const target = document.querySelector(`[data-account-tool="${escapedTool}"]`);
  if (!target) return;
  if (!options.recoveryTaskId) resetAccountEntryForms();
  const nextFocusId = options.focusId || "";
  if (state.activeAccountTool !== tool || state.activeAccountToolFocusId !== nextFocusId) {
    clearAccountAssistTransientStatuses();
  }
  const mode = accountAssistModeForFocus(tool, options.focusId || "");
  const usesSelection = accountToolUsesSelection(tool, mode);
  if (options.aliases) {
    replaceSelectedAliases(splitAliasText(options.aliases));
  } else if (!usesSelection) {
    replaceSelectedAliases([]);
  }
  state.activeAccountTool = tool;
  state.activeAccountToolFocusId = options.focusId || "";
  state.browserCredentialRecoveryAssistTaskId = options.recoveryTaskId || "";
  if (!isPasswordTool(tool)) clearPasswordSelectionMode();
  document.querySelectorAll("[data-account-tool]").forEach((section) => {
    const showInlineSection = !isAccountAssistTool(tool) && section === target;
    section.hidden = !showInlineSection;
    section.classList.toggle("active", showInlineSection);
  });
  prefillAccountToolAliases(tool);
  renderPasswordTargetList(tool);
  renderAccountSelectionStatus();
  renderAccountWorkbenchHeader();
  syncAccountAssistEntryControls();
  const assist = document.getElementById("password-selection-assist");
  (isAccountAssistTool(tool) && assist ? assist : target).scrollIntoView({
    block: "nearest",
    behavior: "smooth",
  });
  if (options.focusId) {
    window.setTimeout(() => document.getElementById(options.focusId)?.focus(), 60);
  }
}

export function closeAccountWorkbench() {
  resetAccountEntryForms();
  state.activeAccountTool = "";
  state.activeAccountToolFocusId = "";
  state.browserCredentialRecoveryAssistTaskId = "";
  clearAccountAssistTransientStatuses();
  clearPasswordSelectionMode();
  clearAccountToolAliasInputs();
  replaceSelectedAliases([]);
  restoreAccountAssistFormsToSections();
  document.querySelectorAll("[data-account-tool]").forEach((section) => {
    section.hidden = true;
    section.classList.remove("active");
  });
  renderAccountSelectionStatus();
  renderAccountWorkbenchHeader();
  syncAccountAssistEntryControls();
}

export function resetAccountEntryForms() {
  ["credential-add-form", "credential-refresh-form", "manual-cookie-form"].forEach((id) => {
    const form = document.getElementById(id);
    if (!form) return;
    form.reset();
    form.querySelectorAll("button[type=submit]").forEach((button) => {
      delete button.dataset.taskId;
      button.disabled = false;
    });
  });
  clearAccountAssistTransientStatuses();
}

export function isPasswordTool(tool) {
  return tool === "passwords" || tool === "overleaf-password";
}

export function isAccountAssistTool(tool) {
  return tool === "credentials" || tool === "io";
}

export function passwordToolShortLabel(tool) {
  return tool === "overleaf-password" ? "远端改密" : "本地密码";
}

export function setPasswordSelectionMode(tool) {
  if (!isPasswordTool(tool)) return;
  const previousTool = state.passwordSelectionTool;
  if (isPasswordTool(previousTool) && previousTool !== tool) {
    clearPasswordToolInputs(previousTool);
  }
  state.passwordSelectionTool = tool;
  const section = document.getElementById("account-section");
  if (section) {
    section.classList.add("password-selection-mode");
    section.dataset.passwordSelectionTool = tool;
  }
  syncAccountSelectionInputs();
  renderPasswordSelectionAssist();
  const status = document.getElementById("account-action-status");
  if (status && !state.selectedAliases.size) {
    status.textContent = `已进入${passwordToolShortLabel(tool)}选择模式，请按顺序勾选账号`;
  }
}

export function clearPasswordSelectionMode() {
  const previousTool = state.passwordSelectionTool;
  if (isPasswordTool(previousTool)) {
    clearPasswordToolInputs(previousTool);
  }
  restorePasswordFormsToSections();
  state.passwordSelectionTool = "";
  const section = document.getElementById("account-section");
  if (section) {
    section.classList.remove("password-selection-mode");
    delete section.dataset.passwordSelectionTool;
  }
  renderPasswordSelectionAssist();
}

export function openPasswordBatchTool(tool) {
  if (!isPasswordTool(tool)) return;
  if (state.passwordSelectionTool !== tool || state.activeAccountTool) {
    clearAccountAssistTransientStatuses();
  }
  state.activeAccountTool = "";
  state.activeAccountToolFocusId = "";
  state.browserCredentialRecoveryAssistTaskId = "";
  document.querySelectorAll("[data-account-tool]").forEach((section) => {
    section.hidden = true;
    section.classList.remove("active");
  });
  setPasswordSelectionMode(tool);
  renderPasswordTargetList(tool);
  renderAccountWorkbenchHeader();
  const focusId = tool === "overleaf-password" ? "overleaf-password-row-0" : "password-row-0";
  const assist = document.getElementById("password-selection-assist");
  if (state.selectedAliases.size && assist) {
    assist.scrollIntoView({ block: "start", behavior: "smooth" });
  }
  syncAccountAssistEntryControls();
  window.setTimeout(() => document.getElementById(focusId)?.focus(), 80);
  if (!state.selectedAliases.size) {
    document.getElementById("account-card-grid")?.scrollIntoView({ block: "start", behavior: "smooth" });
  }
}

export function handlePasswordInputFocus(event) {
  const target = event.target;
  if (!(target instanceof HTMLElement) || !target.matches("[data-password-row]")) return;
  window.setTimeout(() => ensureElementAboveRuntimeLog(target), 60);
}

export function ensureElementAboveRuntimeLog(element) {
  const dock = document.querySelector(".runtime-log-dock");
  if (!dock || !element) return;
  const dockRect = dock.getBoundingClientRect();
  const elementRect = element.getBoundingClientRect();
  const desiredBottom = dockRect.top - 14;
  if (elementRect.bottom <= desiredBottom) return;
  window.scrollBy({
    top: elementRect.bottom - desiredBottom,
    behavior: "smooth",
  });
}

export function renderAccountWorkbenchHeader() {
  const header = document.getElementById("account-workbench-header");
  const title = document.getElementById("account-workbench-title");
  const context = document.getElementById("account-workbench-context");
  if (!header || !title || !context) return;
  const tool = state.activeAccountTool;
  header.hidden = !tool || isAccountAssistTool(tool);
  if (!tool) {
    title.textContent = "账号工具";
    context.innerHTML = "";
    return;
  }
  title.textContent = accountToolTitle(tool);
  const aliases = selectedAliasText();
  const migrateText = accountToolbarMigratesProjects() ? "迁移项目：开启" : "迁移项目：关闭";
  const migrateTagClass = accountToolbarMigratesProjects() ? "ui-tag ui-tag-blue" : "ui-tag";
  context.innerHTML = `
    ${accountToolTargetContextMarkup(tool, aliases)}
    <span class="${migrateTagClass}">${escapeHtml(migrateText)}</span>
  `;
}

export function renderPasswordSelectionAssist() {
  const assist = document.getElementById("password-selection-assist");
  if (!assist) return;
  const tool = state.passwordSelectionTool;
  if (!isPasswordTool(tool)) {
    if (isAccountAssistTool(state.activeAccountTool)) {
      restorePasswordFormsToSections();
      renderAccountInputAssist(state.activeAccountTool);
      return;
    }
    restorePasswordFormsToSections();
    restoreAccountAssistFormsToSections();
    assist.classList.remove("account-input-assist");
    delete assist.dataset.accountAssistTool;
    assist.hidden = true;
    assist.innerHTML = "";
    syncAccountAssistEntryControls();
    return;
  }

  restoreAccountAssistFormsToSections();
  restorePasswordFormsToSections();
  assist.classList.remove("account-input-assist");
  delete assist.dataset.accountAssistTool;
  syncAccountAssistEntryControls();
  const aliases = selectedAliasesInPickOrder();
  const lookup = accountLookupByAlias();
  const modeLabel = passwordToolShortLabel(tool);
  const stepText = aliases.length
    ? "队列已按选择顺序生成。继续点卡片可追加账号，下面逐行输入一号一密码。"
    : "先在下方账号管理区按顺序勾选目标账号，勾选顺序就是密码输入顺序。";
  const miniCards = aliases.length
    ? aliases
        .map((alias, index) => {
          const account = lookup.get(alias);
          const email = account && account.email ? account.email : "未读取邮箱";
          const subscription = account
            ? subscriptionLabelText(account.subscription_label) || account.subscription_status || "未知订阅"
            : "账号未加载";
          return `
          <span class="password-selection-mini-card" title="${escapeAttr(`${alias} · ${email}`)}">
            <span class="password-selection-order">${index + 1}</span>
            <span class="password-selection-mini-meta">
              <strong>${escapeHtml(alias)}</strong>
              <span>${escapeHtml(email)}</span>
              <em>${escapeHtml(subscription)}</em>
            </span>
          </span>
        `;
        })
        .join("")
    : `
        <span class="password-selection-placeholder"><span>1</span><span>2</span><span>3</span></span>
      `;

  assist.hidden = false;
  assist.innerHTML = `
    <div class="password-selection-copy">
      <span class="eyebrow">批量${escapeHtml(modeLabel)}</span>
      <strong>${aliases.length ? `已排队 ${aliases.length} 个账号` : "等待选择账号"}</strong>
      <p>${escapeHtml(stepText)}</p>
    </div>
    <div class="password-selection-mini-list" aria-label="当前密码队列">
      ${miniCards}
    </div>
    <div class="password-selection-form-panel">
      <div id="password-selection-form-slot" class="password-selection-form-slot"></div>
      <span id="password-selection-live-status" class="muted password-selection-live-status"></span>
    </div>
    <div class="password-selection-actions">
      <button class="inline-action" type="button" data-password-selection-clear ${aliases.length ? "" : "disabled"}>清空队列</button>
      <button class="inline-action" type="button" data-password-selection-cancel>退出编排</button>
    </div>
  `;
  mountPasswordFormIntoAssist(tool);
  renderPasswordTargetList(tool);
  syncAccountAssistEntryControls();
}

export function restorePasswordFormsToSections() {
  ["passwords", "overleaf-password"].forEach((tool) => {
    const config = passwordToolConfig(tool);
    if (!config) return;
    const form = document.getElementById(config.formId);
    const section = document.querySelector(config.sectionSelector);
    if (!form || !section || form.parentElement === section) return;
    form.classList.remove("password-assist-form");
    section.appendChild(form);
  });
}

export function mountPasswordFormIntoAssist(tool) {
  const config = passwordToolConfig(tool);
  const slot = document.getElementById("password-selection-form-slot");
  if (!config || !slot) return;
  const form = document.getElementById(config.formId);
  if (!form) return;
  form.classList.add("password-assist-form");
  slot.appendChild(form);
}

export function accountAssistConfig(tool) {
  if (tool === "credentials") {
    return {
      eyebrow: "账号输入",
      title: "账号密码 / Cookie",
      detail: "账号密码登录、按别名刷新 Cookie 都在这里完成；表单提交仍走原有后端流程。",
      statusId: "credential-status",
      statusSectionSelector: '[data-account-tool="credentials"] .section-header',
      formContainerSelector: '[data-account-tool="credentials"] .io-grid',
      modes: [
        {
          id: "credential-add",
          label: "账号密码登录",
          detail: "别名、邮箱、密码写入账号库",
          formId: "credential-add-form",
          focusId: "credential-add-aliases",
          focusIds: ["credential-add-aliases", "credential-add-emails", "credential-add-passwords"],
          usesAccountSelection: false,
        },
        {
          id: "credential-refresh",
          label: "刷新 Cookie",
          detail: "用本地账号密码刷新登录态",
          formId: "credential-refresh-form",
          focusId: "credential-refresh-aliases",
          focusIds: ["credential-refresh-aliases", "credential-refresh-passwords"],
          usesAccountSelection: true,
        },
      ],
    };
  }
  if (tool === "io") {
    return {
      eyebrow: "导入 / 导出",
      title: "文件导入 / 导出",
      detail: "JSON 文件、Cookie 导入和账号导出集中在这里；无需再滚到卡片下方。",
      statusId: "io-status",
      statusSectionSelector: '[data-account-tool="io"] .section-header',
      formContainerSelector: '[data-account-tool="io"] .io-grid',
      modes: [
        {
          id: "json-import",
          label: "JSON 导入",
          detail: "选择 JSON 文件并导入",
          formId: "import-form",
          focusId: "import-choose-file",
          focusIds: ["import-choose-file"],
          usesAccountSelection: false,
        },
        {
          id: "cookie-import",
          label: "Cookie 登录",
          detail: "按别名、邮箱和 Cookie（空格分隔）批量导入",
          formId: "manual-cookie-form",
          focusId: "manual-cookie-entries",
          focusIds: ["manual-cookie-entries"],
          usesAccountSelection: false,
        },
        {
          id: "export",
          label: "导出账号",
          detail: "选择账号、模式、目录或下载 JSON",
          formId: "export-form",
          focusId: "export-aliases",
          focusIds: ["export-aliases", "export-mode", "export-output-dir"],
          usesAccountSelection: true,
        },
      ],
    };
  }
  return null;
}

export function accountAssistModeForFocus(tool, focusId = "") {
  const config = accountAssistConfig(tool);
  if (!config) return null;
  if (focusId) {
    const matched = config.modes.find((mode) => mode.focusIds.includes(focusId) || mode.formId === focusId);
    if (matched) return matched;
  }
  return config.modes[0] || null;
}

export function accountAssistModeById(tool, modeId = "") {
  const config = accountAssistConfig(tool);
  if (!config) return null;
  return config.modes.find((mode) => mode.id === modeId) || config.modes[0] || null;
}

export function currentAccountAssistMode() {
  if (!isAccountAssistTool(state.activeAccountTool)) return null;
  return accountAssistModeForFocus(state.activeAccountTool, state.activeAccountToolFocusId);
}

export function accountAssistModeIsActive(tool, modeId) {
  if (state.activeAccountTool !== tool) return false;
  const mode = currentAccountAssistMode();
  return Boolean(mode && mode.id === modeId);
}

export function accountAssistStatusForMode(tool, modeId, status) {
  return accountAssistModeIsActive(tool, modeId) ? status : null;
}

export function accountToolUsesSelection(tool, mode = null) {
  if (isPasswordTool(tool) || tool === "switch" || tool === "remove") return true;
  if (!isAccountAssistTool(tool)) return false;
  const resolvedMode = mode || currentAccountAssistMode();
  return Boolean(resolvedMode && resolvedMode.usesAccountSelection);
}

export function syncAccountAssistEntryControls() {
  const mode = currentAccountAssistMode();
  const activeModeId = mode ? mode.id : "";
  const addModeIds = new Set(["credential-add", "credential-refresh", "cookie-import"]);
  document.querySelectorAll("[data-account-assist-entry]").forEach((control) => {
    const entry = control.dataset.accountAssistEntry || "";
    const active =
      Boolean(activeModeId) && (entry === activeModeId || (entry === "add" && addModeIds.has(activeModeId)));
    control.classList.toggle("active", active);
    control.classList.toggle("account-assist-entry-active", active);
    control.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

export function restoreAccountAssistFormsToSections() {
  ["credentials", "io"].forEach((tool) => {
    const config = accountAssistConfig(tool);
    if (!config) return;
    const formContainer = document.querySelector(config.formContainerSelector);
    if (formContainer) {
      config.modes.forEach((mode) => {
        const form = document.getElementById(mode.formId);
        if (!form) return;
        form.classList.remove("password-assist-form", "account-assist-form");
        formContainer.appendChild(form);
      });
    }
    const status = document.getElementById(config.statusId);
    const statusSection = document.querySelector(config.statusSectionSelector);
    if (status && statusSection && status.parentElement !== statusSection) {
      status.classList.remove("account-assist-live-status");
      statusSection.appendChild(status);
    }
  });
}

export function renderAccountInputAssist(tool) {
  const assist = document.getElementById("password-selection-assist");
  const config = accountAssistConfig(tool);
  if (!assist || !config) return;

  restoreAccountAssistFormsToSections();
  const mode = accountAssistModeForFocus(tool, state.activeAccountToolFocusId);
  if (!mode) return;
  const aliases = mode.usesAccountSelection ? selectedAliasesInPickOrder() : [];
  const modeButtons = config.modes
    .map(
      (item) => `
    <button class="account-assist-mode${item.id === mode.id ? " active" : ""}" type="button" data-account-assist-mode="${escapeAttr(item.id)}">
      <strong>${escapeHtml(item.label)}</strong>
      <span>${escapeHtml(item.detail)}</span>
    </button>
  `,
    )
    .join("");

  assist.hidden = false;
  assist.classList.add("account-input-assist");
  assist.dataset.accountAssistTool = tool;
  assist.innerHTML = `
    <div class="password-selection-copy">
      <span class="eyebrow">${escapeHtml(config.eyebrow)}</span>
      <strong>${escapeHtml(config.title)}</strong>
      <p>${escapeHtml(config.detail)}</p>
      ${aliases.length ? `<span class="password-target-count">${aliases.length} 个已选账号</span>` : ""}
    </div>
    <div class="account-assist-mode-list" aria-label="${escapeAttr(config.title)}模式">
      ${modeButtons}
    </div>
    <div class="password-selection-form-panel account-assist-form-panel">
      <div id="account-assist-form-slot" class="password-selection-form-slot account-assist-form-slot"></div>
      <div id="account-assist-status-slot" class="account-assist-status-slot"></div>
    </div>
    <div class="password-selection-actions">
      <button class="inline-action" type="button" data-account-assist-cancel>收起</button>
    </div>
  `;

  mountAccountAssistStatus(config);
  mountAccountAssistForm(mode);
  prefillAccountToolAliases(tool);
  syncAccountAssistEntryControls();
}

export function mountAccountAssistStatus(config) {
  const slot = document.getElementById("account-assist-status-slot");
  const status = document.getElementById(config.statusId);
  if (!slot || !status) return;
  status.classList.add("account-assist-live-status");
  slot.appendChild(status);
}

export function mountAccountAssistForm(mode) {
  const slot = document.getElementById("account-assist-form-slot");
  const form = document.getElementById(mode.formId);
  if (!slot || !form) return;
  form.classList.add("password-assist-form", "account-assist-form");
  slot.appendChild(form);
}

export function setPasswordToolStatus(tool, message, clearAfter = 0) {
  const statusId = tool === "overleaf-password" ? "overleaf-password-status" : "password-update-status";
  const status = document.getElementById(statusId);
  const liveStatus = document.getElementById("password-selection-live-status");
  const targets = [status];
  if (liveStatus && state.passwordSelectionTool === tool) targets.push(liveStatus);
  let visibleMessage = "";
  for (const target of targets.filter(Boolean)) {
    visibleMessage = setAccountAssistShortStatus(target, message);
    if (clearAfter > 0) scheduleStatusClear(target, visibleMessage, clearAfter);
  }
  return visibleMessage;
}

export function accountToolTitle(tool) {
  return (
    {
      switch: "无感换号",
      credentials: "账号密码 / Cookie",
      io: "导入 / 导出",
      passwords: "本地密码维护",
      "overleaf-password": "Overleaf 远端改密",
      remove: "删除账号",
    }[tool] || "账号工具"
  );
}

export function accountToolEmptyContext(tool) {
  return (
    {
      switch: "从卡片播放按钮或选择账号后换号",
      credentials: "添加账号或用密码刷新 Cookie",
      io: "JSON 文件和 Cookie 导入",
      passwords: "选择账号后批量更新本地密码",
      "overleaf-password": "选择账号后修改远端密码",
      remove: "选择账号后删除本地记录，可选清理项目",
    }[tool] || "未指定账号"
  );
}

export function splitAliasText(value) {
  return String(value || "")
    .split(/[\n,，]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}

export function replaceSelectedAliases(aliases) {
  state.selectedAliases.clear();
  aliases.filter(Boolean).forEach((alias) => state.selectedAliases.add(alias));
}

export function selectionBoundAliasInputIds() {
  if (state.passwordSelectionTool === "passwords") return ["password-aliases"];
  if (state.passwordSelectionTool === "overleaf-password") return ["overleaf-password-aliases"];
  if (state.activeAccountTool === "switch") return ["switch-alias"];
  if (state.activeAccountTool === "remove") return ["remove-aliases"];
  const mode = currentAccountAssistMode();
  if (!mode || !mode.usesAccountSelection) return [];
  if (mode.id === "credential-refresh") return ["credential-refresh-aliases"];
  if (mode.id === "export") return ["export-aliases"];
  return [];
}

export function syncSelectionBoundAliasInputs(skipInputId = "") {
  const value = selectedAliasText();
  selectionBoundAliasInputIds().forEach((id) => {
    const input = document.getElementById(id);
    if (!input) return;
    if (id !== skipInputId) input.value = value;
    syncAliasInputValidity(input);
  });
}

export function syncAliasInputValidity(input) {
  if (!(input instanceof HTMLInputElement)) return;
  const lookup = accountLookupByAlias();
  const unknown = splitAliasText(input.value).filter((alias) => !lookup.has(alias));
  input.classList.toggle("account-alias-invalid", unknown.length > 0);
  input.setAttribute("aria-invalid", unknown.length ? "true" : "false");
  input.title = unknown.length ? `未找到账号：${unknown.join("、")}` : "";
}

export function handleAccountAliasInput(event) {
  const input = event.target;
  if (!(input instanceof HTMLInputElement)) return;
  if (!selectionBoundAliasInputIds().includes(input.id)) return;
  replaceSelectedAliases(splitAliasText(input.value));
  syncAccountSelectionInputs();
  renderAccountSelectionStatus(null, { skipAliasInputId: input.id });
}

export function clearAccountToolAliasInputs() {
  [
    "switch-alias",
    "credential-refresh-aliases",
    "export-aliases",
    "password-aliases",
    "overleaf-password-aliases",
    "remove-aliases",
  ].forEach((id) => {
    const input = document.getElementById(id);
    if (!input) return;
    input.value = "";
    input.classList.remove("account-alias-invalid");
    input.setAttribute("aria-invalid", "false");
    input.title = "";
  });
}

export function aliasesForAccountTool(explicitAliases = "") {
  const explicit = splitAliasText(explicitAliases);
  if (explicit.length) return explicit;
  return selectedAliasesInPickOrder();
}

export function accountLookupByAlias() {
  return new Map((state.accounts || []).map((account) => [account.alias, account]));
}

export function accountToolTargetContextMarkup(tool, aliasesText) {
  const aliases = splitAliasText(aliasesText);
  if (!aliases.length) {
    return `<span class="ui-tag muted-chip">${escapeHtml(accountToolEmptyContext(tool))}</span>`;
  }
  const lookup = accountLookupByAlias();
  const chips = aliases
    .slice(0, 4)
    .map((alias, index) => {
      const account = lookup.get(alias);
      const label = account && account.email ? `${alias} · ${account.email}` : alias;
      const chipClass = account ? "ui-tag ui-tag-blue" : "ui-tag ui-tag-red";
      const title = account ? label : `未找到账号：${alias}`;
      return `<span class="${chipClass} target-chip" title="${escapeAttr(title)}">${index + 1}. ${escapeHtml(alias)}</span>`;
    })
    .join("");
  const more =
    aliases.length > 4 ? `<span class="ui-tag target-chip muted-chip">+${aliases.length - 4}</span>` : "";
  return `<span class="ui-tag ui-tag-blue target-chip count-chip">${aliases.length} 个账号</span>${chips}${more}`;
}

export function prefillAccountToolAliases() {
  syncSelectionBoundAliasInputs();
}

export function passwordToolConfig(tool) {
  if (tool === "passwords") {
    return {
      formId: "password-form",
      sectionSelector: '[data-account-tool="passwords"]',
      submitId: "password-submit",
      targetListId: "password-target-list",
      aliasesInputId: "password-aliases",
      passwordInputId: "password-values",
      rowPrefix: "password-row",
      empty: "先在账号管理区勾选目标账号，或从单个账号点“本地密码”。",
      placeholder: "输入本地保存的新密码",
    };
  }
  if (tool === "overleaf-password") {
    return {
      formId: "overleaf-password-form",
      sectionSelector: '[data-account-tool="overleaf-password"]',
      submitId: "overleaf-password-submit",
      targetListId: "overleaf-password-target-list",
      aliasesInputId: "overleaf-password-aliases",
      passwordInputId: "overleaf-password-values",
      rowPrefix: "overleaf-password-row",
      empty: "先勾选要修改远端密码的账号，或从单张卡片点“远端改密”。",
      placeholder: "输入 Overleaf 新密码",
    };
  }
  return null;
}

export function renderPasswordTargetList(tool, explicitAliases = "") {
  const config = passwordToolConfig(tool);
  if (!config) return;
  const list = document.getElementById(config.targetListId);
  const aliasesInput = document.getElementById(config.aliasesInputId);
  const passwordInput = document.getElementById(config.passwordInputId);
  if (!list || !aliasesInput || !passwordInput) return;

  const preservedValues = new Map(
    Array.from(document.querySelectorAll(`[data-password-row="${tool}"]`)).map((row) => [
      row.dataset.passwordAlias || "",
      row.value,
    ]),
  );

  const aliases = aliasesForAccountTool(explicitAliases);
  aliasesInput.value = aliases.join(",");
  passwordInput.value = "";

  if (!aliases.length) {
    list.innerHTML = `
      <div class="password-target-empty">
        <strong>${escapeHtml(passwordToolShortLabel(tool))}需要先选择账号</strong>
        <span>${escapeHtml(config.empty)} 当前队列会跟随卡片勾选顺序实时更新。</span>
      </div>
    `;
    return;
  }

  const lookup = accountLookupByAlias();
  list.innerHTML = `
    <div class="password-target-head">
      <div>
        <strong>${tool === "overleaf-password" ? "远端密码修改队列" : "本地密码维护队列"}</strong>
        <span>按卡片选择顺序逐行输入；只填写第一行时，该密码会复用到全部账号。</span>
      </div>
      <span class="password-target-count">${aliases.length} 个</span>
    </div>
    <div class="password-target-stack">
      ${aliases
        .map((alias, index) => {
          const account = lookup.get(alias);
          const email = account && account.email ? account.email : "-";
          const subscriptionText = account
            ? subscriptionLabelText(account.subscription_label) || account.subscription_status || "未知订阅"
            : "账号未加载";
          return `
          <label class="password-target-card" for="${config.rowPrefix}-${index}">
            <span class="password-target-index">${index + 1}</span>
            <span class="account-avatar-chip small" aria-hidden="true">${escapeHtml(account ? accountInitial(account) : alias.slice(0, 1).toUpperCase())}</span>
            <span class="password-target-meta">
              <strong>${escapeHtml(alias)}</strong>
              <span>${escapeHtml(email)}</span>
              <em>${escapeHtml(subscriptionText)}</em>
            </span>
            <input id="${config.rowPrefix}-${index}" data-password-row="${escapeAttr(tool)}" data-password-alias="${escapeAttr(alias)}" type="password" minlength="${MINIMUM_NEW_PASSWORD_LENGTH}" autocomplete="new-password" placeholder="${escapeAttr(config.placeholder)}">
          </label>
        `;
        })
        .join("")}
    </div>
  `;
  document.querySelectorAll(`[data-password-row="${tool}"]`).forEach((row) => {
    const alias = row.dataset.passwordAlias || "";
    if (preservedValues.has(alias)) {
      row.value = preservedValues.get(alias) || "";
    }
  });
}

export function collectPasswordTargetList(tool) {
  const config = passwordToolConfig(tool);
  if (!config) return null;
  const rows = Array.from(document.querySelectorAll(`[data-password-row="${tool}"]`));
  if (!rows.length) return null;

  const aliases = [];
  const passwords = [];
  rows.forEach((row) => {
    aliases.push(row.dataset.passwordAlias || "");
    passwords.push(row.value);
  });
  const firstPassword = String(passwords[0] || "").trim();
  const remainingAreEmpty = passwords.slice(1).every((value) => !String(value || "").trim());
  if (rows.length > 1 && firstPassword && remainingAreEmpty) {
    passwords.fill(firstPassword);
  }
  const missingIndex = passwords.findIndex((value) => !String(value || "").trim());
  if (missingIndex >= 0) {
    rows[missingIndex].focus();
    throw new Error(`请填写第 ${missingIndex + 1} 个账号的密码`);
  }
  const tooShortIndex = passwords.findIndex(
    (value) => Array.from(String(value || "").trim()).length < MINIMUM_NEW_PASSWORD_LENGTH,
  );
  if (tooShortIndex >= 0) {
    rows[tooShortIndex].focus();
    throw new Error(`第 ${tooShortIndex + 1} 个账号的新密码至少需要 ${MINIMUM_NEW_PASSWORD_LENGTH} 位`);
  }

  document.getElementById(config.aliasesInputId).value = aliases.join(",");
  document.getElementById(config.passwordInputId).value = passwords.join(",");
  return { aliases, passwords };
}

export function syncToolbarMigrationOption(event) {
  const checked = Boolean(event.currentTarget && event.currentTarget.checked);
  const switchMigrate = document.getElementById("switch-migrate");
  if (switchMigrate) switchMigrate.checked = checked;
  renderAccountWorkbenchHeader();
  renderAccounts(state.accounts);
}

export function accountToolbarMigratesProjects() {
  const toolbar = document.getElementById("account-toolbar-migrate");
  return Boolean(toolbar && toolbar.checked);
}

export function handlePageClick(event) {
  const onboardingButton = event.target.closest("[data-open-onboarding]");
  if (onboardingButton) {
    showOnboarding();
    return;
  }

  const accountEmptyClearButton = event.target.closest("[data-account-empty-clear-search]");
  if (accountEmptyClearButton) {
    state.accountSearch = "";
    const searchInput = document.getElementById("account-search");
    if (searchInput) {
      searchInput.value = "";
      searchInput.focus();
    }
    renderAccounts(state.accounts);
    return;
  }

  const compactSelectButton = event.target.closest("[data-compact-select-target][data-compact-select-value]");
  if (compactSelectButton) {
    const select = document.getElementById(compactSelectButton.dataset.compactSelectTarget || "");
    if (select) {
      select.value = compactSelectButton.dataset.compactSelectValue || "";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      syncAccountFilterControls();
    }
    closeAccountMenuForElement(compactSelectButton);
    return;
  }

  const secretButton = event.target.closest("[data-copy-account-secret]");
  if (secretButton) {
    event.preventDefault();
    event.stopPropagation();
    copyAccountSecret(secretButton.dataset.secretAlias, secretButton.dataset.secretField, secretButton);
    return;
  }

  const emailButton = event.target.closest("[data-copy-email]");
  if (emailButton) {
    event.preventDefault();
    event.stopPropagation();
    copyAccountEmail(emailButton.dataset.copyEmail, emailButton);
    return;
  }

  const passwordBatchButton = event.target.closest("[data-password-batch-open]");
  if (passwordBatchButton) {
    const tool = passwordBatchButton.dataset.passwordBatchOpen;
    if (tool) openPasswordBatchTool(tool);
    closeAccountMenuForElement(passwordBatchButton);
    return;
  }

  const accountAssistModeButton = event.target.closest("[data-account-assist-mode]");
  if (accountAssistModeButton) {
    const tool = state.activeAccountTool;
    const mode = accountAssistModeById(tool, accountAssistModeButton.dataset.accountAssistMode || "");
    if (mode) {
      state.activeAccountToolFocusId = mode.focusId;
      if (mode.usesAccountSelection) {
        const input = document.getElementById(mode.focusId);
        replaceSelectedAliases(splitAliasText(input && input.value));
      } else {
        replaceSelectedAliases([]);
      }
      renderAccountSelectionStatus();
      window.setTimeout(() => document.getElementById(mode.focusId)?.focus(), 60);
    }
    return;
  }

  const accountAssistCancel = event.target.closest("[data-account-assist-cancel]");
  if (accountAssistCancel) {
    closeAccountWorkbench();
    const status = document.getElementById("account-action-status");
    if (status) status.textContent = "已收起账号输入面板";
    return;
  }

  const passwordSelectionClear = event.target.closest("[data-password-selection-clear]");
  if (passwordSelectionClear) {
    clearAccountSelection();
    const status = document.getElementById("account-action-status");
    if (status) status.textContent = "已清空密码队列，请重新按顺序选择账号";
    return;
  }

  const passwordSelectionCancel = event.target.closest("[data-password-selection-cancel]");
  if (passwordSelectionCancel) {
    closeAccountWorkbench();
    const status = document.getElementById("account-action-status");
    if (status) status.textContent = "已退出密码编排";
    return;
  }

  const selectionRemoveButton = event.target.closest("[data-account-selection-remove]");
  if (selectionRemoveButton) {
    const alias = selectionRemoveButton.dataset.accountSelectionRemove;
    if (alias) {
      state.selectedAliases.delete(alias);
      syncAccountSelectionInputs();
      renderAccountSelectionStatus();
      renderPasswordTargetList(state.passwordSelectionTool || state.activeAccountTool);
      renderAccountWorkbenchHeader();
    }
    return;
  }

  const bulkButton = event.target.closest("[data-account-bulk-action]");
  if (bulkButton) {
    handleAccountBulkAction(bulkButton);
    return;
  }

  const statusSelectButton = event.target.closest("[data-account-status-select]");
  if (statusSelectButton) {
    selectAccountsByStatus(statusSelectButton.dataset.accountStatusSelect);
    closeAccountMenuForElement(statusSelectButton);
    return;
  }

  const accountDeleteButton = event.target.closest("[data-account-delete]");
  if (accountDeleteButton) {
    const alias = accountDeleteButton.dataset.accountDelete;
    openAccountDeleteFlow(alias || "");
    return;
  }

  const accountButton = event.target.closest("[data-account-action]");
  if (accountButton) {
    handleAccountAction(accountButton);
    return;
  }

  const taskCancelButton = event.target.closest("[data-task-cancel]");
  if (taskCancelButton) {
    cancelTask(taskCancelButton);
    return;
  }

  const taskRetryButton = event.target.closest("[data-task-retry]");
  if (taskRetryButton) {
    retryTask(taskRetryButton);
    return;
  }

  const cardBinToggle = event.target.closest("[data-card-bin-toggle]");
  if (cardBinToggle) {
    toggleCardBin(cardBinToggle.dataset.cardBinToggle);
    return;
  }

  const cardButton = event.target.closest("[data-card-action]");
  if (cardButton) {
    handleCardAction(cardButton);
    return;
  }

  const addressButton = event.target.closest("[data-address-action]");
  if (addressButton) {
    handleAddressAction(addressButton);
  }
}
