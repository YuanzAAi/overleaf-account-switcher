import { endpoints, state } from "../state.js";
import { request } from "../http.js";
import { postTaskResult } from "../sync.js";
import { makeTaskId } from "../tasks/list.js";
import { escapeHtml, escapeAttr } from "../format.js";
import { iconSvg } from "../icons.js";
import { recordClientRuntimeError, showToast } from "../feedback.js";
import { resolveTauriDialogSave, resolveTauriWriteBinaryFile, selectedDialogPath } from "../bridge.js";

const windows = new Map();
let layer = 100;
let migrationDialog = null;

function button(action, icon, title) {
  return `<button type="button" class="icon-btn" data-project-action="${action}" title="${title}" aria-label="${title}">${iconSvg(icon)}</button>`;
}

function projectDate(value) {
  const date = new Date(value);
  return value && Number.isFinite(date.getTime()) ? date.toLocaleString() : "";
}

async function call(alias, action, fields = {}) {
  return request(endpoints.projects, { method: "POST", payload: {
    alias, action, ...fields, task_id: makeTaskId(`project-${action}`, alias || "current"),
  } });
}

function failure(error, status) {
  recordClientRuntimeError({ scope: "项目管理", route: endpoints.projects, error });
  status.textContent = "操作失败，请查看运行日志";
}

async function saveDownload(report) {
  const filename = report.filename.replace(/[<>:"/\\|?*\x00-\x1f]/g, "_");
  const save = resolveTauriDialogSave(), write = resolveTauriWriteBinaryFile();
  if (save && write) {
    const extension = filename.split(".").pop();
    const path = selectedDialogPath(await save({defaultPath:filename,
      filters:[{name:extension.toUpperCase(), extensions:[extension]}]}));
    if (path) await write(path, report.data);
    return;
  }
  const bytes = Uint8Array.from(atob(report.data), c => c.charCodeAt(0));
  const url = URL.createObjectURL(new Blob([bytes], {type:report.content_type}));
  const link = document.createElement("a");
  link.href = url; link.download = filename;
  document.body.append(link); link.click(); link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30000);
}

function createWindow(title, modal) {
  const dialog = document.createElement("dialog");
  dialog.className = "project-window";
  dialog.setAttribute("aria-label", title);
  dialog.innerHTML = `<header class="project-window-head"><strong>${escapeHtml(title)}</strong>
    <button class="icon-btn" type="button" data-close title="关闭" aria-label="关闭">${iconSvg("close")}</button></header>
    <div class="project-window-tools"><input type="search" placeholder="搜索项目" aria-label="搜索项目">
    <select aria-label="项目范围"><option value="active">工作区</option><option value="archived">已归档</option><option value="trashed">回收站</option></select>
    ${button("refresh", "refresh", "刷新项目")}</div>
    <div class="project-window-table"><table><thead><tr><th class="project-pick"><input class="ui-checkbox" type="checkbox" data-select-all aria-label="全选项目"></th><th>项目</th><th>Owner</th><th>修改时间</th><th data-actions>操作</th></tr></thead><tbody></tbody></table></div>
    <div class="project-window-confirm" hidden></div>
    <footer><span role="status">正在读取项目...</span><button class="ui-btn" type="button" data-submit hidden>无感换号并迁移</button></footer>`;
  document.body.append(dialog);
  dialog.querySelector("[data-close]").onclick = () => dialog.close();
  if (modal) dialog.showModal();
  else {
    dialog.show();
    const offset = (windows.size % 5) * 24;
    dialog.style.left = `${Math.min(180 + offset, Math.max(12, window.innerWidth - dialog.offsetWidth - 12))}px`;
    dialog.style.top = `${90 + offset}px`;
    dialog.style.margin = "0";
    dialog.style.zIndex = ++layer;
    const fit = () => {
      const box = dialog.getBoundingClientRect();
      const top = Math.max(12, Math.min(window.innerHeight - 120, box.top));
      dialog.style.left = `${Math.max(12, Math.min(window.innerWidth - dialog.offsetWidth - 12, box.left))}px`;
      dialog.style.top = `${top}px`;
      dialog.style.maxHeight = `${window.innerHeight - top - 12}px`;
    };
    fit();
    window.addEventListener("resize", fit);
    dialog.addEventListener("close", () => window.removeEventListener("resize", fit), {once:true});
    dialog.addEventListener("pointerdown", () => { dialog.style.zIndex = ++layer; });
    const head = dialog.querySelector("header");
    head.addEventListener("pointerdown", (event) => {
      if (event.target.closest("button")) return;
      const box = dialog.getBoundingClientRect();
      const dx = event.clientX - box.left, dy = event.clientY - box.top;
      head.setPointerCapture(event.pointerId);
      head.onpointermove = (move) => {
        dialog.style.left = `${Math.max(0, Math.min(window.innerWidth - dialog.offsetWidth, move.clientX - dx))}px`;
        dialog.style.top = `${Math.max(0, Math.min(window.innerHeight - 50, move.clientY - dy))}px`;
        fit();
      };
      head.onlostpointercapture = () => { head.onpointermove = null; };
    });
  }
  return dialog;
}

function projectWorkspace(alias, { target, onSelect } = {}) {
  const picking = Boolean(onSelect);
  const dialog = createWindow(picking ? `迁移项目到 ${target}` : `${alias} · 项目管理`, picking);
  let projects = [], selected = new Set(), busy = false, loaded = false;
  let confirmResolve = null;
  const status = dialog.querySelector('[role="status"]');
  const search = dialog.querySelector('input[type="search"]');
  const scope = dialog.querySelector("select");
  const all = dialog.querySelector("[data-select-all]");
  const submit = dialog.querySelector("[data-submit]");
  const confirmation = dialog.querySelector(".project-window-confirm");
  scope.hidden = picking;
  submit.hidden = !picking;
  dialog.querySelector("[data-actions]").hidden = picking;
  if (!picking) {
    dialog.querySelector("footer").insertAdjacentHTML("beforeend", `<div class="actions" data-selection-actions hidden>
      ${button("archive-selected", "folder", "归档选中项目")}${button("trash-selected", "trash", "选中项目移入回收站")}</div>`);
  }

  function visible() {
    const query = search.value.trim().toLowerCase();
    return projects.filter(p => (scope.value === "trashed" ? p.trashed
      : !p.trashed && Boolean(p.archived) === (scope.value === "archived"))
      && `${p.name} ${p.owner || ""}`.toLowerCase().includes(query));
  }

  function render() {
    const items = visible();
    all.checked = items.length > 0 && items.every(p => selected.has(p.id));
    all.indeterminate = !all.checked && items.some(p => selected.has(p.id));
    all.disabled = busy || !items.length;
    submit.disabled = busy || !loaded;
    submit.textContent = selected.size ? "无感换号并迁移" : "无感换号";
    dialog.querySelector("tbody").innerHTML = items.length ? items.map(p => `<tr data-project-id="${escapeAttr(p.id)}" class="${selected.has(p.id) ? "selected" : ""}">
      <td><input class="ui-checkbox" type="checkbox" ${selected.has(p.id) ? "checked" : ""} ${busy ? "disabled" : ""} aria-label="选择 ${escapeAttr(p.name)}"></td>
      <td class="project-name" title="${escapeAttr(p.name)}">${escapeHtml(p.name)}</td>
      <td>${escapeHtml(p.is_owner ? "我" : p.owner || "协作者")}</td><td class="project-date">${escapeHtml(projectDate(p.last_updated))}</td>
      ${picking ? "" : `<td><div class="actions">${p.trashed
        ? button("restore", "refresh", "恢复项目") + (p.is_owner ? button("delete", "trash", "永久删除") : "")
        : button("copy", "copy", "复制项目") + button("zip", "download", "下载 ZIP") + button("pdf", "play", "编译并下载 PDF")
          + (p.is_owner ? button("rename", "edit", "重命名") : "")
          + button(p.archived ? "unarchive" : "archive", "folder", p.archived ? "取消归档" : "归档")
          + button("trash", "trash", "移入回收站")}</div></td>`}</tr>`).join("")
      : `<tr><td colspan="${picking ? 4 : 5}" class="empty">${loaded ? "没有匹配的项目" : "正在读取项目..."}</td></tr>`;
    dialog.querySelectorAll("tbody button").forEach(b => { b.disabled = busy; });
    const selectionActions = dialog.querySelector("[data-selection-actions]");
    if (selectionActions) {
      selectionActions.hidden = !selected.size || scope.value === "trashed";
      selectionActions.querySelectorAll("button").forEach(b => { b.disabled = busy; });
      selectionActions.querySelector('[data-project-action="archive-selected"]').hidden = scope.value !== "active";
    }
    if (loaded && !busy) status.textContent = `${items.length} 个项目 · 已选 ${selected.size} 个`;
  }

  async function load(initial = false) {
    if (busy) return;
    busy = true; render(); status.textContent = "正在读取项目...";
    try {
      let report;
      try { report = await call(alias, "list"); }
      catch (error) {
        if (!alias || ![401, 403].includes(error.status)) throw error;
        status.textContent = "正在恢复账号登录状态...";
        await postTaskResult(endpoints.refreshCredentials, {
          aliases: alias, passwords: "", task_id: makeTaskId("refresh-session", alias),
        });
        report = await call(alias, "list");
      }
      if (!dialog.open) return;
      alias = report.alias;
      if (picking && alias === target) throw new Error("源账号和目标账号相同");
      projects = report.projects;
      projects.sort((a,b) => String(b.last_updated || "").localeCompare(String(a.last_updated || "")));
      loaded = true;
      const available = visible();
      selected = new Set(initial && picking ? available.map(p => p.id)
        : [...selected].filter(id => available.some(p => p.id === id)));
      busy = false; render();
    } catch (error) { busy = false; render(); failure(error, status); }
    finally { busy = false; }
  }

  function confirm(action, project) {
    const naming = action === "copy" || action === "rename";
    const label = {copy:"复制项目", rename:"重命名", archive:"归档项目", trash:"移入回收站", delete:"永久删除项目"}[action];
    confirmation.hidden = false;
    confirmation.innerHTML = `<strong>${label}：${escapeHtml(project.name)}</strong>
      ${naming ? `<input aria-label="项目名称" value="${escapeAttr(project.name + (action === "copy" ? " (copy)" : ""))}">` : ""}
      <div class="actions"><button type="button" class="ui-btn" data-confirm>确认</button><button type="button" class="ui-btn" data-cancel>取消</button></div>`;
    return new Promise(resolve => {
      confirmResolve = resolve;
      const done = value => { confirmation.hidden = true; confirmResolve = null; resolve(value); };
      confirmation.querySelector("[data-cancel]").onclick = () => done(null);
      confirmation.querySelector("[data-confirm]").onclick = () => {
        const name = confirmation.querySelector("input")?.value.trim();
        if (naming && !name) return;
        done({name, confirm_name:project.name});
      };
      (confirmation.querySelector("input") || confirmation.querySelector("[data-cancel]")).focus();
    });
  }

  dialog.addEventListener("click", async event => {
    const action = event.target.closest("[data-project-action]")?.dataset.projectAction;
    if (action === "refresh") { await load(); return; }
    const row = event.target.closest("tr[data-project-id]");
    const bulk = action?.endsWith("-selected");
    if ((!row && !bulk) || busy) return;
    const targets = bulk ? projects.filter(p => selected.has(p.id)) : projects.filter(p => p.id === row.dataset.projectId);
    if (!targets.length) return;
    const project = bulk ? {name: `${targets.length} 个项目`} : targets[0];
    const operation = bulk ? action.replace("-selected", "") : action;
    if (!action) {
      selected.has(project.id) ? selected.delete(project.id) : selected.add(project.id);
      render(); return;
    }
    busy = true; render();
    try {
      let fields = {};
      if (["copy", "rename", "archive", "trash", "delete"].includes(operation)) {
        fields = await confirm(operation, project);
        if (!fields || !dialog.open) return;
      }
      for (const item of targets) {
        if (!dialog.open) break;
        status.textContent = operation === "pdf" ? "正在编译 PDF..." : `正在处理：${item.name}`;
        const report = await call(alias, operation, {project_id:item.id, ...fields});
        if (report.data && dialog.open) await saveDownload(report);
        selected.delete(item.id);
      }
      busy = false;
      if (dialog.open) await load();
    } catch (error) {
      busy = false;
      if (dialog.open) { await load(); failure(error, status); }
    } finally {
      if (busy) { busy = false; if (dialog.open) render(); }
    }
  });
  all.onchange = () => {
    visible().forEach(p => all.checked ? selected.add(p.id) : selected.delete(p.id));
    render();
  };
  search.oninput = render;
  scope.onchange = () => { selected.clear(); render(); };
  submit.onclick = () => {
    if (busy || !loaded) return;
    onSelect({source_alias:alias, project_ids:[...selected]});
    dialog.close();
  };
  dialog.addEventListener("close", () => {
    confirmResolve?.(null);
    if (picking) { onSelect(null); migrationDialog = null; }
    else windows.delete(alias);
    dialog.remove();
  }, {once:true});
  load(true);
  return dialog;
}

export function openProjectManagers(aliases) {
  for (const alias of aliases) {
    const existing = windows.get(alias);
    if (existing) { existing.style.zIndex = ++layer; existing.focus(); }
    else windows.set(alias, projectWorkspace(alias));
  }
}

export function selectMigrationProjects(target) {
  if (migrationDialog) {
    showToast({tone:"warning", title:"请先完成当前项目选择"});
    return Promise.resolve(null);
  }
  return new Promise(resolve => {
    const source = state.accounts.find(account => account.is_current)?.alias;
    migrationDialog = projectWorkspace(source || null, {target, onSelect:resolve});
  });
}
