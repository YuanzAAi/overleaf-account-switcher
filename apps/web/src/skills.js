import { endpoints, state } from "./state.js";
import { escapeHtml, escapeAttr } from "./format.js";
import { iconSvg } from "./icons.js";
import { fetchJson } from "./http.js";
import { postJson, updateTasks } from "./sync.js";
import { makeTaskId, registerPendingAccountOperation } from "./tasks/list.js";
import { reportUiOperationFailure, reportUiOperationSuccess, showConfirmDialog, showToast } from "./feedback.js";
import { renderConfig, renderOnboarding } from "./settings.js";

const pending = new Set();
const agentNames = { codex: "Codex", claude: "Claude Code" };

export function renderSkills(config = state.config) {
  const container = document.getElementById("skills-list");
  if (!container) return;
  container.innerHTML = (config?.skills || []).map(({ agent, path, installed }) => `
    <div class="skill-row">
      <div class="skill-info"><strong>${escapeHtml(agentNames[agent] || agent)}</strong>
        <span class="pill ${installed ? "ready" : "pending"}">${installed ? "已安装" : "未安装"}</span>
        <span class="muted skill-path">${escapeHtml(path || "未找到用户目录")}</span>
      </div>
      <div class="settings-action-row">
        ${[["install", "download", installed ? "已安装" : "安装"], ...(installed ? [["update", "refresh", "更新"], ["uninstall", "trash", "卸载"]] : [])].map(([action, icon, label]) => `
          <button class="ui-btn" type="button" data-skill-agent="${escapeAttr(agent)}" data-skill-action="${action}" ${pending.has(agent) || !path ? "disabled" : ""}>${iconSvg(icon)}<span>${label}</span></button>
        `).join("")}
      </div>
    </div>`).join("") || '<div class="empty">skills 管理暂不可用</div>';
}

export function syncSkillsOnSwitch() {
  return Boolean(document.getElementById("account-toolbar-sync-skills")?.checked);
}

export async function manageSkill(button) {
  if (!button || button.disabled) return;
  const { skillAgent: agent, skillAction: action } = button.dataset;
  if (pending.has(agent)) return;
  if (action === "install" && state.config?.skills?.find((item) => item.agent === agent)?.installed) {
    showToast({ tone: "info", title: `${agentNames[agent]} skills 已安装` });
    return;
  }
  if (action === "uninstall" && !await showConfirmDialog({ title: `卸载 ${agentNames[agent]} 的 Overleaf skills`, message: "仅移除 skills 文件，账号状态、缓存和自定义文件会保留。", confirmText: "卸载", tone: "warning" })) return;
  const title = `${agentNames[agent]} skills ${{ install: "安装", update: "更新", uninstall: "卸载" }[action]}`;
  pending.add(agent);
  renderSkills();
  try {
    const taskId = makeTaskId("skills", agent);
    const task = await postJson(endpoints.skills, { agent, action, task_id: taskId });
    const terminal = await new Promise((resolve) => {
      registerPendingAccountOperation(taskId, resolve);
      updateTasks([task], { incremental: true });
    });
    if (terminal.phase === "failed") throw new Error(terminal.error || terminal.message);
    reportUiOperationSuccess({ title, scope: title, route: endpoints.skills,
      message: terminal.message, tone: terminal.phase === "cancelled" ? "info" : "success" });
    state.config = await fetchJson(endpoints.config);
    renderConfig(state.config);
    renderOnboarding(state.config);
  } catch (error) {
    reportUiOperationFailure({ title: `${title}失败`, scope: title, route: endpoints.skills, error });
  } finally {
    pending.delete(agent);
    renderSkills();
  }
}
