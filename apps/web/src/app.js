import { manageSkill } from "./skills.js";
import { checkForUpdates, copyDockerUpdateCommand, installUpdate } from "./updates.js";
import { showRuntimeDiagnostics } from "./settings.js";
import {
  closeConfirmDialog,
  handleToastClick,
} from "./feedback.js";
import { setupIcons } from "./icons.js";
import {
  closeAccountDeleteModal,
  closeAccountWorkbench,
  handleAccountAliasInput,
  handleAccountCardSelectionClick,
  handleAccountCardToolClick,
  handleAccountDeleteBackdropClick,
  handleAccountToolOpen,
  handleConfirmBackdropClick,
  handleConfirmDialogKeydown,
  handlePageClick,
  handlePasswordInputFocus,
  openAccountDeleteFlow,
  refreshAccountWorkbench,
  setupAccountMenus,
  setupAccountWorkbench,
  setupPageNavigation,
  setupSidebarToggle,
  setupUiTheme,
  submitAccountDeleteModal,
  syncToolbarMigrationOption,
  toggleUiTheme,
} from "./workspace.js";
import { detectBrowserAccount, refresh, startServiceHealthMonitor } from "./sync.js";
import {
  checkExtensionConnection,
  cleanupRuntimeArtifacts,
  cleanupTempProfiles,
  copyExtensionDirectory,
  dismissOnboardingForSession,
  dismissOnboardingPermanently,
  handleProfileSelectionChange,
  openChromeExtensionsPage,
  refreshProfileAndBridge,
  renderRuntimeArtifacts,
  renderTopProfile,
  resetOnboardingGuide,
  restoreSelectedProfile,
  saveBrowserProxyPolicy,
  scanRuntimeArtifacts,
  syncBrowserProxyFields,
} from "./settings.js";
import {
  addCredentialAccounts,
  changeOverleafPassword,
  executeAccountSwitch,
  planAccountSwitch,
  previewAccountSwitchProjects,
  previewRemoteCleanup,
  refreshCredentialAccounts,
  removeAccount,
  updateLocalPassword,
} from "./accounts/actions.js";
import {
  refreshRegistrationEligibilityForSelectedTrial,
  startRegistration,
  syncRegistrationSource,
} from "./registration.js";
import {
  clearTerminalTasks,
  copyRuntimeLog,
  focusLatestRetryableTask,
  handleRuntimeLogScroll,
  restoreRuntimeLogDockState,
  submitTaskInput,
  toggleRuntimeLogDock,
} from "./tasks/list.js";
import {
  chooseExportDirectory,
  chooseImportFile,
  chooseWebImportFiles,
  removeImportFile,
  downloadExportedAccounts,
  exportAccounts,
  importAccounts,
  importManualCookieAccounts,
  saveDefaultExportDirectory,
} from "./accounts/io.js";
import { addCard, fetchAddress, handleAddressDisplayClick, selectFallbackAddress, selectCardRow, exportSelectedCards, deleteSelectedCards } from "./resources.js";
import {
  handleAccountFilterChange,
  handleAccountSearch,
  handleAccountSelectionChange,
  handleAccountSortChange,
  renderAccountViewMode,
  setAccountView,
  syncAccountFilterControls,
} from "./accounts/list.js";

export function setupControls() {
  document.getElementById("check-updates").addEventListener("click", checkForUpdates);
  document.getElementById("install-update").addEventListener("click", installUpdate);
  document.getElementById("copy-update-command").addEventListener("click", copyDockerUpdateCommand);
  document.getElementById("skills-list").addEventListener("click", (event) => manageSkill(event.target.closest("[data-skill-action]")));
  document.getElementById("runtime-diagnostics").addEventListener("click", showRuntimeDiagnostics);
  document.querySelectorAll("input:not([type='checkbox']):not([type='radio'])").forEach((input) => {
    input.classList.add("ui-input");
  });
  document.querySelectorAll("textarea").forEach((textarea) => {
    textarea.classList.add("ui-textarea");
  });
  document.querySelectorAll("select").forEach((select) => {
    select.classList.add("ui-select");
  });
  document.querySelectorAll("input[type='checkbox']").forEach((checkbox) => {
    checkbox.classList.add("ui-checkbox");
  });

  document.querySelectorAll("button").forEach((button) => {
    if (
      button.classList.contains("icon-btn") ||
      button.classList.contains("copy-btn") ||
      button.classList.contains("account-power-button")
    ) {
      return;
    }
    if (button.classList.contains("account-toolbar-icon-button")) {
      button.classList.add("icon-btn");
      return;
    }
    button.classList.add("ui-btn");
  });

  [
    "switch-execute",
    "credential-add-submit",
    "credential-refresh-submit",
    "registration-submit",
    "import-submit",
    "manual-cookie-submit",
    "export-submit",
    "card-submit",
    "address-submit",
  ].forEach((id) => document.getElementById(id)?.classList.add("ui-btn-primary"));

  [
    "remove-submit",
    "overleaf-password-submit",
    "cleanup-artifacts",
    "account-delete-submit",
    "confirm-submit",
  ].forEach((id) => document.getElementById(id)?.classList.add("ui-btn-danger"));

  document
    .querySelectorAll(
      ".account-bulk-bar button, .account-add-menu-panel button, .compact-select-panel button, .settings-action-row button",
    )
    .forEach((button) => {
      button.classList.add("ui-btn-sm");
    });

  document.querySelectorAll(".stack-form, .inline-form, .compact-form").forEach((form) => {
    form.classList.add("ui-form");
  });
}

function setupPageMotion() {
  const content = document.querySelector(".app-content");
  const motion = window.matchMedia("(hover: hover) and (prefers-reduced-motion: no-preference)");
  let frame = 0;
  let panel;
  let x = 0;
  let y = 0;
  const reset = () => {
    window.cancelAnimationFrame(frame);
    frame = 0;
    document.querySelectorAll(".page-panel").forEach((item) => {
      item.style.removeProperty("--motif-x");
      item.style.removeProperty("--motif-y");
    });
  };
  content.addEventListener("pointermove", (event) => {
    if (!motion.matches || event.pointerType === "touch") return;
    const target = event.target.closest(".page-panel");
    if (!target) return;
    panel = target;
    const bounds = content.getBoundingClientRect();
    x = ((event.clientX - bounds.left) / bounds.width - 0.5) * 8;
    y = ((event.clientY - bounds.top) / bounds.height - 0.5) * 6;
    if (frame) return;
    frame = window.requestAnimationFrame(() => {
      panel.style.setProperty("--motif-x", `${x}px`);
      panel.style.setProperty("--motif-y", `${y}px`);
      frame = 0;
    });
  });
  content.addEventListener("pointerleave", reset);
  motion.addEventListener("change", reset);
}

document.addEventListener("DOMContentLoaded", () => {
  setupIcons();
  setupUiTheme();
  setupControls();
  setupAccountWorkbench();
  setupPageNavigation();
  setupPageMotion();
  setupAccountMenus();
  setupSidebarToggle();
  document.getElementById("refresh-button").addEventListener("click", () => refresh({ refreshSubscriptions: true }));
  document.getElementById("profile-select").addEventListener("change", handleProfileSelectionChange);
  document.getElementById("theme-toggle")?.addEventListener("click", toggleUiTheme);
  document.getElementById("profile-redetect").addEventListener("click", refreshProfileAndBridge);
  document.getElementById("detect-browser-account").addEventListener("click", detectBrowserAccount);
  document.getElementById("credential-add-form").addEventListener("submit", addCredentialAccounts);
  document.getElementById("registration-form").addEventListener("submit", startRegistration);
  document.getElementById("registration-source").addEventListener("change", syncRegistrationSource);
  document.getElementById("registration-existing-method").addEventListener("change", syncRegistrationSource);
  document
    .getElementById("registration-trial-days")
    .addEventListener("change", refreshRegistrationEligibilityForSelectedTrial);
  document.getElementById("tasks-list").addEventListener("submit", submitTaskInput);
  document.getElementById("tasks-list").addEventListener("scroll", handleRuntimeLogScroll);
  document.getElementById("runtime-log-toggle").addEventListener("click", toggleRuntimeLogDock);
  document.getElementById("copy-runtime-log").addEventListener("click", copyRuntimeLog);
  document.getElementById("focus-retryable-task").addEventListener("click", focusLatestRetryableTask);
  document.getElementById("clear-terminal-tasks").addEventListener("click", clearTerminalTasks);
  document.getElementById("switch-form").addEventListener("submit", planAccountSwitch);
  document.getElementById("switch-project-preview").addEventListener("click", previewAccountSwitchProjects);
  document.getElementById("switch-execute").addEventListener("click", executeAccountSwitch);
  document.getElementById("credential-refresh-form").addEventListener("submit", refreshCredentialAccounts);
  document.getElementById("import-form").addEventListener("submit", importAccounts);
  document.getElementById("import-choose-file").addEventListener("click", chooseImportFile);
  document.getElementById("import-web-files").addEventListener("change", chooseWebImportFiles);
  document.getElementById("import-file-queue").addEventListener("click", removeImportFile);
  document.getElementById("manual-cookie-form").addEventListener("submit", importManualCookieAccounts);
  document.getElementById("export-form").addEventListener("submit", exportAccounts);
  document.getElementById("export-choose-dir").addEventListener("click", chooseExportDirectory);
  document.getElementById("export-save-default-dir").addEventListener("click", saveDefaultExportDirectory);
  document.getElementById("export-download").addEventListener("click", downloadExportedAccounts);
  document.getElementById("card-form").addEventListener("submit", addCard);
  document.getElementById("cards-body").addEventListener("click", selectCardRow);
  document.getElementById("card-export").addEventListener("click", exportSelectedCards);
  document.getElementById("card-delete").addEventListener("click", deleteSelectedCards);
  document.getElementById("address-form").addEventListener("submit", fetchAddress);
  document.getElementById("address-form").addEventListener("click", handleAddressDisplayClick);
  document.getElementById("address-fallback").addEventListener("click", selectFallbackAddress);
  document.getElementById("password-form").addEventListener("submit", updateLocalPassword);
  document.getElementById("overleaf-password-form").addEventListener("submit", changeOverleafPassword);
  document.getElementById("remove-preview").addEventListener("click", previewRemoteCleanup);
  document.getElementById("remove-form").addEventListener("submit", removeAccount);
  document.getElementById("scan-artifacts").addEventListener("click", scanRuntimeArtifacts);
  document.getElementById("cleanup-artifacts").addEventListener("click", cleanupRuntimeArtifacts);
  document.getElementById("cleanup-temp-profiles").addEventListener("click", cleanupTempProfiles);
  document.getElementById("browser-proxy-form").addEventListener("submit", saveBrowserProxyPolicy);
  document.getElementById("browser-proxy-mode").addEventListener("change", syncBrowserProxyFields);
  document.getElementById("accounts-table")?.addEventListener("change", handleAccountSelectionChange);
  document.getElementById("accounts-select-all").addEventListener("change", handleAccountSelectionChange);
  document.getElementById("account-card-grid").addEventListener("change", handleAccountSelectionChange);
  document.getElementById("account-card-grid").addEventListener("click", handleAccountCardToolClick);
  document.getElementById("account-card-grid").addEventListener("click", handleAccountCardSelectionClick);
  document.getElementById("account-search").addEventListener("input", handleAccountSearch);
  document.getElementById("account-search").addEventListener("focus", syncAccountFilterControls);
  document.getElementById("account-search").addEventListener("blur", syncAccountFilterControls);
  document.getElementById("account-filter").addEventListener("change", handleAccountFilterChange);
  document.getElementById("account-sort").addEventListener("change", handleAccountSortChange);
  document.getElementById("account-view-cards")?.addEventListener("click", () => setAccountView("cards"));
  document.getElementById("account-view-table")?.addEventListener("click", () => setAccountView("table"));
  document.getElementById("account-toolbar-migrate").addEventListener("change", syncToolbarMigrationOption);
  document.getElementById("account-toolbar-refresh").addEventListener("click", refreshAccountWorkbench);
  document.getElementById("account-toolbar-delete").addEventListener("click", () => openAccountDeleteFlow());
  document.getElementById("account-workbench-close").addEventListener("click", closeAccountWorkbench);
  document.getElementById("account-delete-cancel").addEventListener("click", closeAccountDeleteModal);
  document.getElementById("account-delete-submit").addEventListener("click", submitAccountDeleteModal);
  document.getElementById("account-delete-modal").addEventListener("click", handleAccountDeleteBackdropClick);
  document.querySelectorAll("[data-account-tool-open]").forEach((button) => {
    button.addEventListener("click", handleAccountToolOpen);
  });
  document
    .getElementById("onboarding-dismiss-session")
    .addEventListener("click", dismissOnboardingForSession);
  document
    .getElementById("onboarding-dismiss-permanent")
    .addEventListener("click", dismissOnboardingPermanently);
  document.getElementById("onboarding-copy-path").addEventListener("click", copyExtensionDirectory);
  document.getElementById("onboarding-open-extensions").addEventListener("click", openChromeExtensionsPage);
  document.getElementById("onboarding-check-connection").addEventListener("click", checkExtensionConnection);
  document.getElementById("reset-onboarding").addEventListener("click", resetOnboardingGuide);
  document.getElementById("confirm-cancel").addEventListener("click", () => closeConfirmDialog(false));
  document.getElementById("confirm-submit").addEventListener("click", () => closeConfirmDialog(true));
  document.getElementById("confirm-modal").addEventListener("click", handleConfirmBackdropClick);
  document.addEventListener("keydown", handleConfirmDialogKeydown);
  document.getElementById("toast-region").addEventListener("click", handleToastClick);
  document.body.addEventListener("click", handlePageClick);
  document.body.addEventListener("input", handleAccountAliasInput);
  document.body.addEventListener("focusin", handlePasswordInputFocus);
  renderAccountViewMode();
  syncRegistrationSource();
  restoreRuntimeLogDockState();
  restoreSelectedProfile();
  renderTopProfile(null);
  renderRuntimeArtifacts(null);
  refresh({ refreshSubscriptions: true, focusTask: false });
  startServiceHealthMonitor();
});
