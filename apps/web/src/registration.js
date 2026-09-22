import {
  recordClientRuntimeError,
  setAccountAssistShortStatus,
} from "./feedback.js";
import { MINIMUM_NEW_PASSWORD_LENGTH, endpoints, state } from "./state.js";
import { credentialRecoveryTask, postJson, refresh, updateTasks } from "./sync.js";
import {
  isActiveTaskSnapshot,
  makeTaskId,
  registerPendingAccountOperation,
  scheduleStatusClear,
  validateNewPasswordValue,
} from "./tasks/list.js";

import { registrationActions } from "./capabilities.js";

export function registrationUsesExistingAccount() {
  return document.getElementById("registration-source")?.value === "existing";
}

export function clearRegistrationAccountInputs() {
  ["registration-alias", "registration-email", "registration-password", "registration-session-cookie"].forEach((id) => {
    const input = document.getElementById(id);
    if (input) input.value = "";
  });
  setRegistrationStatus("");
}

export function registrationExistingLoginSource() {
  if (!registrationUsesExistingAccount()) return "";
  return document.getElementById("registration-existing-method")?.value || "alias";
}

export function registrationTrialDays() {
  const trialDays = Number(document.getElementById("registration-trial-days")?.value);
  return [7, 21].includes(trialDays) ? trialDays : 21;
}

export function refreshRegistrationEligibilityForSelectedTrial() {
  if (!registrationUsesExistingAccount() || registrationExistingLoginSource() !== "alias") return;
  const version = ++state.registrationEligibilityRequestVersion;
  state.registrationEligibilityLoading = false;
  loadRegistrationEligibleAliases(version);
}

export function syncRegistrationSource() {
  const eligibilityRequestVersion = ++state.registrationEligibilityRequestVersion;
  state.registrationEligibilityLoading = false;
  const existing = registrationUsesExistingAccount();
  const existingMethod = registrationExistingLoginSource();
  const usesSavedAlias = existing && existingMethod === "alias";
  const usesCredentials = existing && existingMethod === "credentials";
  const usesCookie = existing && existingMethod === "cookie";
  const newFields = document.getElementById("registration-new-fields");
  const existingMethodField = document.getElementById("registration-existing-method-field");
  const existingMethodSelect = document.getElementById("registration-existing-method");
  const existingField = document.getElementById("registration-existing-field");
  const existingAlias = document.getElementById("registration-existing-alias");
  const aliasField = document.getElementById("registration-alias-field");
  const alias = document.getElementById("registration-alias");
  const emailField = document.getElementById("registration-email-field");
  const email = document.getElementById("registration-email");
  const passwordField = document.getElementById("registration-password-field");
  const password = document.getElementById("registration-password");
  const cookieField = document.getElementById("registration-cookie-field");
  const sessionCookie = document.getElementById("registration-session-cookie");
  const submit = document.getElementById("registration-submit");
  if (
    !newFields ||
    !existingMethodField ||
    !existingMethodSelect ||
    !existingField ||
    !existingAlias ||
    !aliasField ||
    !alias ||
    !emailField ||
    !email ||
    !passwordField ||
    !password ||
    !cookieField ||
    !sessionCookie ||
    !submit
  )
    return;

  newFields.hidden = usesSavedAlias;
  existingMethodField.hidden = !existing;
  existingMethodSelect.disabled = !existing;
  existingField.hidden = !usesSavedAlias;
  existingAlias.disabled = !usesSavedAlias;
  existingAlias.required = usesSavedAlias;

  const showIdentityFields = !existing || usesCredentials || usesCookie;
  aliasField.hidden = !showIdentityFields;
  alias.disabled = !showIdentityFields;
  emailField.hidden = !showIdentityFields;
  email.disabled = !showIdentityFields;
  email.required = showIdentityFields;
  passwordField.hidden = existing && !usesCredentials;
  password.disabled = existing && !usesCredentials;
  password.required = !existing || usesCredentials;
  if (existing) {
    password.removeAttribute("minlength");
  } else {
    password.minLength = MINIMUM_NEW_PASSWORD_LENGTH;
  }
  cookieField.hidden = !usesCookie;
  sessionCookie.disabled = !usesCookie;
  sessionCookie.required = usesCookie;

  if (!usesCredentials && existing) password.value = "";
  if (!usesCookie) sessionCookie.value = "";
  submit.textContent = existing ? "开始试用" : "开始注册";
  if (usesSavedAlias) {
    loadRegistrationEligibleAliases(eligibilityRequestVersion);
  } else {
    state.registrationEligibleAliases = [];
    existingAlias.disabled = true;
    replaceSelectOptions(existingAlias, [], "切换到账号别名后检查资格");
    clearRegistrationEligibilityStatus();
  }
}

export function clearRegistrationEligibilityStatus() {
  const status = document.getElementById("registration-status");
  const text = status?.textContent?.trim() || "";
  if (
    text === "正在检查试用资格" ||
    text === "没有已保存账号" ||
    text === "没有确认可用的试用账号" ||
    text === "资格检查失败，请查看运行日志" ||
    text.startsWith("可试用账号 ")
  ) {
    setRegistrationStatus("", "info");
  }
}

export async function loadRegistrationEligibleAliases(requestVersion = null) {
  const version = requestVersion == null ? ++state.registrationEligibilityRequestVersion : requestVersion;
  if (
    state.registrationEligibilityLoading ||
    !registrationUsesExistingAccount() ||
    registrationExistingLoginSource() !== "alias"
  )
    return;
  const select = document.getElementById("registration-existing-alias");
  const aliases = (state.accounts || []).map((account) => account.alias).filter(Boolean);
  const trialDays = registrationTrialDays();
  if (!select) return;
  select.disabled = true;
  replaceSelectOptions(select, [], aliases.length ? "正在检查资格" : "没有可检查的账号");
  if (!aliases.length) {
    state.registrationEligibleAliases = [];
    setRegistrationStatus("没有已保存账号", "warning");
    return;
  }

  state.registrationEligibilityLoading = true;
  setRegistrationStatus("正在检查试用资格", "info");
  try {
    const taskId = makeTaskId("trial-eligibility", aliases.join(","));
    let report = await postJson(endpoints.trialEligibility, {
      aliases: aliases.join(","),
      trial_days: trialDays,
      task_id: taskId,
    });
    if (isActiveTaskSnapshot(report)) {
      const task = await new Promise((resolve) => {
        registerPendingAccountOperation(taskId, resolve);
        updateTasks([report], { incremental: true });
      });
      if (task.phase === "cancelled" || !task.result) {
        throw new Error(task.error || task.message || "资格检查未返回结果");
      }
      report = task.result;
    }
    const eligibleAliases = (Array.isArray(report.items) ? report.items : [])
      .filter((item) => item?.report?.eligibility === "eligible")
      .map((item) => item.alias)
      .filter(Boolean);
    if (
      version !== state.registrationEligibilityRequestVersion ||
      !registrationUsesExistingAccount() ||
      registrationExistingLoginSource() !== "alias"
    )
      return;
    state.registrationEligibleAliases = eligibleAliases;
    replaceSelectOptions(select, eligibleAliases, "没有可用试用资格的账号");
    select.disabled = eligibleAliases.length === 0;
    setRegistrationStatus(
      eligibleAliases.length ? `可试用账号 ${eligibleAliases.length} 个` : "没有确认可用的试用账号",
      eligibleAliases.length ? "success" : "warning",
    );
  } catch (error) {
    if (
      version !== state.registrationEligibilityRequestVersion ||
      !registrationUsesExistingAccount() ||
      registrationExistingLoginSource() !== "alias"
    )
      return;
    state.registrationEligibleAliases = [];
    replaceSelectOptions(select, [], "资格检查失败");
    setRegistrationStatus("资格检查失败，请查看运行日志", "error");
    recordClientRuntimeError({ scope: "试用资格检查", route: endpoints.trialEligibility, error });
  } finally {
    if (version === state.registrationEligibilityRequestVersion) {
      state.registrationEligibilityLoading = false;
    }
  }
}

export function replaceSelectOptions(select, values, emptyLabel) {
  const options = [];
  if (!values.length) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = emptyLabel;
    options.push(option);
  } else {
    values.forEach((value) => {
      const account = (state.accounts || []).find((item) => item.alias === value);
      const option = document.createElement("option");
      option.value = value;
      option.textContent = account?.email ? `${value} · ${account.email}` : value;
      options.push(option);
    });
  }
  select.replaceChildren(...options);
}

export async function startRegistration(event) {
  event.preventDefault();
  const existing = registrationUsesExistingAccount();
  const existingLoginSource = registrationExistingLoginSource();
  const alias = document.getElementById("registration-alias").value;
  const email = document.getElementById("registration-email").value;
  const passwordInput = document.getElementById("registration-password");
  const password = passwordInput.value;
  const existingAlias = document.getElementById("registration-existing-alias").value;
  const sessionCookieInput = document.getElementById("registration-session-cookie");
  const sessionCookie = sessionCookieInput.value;
  const trialDays = registrationTrialDays();
  const cardSelectionStrategy = document.getElementById("registration-card-strategy").value;
  const cardBin = document.getElementById("registration-card-bin")?.value || "";
  const actions = registrationActions();
  const autoFetchGitToken =
    actions.includes("auto-fetch-git-token") && document.getElementById("registration-git-token").checked;
  const submit = document.getElementById("registration-submit");
  if (!actions.includes("start")) {
    setRegistrationStatus("当前后端不支持注册", "warning");
    return;
  }
  if (existing && existingLoginSource === "alias" && !existingAlias) {
    setRegistrationStatus("请选择有试用资格的账号", "warning");
    return;
  }
  if (existing && ["credentials", "cookie"].includes(existingLoginSource) && !email.trim()) {
    setRegistrationStatus("请输入老号邮箱", "warning");
    return;
  }
  if (existing && existingLoginSource === "credentials" && !password.trim()) {
    setRegistrationStatus("请输入老号密码", "warning");
    return;
  }
  if (existing && existingLoginSource === "cookie" && !sessionCookie.trim()) {
    setRegistrationStatus("请输入 overleaf_session2", "warning");
    return;
  }
  if (!existing) {
    try {
      validateNewPasswordValue(password, passwordInput);
    } catch (error) {
      setRegistrationStatus(error.message, "warning");
      return;
    }
  }

  submit.disabled = true;
  setRegistrationStatus("正在启动", "warning");
  let requestStarted = false;

  try {
    const recoveryTaskId = state.registrationCredentialRecoveryTaskId;
    const recoveryKind = state.registrationCredentialRecoveryKind;
    const recovery = await credentialRecoveryTask(recoveryTaskId, recoveryKind);
    if (recovery) {
      if (!["new_registration_credentials", "new_browser_credentials"].includes(recoveryKind)) {
        setRegistrationStatus("当前试用任务的恢复状态无效，请刷新后重试", "error");
        return;
      }
      if (
        recoveryKind === "new_browser_credentials" &&
        (!existing || existingLoginSource !== "credentials")
      ) {
        setRegistrationStatus("请使用老号账号密码继续当前试用任务", "warning");
        return;
      }
      if (recoveryKind === "new_registration_credentials" && existing) {
        setRegistrationStatus("请使用新号注册信息继续当前试用任务", "warning");
        return;
      }
      requestStarted = true;
      const snapshot = await postJson(endpoints.taskInput, {
        task_id: recoveryTaskId,
        kind: recoveryKind,
        value:
          recoveryKind === "new_registration_credentials"
            ? JSON.stringify({ alias, email, password })
            : JSON.stringify({ email, password }),
      });
      state.registrationCredentialRecoveryTaskId = "";
      state.registrationCredentialRecoveryKind = "";
      setRegistrationStatus("已提交凭据，正在继续原试用任务", "warning");
      updateTasks([snapshot], { incremental: true });
      return;
    }

    state.registrationCredentialRecoveryTaskId = "";
    state.registrationCredentialRecoveryKind = "";

    const payload = {
      trial_days: trialDays,
      card_selection_strategy: cardSelectionStrategy,
      card_bin: cardBin,
      auto_fetch_git_token: autoFetchGitToken,
      task_id: makeTaskId(
        existing ? `trial-existing-${existingLoginSource}` : "register",
        existingAlias || alias || email,
      ),
    };
    if (!existing) {
      Object.assign(payload, { alias, email, password });
    } else if (existingLoginSource === "alias") {
      Object.assign(payload, {
        existing_login_source: "alias",
        existing_alias: existingAlias,
      });
    } else if (existingLoginSource === "credentials") {
      Object.assign(payload, {
        existing_login_source: "credentials",
        alias,
        email,
        password,
      });
    } else {
      Object.assign(payload, {
        existing_login_source: "cookie",
        alias,
        email,
        session_cookie: sessionCookie,
      });
    }
    requestStarted = true;
    state.registrationTaskId = payload.task_id;
    const report = await postJson(endpoints.registerAccount, payload);
    if (report.phase && !["completed", "failed", "cancelled"].includes(report.phase)) {
      setRegistrationStatus("试用任务已启动，进度见运行日志", "warning");
      return;
    }
    const skipped = report.status === "skipped_duplicate_email";
    const updated = report.status === "updated_existing";
    setRegistrationStatus(
      skipped
        ? `已跳过重复邮箱：${report.existing_alias || "已有账号"}`
        : updated
          ? `试用已更新：${report.alias}`
          : `已注册 ${report.alias}`,
      skipped ? "warning" : "success",
      4600,
    );
    state.registrationTaskId = "";
    await refresh();
  } catch (error) {
    state.registrationTaskId = "";
    setRegistrationStatus("试用任务失败，请查看运行日志", "error", 7200);
    recordClientRuntimeError({ scope: "注册试用", route: endpoints.registerAccount, error });
    await refresh();
  } finally {
    if (requestStarted) {
      passwordInput.value = "";
      sessionCookieInput.value = "";
    }
    submit.disabled = false;
  }
}

export function setRegistrationStatus(message, tone = "info", clearAfter = 0) {
  const status = document.getElementById("registration-status");
  if (!status) return;
  status.classList.remove("ui-tag-blue", "ui-tag-green", "ui-tag-red", "ui-tag-orange");
  const toneClass =
    {
      success: "ui-tag-green",
      error: "ui-tag-red",
      warning: "ui-tag-orange",
      info: "ui-tag-blue",
    }[tone] || "ui-tag-blue";
  status.classList.add(toneClass);
  const visibleMessage = setAccountAssistShortStatus(status, message);
  if (clearAfter > 0) scheduleStatusClear(status, visibleMessage, clearAfter);
  return visibleMessage;
}
