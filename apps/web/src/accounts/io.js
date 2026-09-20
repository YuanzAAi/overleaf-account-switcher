import {
  clearTransientUiStatus,
  recordClientRuntimeInfo,
  reportUiOperationFailure,
  reportUiOperationStart,
  reportUiOperationSuccess,
  setTransientUiStatus,
  showConfirmDialog,
  showToast,
} from "../feedback.js";
import {
  accountAssistModeIsActive,
  accountAssistStatusForMode,
} from "../workspace.js";
import { endpoints, state } from "../state.js";
import {
  accountExportLayoutModes,
  accountExportModes,
  accountExportSafety,
  accountImportPostActions,
  accountImportSources,
} from "../capabilities.js";
import {
  isActiveTaskSnapshot,
  makeTaskId,
  nonNegativeCount,
  registerPendingAccountIoOperation,
  registerPendingAccountOperation,
  reportCount,
} from "../tasks/list.js";
import { postText } from "../http.js";
import { postJson, refresh, updateTasks } from "../sync.js";
import { aliasesFromInputOrSelection } from "./list.js";
import {
  resolveTauriDialogOpen,
  resolveTauriDialogSave,
  resolveTauriWriteTextFile,
  selectedDialogPath,
} from "../bridge.js";
import { renderConfig } from "../settings.js";
import { downloadTextFile, safeDownloadName, splitAliasInput, escapeHtml, escapeAttr } from "../format.js";
import { iconSvg } from "../icons.js";

export async function importAccounts(event) {
  event.preventDefault();
  if (state.importingFiles) return;
  const submit = document.getElementById("import-submit");
  const postActions = accountImportPostActions();
  const options = {
    refresh_session_metadata: postActions.includes("refresh_session_metadata") && document.getElementById("import-refresh").checked,
    fetch_git_token: postActions.includes("fetch_git_token") && document.getElementById("import-git-token").checked,
  };
  if (!state.importFiles.length) {
    showToast({ tone: "warning", title: "请先选择 JSON 文件" });
    return;
  }
  state.importFiles.forEach((file) => { if (file.status !== "done") file.status = "queued"; });
  state.importingFiles = true;
  submit.disabled = true;
  try {
    // 每个文件连同后置任务完成后再处理下一个，避免账号库并发写入。
    for (let file; (file = state.importFiles.find((item) => item.status === "queued"));) {
      file.status = "running";
      renderImportQueue();
      const taskId = makeTaskId("import-accounts", file.id);
      try {
        const payload = file.path ? { path: file.path } : { json: await file.file.text() };
        if (new Blob([JSON.stringify({ ...payload, ...options, task_id: taskId })]).size > 2 * 1024 * 1024) {
          throw new Error("文件超过服务的 2 MB 请求上限，请拆分后导入");
        }
        let report = await postJson(endpoints.importAccounts, { ...payload, ...options, task_id: taskId });
        let phase = "completed";
        if (isActiveTaskSnapshot(report)) {
          const task = await new Promise((resolve) => {
            registerPendingAccountOperation(taskId, resolve);
            updateTasks([report], { incremental: true });
          });
          phase = task.phase;
          if (phase === "cancelled") {
            file.status = "queued";
            break;
          }
          if (!task.result) throw new Error(task.error || task.message || "导入未返回结果");
          report = task.result;
        }
        const imported = report.import || report;
        const partial = phase !== "completed" || Number(imported.alias_conflict_count || 0) > 0 ||
          (report.post_action_errors || []).length > 0;
        file.status = partial ? "failed" : "done";
        reportUiOperationSuccess({
          status: null, shortMessage: "", title: partial ? "账号文件导入部分完成" : "账号文件导入完成",
          scope: "账号文件导入", route: endpoints.importAccounts,
          message: `${file.name}：${Number(imported.imported_count || 0)} 个已导入，${Number(imported.skipped_duplicate_email_count || 0)} 个重复邮箱已跳过，${Number(imported.alias_conflict_count || 0)} 个别名冲突${importPostActionSummary(report, { refreshSessionMetadata: options.refresh_session_metadata, fetchGitToken: options.fetch_git_token })}`,
          tone: partial ? "warning" : "success",
        });
      } catch (error) {
        file.status = "failed";
        reportUiOperationFailure({ status: null, shortMessage: "", title: "账号文件导入失败",
          scope: "账号文件导入", route: endpoints.importAccounts, error });
      }
      renderImportQueue();
    }
    await refresh();
  } finally {
    state.importingFiles = false;
    submit.disabled = false;
    renderImportQueue();
  }
}

export function renderImportQueue() {
  const list = document.getElementById("import-file-queue");
  list.innerHTML = state.importFiles.map((file) => `
    <div class="import-queue-row">
      <span class="import-queue-name">${escapeHtml(file.name)}</span>
      <span class="ui-tag">${({ queued: "待导入", running: "导入中", done: "已导入", failed: "待重试" })[file.status]}</span>
      <button type="button" class="icon-btn" data-import-remove="${escapeAttr(file.id)}" title="移除文件" aria-label="移除文件" ${file.status === "running" ? "disabled" : ""}>${iconSvg("close")}</button>
    </div>`).join("");
}

export function removeImportFile(event) {
  const button = event.target.closest("[data-import-remove]");
  if (!button || button.disabled) return;
  state.importFiles = state.importFiles.filter((file) => file.id !== button.dataset.importRemove);
  renderImportQueue();
}

function appendImportFiles(files) {
  for (const file of files) {
    if (!state.importFiles.some((item) => item.id === file.id)) state.importFiles.push({ ...file, status: "queued" });
  }
  renderImportQueue();
}

export function chooseWebImportFiles(event) {
  appendImportFiles(Array.from(event.target.files || [], (file) => ({
    id: `web:${file.name}:${file.size}:${file.lastModified}`, name: file.name, file,
  })));
  event.target.value = "";
}

export async function importManualCookieAccounts(event) {
  event.preventDefault();
  const entriesInput = document.getElementById("manual-cookie-entries");
  const status = document.getElementById("io-status");
  const submit = document.getElementById("manual-cookie-submit");
  const modeId = "cookie-import";
  let pendingTask = false;
  if (!accountImportSources().includes("manual_cookie")) {
    reportUiOperationFailure({
      status,
      shortMessage: "Cookie 导入不可用",
      title: "Cookie 导入不可用",
      scope: "Cookie 登录",
      route: endpoints.importCookieAccounts,
      error: new Error("当前后端不支持手动 Cookie 导入"),
    });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在导入 Cookie",
    scope: "Cookie 登录",
    route: endpoints.importCookieAccounts,
    message: "已提交 Cookie 登录任务。",
  });

  let submittedEntriesText = "";
  try {
    submittedEntriesText = entriesInput.value;
    const entries = parseManualCookieEntries(submittedEntriesText);
    const postActions = accountImportPostActions();
    const refreshSessionMetadata =
      postActions.includes("refresh_session_metadata") &&
      document.getElementById("manual-cookie-refresh").checked;
    const fetchGitToken =
      postActions.includes("fetch_git_token") && document.getElementById("manual-cookie-git-token").checked;
    const taskId = makeTaskId("import-cookie", entries.map((entry) => entry.alias || entry.email).join(","));
    const applyReport = (finalReport, phase = "completed", { refreshBeforeReport = true } = {}) => {
      const importReport = finalReport.import || finalReport;
      const validation = finalReport.validation || { failed_count: 0, items: [] };
      const validationFailures = Array.isArray(validation.items)
        ? validation.items.filter((item) => item && item.valid === false)
        : [];
      const postActionSummary = importPostActionSummary(finalReport, {
        refreshSessionMetadata,
        fetchGitToken,
      });
      const validationSummary =
        validation.failed_count > 0 ? `，${validation.failed_count} 个 Cookie 无效待重新粘贴` : "";
      const failedPhase = String(phase || "").toLowerCase() !== "completed";
      const hasPostActionFailure =
        Array.isArray(finalReport.post_action_errors) && finalReport.post_action_errors.length > 0;
      const summary = `${Number(importReport.imported_count || 0)} 个已导入，${Number(importReport.skipped_duplicate_email_count || 0)} 个重复邮箱已跳过，${Number(importReport.alias_conflict_count || 0)} 个别名冲突${validationSummary}${postActionSummary}`;
      const publishReport = () => {
        const currentStatus = accountAssistStatusForMode("io", modeId, status);
        const inputUnchanged = entriesInput.value === submittedEntriesText;
        if (currentStatus && validationFailures.length > 0 && inputUnchanged) {
          entriesInput.value = validationFailures
            .map((item) => {
              const entry = entries[Number(item.index)];
              if (!entry) return "";
              const cookie = String(entry.cookie || "").replace(/^overleaf_session2\s*=\s*/i, "");
              return `${entry.alias ? `${entry.alias} ` : ""}${entry.email} ${cookie}`;
            })
            .filter(Boolean)
            .join("\n");
        } else if (currentStatus && !failedPhase && !hasPostActionFailure && inputUnchanged) {
          entriesInput.value = "";
        }
        if (validationFailures.length > 0) {
          reportUiOperationSuccess({
            status: currentStatus,
            shortMessage: "部分 Cookie 待重填",
            title: "部分 Cookie 未通过验证",
            scope: "Cookie 登录",
            route: endpoints.importCookieAccounts,
            message: `${summary}。`,
            closeAssist: false,
            tone: "warning",
          });
          if (currentStatus && inputUnchanged) entriesInput.focus();
        } else if (failedPhase || hasPostActionFailure) {
          reportUiOperationSuccess({
            status: currentStatus,
            shortMessage: "导入完成，部分后置动作失败",
            title: "Cookie 导入部分完成",
            scope: "Cookie 登录",
            route: endpoints.importCookieAccounts,
            message: `${summary}。失败详情已写入运行日志；可在账号管理中单独重试对应操作。`,
            closeAssist: false,
            tone: "warning",
          });
        } else {
          reportUiOperationSuccess({
            status: currentStatus,
            shortMessage: "Cookie 登录完成",
            title: "Cookie 登录完成",
            scope: "Cookie 登录",
            route: endpoints.importCookieAccounts,
            message: `${summary}。`,
            closeAssist: Boolean(currentStatus),
          });
        }
      };
      if (!refreshBeforeReport) {
        publishReport();
        return Promise.resolve();
      }
      return refresh().then(publishReport);
    };
    const report = await postJson(endpoints.importCookieAccounts, {
      entries,
      refresh_session_metadata: refreshSessionMetadata,
      fetch_git_token: fetchGitToken,
      task_id: taskId,
    });
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      registerPendingAccountIoOperation({
        taskId,
        task: report,
        submit,
        status,
        modeId,
        operationTitle: "Cookie 导入",
        scope: "Cookie 登录",
        route: endpoints.importCookieAccounts,
        cancelledShortMessage: "Cookie 导入任务已取消",
        missingShortMessage: "Cookie 导入未完成，请检查日志",
        resultFailureShortMessage: "Cookie 导入结果处理失败，请检查日志",
        allowFailedResult: true,
        onResult: (finalReport, phase) => applyReport(finalReport, phase, { refreshBeforeReport: false }),
      });
      return;
    }
    await applyReport(report);
  } catch (error) {
    if (
      error.kind === "invalid_session_cookie" &&
      accountAssistModeIsActive("io", modeId) &&
      entriesInput.value === submittedEntriesText
    ) {
      entriesInput.value = "";
      entriesInput.focus();
    }
    reportUiOperationFailure({
      status: accountAssistStatusForMode("io", modeId, status),
      shortMessage: "Cookie 登录失败，请检查日志",
      title: "Cookie 登录失败",
      scope: "Cookie 登录",
      route: endpoints.importCookieAccounts,
      error,
    });
  } finally {
    if (!pendingTask) submit.disabled = false;
  }
}

export async function exportAccounts(event) {
  event.preventDefault();
  const mode = document.getElementById("export-mode").value;
  const outputDir = document.getElementById("export-output-dir").value;
  const confirmOverwrite = document.getElementById("export-confirm-overwrite").checked;
  const status = document.getElementById("io-status");
  const submit = document.getElementById("export-submit");
  const modeId = "export";
  let pendingTask = false;
  if (!accountExportModes().includes(mode)) {
    reportUiOperationFailure({
      status,
      shortMessage: "导出模式不可用",
      title: "导出不可用",
      scope: "账号导出",
      route: endpoints.exportAccounts,
      error: new Error("当前后端不支持所选导出模式"),
    });
    return;
  }

  if (!(await confirmSensitiveAccountExport())) {
    clearTransientUiStatus(status);
    recordClientRuntimeInfo({
      scope: "账号导出",
      route: endpoints.exportAccounts,
      message: "用户取消了账号导出。",
    });
    showToast({ tone: "info", title: "已取消导出", message: "账号 JSON 未生成。" });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在导出",
    scope: "账号导出",
    route: endpoints.exportAccounts,
    message: "已提交账号导出任务。",
  });

  try {
    const aliases = aliasesFromInputOrSelection("export-aliases");
    const taskId = makeTaskId("export-accounts", aliases);
    const publishReport = (finalReport) => {
      const reportStatus = accountAssistStatusForMode("io", modeId, status);
      const overwritten = nonNegativeCount(finalReport.overwritten_file_count);
      reportUiOperationSuccess({
        status: reportStatus,
        shortMessage: "导出完成",
        title: "账号导出完成",
        scope: "账号导出",
        route: endpoints.exportAccounts,
        message: `已导出 ${nonNegativeCount(finalReport.exported_account_count)} 个账号到 ${Array.isArray(finalReport.files) ? finalReport.files.length : 0} 个文件，覆盖 ${overwritten} 个文件。`,
        closeAssist: Boolean(reportStatus),
      });
    };
    const report = await postJson(endpoints.exportAccounts, {
      aliases,
      mode,
      output_dir: outputDir,
      confirm_overwrite: confirmOverwrite,
      task_id: taskId,
    });
    if (isActiveTaskSnapshot(report)) {
      pendingTask = true;
      registerPendingAccountIoOperation({
        taskId,
        task: report,
        submit,
        status,
        modeId,
        operationTitle: "账号导出",
        scope: "账号导出",
        route: endpoints.exportAccounts,
        cancelledShortMessage: "导出已取消",
        missingShortMessage: "导出失败，请检查日志",
        resultFailureShortMessage: "导出结果处理失败，请检查日志",
        onResult: publishReport,
      });
      return;
    }
    publishReport(report);
  } catch (error) {
    reportUiOperationFailure({
      status: accountAssistStatusForMode("io", modeId, status),
      shortMessage: "导出失败，请检查日志",
      title: "账号导出失败",
      scope: "账号导出",
      route: endpoints.exportAccounts,
      error,
    });
  } finally {
    if (!pendingTask) submit.disabled = false;
  }
}

export async function chooseExportDirectory() {
  const status = document.getElementById("io-status");
  const button = document.getElementById("export-choose-dir");
  const input = document.getElementById("export-output-dir");
  if (!accountExportModes().some((mode) => ["single_file", "multiple_files"].includes(mode))) {
    reportUiOperationSuccess({
      status,
      shortMessage: "目录选择不可用",
      title: "导出目录不可用",
      scope: "账号导出",
      route: endpoints.exportAccounts,
      message: "当前后端不支持服务端导出。",
      tone: "warning",
    });
    return;
  }
  const openDialog = resolveTauriDialogOpen();
  if (!openDialog) {
    reportUiOperationSuccess({
      status,
      shortMessage: "请手动输入导出目录",
      title: "当前环境不支持目录选择",
      scope: "账号导出",
      route: "desktop-dialog",
      message: "浏览器模式没有桌面文件夹选择器，请手动输入导出目录。",
      tone: "warning",
    });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在选择目录",
    scope: "账号导出",
    route: "desktop-dialog",
    message: "已打开导出目录选择器。",
  });
  try {
    const selected = await openDialog({ directory: true, multiple: false });
    const directory = selectedDialogPath(selected);
    if (directory) {
      input.value = directory;
      reportUiOperationSuccess({
        status,
        shortMessage: "已选择导出目录",
        title: "导出目录已选择",
        scope: "账号导出",
        route: "desktop-dialog",
        message: `已选择导出目录：${directory}`,
      });
    } else {
      clearTransientUiStatus(status);
      recordClientRuntimeInfo({
        scope: "账号导出",
        route: "desktop-dialog",
        message: "用户取消了导出目录选择。",
      });
      showToast({ tone: "info", title: "已取消目录选择", message: "导出目录未更改。" });
    }
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "目录选择失败，请查看日志",
      title: "导出目录选择失败",
      scope: "账号导出",
      route: "desktop-dialog",
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export async function saveDefaultExportDirectory() {
  const status = document.getElementById("io-status");
  const button = document.getElementById("export-save-default-dir");
  const input = document.getElementById("export-output-dir");
  const directory = input.value.trim();
  if (!directory) {
    reportUiOperationSuccess({
      status,
      shortMessage: "请先选择导出目录",
      title: "默认目录未保存",
      scope: "账号导出",
      route: endpoints.updateDefaultExportDir,
      message: "保存默认目录前需要先选择或输入导出目录。",
      tone: "warning",
    });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在保存默认目录",
    scope: "账号导出",
    route: endpoints.updateDefaultExportDir,
    message: "已提交默认导出目录保存。",
  });
  try {
    const config = await postJson(endpoints.updateDefaultExportDir, {
      default_export_dir: directory,
    });
    state.config = config;
    renderConfig(config);
    input.value = config.default_export_dir || directory;
    reportUiOperationSuccess({
      status,
      shortMessage: "默认目录已保存",
      title: "默认导出目录已保存",
      scope: "账号导出",
      route: endpoints.updateDefaultExportDir,
      message: `默认导出目录已保存：${input.value}`,
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "默认目录保存失败，请查看日志",
      title: "默认导出目录保存失败",
      scope: "账号导出",
      route: endpoints.updateDefaultExportDir,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export async function chooseImportFile() {
  const button = document.getElementById("import-choose-file");
  const openDialog = resolveTauriDialogOpen();
  if (!openDialog) {
    document.getElementById("import-web-files").click();
    return;
  }
  button.disabled = true;
  try {
    const selected = await openDialog({ directory: false, multiple: true, filters: [{ name: "JSON", extensions: ["json"] }] });
    const paths = (Array.isArray(selected) ? selected : [selected]).filter((path) => typeof path === "string" && path);
    appendImportFiles(paths.map((path) => ({ id: path, path, name: path.split(/[\\/]/).pop() })));
    if (paths.length) recordClientRuntimeInfo({ scope: "账号文件导入", route: "desktop:file-dialog", message: `已选择 ${paths.length} 个 JSON 文件。` });
  } catch (error) {
    reportUiOperationFailure({ status: null, shortMessage: "", title: "选择 JSON 文件失败", scope: "账号文件导入", route: "desktop:file-dialog", error });
  } finally {
    button.disabled = false;
  }
}

export async function downloadExportedAccounts() {
  const mode = document.getElementById("export-mode").value;
  const status = document.getElementById("io-status");
  const button = document.getElementById("export-download");
  const exportModes = accountExportModes();
  const exportLayoutModes = accountExportLayoutModes();
  if (!exportModes.includes("browser_download_json")) {
    reportUiOperationFailure({
      status,
      shortMessage: "下载不可用",
      title: "账号 JSON 下载不可用",
      scope: "账号 JSON 下载",
      route: endpoints.exportAccountsJson,
      error: new Error("当前后端不支持浏览器 JSON 下载"),
    });
    return;
  }
  if (!exportLayoutModes.includes(mode)) {
    reportUiOperationFailure({
      status,
      shortMessage: "导出模式不可用",
      title: "账号 JSON 下载不可用",
      scope: "账号 JSON 下载",
      route: endpoints.exportAccountsJson,
      error: new Error("当前后端不支持所选导出模式"),
    });
    return;
  }

  if (!(await confirmSensitiveAccountExport())) {
    setTransientUiStatus(status, "已取消下载");
    recordClientRuntimeInfo({
      scope: "账号 JSON 下载",
      route: endpoints.exportAccountsJson,
      message: "用户取消了账号 JSON 下载。",
    });
    showToast({ tone: "info", title: "已取消下载", message: "账号 JSON 未保存。" });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在准备下载",
    scope: "账号 JSON 下载",
    route: endpoints.exportAccountsJson,
    message: "已开始准备账号 JSON 下载。",
  });

  try {
    const aliases = aliasesFromInputOrSelection("export-aliases");
    const aliasList = splitAliasInput(aliases);
    if (!aliasList.length) {
      throw new Error("缺少账号别名");
    }
    if (mode === "multiple_files") {
      const deliveries = [];
      for (const alias of aliasList) {
        const json = await postText(endpoints.exportAccountsJson, { aliases: alias });
        const delivery = await saveOrDownloadTextFile(
          `${safeDownloadName(alias)}.json`,
          json,
          "application/json",
        );
        if (!delivery) {
          clearTransientUiStatus(status);
          recordClientRuntimeInfo({
            scope: "账号 JSON 下载",
            route: endpoints.exportAccountsJson,
            message: `已准备 ${deliveries.length} 个 JSON 文件，随后保存被取消。`,
          });
          showToast({ tone: "info", title: "保存已取消", message: "已停止后续文件保存。" });
          return;
        }
        deliveries.push(delivery);
      }
      reportUiOperationSuccess({
        status,
        shortMessage: "下载准备完成",
        title: "账号 JSON 已准备",
        scope: "账号 JSON 下载",
        route: endpoints.exportAccountsJson,
        message: `${exportDeliveryStatus(deliveries, aliasList.length)}。`,
        closeAssist: true,
      });
    } else {
      const json = await postText(endpoints.exportAccountsJson, { aliases });
      const delivery = await saveOrDownloadTextFile("accounts.json", json, "application/json");
      if (delivery) {
        reportUiOperationSuccess({
          status,
          shortMessage: "下载准备完成",
          title: "账号 JSON 已准备",
          scope: "账号 JSON 下载",
          route: endpoints.exportAccountsJson,
          message: `${exportDeliveryStatus([delivery], 1)}。`,
          closeAssist: true,
        });
      } else {
        clearTransientUiStatus(status);
        recordClientRuntimeInfo({
          scope: "账号 JSON 下载",
          route: endpoints.exportAccountsJson,
          message: "用户取消了账号 JSON 保存。",
        });
        showToast({ tone: "info", title: "保存已取消", message: "账号 JSON 未保存。" });
      }
    }
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "下载失败，请检查日志",
      title: "账号 JSON 下载失败",
      scope: "账号 JSON 下载",
      route: endpoints.exportAccountsJson,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export async function confirmSensitiveAccountExport() {
  if (!accountExportSafety().includes("sensitive_export_warning")) {
    return true;
  }
  return showConfirmDialog({
    eyebrow: "敏感导出",
    title: "确认导出账号 JSON",
    message: "导出的账号 JSON 可能包含已保存的密码、Cookie 和 Git 集成令牌。",
    points: [
      "只保存到你信任的位置，不要上传到仓库、聊天窗口或共享网盘。",
      "导出后如需清理，请先确认文件路径，避免误删备份。",
    ],
    confirmText: "继续导出",
    tone: "danger",
  });
}

export async function saveOrDownloadTextFile(filename, text, type) {
  const saveDialog = resolveTauriDialogSave();
  const writeTextFile = resolveTauriWriteTextFile();
  if (saveDialog && writeTextFile) {
    const selected = await saveDialog({
      defaultPath: filename,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    const path = selectedDialogPath(selected);
    if (!path) {
      return null;
    }
    await writeTextFile(path, text);
    return "desktop_save";
  }

  downloadTextFile(filename, text, type);
  return "browser_download";
}

export function exportDeliveryStatus(deliveries, count) {
  const desktopCount = deliveries.filter((delivery) => delivery === "desktop_save").length;
  const browserCount = deliveries.filter((delivery) => delivery === "browser_download").length;
  if (desktopCount === count) {
    return `已保存 ${count} 个 JSON 文件`;
  }
  if (browserCount === count) {
    return `已准备 ${count} 个 JSON 下载`;
  }
  return `已保存 ${desktopCount} 个，已准备下载 ${browserCount} 个`;
}

export function parseManualCookieEntries(text) {
  const lines = String(text || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (!lines.length) {
    throw new Error("missing manual cookie entries");
  }

  return lines.map((line, index) => {
    const emailMatch = line.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);
    if (!emailMatch) {
      throw new Error(`第 ${index + 1} 行需要包含邮箱和 Cookie（空格分隔）`);
    }
    const email = emailMatch[0];
    const prefix = line
      .slice(0, emailMatch.index)
      .replace(/[|;,\s]+$/g, "")
      .trim();
    const alias = prefix.split(/[|;,\s]+/).filter(Boolean)[0] || "";
    const suffix = line.slice((emailMatch.index || 0) + email.length).replace(/^[|;,\s]+/, "");
    const cookieToken = suffix.split(/[|;,\s]+/).filter(Boolean)[0] || "";
    const cookieValue = cookieToken.replace(/^overleaf_session2\s*=\s*/i, "");
    if (!cookieValue) {
      throw new Error(`第 ${index + 1} 行需要包含邮箱和 Cookie（空格分隔）`);
    }
    const cookie = `overleaf_session2=${cookieValue}`;
    return {
      alias: alias || null,
      email,
      cookie,
    };
  });
}

export function importPostActionSummary(report, options = {}) {
  if (!report || !report.import) {
    return "";
  }
  const importedCount = nonNegativeCount(report.import.imported_count);
  const errors = Array.isArray(report.post_action_errors) ? report.post_action_errors : [];
  const parts = [];
  if (options.refreshSessionMetadata) {
    const batch = report.session_refresh;
    const items = Array.isArray(batch && batch.items) ? batch.items : [];
    const actionFailed = errors.some((item) => item && item.action === "refresh_session_metadata");
    const refreshed = reportCount(
      batch,
      ["refreshed_count"],
      items.filter((item) => item && item.refreshed).length,
    );
    const failed = batch
      ? reportCount(batch, ["failed_count"], items.filter((item) => item && item.error).length)
      : actionFailed
        ? Math.max(importedCount, 1)
        : 0;
    const skipped = Math.max(0, importedCount - refreshed - failed);
    parts.push(`状态刷新：成功 ${refreshed}，跳过 ${skipped}，失败 ${failed}`);
  }
  if (options.fetchGitToken) {
    const batch = report.git_token_generation;
    const items = Array.isArray(batch && batch.items) ? batch.items : [];
    const actionFailed = errors.some((item) => item && item.action === "fetch_git_token");
    const refreshed = reportCount(
      batch,
      ["refreshed_count"],
      items.filter((item) => item && item.refreshed).length,
    );
    const failed = batch
      ? reportCount(
          batch,
          ["failed_count"],
          items.filter((item) => item && !item.skipped && item.error).length,
        )
      : actionFailed
        ? Math.max(importedCount, 1)
        : 0;
    const reportedSkipped = batch
      ? reportCount(batch, ["skipped_count"], items.filter((item) => item && item.skipped).length)
      : 0;
    const skipped = reportedSkipped + Math.max(0, importedCount - refreshed - reportedSkipped - failed);
    parts.push(`Git 令牌：成功 ${refreshed}，跳过 ${skipped}，失败 ${failed}`);
  }
  const unknownErrors = errors.filter(
    (item) => !item || !["refresh_session_metadata", "fetch_git_token"].includes(item.action),
  );
  if (unknownErrors.length > 0) {
    parts.push(`${unknownErrors.length} 个后续操作异常`);
  }
  return parts.length ? `，${parts.join("；")}` : "";
}
