import { syncSkillsOnSwitch } from "../skills.js";
import { formatTaskKey, isPlainObject, taskErrorDetails } from "../tasks/results.js";
import {
  recordClientRuntimeError,
  recordClientRuntimeInfo,
  reportUiOperationFailure,
  reportUiOperationStart,
  reportUiOperationSuccess,
  setAccountAssistShortStatus,
  showToast,
} from "../feedback.js";
import {
  accountAssistStatusForMode,
  accountToolbarMigratesProjects,
  collectPasswordTargetList,
  passwordToolConfig,
  setPasswordToolStatus,
  splitAliasText,
} from "../workspace.js";
import {
  accountActionDescriptor,
  aliasesFromInputOrSelection,
  beginActionButtonBusy,
  clearAccountSelection,
  restoreActionButtonBusy,
} from "./list.js";
import {
  accountBulkActionNeedsBridge,
  accountBulkActions,
  accountCredentialActions,
  accountPasswordActions,
  accountRemovalActions,
  accountRowActions,
  accountSecretActions,
  accountSwitchActions,
} from "../capabilities.js";
import {
  accountActionOutcome,
  isActiveTaskSnapshot,
  makeTaskId,
  nonNegativeCount,
  registerPendingAccountAction,
  registerPendingAccountOperation,
  reportAccountActionTerminal,
  reportCount,
  scheduleStatusClear,
  settlePasswordToolSubmission,
  settlePasswordToolTask,
} from "../tasks/list.js";
import { extensionBridgeConnected } from "../settings.js";
import { endpoints, state } from "../state.js";
import { fetchJson } from "../http.js";
import { postJson, postTaskResult, refresh, updateTasks } from "../sync.js";
import { escapeHtml, pill, writeClipboard } from "../format.js";
import { iconSvg } from "../icons.js";

export function performAccountBulkAction(action, aliases, button) {
  return performAccountAction(action, splitAliasText(aliases), button, true);
}

export function handleAccountAction(button) {
  return performAccountAction(button.dataset.accountAction, [button.dataset.accountAlias], button, false);
}

async function performAccountAction(action, aliases, button, bulk) {
  const aliasList = aliases.filter(Boolean);
  if (!action || !aliasList.length || !button || button.disabled) return;
  const status = document.getElementById("account-action-status");
  const descriptor = accountActionDescriptor(action, bulk);
  const actions = bulk ? accountBulkActions() : accountRowActions();
  if (!actions.includes(action)) {
    const shortMessage = setAccountAssistShortStatus(status, "操作不可用");
    scheduleStatusClear(status, shortMessage, 7200);
    if (bulk) showToast({ tone: "warning", title: "操作不可用", message: shortMessage });
    return;
  }
  if (bulk && accountBulkActionNeedsBridge(action) && !extensionBridgeConnected()) {
    setAccountAssistShortStatus(status, "需要扩展桥");
    recordClientRuntimeInfo({
      scope: "批量账号操作", route: endpoints.browserLogin,
      message: "打开浏览器登录需要先连接 Chrome 扩展桥；其他账号操作不受影响。",
    });
    showToast({ tone: "warning", title: "需要扩展桥", message: "请先在教程页连接 Chrome 扩展桥。" });
    scheduleStatusClear(status, "需要扩展桥", 4600);
    return;
  }
  if (!descriptor) return;
  const aliasText = aliasList.join(",");
  const busySnapshot = beginActionButtonBusy(button);
  const terminalOptions = { action, aliasList, button, busySnapshot, status, descriptor };
  let pendingTask = false;
  reportUiOperationStart({
    status, shortMessage: "处理中", scope: descriptor.scope, route: descriptor.endpoint,
    message: bulk
      ? `已提交 ${descriptor.title}，目标 ${aliasList.length} 个账号。`
      : `已为账号 ${aliasText} 提交${descriptor.title}。`,
  });

  try {
    const payload = { task_id: makeTaskId(action, aliasText) };
    if (action === "switch-plan" || action === "switch-execute") {
      payload.alias = aliasText;
      payload.migrate_projects = accountToolbarMigratesProjects();
      if (action === "switch-execute") payload.sync_skills = syncSkillsOnSwitch();
    } else {
      payload.aliases = aliasText;
      if (action === "refresh-session") payload.passwords = "";
    }
    const report = await postJson(descriptor.endpoint, payload);
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      registerPendingAccountAction({ ...terminalOptions, taskId: payload.task_id, task: report });
      return;
    }
    await refresh();
    reportAccountActionTerminal({
      ...terminalOptions,
      task: report.phase ? report : { phase: "completed", result: report },
    });
  } catch (error) {
    reportUiOperationFailure({
      status, shortMessage: "操作失败，请查看运行日志", title: `${descriptor.title}失败`,
      scope: descriptor.scope, route: descriptor.endpoint, error,
    });
  } finally {
    if (!pendingTask) restoreActionButtonBusy(button, busySnapshot);
  }
}

export async function planAccountSwitch(event) {
  event.preventDefault();
  const migrateProjects =
    document.getElementById("switch-migrate").checked || accountToolbarMigratesProjects();
  const status = document.getElementById("switch-status");
  const submit = document.getElementById("switch-submit");

  if (!accountSwitchActions().includes("plan")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "换号计划不可用",
      scope: "生成换号计划",
      route: endpoints.switchPlan,
      error: new Error("当前后端不支持生成换号计划"),
    });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在生成",
    scope: "生成换号计划",
    route: endpoints.switchPlan,
    message: "正在生成扩展换号命令。",
  });

  try {
    const alias = aliasesFromInputOrSelection("switch-alias", { single: true });
    const report = await postJson(endpoints.switchPlan, {
      alias,
      migrate_projects: migrateProjects,
      task_id: makeTaskId("switch-plan", alias),
    });
    reportUiOperationSuccess({
      status,
      shortMessage: "计划已生成",
      title: "换号计划已生成",
      scope: "生成换号计划",
      route: endpoints.switchPlan,
      message: `已准备 ${Number(report.command_count || 0)} 条扩展命令。`,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "生成失败，请查看运行日志",
      title: "换号计划生成失败",
      scope: "生成换号计划",
      route: endpoints.switchPlan,
      error,
    });
  } finally {
    submit.disabled = false;
  }
}

export async function previewAccountSwitchProjects() {
  const status = document.getElementById("switch-status");
  const button = document.getElementById("switch-project-preview");

  if (!accountSwitchActions().includes("preview-projects")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "项目预览不可用",
      scope: "预览换号项目",
      route: endpoints.switchProjectPreview,
      error: new Error("当前后端不支持预览换号项目"),
    });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在预览",
    scope: "预览换号项目",
    route: endpoints.switchProjectPreview,
    message: "正在读取源账号和目标账号的项目差异。",
  });

  try {
    const alias = aliasesFromInputOrSelection("switch-alias", { single: true });
    const migrateCheckbox = document.getElementById("account-toolbar-migrate");
    if (migrateCheckbox) migrateCheckbox.checked = true;
    const report = await postJson(endpoints.switchProjectPreview, {
      alias,
      task_id: makeTaskId("project-preview", alias),
    });
    renderProjectMigrationPreview(report);
    reportUiOperationSuccess({
      status,
      shortMessage: "预览完成",
      title: "项目迁移预览完成",
      scope: "预览换号项目",
      route: endpoints.switchProjectPreview,
      message: `${Number(report.will_migrate_count || 0)} 个将迁移，${Number(report.skipped_existing_count || 0)} 个已存在。`,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "预览失败，请查看运行日志",
      title: "项目迁移预览失败",
      scope: "预览换号项目",
      route: endpoints.switchProjectPreview,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export function renderProjectMigrationPreview(report) {
  const container = document.getElementById("switch-project-preview-result");
  const items = Array.isArray(report.items) ? report.items : [];
  if (!items.length) {
    container.innerHTML = `<div class="empty">没有可迁移的源项目。</div>`;
    return;
  }

  container.innerHTML = `
    <h3>项目迁移预览</h3>
    <div class="table-wrap compact">
      <table>
        <thead>
          <tr>
            <th>状态</th>
            <th>项目</th>
            <th>策略</th>
            <th>目标匹配</th>
          </tr>
        </thead>
        <tbody>
          ${items
            .map(
              (item) => `
            <tr>
              <td>${pill(item.status || "unknown")}</td>
              <td><strong>${escapeHtml(item.project_name || "-")}</strong><br><span class="muted">${escapeHtml(item.project_id || "-")}</span></td>
              <td>${escapeHtml(formatTaskKey(item.strategy || "unknown"))}</td>
              <td>${escapeHtml(item.target_project_id || "-")}</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    </div>
  `;
}

export async function executeAccountSwitch() {
  const migrateProjects =
    document.getElementById("switch-migrate").checked || accountToolbarMigratesProjects();
  const status = document.getElementById("switch-status");
  const button = document.getElementById("switch-execute");
  const action = "switch-execute";
  const descriptor = accountActionDescriptor(action);

  if (!accountSwitchActions().includes("execute")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "换号执行不可用",
      scope: "执行账号换号",
      route: endpoints.switchExecute,
      error: new Error("当前后端不支持执行账号换号"),
    });
    return;
  }

  const busySnapshot = beginActionButtonBusy(button);
  let pendingTask = false;
  reportUiOperationStart({
    status,
    shortMessage: "正在切换",
    scope: "执行账号换号",
    route: endpoints.switchExecute,
    message: "正在执行扩展换号命令。",
  });

  try {
    const alias = aliasesFromInputOrSelection("switch-alias", { single: true });
    const taskId = makeTaskId(action, alias);
    const report = await postJson(endpoints.switchExecute, {
      alias,
      migrate_projects: migrateProjects,
      sync_skills: syncSkillsOnSwitch(),
      task_id: taskId,
    });
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      registerPendingAccountAction({
        taskId,
        task: report,
        action,
        aliasList: [alias],
        button,
        busySnapshot,
        status,
        descriptor,
      });
      return;
    }
    const outcome = accountActionOutcome(action, report, 1, [alias]);
    reportUiOperationSuccess({
      status,
      shortMessage: outcome.shortMessage,
      title: "账号换号完成",
      scope: "执行账号换号",
      route: endpoints.switchExecute,
      message: outcome.detail,
      tone: outcome.tone,
      toastMessage: outcome.toastMessage,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "切换失败，请查看运行日志",
      title: "账号换号失败",
      scope: "执行账号换号",
      route: endpoints.switchExecute,
      error,
    });
  } finally {
    if (!pendingTask) restoreActionButtonBusy(button, busySnapshot);
  }
}

export function accountSecretLabel(field) {
  return (
    {
      cookie: "Cookie",
      git_token: "Git 令牌",
      password: "密码",
    }[field] || "敏感值"
  );
}

async function copyWithFeedback(button, readValue) {
  const originalHtml = button.innerHTML;
  button.disabled = true;
  button.setAttribute("aria-busy", "true");
  try {
    await writeClipboard(await readValue());
    button.classList.add("copied");
    button.innerHTML = iconSvg("check");
    window.setTimeout(() => {
      button.classList.remove("copied");
      button.innerHTML = originalHtml;
      button.disabled = false;
    }, 1200);
  } catch (error) {
    button.disabled = false;
    throw error;
  } finally {
    button.removeAttribute("aria-busy");
  }
}

export async function copyAccountSecret(alias, field, button) {
  if (!alias || !field || !button || button.disabled) return;
  const label = accountSecretLabel(field);
  if (!accountSecretActions().includes("copy-account-secret")) {
    showToast({ tone: "warning", title: `无法复制${label}`, message: "当前后端不支持该操作。" });
    return;
  }

  try {
    await copyWithFeedback(button, async () => {
      const secret = await fetchJson(endpoints.accountSecret(alias, field));
      if (
        !secret ||
        secret.alias !== alias ||
        secret.field !== field ||
        typeof secret.value !== "string" ||
        !secret.value
      ) {
        throw new Error("服务未返回可复制的账号敏感值");
      }
      return secret.value;
    });
    recordClientRuntimeInfo({
      scope: "账号敏感值复制",
      route: "/secrets/account",
      message: `已复制 ${alias} 的${label}。`,
    });
    showToast({
      tone: "success",
      title: `${label}已复制`,
      message: `${alias} 的${label}已复制到剪贴板。`,
    });
  } catch (error) {
    recordClientRuntimeError({
      scope: "账号敏感值复制",
      route: "/secrets/account",
      error,
    });
    showToast({
      tone: "error",
      title: `复制${label}失败`,
      message: "详细原因已写入运行日志。",
    });
  }
}

export async function copyAccountEmail(email, button) {
  if (!email || !button || button.disabled) return;
  try {
    await copyWithFeedback(button, () => email);
    showToast({ tone: "success", title: "邮箱已复制", message: email });
  } catch (error) {
    recordClientRuntimeError({ scope: "邮箱复制", route: "clipboard", error });
    showToast({ tone: "error", title: "复制邮箱失败", message: "详细原因已写入运行日志。" });
  }
}

export async function updateLocalPassword(event) {
  return submitPasswordChange(event, "passwords");
}

export async function changeOverleafPassword(event) {
  return submitPasswordChange(event, "overleaf-password");
}

async function submitPasswordChange(event, tool) {
  event.preventDefault();
  const remote = tool === "overleaf-password";
  const config = passwordToolConfig(tool);
  const route = remote ? endpoints.overleafPassword : endpoints.localPassword;
  const scope = remote ? "修改 Overleaf 密码" : "更新本地密码";
  const label = remote ? "远端密码修改" : "本地密码更新";
  const verb = remote ? "修改" : "更新";
  let passwords = document.getElementById(config.passwordInputId).value;
  const submit = document.getElementById(config.submitId);
  let requestStarted = false;
  let pendingTask = false;
  let succeeded = false;
  let submittedAliases = [];

  if (!accountPasswordActions().includes(remote ? "change-overleaf-password" : "update-local-password")) {
    setPasswordToolStatus(tool, `当前后端不支持${scope}`, 7200);
    return;
  }

  submit.disabled = true;
  setPasswordToolStatus(tool, `正在${verb}`);

  try {
    const composed = collectPasswordTargetList(tool);
    if (composed) passwords = composed.passwords.join(",");
    const aliases = aliasesFromInputOrSelection(config.aliasesInputId);
    submittedAliases = splitAliasText(aliases);
    recordClientRuntimeInfo({
      scope,
      route,
      message: `已提交${label}，目标 ${submittedAliases.length} 个账号。`,
    });
    const taskId = makeTaskId(remote ? "change-overleaf-password" : "update-password", aliases);
    requestStarted = true;
    const report = await postJson(route, {
      aliases,
      passwords,
      task_id: taskId,
    });
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      setPasswordToolStatus(tool, "任务已开始，结果见运行日志");
      registerPendingAccountOperation(taskId, (task) => {
        settlePasswordToolTask(tool, task, submittedAliases, submit);
      });
      return;
    }
    const changedCount = Number(report[remote ? "changed_count" : "updated_count"] || 0);
    const failedCount = remote ? Number(report.failed_count || 0) : 0;
    succeeded = failedCount === 0;
    setPasswordToolStatus(
      tool,
      succeeded ? `${label}完成` : `${label}未完全完成，请查看运行日志`,
      succeeded ? 4600 : 7200,
    );
    recordClientRuntimeInfo({
      scope,
      route,
      message: remote
        ? `${label}完成，成功 ${changedCount} 个，失败 ${failedCount} 个。`
        : `${label}完成，成功 ${changedCount} 个账号。`,
    });
    await refresh();
  } catch (error) {
    setPasswordToolStatus(tool, `${verb}失败，请查看运行日志`, 7200);
    recordClientRuntimeError({ scope, route, error });
  } finally {
    if (requestStarted && !pendingTask) {
      settlePasswordToolSubmission(tool, submittedAliases, succeeded ? "completed" : "failed");
    }
    if (!pendingTask) submit.disabled = false;
  }
}

export function credentialBatchOutcome(report, requestedCount, modeId) {
  const items = Array.isArray(report && report.items) ? report.items : [];
  const itemImported = items.filter(
    (item) => item && (item.status === "imported" || item.imported === true),
  ).length;
  const itemUpdated = items.filter(
    (item) => item && (item.status === "updated" || item.updated === true || item.refreshed === true),
  ).length;
  const itemSkipped = items.filter(
    (item) => item && (item.status === "skipped_duplicate_email" || item.skipped === true),
  ).length;
  const itemFailed = items.filter(
    (item) =>
      item && (item.status === "failed" || item.error || item.refreshed === false || item.updated === false),
  ).length;
  const imported = reportCount(report, ["imported_count"], itemImported);
  const updated = reportCount(report, ["updated_count"], itemUpdated);
  const skipped = reportCount(report, ["skipped_duplicate_email_count", "skipped_count"], itemSkipped);
  const reportedFailed = reportCount(report, ["failed_count"], itemFailed);
  const expected = nonNegativeCount(requestedCount);
  const reportedTotal = imported + updated + skipped + reportedFailed;
  const missing = expected > 0 ? Math.max(0, expected - reportedTotal) : 0;
  const unexpected = expected > 0 ? Math.max(0, reportedTotal - expected) : 0;
  const failed = reportedFailed + missing;
  const target = expected || Math.max(items.length, reportedTotal);
  const mismatch = unexpected ? `，结果超出目标 ${unexpected} 个` : "";
  const detail =
    modeId === "credential-refresh"
      ? `目标 ${target} 个账号，刷新 ${updated} 个，跳过 ${skipped} 个，失败 ${failed} 个${mismatch}。`
      : `目标 ${target} 个账号，新增 ${imported} 个，更新 ${updated} 个，跳过 ${skipped} 个，失败 ${failed} 个${mismatch}。`;
  return {
    detail,
    hasFailure: failed > 0 || unexpected > 0,
  };
}

export function reportCredentialTaskTerminal({
  task,
  submit,
  status,
  modeId,
  operationTitle,
  scope,
  route,
  requestedCount,
}) {
  if (submit) submit.disabled = false;
  const ownedStatus = accountAssistStatusForMode("credentials", modeId, status);
  const phase = String((task && task.phase) || "").toLowerCase();
  if (phase === "cancelled") {
    reportUiOperationSuccess({
      status: ownedStatus,
      shortMessage: `${operationTitle}已取消`,
      title: `${operationTitle}已取消`,
      scope,
      route,
      message: `${operationTitle}任务已取消。`,
      tone: "info",
    });
    return;
  }

  const report = task && isPlainObject(task.result) ? task.result : null;
  if (!report) {
    reportUiOperationFailure({
      status: ownedStatus,
      shortMessage: `${operationTitle}未完成，请查看日志`,
      title: `${operationTitle}未完成`,
      scope,
      route,
      error: new Error((task && (task.error || task.message)) || `${operationTitle}任务未返回结构化结果`),
    });
    return;
  }

  const outcome = credentialBatchOutcome(report, requestedCount, modeId);
  if (phase === "failed" || outcome.hasFailure) {
    const taskMessage = String((task && task.message) || "").trim();
    reportUiOperationFailure({
      status: ownedStatus,
      shortMessage: `${operationTitle}未完全完成`,
      title: `${operationTitle}未完全完成`,
      scope,
      route,
      error: new Error(taskMessage ? `${outcome.detail} ${taskMessage}` : outcome.detail),
    });
    return;
  }

  reportUiOperationSuccess({
    status: ownedStatus,
    shortMessage: `${operationTitle}完成`,
    title: `${operationTitle}完成`,
    scope,
    route,
    message: outcome.detail,
    closeAssist: Boolean(ownedStatus),
  });
}

export function addCredentialAccounts(event) {
  return submitCredentialAccounts(event, false);
}

export function refreshCredentialAccounts(event) {
  return submitCredentialAccounts(event, true);
}

async function submitCredentialAccounts(event, refreshing) {
  event.preventDefault();
  const modeId = refreshing ? "credential-refresh" : "credential-add";
  const scope = refreshing ? "刷新 Cookie" : "账号密码登录";
  const operationTitle = refreshing ? "Cookie 刷新" : scope;
  const route = refreshing ? endpoints.refreshCredentials : endpoints.addCredentials;
  const action = refreshing ? "refresh-cookie-with-password" : "add-with-password";
  const passwordInput = document.getElementById(`${modeId}-passwords`);
  const status = document.getElementById("credential-status");
  const submit = document.getElementById(`${modeId}-submit`);
  let requestStarted = false;
  let pendingTask = false;

  if (!accountCredentialActions().includes(action)) {
    reportUiOperationFailure({
      status, shortMessage: "操作不可用", title: `${scope}不可用`, scope, route,
      error: new Error(`当前后端不支持${scope}`),
    });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status, shortMessage: refreshing ? "正在刷新" : "正在登录", scope, route,
    message: `已提交${operationTitle}任务。`,
  });

  try {
    const aliases = refreshing
      ? aliasesFromInputOrSelection(`${modeId}-aliases`)
      : document.getElementById(`${modeId}-aliases`).value;
    const emails = refreshing ? "" : document.getElementById("credential-add-emails").value;
    const passwords = passwordInput.value;
    const aliasValues = splitAliasText(aliases);
    const emailValues = splitAliasText(emails);
    const requestedCount = Math.max(aliasValues.length, emailValues.length);
    requestStarted = true;

    const recoveryTaskId = state.browserCredentialRecoveryTaskId;
    if (recoveryTaskId) {
      const recoveryIndex = aliasValues.indexOf(state.browserCredentialRecoveryAlias || aliasValues[0] || "");
      const email = recoveryIndex >= 0 ? emailValues[recoveryIndex] || "" : emailValues.length === 1 ? emailValues[0] : "";
      const payload = {
        task_id: recoveryTaskId,
        kind: "new_browser_credentials",
        value: JSON.stringify({ email, password: passwords }),
      };
      if (state.browserCredentialRecoveryItemId) payload.item_id = state.browserCredentialRecoveryItemId;
      const snapshot = await postJson(endpoints.taskInput, payload);
      state.browserCredentialRecoveryItemId = "";
      state.browserCredentialRecoveryAlias = "";
      setAccountAssistShortStatus(status, "已提交密码，正在继续原任务");
      recordClientRuntimeInfo({
        scope, route: endpoints.taskInput, message: "已提交补充账号密码，正在继续原任务。",
      });
      updateTasks([snapshot], { incremental: true });
      return;
    }

    const taskId = makeTaskId(refreshing ? "refresh-credentials" : "add-credentials", aliases || emails);
    const payload = { aliases, passwords, task_id: taskId };
    if (!refreshing) payload.emails = emails;
    const report = await postJson(route, payload);
    const terminalOptions = { submit, status, modeId, operationTitle, scope, route, requestedCount };
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      if (refreshing) {
        setAccountAssistShortStatus(status, "任务已开始");
      } else {
        reportUiOperationSuccess({
          status, shortMessage: "任务已开始", title: `${operationTitle}已排队`, scope, route,
          message: "浏览器任务已进入后台队列，结果将持续写入运行日志。", tone: "info",
        });
      }
      registerPendingAccountOperation(taskId, (task) => {
        reportCredentialTaskTerminal({ ...terminalOptions, task });
      });
      updateTasks([report], { incremental: true });
      return;
    }
    await refresh();
    reportCredentialTaskTerminal({
      ...terminalOptions, task: { phase: "completed", result: report },
    });
  } catch (error) {
    reportUiOperationFailure({
      status, shortMessage: refreshing ? "刷新失败，请检查日志" : "登录失败，请检查日志",
      title: `${operationTitle}失败`, scope, route, error,
    });
  } finally {
    if (requestStarted || !refreshing) passwordInput.value = "";
    if (!pendingTask) submit.disabled = false;
  }
}

export async function previewRemoteCleanup() {
  const status = document.getElementById("remove-status");
  const button = document.getElementById("remove-preview");
  if (!accountRemovalActions().includes("preview-remote-cleanup")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "清理预览不可用",
      scope: "预览远端清理",
      route: endpoints.removeAccountPreview,
      error: new Error("当前后端不支持远端清理预览"),
    });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在预览",
    scope: "预览远端清理",
    route: endpoints.removeAccountPreview,
    message: "正在读取远端项目清理范围。",
  });

  try {
    const aliases = aliasesFromInputOrSelection("remove-aliases");
    const report = await postTaskResult(endpoints.removeAccountPreview, {
      aliases,
      task_id: makeTaskId("cleanup-preview", aliases),
    });
    renderRemoteCleanupPreview(report);
    reportUiOperationSuccess({
      status,
      shortMessage: "预览完成",
      title: "远端清理预览完成",
      scope: "预览远端清理",
      route: endpoints.removeAccountPreview,
      message: `共 ${Number(report.total_projects || 0)} 个项目，删除 ${Number(report.delete_count || 0)} 个，保留 ${Number(report.leave_count || 0)} 个。`,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "预览失败，请查看运行日志",
      title: "远端清理预览失败",
      scope: "预览远端清理",
      route: endpoints.removeAccountPreview,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export function renderRemoteCleanupPreview(report) {
  const container = document.getElementById("remove-preview-result");
  const rows = [];
  const items = Array.isArray(report.items) ? report.items : [];

  items.forEach((entry) => {
    const alias = entry.alias || "-";
    if (entry.report && Array.isArray(entry.report.items)) {
      entry.report.items.forEach((item) => {
        rows.push({
          alias,
          projectName: item.project_name || "-",
          projectId: item.project_id || "-",
          action: item.action || "unknown",
          error: "",
        });
      });
      return;
    }
    if (entry.error) {
      rows.push({
        alias,
        projectName: "-",
        projectId: "-",
        action: "failed",
        error: taskErrorDetails(entry.error),
      });
    }
  });

  if (!rows.length) {
    container.innerHTML = `<div class="empty">没有发现远端项目。</div>`;
    return;
  }

  container.innerHTML = `
    <h3>远端清理预览</h3>
    <div class="table-wrap compact">
      <table>
        <thead>
          <tr>
            <th>别名</th>
            <th>动作</th>
            <th>项目</th>
            <th>错误</th>
          </tr>
        </thead>
        <tbody>
          ${rows
            .map(
              (row) => `
            <tr>
              <td>${escapeHtml(row.alias)}</td>
              <td>${pill(row.action)}</td>
              <td><strong>${escapeHtml(row.projectName)}</strong><br><span class="muted">${escapeHtml(row.projectId)}</span></td>
              <td>${escapeHtml(row.error || "-")}</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    </div>
  `;
}

export async function removeAccount(event) {
  event.preventDefault();
  const cleanupRemote = document.getElementById("remove-cleanup").checked;
  const confirmLocalOnly = document.getElementById("remove-confirm").checked;
  const status = document.getElementById("remove-status");
  const submit = document.getElementById("remove-submit");
  const actions = accountRemovalActions();
  const requiredAction = cleanupRemote ? "cleanup-remote-and-remove" : "remove-local";
  let requestStarted = false;
  let succeeded = false;

  if (!actions.includes(requiredAction)) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "删除账号不可用",
      scope: "删除账号",
      route: endpoints.removeAccount,
      error: new Error("当前后端不支持所选删除操作"),
    });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: cleanupRemote ? "正在清理" : "正在删除",
    scope: "删除账号",
    route: endpoints.removeAccount,
    message: "已提交账号删除任务。",
  });

  try {
    const aliases = aliasesFromInputOrSelection("remove-aliases");
    requestStarted = true;
    const report = await postTaskResult(endpoints.removeAccount, {
      aliases,
      cleanup_remote_projects: cleanupRemote,
      confirm_local_only: confirmLocalOnly,
      task_id: makeTaskId(cleanupRemote ? "clean-remove" : "remove-account", aliases),
    });
    const removedCount = Number(report.removed_count || 0);
    const incompleteCount = Number(report.incomplete_count || 0);
    const failedCount = Number(report.failed_count || 0);
    const hasIncomplete = incompleteCount > 0 || failedCount > 0;
    succeeded = !hasIncomplete;
    reportUiOperationSuccess({
      status,
      shortMessage: hasIncomplete ? "删除未完全完成" : "删除完成",
      title: "账号删除完成",
      scope: "删除账号",
      route: endpoints.removeAccount,
      message: `已删除 ${removedCount} 个，未完成 ${incompleteCount} 个，失败 ${failedCount} 个。`,
      tone: hasIncomplete ? "warning" : "success",
    });
    if (removedCount > 0) {
      document.getElementById("remove-aliases").value = "";
      document.getElementById("remove-confirm").checked = false;
    }
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
    if (requestStarted && succeeded) clearAccountSelection();
    submit.disabled = false;
  }
}
