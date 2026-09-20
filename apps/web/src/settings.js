import {
  recordClientRuntimeError,
  recordClientRuntimeInfo,
  reportUiOperationFailure,
  reportUiOperationStart,
  reportUiOperationSuccess,
  setTransientUiStatus,
  showConfirmDialog,
  showToast,
} from "./feedback.js";
import {
  ONBOARDING_STORAGE_KEY,
  PROFILE_STORAGE_KEY,
  endpoints,
  state,
} from "./state.js";
import { escapeAttr, escapeHtml, statusText, writeClipboard } from "./format.js";

import { fetchJson } from "./http.js";
import { checkServiceHealth, detectCurrentBrowserAccount, postJson, refresh } from "./sync.js";
import {
  browserProfileActions,
  browserProfileDiscoverySupported,
  renderAccountCapabilityControls,
  renderAccountCredentialCapabilityControls,
  renderAccountIoCapabilityControls,
  renderAccountPasswordCapabilityControls,
  renderAccountRemovalCapabilityControls,
  renderAccountSwitchCapabilityControls,
  renderAddressCapabilityControls,
  renderBrowserProfileCapabilityControls,
  renderCardCapabilityControls,
  renderRegistrationCapabilityOptions,
  renderRuntimeCapabilityControls,
  renderServiceApiRoutes,
  renderTaskListCapabilityControls,
  runtimeCleanupActions,
  runtimeDiagnosticActions,
  tempProfilePreviewSupported,
} from "./capabilities.js";
import { desktopBridgeStatus, taskBrowserUrl } from "./bridge.js";
import { renderSkills } from "./skills.js";
import { applyRuntimeLogCollapsed, nonNegativeCount } from "./tasks/list.js";

export function extensionBridgeStatus(config = state.config) {
  const extensionDir = extensionDirectory(config);
  const configured = Boolean(config && config.extension_bridge_configured);
  const connected = configured && Boolean(config && config.extension_bridge_connected);
  const clients = Number((config && config.extension_bridge_client_count) || 0);
  if (state.serviceHealth === "unavailable") {
    return { configured, connected: false, clients: 0, label: "扩展桥状态待确认",
      detail: "本地服务未响应，暂时无法确认扩展连接。", tone: "waiting" };
  }
  if (!configured) {
    return {
      configured,
      connected: false,
      clients: 0,
      label: "扩展桥未配置",
      detail: "账号列表、导入导出和只读状态仍可用；无感换号和当前浏览器账号检测需要 Chrome 扩展桥。",
      tone: "missing",
    };
  }
  if (!connected) {
    return {
      configured,
      connected: false,
      clients,
      label: "扩展桥等待连接",
      detail: `当前没有 Chrome 扩展客户端连接。请加载 ${extensionDir}，然后重新检测连接。`,
      tone: "waiting",
    };
  }
  return {
    configured,
    connected,
    clients,
    label: "扩展桥已连接",
    detail: "浏览器连接已就绪。",
    tone: "present",
  };
}

export function extensionBridgeConnected(config = state.config) {
  return extensionBridgeStatus(config).connected;
}

export function renderExtensionStatus(config = state.config) {
  const status = extensionBridgeStatus(config);
  document.querySelectorAll("[data-extension-status]").forEach((element) => {
    element.textContent = state.serviceHealth === "unavailable" ? "待确认" : status.connected ? "已连接" : "等待连接";
  });
  const topbarStatus = document.getElementById("extension-bridge-status");
  if (topbarStatus) {
    const toneClass =
      status.tone === "present" ? "ui-tag-green" : status.tone === "waiting" ? "ui-tag-orange" : "ui-tag-red";
    const ledClass = status.tone === "present" ? "on" : status.tone === "waiting" ? "warn" : "off";
    topbarStatus.className = `ui-tag ${toneClass}`;
    topbarStatus.innerHTML = `<span class="status-led ${ledClass}" aria-hidden="true"></span>${escapeHtml(status.label)}`;
    topbarStatus.title = status.detail;
  }
  const banner = document.getElementById("extension-status-banner");
  if (!banner) return;
  banner.className = `extension-status-card ${status.tone}`;
  banner.innerHTML = `
    <div>
      <strong>${escapeHtml(status.label)}</strong>
      <span>${escapeHtml(status.detail)}</span>
    </div>
    <button class="inline-action" type="button" data-open-onboarding>安装引导</button>
  `;
}

export function renderOnboarding(config = state.config) {
  const modal = document.getElementById("onboarding-modal");
  const statusBox = document.getElementById("onboarding-status");
  if (!modal || !statusBox) return;
  renderExtensionDirectory(config);
  const wasHidden = modal.hidden;
  const status = extensionBridgeStatus(config);
  statusBox.className = `extension-status-card ${status.tone}`;
  statusBox.innerHTML = onboardingStatusMarkup(config, status);
  const shouldShow =
    !modal.hidden || (!state.onboardingDismissedForSession && !onboardingPermanentlyDismissed());
  modal.hidden = !shouldShow;
  if (shouldShow && wasHidden) {
    resetOnboardingScroll();
  }
}

export function showOnboarding() {
  state.onboardingDismissedForSession = false;
  renderExtensionDirectory(state.config);
  const modal = document.getElementById("onboarding-modal");
  if (modal) modal.hidden = false;
  const statusBox = document.getElementById("onboarding-status");
  if (statusBox) {
    const status = extensionBridgeStatus(state.config);
    statusBox.className = `extension-status-card ${status.tone}`;
    statusBox.innerHTML = onboardingStatusMarkup(state.config, status);
  }
  resetOnboardingScroll();
}

export function resetOnboardingScroll() {
  window.requestAnimationFrame(() => {
    const grid = document.querySelector(".onboarding-grid");
    if (grid) {
      grid.scrollTop = 0;
      grid.scrollLeft = 0;
    }
  });
}

export function onboardingStatusMarkup(config, status = extensionBridgeStatus(config)) {
  const configured = Boolean(config && config.extension_bridge_configured);
  const connected = Boolean(config && config.extension_bridge_connected);
  const checks = [
    {
      label: "服务配置",
      value: configured ? "已启用" : "未配置",
      ok: configured,
    },
    {
      label: "扩展连接",
      value: connected ? "已连接" : "等待连接",
      ok: connected,
    },
    ...(config?.skills || []).map((item) => ({ label: item.agent === "codex" ? "Codex skills" : "Claude skills", value: item.installed ? "已安装" : "未安装", ok: item.installed })),
  ];
  return `
    <div class="extension-status-copy">
      <strong>${escapeHtml(status.label)}</strong>
      <span>${escapeHtml(status.detail)}</span>
    </div>
    <div class="extension-check-grid" aria-label="扩展连接检查">
      ${checks
        .map(
          (check) => `
        <span class="extension-check ${check.ok ? "ok" : "pending"}">
          <span>${escapeHtml(check.label)}</span>
          <strong>${escapeHtml(check.value)}</strong>
        </span>
      `,
        )
        .join("")}
    </div>
  `;
}

export function dismissOnboardingForSession() {
  state.onboardingDismissedForSession = true;
  const modal = document.getElementById("onboarding-modal");
  if (modal) modal.hidden = true;
}

export function dismissOnboardingPermanently() {
  setOnboardingDismissed(true);
  dismissOnboardingForSession();
  showToast({
    tone: "success",
    title: "已隐藏首次引导",
    message: "扩展状态仍会在账号管理和设置运行里显示。",
  });
}

export function resetOnboardingGuide() {
  setOnboardingDismissed(false);
  state.onboardingDismissedForSession = false;
  const status = document.getElementById("runtime-action-status");
  if (status) status.textContent = "扩展引导已重置";
  showOnboarding();
  showToast({
    tone: "success",
    title: "扩展引导已重置",
    message: "首次连接弹层已重新打开。",
  });
}

export function onboardingPermanentlyDismissed() {
  try {
    return window.localStorage.getItem(ONBOARDING_STORAGE_KEY) === "true";
  } catch (_error) {
    return false;
  }
}

export function setOnboardingDismissed(value) {
  try {
    if (value) {
      window.localStorage.setItem(ONBOARDING_STORAGE_KEY, "true");
    } else {
      window.localStorage.removeItem(ONBOARDING_STORAGE_KEY);
    }
  } catch (_error) {}
}

export async function copyExtensionDirectory() {
  const button = document.getElementById("onboarding-copy-path");
  const extensionDir = extensionDirectory();
  if (button) button.textContent = "复制中";
  recordClientRuntimeInfo({
    scope: "复制扩展目录",
    route: "clipboard",
    message: "正在复制扩展目录。",
  });
  try {
    if (!extensionDir) throw new Error("扩展目录尚未读取，请先检查本地服务连接");
    await writeClipboard(extensionDir);
    if (button) button.textContent = "已复制";
    recordClientRuntimeInfo({
      scope: "复制扩展目录",
      route: "clipboard",
      message: "扩展目录已复制。",
    });
    showToast({
      tone: "success",
      title: "扩展目录已复制",
      message: "可返回扩展管理页加载本地扩展。",
    });
  } catch (error) {
    if (button) {
      button.textContent = "复制失败";
    }
    recordClientRuntimeError({ scope: "复制扩展目录", route: "clipboard", error });
    showToast({
      tone: "error",
      title: "复制扩展目录失败",
      message: "详细原因已写入运行日志。",
    });
  } finally {
    window.setTimeout(() => {
      if (button) button.textContent = "复制扩展目录";
    }, 1600);
  }
}

export async function openChromeExtensionsPage() {
  const desktopOpen = window.__OVERLEAF_DESKTOP__ && window.__OVERLEAF_DESKTOP__.openChromeExtensions;
  const profile = selectedChromeProfile();
  try {
    if (typeof desktopOpen === "function") {
      await desktopOpen((profile && profile.name) || state.selectedProfileName || null);
    } else {
      const opened = window.open("chrome://extensions/", "_blank", "noopener,noreferrer");
      if (!opened) throw new Error("当前环境无法打开 Chrome 扩展管理页");
    }
    showToast({
      tone: "success",
      title: "已打开扩展管理页",
      message: profile ? `目标档案：${profile.display_name || profile.name}` : "使用 Chrome 默认档案",
    });
  } catch (error) {
    recordClientRuntimeError({ scope: "打开扩展管理页", route: "chrome://extensions/", error });
    showToast({
      tone: "error",
      title: "打开扩展管理页失败",
      message: "详细原因已写入运行日志。",
    });
  }
}

export async function checkExtensionConnection() {
  const button = document.getElementById("onboarding-check-connection");
  if (button) {
    button.disabled = true;
    button.textContent = "检测中";
  }
  recordClientRuntimeInfo({
    scope: "检测扩展连接",
    route: "/config",
    message: "已开始刷新扩展桥连接状态。",
  });
  try {
    await refresh();
    const status = extensionBridgeStatus(state.config);
    recordClientRuntimeInfo({
      scope: "检测扩展连接",
      route: "/config",
      message: status.connected ? "扩展桥连接检测完成。" : "扩展桥连接检测完成，仍在等待客户端。",
    });
    showToast({
      tone: status.connected ? "success" : "warning",
      title: status.connected ? "扩展桥已连接" : "扩展桥仍在等待连接",
      message: status.detail,
    });
  } catch (error) {
    reportUiOperationFailure({
      status: null,
      shortMessage: "检测失败，请查看日志",
      title: "扩展连接检测失败",
      scope: "检测扩展连接",
      route: "/config",
      error,
    });
  } finally {
    if (button) {
      button.disabled = false;
      button.textContent = "检测扩展连接";
    }
  }
}

export function renderConfig(config) {
  const taskBrowser = document.getElementById("task-browser-open");
  if (taskBrowser) {
    const url = taskBrowserUrl(config);
    taskBrowser.hidden = !url;
    if (url) taskBrowser.href = url;
    else taskBrowser.removeAttribute("href");
  }
  applyDefaultExportDir(config);
  renderBrowserProxyPolicy(config && config.browser_proxy_policy);
  renderAppVersion(config);
  renderExtensionDirectory(config);
  renderSecretStorageFields(config && config.secret_storage);
  renderRegistrationCapabilityOptions(config && config.api_capabilities);
  renderAccountCapabilityControls(config && config.api_capabilities);
  renderAccountSwitchCapabilityControls(config && config.api_capabilities);
  renderAccountRemovalCapabilityControls(config && config.api_capabilities);
  renderAccountCredentialCapabilityControls(config && config.api_capabilities);
  renderAccountPasswordCapabilityControls(config && config.api_capabilities);
  renderAccountIoCapabilityControls(config && config.api_capabilities);
  renderBrowserProfileCapabilityControls(config && config.api_capabilities);
  renderRuntimeCapabilityControls(config && config.api_capabilities);
  renderTaskListCapabilityControls(config && config.api_capabilities);
  renderCardCapabilityControls(config && config.api_capabilities);
  renderAddressCapabilityControls(config && config.api_capabilities);
  renderServiceApiRoutes(config && config.api_capabilities);
  renderRuntimeHealthStrip(config);
  renderSkills(config);

  const groups = [
    { title: "本地数据", rows: [["工作区", config.workspace_dir], ["数据目录", config.data_dir]] },
    { title: "存储与浏览器", rows: [
      ["密钥存储", config.secret_storage?.encrypted ? "已加密" : "未启用加密"],
      ["本地数据", config.storage_status?.data_dir_exists ? "已就绪" : "尚未创建"],
      ["浏览器自动化", config.browser_automation_configured ? "已就绪" : "不可用"],
      ["扩展桥", extensionBridgeStatus(config).connected ? "已连接" : "等待连接", "data-extension-status"],
    ] },
    { title: "服务", rows: [
      ["本地服务", serviceHealthInfo().label, "data-service-health"],
      ["运行环境", desktopBridgeStatus().openDialog ? "桌面端" : "WebUI"],
    ] },
  ];

  document.getElementById("config-list").innerHTML = groups
    .map(
      (group) => `
      <div class="config-group">
        <div class="config-group-head">
          <strong>${escapeHtml(group.title)}</strong>
        </div>
        <div class="config-group-body">
          ${group.rows
            .map(
              ([label, value, attribute = ""]) => `
            <div class="config-row">
              <dt>${escapeHtml(label)}</dt>
              <dd ${attribute}>${escapeHtml(value || "-")}</dd>
            </div>
          `,
            )
            .join("")}
        </div>
      </div>
    `,
    )
    .join("");
}

export function renderAppVersion(config = state.config) {
  const version = typeof (config && config.app_version) === "string" ? config.app_version.trim() : "";
  if (!version) return;
  const label = version.startsWith("v") ? version : `v${version}`;
  document.querySelectorAll(".app-version").forEach((element) => {
    element.textContent = label;
    element.title = `当前版本 ${label}`;
  });
}

export function extensionDirectory(config = state.config) {
  return typeof config?.extension_dir === "string" ? config.extension_dir.trim() : "";
}

export function renderExtensionDirectory(config = state.config) {
  const element = document.getElementById("onboarding-extension-dir");
  if (!element) return;
  const extensionDir = extensionDirectory(config);
  element.textContent = extensionDir || "等待服务读取";
  element.title = extensionDir;
  document.getElementById("onboarding-copy-path").disabled = !extensionDir;
}

export function serviceHealthInfo() {
  if (state.serviceHealth === "ok") return { tone: "ready", label: "已就绪" };
  if (state.serviceHealth === "unavailable") return { tone: "danger", label: "无响应" };
  return { tone: "warning", label: "待检测" };
}

export function renderRuntimeHealthStrip(config) {
  const container = document.getElementById("runtime-health-strip");
  if (!container) return;
  if (!config) {
    container.innerHTML = `
      <div class="runtime-health-item warning">
        <span>运行态</span>
        <strong>等待配置</strong>
        <em>正在读取本地 service 配置</em>
      </div>
    `;
    return;
  }

  const bridge = extensionBridgeStatus(config);
  const bridgeTone = bridge.connected ? "ready" : bridge.configured ? "warning" : "danger";
  const profiles = discoveredProfiles(state.browserProfiles);
  const selectedProfile = selectedChromeProfile(state.browserProfiles);
  const profileTone = selectedProfile
    ? selectedProfile.cookie_store_present
      ? "ready"
      : "warning"
    : profiles.length
      ? "warning"
      : "danger";
  const profileLabel = selectedProfile
    ? selectedProfile.display_name || selectedProfile.name || "Chrome 档案"
    : profiles.length
      ? `${profiles.length} 个档案`
      : "未检测到档案";
  const profileDetail = selectedProfile
    ? selectedProfile.email || selectedProfile.profile_dir || "本机档案已选择"
    : "需要先扫描 Chrome 档案";
  const secretStorage = config.secret_storage || {};
  const secretTone = secretStorage.encrypted ? "ready" : "warning";
  const secretLabel = secretStorage.encrypted ? "已加密" : "安全存储未启用";
  const artifactItems =
    state.runtimeArtifacts && Array.isArray(state.runtimeArtifacts.items)
      ? state.runtimeArtifacts.items.length
      : null;
  const artifactBytes = state.runtimeArtifacts ? Number(state.runtimeArtifacts.total_bytes || 0) : 0;
  const tempPreview = state.tempProfilesPreview;
  const tempCount = tempPreview ? Number(tempPreview.candidate_count || 0) : null;
  const cleanupLabel = artifactItems === null ? "待扫描" : `${artifactItems} 项`;
  const cleanupDetail =
    artifactItems === null
      ? `临时档案 ${tempCount === null ? "待预览" : `${tempCount} 个候选`}`
      : `${byteCountText(artifactBytes)} · 临时档案 ${tempCount === null ? "待预览" : `${tempCount} 个候选`}`;

  const service = serviceHealthInfo();
  const items = [
    [service.tone, "本地服务", service.label, ""],
    [bridgeTone, "扩展桥", state.serviceHealth === "unavailable" ? "待确认" : bridge.connected ? "已连接" : "等待连接", ""],
    [profileTone, "Chrome 档案", profileLabel, profileDetail],
    [secretTone, "密钥存储", secretLabel, ""],
    [artifactItems === null ? "warning" : "ready", "清理边界", cleanupLabel, cleanupDetail],
  ];

  container.innerHTML = items
    .map(([tone, label, value, detail]) => runtimeHealthItemMarkup(tone, label, value, detail))
    .join("");
}

export function runtimeHealthItemMarkup(tone, label, value, detail) {
  return `
    <div class="runtime-health-item ${escapeAttr(tone || "info")}" title="${escapeAttr([value, detail].filter(Boolean).join(" · "))}">
      <span>${escapeHtml(label || "-")}</span>
      <strong>${escapeHtml(value || "-")}</strong>
      ${detail ? `<em>${escapeHtml(detail)}</em>` : ""}
    </div>
  `;
}

export function applyDefaultExportDir(config) {
  const input = document.getElementById("export-output-dir");
  const value = config && config.default_export_dir;
  if (!input || input.value.trim() || typeof value !== "string" || !value.trim()) return;
  input.value = value;
}

export function renderBrowserProxyPolicy(policy) {
  const mode = document.getElementById("browser-proxy-mode");
  const server = document.getElementById("browser-proxy-server");
  if (!mode || !server) return;
  const nextMode = policy && typeof policy.mode === "string" ? policy.mode : "auto";
  mode.value = ["auto", "inherit_system", "direct", "custom"].includes(nextMode) ? nextMode : "auto";
  server.value = policy && typeof policy.server === "string" ? policy.server : "";
  syncBrowserProxyFields();
}

export function syncBrowserProxyFields() {
  const mode = document.getElementById("browser-proxy-mode");
  const field = document.getElementById("browser-proxy-server-field");
  const server = document.getElementById("browser-proxy-server");
  if (!mode || !field || !server) return;
  const custom = mode.value === "custom";
  field.hidden = !custom;
  server.disabled = !custom;
  server.required = custom;
}

export function restoreSelectedProfile() {
  try {
    state.selectedProfileName = window.localStorage.getItem(PROFILE_STORAGE_KEY) || "";
  } catch (_error) {
    state.selectedProfileName = "";
  }
}

export function saveSelectedProfile(name) {
  state.selectedProfileName = name || "";
  try {
    if (state.selectedProfileName) {
      window.localStorage.setItem(PROFILE_STORAGE_KEY, state.selectedProfileName);
    } else {
      window.localStorage.removeItem(PROFILE_STORAGE_KEY);
    }
  } catch (_error) {}
}

export function discoveredProfiles(discovery = state.browserProfiles) {
  return discovery && Array.isArray(discovery.profiles) ? discovery.profiles : [];
}

export function selectedChromeProfile(discovery = state.browserProfiles) {
  const profiles = discoveredProfiles(discovery);
  if (!profiles.length) return null;
  const selected = profiles.find((profile) => profile.name === state.selectedProfileName);
  if (selected) return selected;
  const withCookie = profiles.find((profile) => profile.cookie_store_present);
  const fallback = withCookie || profiles[0];
  if (fallback && state.selectedProfileName !== fallback.name) {
    saveSelectedProfile(fallback.name);
  }
  return fallback;
}

export function renderTopProfile(discovery = state.browserProfiles) {
  const switcher = document.getElementById("profile-switcher");
  const select = document.getElementById("profile-select");
  const name = document.getElementById("profile-display-name");
  const email = document.getElementById("profile-email");
  const avatar = document.getElementById("profile-avatar");
  const storage = document.getElementById("profile-storage");
  if (!select || !avatar) return;

  const profiles = discoveredProfiles(discovery);
  const selected = selectedChromeProfile(discovery);
  const bridge = extensionBridgeStatus(state.config);
  const overleaf = currentOverleafAccountDisplay(discovery);
  select.innerHTML = profiles.length
    ? profiles
        .map((profile) => {
          const label = `${profile.display_name || profile.name}${profile.email ? ` · ${profile.email}` : ""}`;
          return `<option value="${escapeHtml(profile.name)}">${escapeHtml(label)}</option>`;
        })
        .join("")
    : `<option value="">未检测到档案</option>`;
  select.disabled = profiles.length === 0;
  if (selected) {
    select.value = selected.name;
    if (name) name.textContent = selected.display_name || selected.name || "Chrome 档案";
    if (email) email.textContent = selected.email || selected.name || "未登录 Google";
    if (storage) {
      const cookieText = selected.cookie_store_present ? "Cookie 存储存在" : "缺少 Cookie 存储";
      storage.textContent = `已发现 ${profiles.length} 个档案 · ${cookieText}`;
      storage.className = `profile-storage ${selected.cookie_store_present ? "present" : "missing"}`;
      storage.title = selected.profile_dir || cookieText;
    }
    const avatarUrl = profileAvatarUrl(selected);
    avatar.textContent = avatarUrl ? "" : profileInitial(selected);
    avatar.style.backgroundImage = avatarUrl ? `url("${cssUrlEscape(avatarUrl)}")` : "";
    avatar.title = selected.profile_dir || "";
  } else {
    select.value = "";
    if (name) name.textContent = "未检测";
    if (email) email.textContent = discovery && discovery.error ? discovery.error : "等待扫描";
    if (storage) {
      storage.textContent = "等待 Cookie 存储检测";
      storage.className = "profile-storage missing";
      storage.title = "";
    }
    avatar.textContent = "?";
    avatar.style.backgroundImage = "";
    avatar.title = "";
  }
  if (switcher) {
    switcher.className = [
      "profile-switcher",
      "profile-panel",
      selected ? "has-profile" : "missing-profile",
      bridge.connected ? "bridge-connected" : bridge.configured ? "bridge-waiting" : "bridge-missing",
      `overleaf-${overleaf.state || "idle"}`,
    ].join(" ");
    switcher.dataset.profileState = selected ? "connected" : "idle";
    switcher.dataset.bridgeState = bridge.connected ? "connected" : bridge.configured ? "warning" : "error";
    switcher.dataset.overleafState = overleaf.state || "idle";
  }
  renderTopProfileRoute(selected, discovery, { bridge, overleaf });
  renderProfileChainDetail(selected, discovery, { bridge, overleaf });
}

export function renderTopProfileRoute(selectedProfile, discovery = state.browserProfiles, known = {}) {
  setProfileRouteState("profile-route-chrome", selectedProfile ? "connected" : "idle");
  const bridge = known.bridge || extensionBridgeStatus(state.config);
  setProfileRouteState(
    "profile-route-bridge",
    bridge.connected ? "connected" : bridge.configured ? "warning" : "error",
  );
  const overleaf = known.overleaf || currentOverleafAccountDisplay(discovery);
  setProfileRouteState("profile-route-overleaf", overleaf.state);
  const chip = document.getElementById("profile-overleaf-account");
  if (chip) {
    chip.textContent = overleaf.text;
    chip.className = `profile-overleaf-account ${overleaf.state}`;
    chip.title = overleaf.detail || overleaf.text;
  }
}

export function setProfileRouteState(id, stateName) {
  const element = document.getElementById(id);
  if (element) {
    element.dataset.state = stateName || "idle";
  }
}

export function currentOverleafAccountDisplay(discovery = state.browserProfiles) {
  const report = state.browserCurrentAccount;
  const savedAlias = report && (report.saved_alias || report.alias);
  if (savedAlias) {
    const email = report.email ? ` · ${report.email}` : "";
    return {
      state: "connected",
      text: `当前账号：${savedAlias}${email}`,
      detail: "浏览器 Cookie 与本地已保存账号匹配",
    };
  }
  if (report && report.email) {
    return {
      state: "warning",
      text: `未保存：${report.email}`,
      detail: "浏览器中有 Overleaf 登录态，但没有匹配到本地账号",
    };
  }
  if (report && report.cookie_present) {
    return {
      state: "warning",
      text: "Cookie 存在，账号未识别",
      detail: "扩展已读取到 Overleaf Cookie，但账号信息不足",
    };
  }
  if (state.browserCurrentAccountError) {
    return {
      state: "error",
      text: "账号检测失败",
      detail: state.browserCurrentAccountError,
    };
  }
  if (!extensionBridgeConnected()) {
    return {
      state: "error",
      text: "扩展桥未连接",
      detail: "连接 Chrome 扩展后才能检测当前 Overleaf 账号",
    };
  }
  const email = state.dashboard && state.dashboard.browser_account_email;
  if (email) {
    return {
      state: "connected",
      text: `浏览器账号：${email}`,
      detail: "来自运行态摘要的最近检测结果",
    };
  }
  const profiles = discoveredProfiles(discovery);
  return {
    state: profiles.length ? "idle" : "warning",
    text: profiles.length ? "等待检测账号" : "等待扫描档案",
    detail: profiles.length ? "点击顶部重检可刷新档案、扩展桥和当前 Overleaf 账号" : "需要先扫描 Chrome 档案",
  };
}

export function renderProfileChainDetail(selectedProfile, discovery = state.browserProfiles, known = {}) {
  const container = document.getElementById("profile-chain-detail");
  if (!container) return;
  const profiles = discoveredProfiles(discovery);
  const bridge = known.bridge || extensionBridgeStatus(state.config);
  const overleaf = known.overleaf || currentOverleafAccountDisplay(discovery);
  const profileState = selectedProfile
    ? selectedProfile.cookie_store_present
      ? "connected"
      : "warning"
    : "warning";
  const profileTitle = selectedProfile
    ? selectedProfile.display_name || selectedProfile.name || "Chrome 档案"
    : "未检测到档案";
  const profileDetail = selectedProfile
    ? `${selectedProfile.email || selectedProfile.name || "未登录 Google"} · ${profiles.length || 0} 个档案`
    : "先扫描本机 Chrome 档案";
  const bridgeState = bridge.connected ? "connected" : bridge.configured ? "warning" : "error";
  const bridgeDetail = bridge.connected
    ? `${bridge.clients || 0} 个扩展客户端在线`
    : bridge.configured
      ? "等待扩展连接"
      : "扩展桥未配置";
  container.innerHTML = [
    profileChainItemMarkup("Chrome 档案", profileTitle, profileDetail, profileState),
    profileChainItemMarkup("扩展桥", bridge.label, bridgeDetail, bridgeState),
    profileChainItemMarkup(
      "Overleaf",
      overleaf.text,
      overleaf.detail || "等待检测浏览器账号",
      overleaf.state || "idle",
    ),
  ].join("");
}

export function profileChainItemMarkup(label, value, detail, stateName) {
  return `
    <div class="profile-chain-item" data-state="${escapeAttr(stateName || "idle")}" title="${escapeAttr(`${value} · ${detail}`)}">
      <span>${escapeHtml(label)}</span>
      <strong>${escapeHtml(value || "-")}</strong>
      <em>${escapeHtml(detail || "-")}</em>
    </div>
  `;
}

export function profileInitial(profile) {
  const text = (profile && (profile.display_name || profile.email || profile.name)) || "?";
  return text.trim().slice(0, 1).toUpperCase() || "?";
}

export function profileAvatarUrl(profile) {
  if (!profile) return "";
  return profile.avatar_url || profile.picture_url || profile.picture || profile.image_url || "";
}

export function cssUrlEscape(value) {
  return String(value || "").replace(/["\\\n\r]/g, "");
}

export async function handleProfileSelectionChange(event) {
  saveSelectedProfile(event.target.value || "");
  state.browserCurrentAccount = null;
  state.browserCurrentAccountError = "";
  renderTopProfile(state.browserProfiles);
  const status = document.getElementById("browser-account-status");
  if (status) status.textContent = "档案已切换，正在重新检测扩展状态";
  await refresh();
}

export async function refreshProfileAndBridge() {
  const button = document.getElementById("profile-redetect");
  const status = document.getElementById("browser-account-status");
  const originalText = button ? button.textContent : "";
  if (button) {
    button.disabled = true;
    button.textContent = "重检中";
  }
  try {
    await refresh();
    if (browserProfileActions().includes("detect-current-account") && extensionBridgeConnected()) {
      if (button) button.textContent = "检测中";
      await detectCurrentBrowserAccount({ status, refreshAfter: false });
    } else {
      await detectCurrentBrowserAccount({ status, refreshAfter: false });
    }
  } finally {
    if (button) {
      button.disabled = false;
      button.textContent = originalText || "重检";
    }
  }
}

export function renderBrowserProfiles(discovery) {
  const container = document.getElementById("browser-profiles-list");
  if (!container) return;
  if (!browserProfileDiscoverySupported()) {
    container.innerHTML = `<div class="empty">当前后端不支持 Chrome 档案发现。</div>`;
    return;
  }
  if (!discovery) {
    container.innerHTML = `<div class="empty">Chrome 档案暂不可用。</div>`;
    return;
  }

  const source = discovery.user_data_dir || discovery.local_state_path || "";
  const sourceLine = source ? `<div class="muted">来源：${escapeHtml(source)}</div>` : "";
  if (discovery.error) {
    container.innerHTML = `<div class="empty">Chrome 档案暂不可用：${escapeHtml(discovery.error)}${sourceLine}</div>`;
    return;
  }

  const profiles = Array.isArray(discovery.profiles) ? discovery.profiles : [];
  if (!profiles.length) {
    const message = discovery.local_state_present
      ? "没有找到 Chrome 档案。"
      : "没有找到 Chrome Local State。";
    container.innerHTML = `<div class="empty">${escapeHtml(message)}${sourceLine}</div>`;
    return;
  }

  container.innerHTML = profiles
    .map((profile) => {
      const cookieClass = profile.cookie_store_present ? "present" : "missing";
      const cookieText = profile.cookie_store_present ? "Cookie 存储存在" : "缺少 Cookie 存储";
      const selected = profile.name === state.selectedProfileName;
      return `
        <div class="list-item browser-profile-item">
          <div class="browser-profile-main">
            <strong>${escapeHtml(profile.display_name || profile.name || "Chrome 档案")}</strong>
            <span class="muted">${escapeHtml(profile.name || "-")} · ${escapeHtml(profile.email || "-")}</span>
          </div>
          <div class="browser-profile-badges" aria-label="档案状态">
            ${selected ? `<span class="pill present">当前目标</span>` : `<span class="pill unknown">可切换</span>`}
            <span class="pill ${cookieClass}">${escapeHtml(cookieText)}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

export function renderTempProfilePreview(preview) {
  const container = document.getElementById("temp-profiles-preview");
  if (!container) return;
  if (!tempProfilePreviewSupported()) {
    container.innerHTML = `<div class="empty">当前后端不支持临时档案预览。</div>`;
    return;
  }
  if (!preview) {
    container.innerHTML = `<div class="empty">临时档案预览暂不可用。</div>`;
    return;
  }

  const candidateCount = Number(preview.candidate_count || 0);
  const candidateBytes = Number(preview.candidate_bytes || 0);
  const skippedCount = Number(preview.skipped_user_window_count || 0);
  const skippedBytes = Number(preview.skipped_user_window_bytes || 0);
  const items = Array.isArray(preview.items) ? preview.items : [];
  const summary = `
    <div class="list-item">
      <strong>临时档案清理预览</strong>
      <div class="muted">${candidateCount} 个候选 (${byteCountText(candidateBytes)})，已跳过 ${skippedCount} 个用户窗口档案 (${byteCountText(skippedBytes)})</div>
    </div>
  `;

  if (!items.length) {
    container.innerHTML = `${summary}<div class="empty">没有需要清理的临时 Chrome 档案。</div>`;
    return;
  }

  container.innerHTML =
    summary +
    items
      .map((item) => {
        const status = item.status || "unknown";
        const statusClass = status === "candidate" ? "pending" : "unknown";
        return `
          <div class="list-item">
            <strong>${escapeHtml(item.profile_name || "-")}</strong>
            <span class="pill ${statusClass}">${escapeHtml(statusText(status))}</span>
            <div class="muted">${byteCountText(item.bytes || 0)}</div>
          </div>
        `;
      })
      .join("");
}

export async function scanRuntimeArtifacts() {
  const button = document.getElementById("scan-artifacts");
  const status = document.getElementById("runtime-action-status");
  if (!runtimeDiagnosticActions().includes("runtime_artifacts")) {
    setTransientUiStatus(status, "操作不可用");
    recordClientRuntimeError({
      scope: "扫描项目产物",
      route: endpoints.runtimeArtifacts,
      error: new Error("当前后端不支持产物扫描"),
    });
    return;
  }
  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在扫描",
    scope: "扫描项目产物",
    route: endpoints.runtimeArtifacts,
    message: "已开始扫描项目产物。",
  });
  try {
    const report = await fetchJson(endpoints.runtimeArtifacts);
    state.runtimeArtifacts = report;
    renderRuntimeArtifacts(report);
    renderRuntimeHealthStrip(state.config);
    reportUiOperationSuccess({
      status,
      shortMessage: "扫描完成",
      title: "项目产物扫描完成",
      scope: "扫描项目产物",
      route: endpoints.runtimeArtifacts,
      message: `发现 ${nonNegativeCount(report.item_count)} 个产物项，占用 ${byteCountText(report.total_bytes || 0)}。`,
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "扫描失败，请查看日志",
      title: "项目产物扫描失败",
      scope: "扫描项目产物",
      route: endpoints.runtimeArtifacts,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

function artifactKindLabel(kind) {
  if (String(kind).includes("node_modules")) return "前端依赖";
  if (String(kind).includes("cache")) return "临时缓存";
  if (kind === "web_build") return "网页构建";
  return "应用构建";
}

export function renderRuntimeArtifacts(report) {
  const container = document.getElementById("runtime-artifacts-list");
  if (!container) return;
  if (!runtimeDiagnosticActions().includes("runtime_artifacts")) {
    container.innerHTML = `<div class="empty">当前后端不支持产物扫描。</div>`;
    return;
  }
  if (!report) {
    container.innerHTML = `<div class="empty">尚未扫描产物</div>`;
    return;
  }

  const items = Array.isArray(report.items) ? report.items : [];
  const summary = `
    <div class="list-item">
      <strong>项目产物扫描</strong>
      <div class="muted">${items.length} 个项目，${byteCountText(report.total_bytes || 0)}</div>
      <div class="muted">${escapeHtml(report.workspace_dir || "-")}</div>
    </div>
  `;

  if (!items.length) {
    container.innerHTML = `${summary}<div class="empty">没有发现可跟踪的项目产物。</div>`;
    return;
  }

  container.innerHTML =
    summary +
    items
      .map((item) => {
        return `
          <div class="list-item">
            <strong>${escapeHtml(item.name || item.kind || "artifact")}</strong>
            <span class="pill pending">${escapeHtml(artifactKindLabel(item.kind))}</span>
            <div class="muted">${byteCountText(item.bytes || 0)} · ${escapeHtml(item.path || "-")}</div>
            <div class="muted">${escapeHtml(item.kind?.includes("cache") ? "可清理的临时缓存" : "可重新生成的构建产物")}</div>
          </div>
        `;
      })
      .join("");
}

export function renderSecretStorageFields(secretStorage) {
  const container = document.getElementById("secret-storage-fields");
  if (!container) return;
  const labels = { password: "账号密码", cookie: "登录 Cookie", git_token: "Git 令牌", card_number: "银行卡号", card_cvc: "卡片安全码" };
  container.innerHTML = (secretStorage?.sensitive_fields || []).map((field) => `
    <div class="config-row"><dt>${escapeHtml(labels[field.category] || "敏感字段")}</dt>
      <dd>${secretStorage.encrypted ? "已加密" : "未加密"}</dd></div>`).join("");
}

export function showRuntimeDiagnostics() {
  const config = state.config;
  if (!config) return;
  recordClientRuntimeInfo({ scope: "运行诊断", route: endpoints.config,
    message: JSON.stringify({ browser: config.browser_automation, storage: config.storage_status,
      secrets: config.secret_storage, capabilities: config.api_capabilities, desktop: desktopBridgeStatus(),
      artifacts: state.runtimeArtifacts }, null, 2) });
  applyRuntimeLogCollapsed(false);
  showToast({ tone: "info", title: "运行诊断已写入日志" });
}

export function byteCountText(value) {
  if (value === null || value === undefined) return "未创建";
  const bytes = Number(value) || 0;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export async function cleanupTempProfiles() {
  return cleanupRuntime(true);
}

export async function cleanupRuntimeArtifacts() {
  return cleanupRuntime(false);
}

async function cleanupRuntime(profiles) {
  const button = document.getElementById(profiles ? "cleanup-temp-profiles" : "cleanup-artifacts");
  const status = document.getElementById("runtime-action-status");
  const action = profiles ? "runtime_temp_profiles_cleanup" : "runtime_artifacts_cleanup";
  const route = profiles ? endpoints.cleanupTempProfiles : endpoints.cleanupRuntimeArtifacts;
  const label = profiles ? "临时档案" : "构建产物";
  const scope = `清理${label}`;
  if (!runtimeCleanupActions().includes(action)) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: `${label}清理不可用`,
      scope,
      route,
      error: new Error(profiles ? "当前后端不支持临时档案清理" : "当前后端不支持产物清理"),
    });
    return;
  }
  const confirmed = await showConfirmDialog({
    eyebrow: profiles ? "运行清理" : "产物清理",
    title: profiles ? "清理临时 Chrome 档案" : "清理可跟踪的构建产物",
    message: profiles
      ? "只清理本程序创建的临时 Chrome 档案，浏览器自动登录保留的用户窗口会跳过。"
      : "仅清理当前 workspace 内可跟踪的构建产物和 Python cache，不包含 data、backups 和账号文件。",
    points: profiles
      ? ["不会删除账号数据、Cookie、卡片、地址或备份。", "清理失败的条目会保留在结果里，便于后续复查。"]
      : ["建议先查看产物扫描结果，再执行清理。", "可复用构建缓存默认保留，除非后端报告为可清理产物。"],
    confirmText: scope,
    tone: profiles ? "warning" : "danger",
  });
  if (!confirmed) {
    showToast({ tone: "info", title: "已取消清理", message: profiles ? "临时 Chrome 档案未被修改。" : "构建产物未被修改。" });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在清理",
    scope,
    route,
    message: `已提交${label}清理。`,
  });
  try {
    const report = await postJson(route, { confirm_cleanup: true });
    const removed = nonNegativeCount(report.removed_count);
    const failed = nonNegativeCount(report.failed_count);
    const detail = profiles
      ? `已清理 ${removed} 个，失败 ${failed} 个，跳过 ${nonNegativeCount(report.skipped_user_window_count)} 个用户档案。`
      : `已清理 ${removed} 个产物项，失败 ${failed} 个，释放 ${byteCountText(report.removed_bytes || 0)}。`;
    reportUiOperationSuccess({
      status,
      shortMessage: failed ? "清理部分完成" : "清理完成",
      title: `${label}清理完成`,
      scope,
      route,
      message: detail,
      tone: failed ? "warning" : "success",
    });
    if (profiles) {
      await refresh();
    } else if (runtimeDiagnosticActions().includes("runtime_artifacts")) {
      const scan = await fetchJson(endpoints.runtimeArtifacts);
      state.runtimeArtifacts = scan;
      renderRuntimeArtifacts(scan);
      renderRuntimeHealthStrip(state.config);
    }
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "清理失败，请查看日志",
      title: `${label}清理失败`,
      scope,
      route,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export function renderError(error) {
  if (state.serviceHealth === "ok") void checkServiceHealth();
  document.getElementById("dashboard-grid").innerHTML =
    `<div class="error">服务暂不可用，请查看运行日志。</div>`;
}

export async function saveBrowserProxyPolicy(event) {
  event.preventDefault();
  const mode = document.getElementById("browser-proxy-mode");
  const server = document.getElementById("browser-proxy-server");
  const button = document.getElementById("browser-proxy-save");
  const status = document.getElementById("browser-proxy-status");
  if (!mode || !server || !button || !status) return;

  const customServer = server.value.trim();
  if (mode.value === "custom" && !customServer) {
    setTransientUiStatus(status, "请输入代理地址", 4600);
    recordClientRuntimeInfo({
      scope: "浏览器网络策略",
      route: endpoints.updateBrowserProxy,
      message: "自定义代理模式缺少代理地址。",
    });
    return;
  }

  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在保存",
    scope: "浏览器网络策略",
    route: endpoints.updateBrowserProxy,
    message: "已提交浏览器网络策略保存。",
  });
  try {
    const config = await postJson(endpoints.updateBrowserProxy, {
      mode: mode.value,
      server: mode.value === "custom" ? customServer : null,
    });
    state.config = config;
    renderConfig(config);
    reportUiOperationSuccess({
      status,
      shortMessage: "已保存",
      title: "浏览器网络策略已保存",
      scope: "浏览器网络策略",
      route: endpoints.updateBrowserProxy,
      message: `已切换为 ${browserProxyModeLabel(mode.value)}。`,
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "网络策略保存失败，请查看运行日志",
      title: "网络策略保存失败",
      scope: "浏览器网络策略",
      route: endpoints.updateBrowserProxy,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export function browserProxyModeLabel(mode) {
  return (
    {
      auto: "自动（直连优先）",
      inherit_system: "继承系统代理",
      direct: "直连",
      custom: "自定义代理",
    }[mode] || "自动（直连优先）"
  );
}
