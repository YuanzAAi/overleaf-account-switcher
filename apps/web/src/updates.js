import { endpoints, state } from "./state.js";
import { fetchJson } from "./http.js";
import { reportUiOperationFailure, showConfirmDialog, showToast } from "./feedback.js";
import { writeClipboard } from "./format.js";

const dockerCommand = "docker compose -f docker/compose.yml pull\ndocker compose -f docker/compose.yml up -d --no-build";
let release = null;

function updateStatus(message) {
  document.getElementById("update-status").textContent = message;
}

function renderUpdateActions() {
  const available = Boolean(release?.available);
  const docker = state.config?.deployment === "docker";
  const native = typeof window.__OVERLEAF_DESKTOP__?.installUpdate === "function";
  const download = document.getElementById("download-update");
  document.getElementById("install-update").hidden = !available || !native || docker;
  download.hidden = !available || native || docker || !release.asset;
  if (!download.hidden) download.href = release.asset.browser_download_url;
  else download.removeAttribute("href");
  document.getElementById("docker-update").hidden = !available || !docker;
  document.getElementById("docker-update-image").textContent = release?.image || "";
  document.getElementById("docker-update-command").textContent = dockerCommand;
}

export async function checkForUpdates() {
  const button = document.getElementById("check-updates");
  if (button.disabled) return;
  button.disabled = true;
  release = null;
  renderUpdateActions();
  updateStatus("正在检查更新");
  try {
    release = await fetchJson(endpoints.updates, 25000);
    updateStatus(!release.latest_version ? "暂无公开稳定版本"
      : release.available ? `发现新版本 v${release.latest_version}` : "当前已是最新版本");
    renderUpdateActions();
  } catch (error) {
    updateStatus("检查失败，请稍后重试");
    reportUiOperationFailure({ scope: "检查应用更新", route: endpoints.updates, error, title: "检查更新失败" });
  } finally {
    button.disabled = false;
  }
}

export async function installUpdate() {
  const install = window.__OVERLEAF_DESKTOP__?.installUpdate;
  const button = document.getElementById("install-update");
  if (!release?.available || !install || button.disabled) return;
  const accepted = await showConfirmDialog({
    eyebrow: "应用更新",
    title: `更新到 v${release.latest_version}`,
    message: "下载并校验更新后，应用将退出并重新打开。账号数据保持不变。",
    points: ["请先完成或取消正在运行和等待验证的任务。"],
    confirmText: "下载并更新",
    tone: "info",
  });
  if (!accepted) return;
  button.disabled = true;
  document.getElementById("check-updates").disabled = true;
  updateStatus("正在下载并校验更新");
  try {
    await install(release.tag);
    updateStatus("更新就绪，正在重启");
  } catch (error) {
    updateStatus("更新未完成");
    reportUiOperationFailure({ scope: "应用更新", route: endpoints.updates, error: error instanceof Error ? error : new Error(String(error)), title: "应用更新失败" });
  } finally {
    button.disabled = false;
    document.getElementById("check-updates").disabled = false;
  }
}

export async function copyDockerUpdateCommand() {
  try {
    await writeClipboard(dockerCommand);
    showToast({ tone: "success", title: "更新命令已复制" });
  } catch (error) {
    reportUiOperationFailure({ scope: "复制更新命令", route: "clipboard", error, title: "复制失败" });
  }
}
