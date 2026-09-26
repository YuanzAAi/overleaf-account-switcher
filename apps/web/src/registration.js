import {
  recordClientRuntimeError,
  setAccountAssistShortStatus,
  showToast,
} from "./feedback.js";
import { MINIMUM_NEW_PASSWORD_LENGTH, endpoints, state } from "./state.js";
import { credentialRecoveryTask, postJson, refresh, syncRegistrationTaskStatus, updateTasks } from "./sync.js";
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

export function resetRegistrationForm(preserveConcurrency = false) {
  const form = document.getElementById("registration-form");
  if (!form) return;
  if (form.dataset.registrationLocked === "true" && !preserveConcurrency) return;
  const concurrency = registrationConcurrency();
  form.reset();
  const input = document.getElementById("registration-concurrency");
  if (input) {
    input.value = preserveConcurrency ? String(concurrency) : "1";
  }
  document.getElementById("registration-batch-list")?.replaceChildren();
  if (!preserveConcurrency) setRegistrationStatus("");
  if (form.dataset.registrationLocked === "true") syncRegistrationFormLock();
  else syncRegistrationSource();
  if (!preserveConcurrency) showToast({ tone: "success", title: "刷新成功" });
}

export function registrationExistingLoginSource() {
  if (!registrationUsesExistingAccount()) return "";
  return document.getElementById("registration-existing-method")?.value || "alias";
}

export function registrationTrialDays() {
  const trialDays = Number(document.getElementById("registration-trial-days")?.value);
  return [7, 21].includes(trialDays) ? trialDays : 21;
}

export function registrationConcurrency() {
  const value = Number(document.getElementById("registration-concurrency")?.value);
  return Number.isSafeInteger(value) && value > 0 ? value : 1;
}

export function syncRegistrationConcurrency() {
  const source = document.getElementById("registration-source");
  const field = document.getElementById("registration-concurrency-field");
  const input = document.getElementById("registration-concurrency");
  const oneCardField = document.getElementById("registration-one-card-field");
  const oneCard = document.getElementById("registration-one-card");
  const submit = document.getElementById("registration-submit");
  if (!input || !oneCard || !field || !oneCardField) return;

  const isNewRegistration = source?.value !== "existing";
  const form = document.getElementById("registration-form");
  const locked = form?.dataset.registrationLocked === "true" || Boolean(submit?.dataset.registrationSubmitting);
  field.hidden = !isNewRegistration;
  if (input.value && (!Number.isSafeInteger(Number(input.value)) || Number(input.value) < 1)) input.value = "1";
  input.disabled = !isNewRegistration;
  let concurrency = registrationConcurrency();
  const available = isNewRegistration ? availableRegistrationCardCount() : null;
  if (available !== null && oneCard.checked) {
    input.max = String(Math.max(1, available));
    if (!locked && available > 0 && concurrency > available) {
      input.value = String(available);
      concurrency = available;
    }
  } else {
    input.removeAttribute("max");
  }
  oneCardField.hidden = !isNewRegistration || (concurrency <= 1 && !oneCard.checked);
  oneCard.disabled = !isNewRegistration;

  const multiple = isNewRegistration && concurrency > 1;
  if (!isNewRegistration || input.value) syncRegistrationBatchFields(isNewRegistration, concurrency);
  const batchRows = document.querySelectorAll("#registration-batch-list .registration-batch-card");
  const batchReady = batchRows.length === concurrency && Array.from(batchRows).every((row) =>
    row.querySelector('[data-field="email"]')?.value.trim()
    && row.querySelector('[data-field="password"]')?.value.trim());
  const validationMessage = available === 0 && !state.registrationCredentialRecoveryTaskId
    ? "当前卡片范围没有符合策略的可用卡" : "";
  if (submit && !submit.dataset.registrationSubmitting && !state.registrationTaskIds.length
    && form?.dataset.registrationLocked !== "true") {
    submit.disabled = (isNewRegistration && !input.value) || Boolean(validationMessage) || (multiple && !batchReady);
    const status = document.getElementById("registration-status");
    const previous = status?.textContent || "";
    if (previous === "当前卡片范围没有符合策略的可用卡" || !previous) {
      if (validationMessage !== previous) setRegistrationStatus(validationMessage, "warning");
    }
  }
  syncRegistrationFormLock();
}

function syncRegistrationBatchFields(isNewRegistration, concurrency) {
  const batch = document.getElementById("registration-batch-fields");
  const list = document.getElementById("registration-batch-list");
  const single = document.getElementById("registration-new-fields");
  if (!batch || !list || !single) return;
  const multiple = isNewRegistration && concurrency > 1;
  const singleValues = ["alias", "email", "password"].map((field) =>
    document.getElementById(`registration-${field}`)?.value || "");
  if (!multiple && !batch.hidden && isNewRegistration) {
    const first = list.firstElementChild;
    ["alias", "email", "password"].forEach((field) => {
      document.getElementById(`registration-${field}`).value = first?.querySelector(`[data-field="${field}"]`)?.value || "";
    });
  }
  if (!multiple) list.replaceChildren();
  batch.hidden = !multiple;
  single.hidden = multiple || (!isNewRegistration && registrationExistingLoginSource() === "alias");
  ["alias", "email", "password"].forEach((field) => {
    const control = document.getElementById(`registration-${field}`);
    if (control && isNewRegistration) control.disabled = multiple;
  });
  if (!multiple) return;
  while (list.children.length > concurrency) list.lastElementChild.remove();
  while (list.children.length < concurrency) {
    const number = list.children.length + 1;
    const row = document.createElement("div");
    row.className = "registration-batch-card";
    row.innerHTML = `<strong>号 ${number}</strong>
      <label>别名<input class="ui-input" data-field="alias" type="text" placeholder="留空使用邮箱前缀" autocomplete="off"></label>
      <label>邮箱<input class="ui-input" data-field="email" type="email" required autocomplete="off"></label>
      <label>密码<input class="ui-input" data-field="password" type="password" minlength="8" required autocomplete="new-password"></label>`;
    list.append(row);
    if (number === 1) {
      ["alias", "email", "password"].forEach((field, index) => {
        row.querySelector(`[data-field="${field}"]`).value = singleValues[index];
      });
    }
  }
}

export function syncRegistrationFormLock() {
  const form = document.getElementById("registration-form");
  if (!form) return;
  const active = Boolean(form.querySelector("#registration-submit")?.dataset.registrationSubmitting)
    || state.registrationTaskIds.length > 0
    || state.tasks.some((task) => task?.operation_kind === "account_registration" && isActiveTaskSnapshot(task));
  const wasLocked = form.dataset.registrationLocked === "true";
  const resetButton = document.getElementById("registration-reset");
  if (resetButton) resetButton.disabled = active;
  if (active) {
    form.dataset.registrationLocked = "true";
    const recovery = Boolean(state.registrationCredentialRecoveryTaskId);
    form.querySelectorAll("input,select,button").forEach((control) => {
      const editableRecovery = recovery && ["registration-alias", "registration-email", "registration-password", "registration-submit"].includes(control.id);
      control.disabled = !editableRecovery;
    });
  } else if (wasLocked) {
    delete form.dataset.registrationLocked;
    form.querySelectorAll("input,select,button").forEach((control) => { control.disabled = false; });
    syncRegistrationSource();
  }
}

function availableRegistrationCardCount() {
  const cardBin = document.getElementById("registration-card-bin")?.value || "";
  const strategy = document.getElementById("registration-card-strategy")?.value || "new_then_used_then_failed";
  const now = Date.now();
  return state.cards.filter((card) => {
    if (cardBin && String(card.bin || "") !== cardBin) return false;
    if (strategy === "new_only" && String(card.status || "").toLowerCase() !== "new") return false;
    const month = Number(card.exp_month);
    const rawYear = Number(card.exp_year);
    const year = rawYear >= 0 && rawYear < 100 ? rawYear + 2000 : rawYear;
    if (!Number.isInteger(month) || month < 1 || month > 12 || !Number.isInteger(year)) return false;
    return now < Date.UTC(year, month, 1);
  }).length;
}

function validateRegistrationBatch(concurrency) {
  const rows = Array.from(document.querySelectorAll("#registration-batch-list .registration-batch-card"));
  if (rows.length !== concurrency) throw new Error("账号数量与并发任务数不一致");
  const entries = rows.map((row) => {
    const passwordInput = row.querySelector('[data-field="password"]');
    const email = row.querySelector('[data-field="email"]').value.trim();
    const alias = row.querySelector('[data-field="alias"]').value.trim();
    const password = passwordInput.value;
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) throw new Error(`邮箱格式无效: ${email || "空项"}`);
    validateNewPasswordValue(password, passwordInput);
    return { alias, email, password };
  });
  if (new Set(entries.map(({ email }) => email.toLowerCase())).size !== concurrency) {
    throw new Error("并发邮箱不能重复");
  }
  const resolvedAliases = entries.map(({ alias, email }) => alias || email.split("@")[0]);
  if (new Set(resolvedAliases).size !== concurrency) {
    throw new Error("并发账号别名不能重复");
  }
  return entries;
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
  syncRegistrationConcurrency();
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
    const failedCount = Number(report.failed_count || 0);
    replaceSelectOptions(select, eligibleAliases, failedCount ? "部分账号检测失败，请重试" : "没有可用试用资格的账号");
    select.disabled = eligibleAliases.length === 0;
    syncRegistrationFormLock();
    setRegistrationStatus(
      failedCount ? `${eligibleAliases.length} 个可试用，${failedCount} 个检测失败，请重试`
        : eligibleAliases.length ? `可试用账号 ${eligibleAliases.length} 个` : "没有可用试用资格的账号",
      eligibleAliases.length && !failedCount ? "success" : "warning",
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
  if (document.getElementById("registration-form")?.dataset.registrationLocked === "true"
    && !state.registrationCredentialRecoveryTaskId) return;
  syncRegistrationConcurrency();
  const existing = registrationUsesExistingAccount();
  if (!existing && !document.getElementById("registration-concurrency")?.value) return;
  const existingLoginSource = registrationExistingLoginSource();
  const alias = document.getElementById("registration-alias").value;
  const email = document.getElementById("registration-email").value;
  const passwordInput = document.getElementById("registration-password");
  const password = passwordInput.value;
  const existingAlias = document.getElementById("registration-existing-alias").value;
  const sessionCookieInput = document.getElementById("registration-session-cookie");
  const sessionCookie = sessionCookieInput.value;
  const trialDays = registrationTrialDays();
  const concurrency = existing ? 1 : registrationConcurrency();
  const cardSelectionStrategy = document.getElementById("registration-card-strategy").value;
  const cardBin = document.getElementById("registration-card-bin")?.value || "";
  const oneCardChecked = !existing && document.getElementById("registration-one-card")?.checked;
  const oneCardPerAccount = concurrency > 1 && oneCardChecked;
  const actions = registrationActions();
  const autoFetchGitToken =
    actions.includes("auto-fetch-git-token") && document.getElementById("registration-git-token").checked;
  const submit = document.getElementById("registration-submit");
  if (submit.dataset.registrationSubmitting) return;
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
  let entries;
  if (!existing) {
    try {
      if (concurrency > 1) entries = validateRegistrationBatch(concurrency);
      else validateNewPasswordValue(password, passwordInput);
    } catch (error) {
      setRegistrationStatus(error.message, "warning");
      return;
    }
  }

  state.registrationBatchHadStartFailure = false;
  submit.disabled = true;
  submit.dataset.registrationSubmitting = "true";
  syncRegistrationFormLock();
  setRegistrationStatus("正在启动", "warning");
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

    if (!existing && availableRegistrationCardCount() < 1) {
      setRegistrationStatus("当前卡片范围没有符合策略的可用卡", "warning");
      return;
    }

    const payload = {
      trial_days: trialDays,
      card_selection_strategy: cardSelectionStrategy,
      card_bin: cardBin,
      auto_fetch_git_token: autoFetchGitToken,
    };
    const oneCardOption = { one_card_per_account: Boolean(oneCardPerAccount) };
    if (!existing && concurrency > 1) {
      const batchId = makeTaskId("register-batch", "");
      setRegistrationStatus(`正在启动 ${concurrency} 个注册任务`, "warning");
      const taskIds = entries.map((_, index) => `${batchId}-${String(index + 1).padStart(String(concurrency).length, "0")}`);
      state.registrationTaskIds = taskIds;
      const results = await Promise.allSettled(entries.map((entry, index) => {
        const taskId = taskIds[index];
        return postJson(endpoints.registerAccount, {
          ...payload,
          ...entry,
          ...oneCardOption,
          registration_batch_id: batchId,
          registration_batch_size: concurrency,
          registration_batch_index: index + 1,
          task_id: taskId,
        }, { focusTask: index === 0 });
      }));
      const started = results.filter((result) => result.status === "fulfilled").length;
      const failed = results.length - started;
      state.registrationTaskIds = taskIds.filter((_, index) => results[index].status === "fulfilled");
      state.registrationBatchHadStartFailure = failed > 0;
      setRegistrationStatus(
        failed ? `已启动 ${started} 个任务，${failed} 个未能启动` : `已启动 ${started} 个注册任务，进度见运行日志`,
        failed ? "warning" : "success",
      );
      await refresh();
      return;
    }
    payload.task_id = makeTaskId(
      existing ? `trial-existing-${existingLoginSource}` : "register",
      existingAlias || alias || email,
    );
    if (!existing) {
      Object.assign(payload, { alias, email, password, ...oneCardOption });
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
    state.registrationTaskId = payload.task_id;
    state.registrationTaskIds = [payload.task_id];
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
    await refresh();
  } catch (error) {
    state.registrationTaskId = "";
    state.registrationTaskIds = [];
    state.registrationBatchHadStartFailure = false;
    setRegistrationStatus("试用任务失败，请查看运行日志", "error", 7200);
    recordClientRuntimeError({ scope: "注册试用", route: endpoints.registerAccount, error });
    await refresh();
  } finally {
    delete submit.dataset.registrationSubmitting;
    syncRegistrationTaskStatus(state.tasks);
    syncRegistrationConcurrency();
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
