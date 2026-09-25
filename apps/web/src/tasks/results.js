import { escapeHtml, pill, statusText } from "../format.js";
import { taskResultSectionKinds, taskResultSensitiveKeyRules, taskResultSensitiveValueMarkers, taskResultSummarySourceKinds } from "../capabilities.js";

export function taskResultDetails(task) {
  const result = task && task.result;
  if (!isPlainObject(result)) {
    return "";
  }

  const resultSections = taskResultSectionKinds();
  const summary = resultSections.includes("summary_chips") ? taskResultSummary(result) : "";
  const sections = taskResultSections(result);
  if (!summary && !sections.length) {
    return "";
  }

  return `
    <div class="task-result">
      <div class="task-result-title">结果</div>
      ${summary ? `<div class="task-result-summary">${summary}</div>` : ""}
      ${sections.join("")}
    </div>
  `;
}

export function taskResultSummary(result) {
  const chips = [];
  const sources = new Set(taskResultSummarySourceKinds());
  const collectCounts = (object, prefix = "") => {
    if (!isPlainObject(object)) return;
    Object.entries(object).forEach(([key, value]) => {
      if (typeof value === "number" && key.endsWith("_count")) {
        chips.push(`${prefix}${formatTaskKey(key)}: ${value}`);
      }
    });
  };

  if (typeof result.status === "string") {
    chips.push(`status: ${result.status}`);
  }
  if (sources.has("root")) {
    collectCounts(result);
  }
  ["import", "session_refresh", "git_token_generation", "project_migration"].forEach((key) => {
    if (sources.has(key) && isPlainObject(result[key])) {
      collectCounts(result[key], `${formatTaskKey(key)} `);
    }
  });
  if (
    taskResultSectionKinds().includes("post_action_errors") &&
    Array.isArray(result.post_action_errors) &&
    result.post_action_errors.length > 0
  ) {
    chips.push(`post action errors: ${result.post_action_errors.length}`);
  }

  return chips
    .slice(0, 12)
    .map((chip) => `<span class="task-result-chip">${escapeHtml(chip)}</span>`)
    .join("");
}

export function taskResultSections(result) {
  const sections = [];
  const renderers = new Set(taskResultSectionKinds());
  const decisions = renderers.has("import_decisions") ? taskImportDecisions(result) : [];
  if (decisions.length) {
    sections.push(renderTaskImportDecisions(decisions));
  }
  const migration = renderers.has("project_migration") ? taskProjectMigrationResult(result) : null;
  if (migration) {
    sections.push(renderTaskProjectMigrationResult(migration));
  }
  if (renderers.has("top_level_items") && Array.isArray(result.items) && result.items.length) {
    sections.push(renderTaskResultItems("Items", result.items));
  }
  if (renderers.has("nested_items")) {
    Object.entries(result).forEach(([key, value]) => {
      if (!isPlainObject(value) || !Array.isArray(value.items) || !value.items.length) return;
      if (migration && value === migration) return;
      sections.push(renderTaskResultItems(formatTaskKey(key), value.items));
    });
  }
  if (
    renderers.has("post_action_errors") &&
    Array.isArray(result.post_action_errors) &&
    result.post_action_errors.length
  ) {
    sections.push(renderTaskResultErrors(result.post_action_errors));
  }
  return sections;
}

export function taskImportDecisions(result) {
  if (Array.isArray(result.decisions)) {
    return result.decisions;
  }
  if (isPlainObject(result.import) && Array.isArray(result.import.decisions)) {
    return result.import.decisions;
  }
  return [];
}

export function taskProjectMigrationResult(result) {
  if (isProjectMigrationReport(result)) {
    return result;
  }
  if (isProjectMigrationReport(result.project_migration)) {
    return result.project_migration;
  }
  return null;
}

export function isProjectMigrationReport(value) {
  return (
    isPlainObject(value) &&
    Array.isArray(value.items) &&
    typeof value.source_alias === "string" &&
    typeof value.target_alias === "string" &&
    Object.prototype.hasOwnProperty.call(value, "audit_missing_count")
  );
}

export function renderTaskImportDecisions(decisions) {
  return `
    <div class="task-result-table">
      <div class="muted">导入决策</div>
      <table class="compact">
        <thead>
          <tr>
            <th>状态</th>
            <th>请求别名</th>
            <th>最终别名</th>
            <th>邮箱</th>
            <th>已有别名</th>
          </tr>
        </thead>
        <tbody>
          ${decisions
            .map(
              (decision) => `
            <tr>
              <td>${pill(safeTaskValue(decision.status || "unknown", "status"))}</td>
              <td>${escapeHtml(safeTaskValue(decision.requested_alias || "-", "alias"))}</td>
              <td>${escapeHtml(safeTaskValue(decision.resolved_alias || "-", "alias"))}</td>
              <td>${escapeHtml(safeTaskValue(decision.email || "-", "email"))}</td>
              <td>${escapeHtml(safeTaskValue(decision.existing_alias || "-", "alias"))}</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    </div>
  `;
}

export function renderTaskProjectMigrationResult(report) {
  const items = Array.isArray(report.items) ? report.items : [];
  const missing = Array.isArray(report.missing_projects) ? report.missing_projects : [];
  return `
    <div class="task-result-table">
      <div class="muted">项目迁移 ${escapeHtml(report.source_alias || "-")} -> ${escapeHtml(report.target_alias || "-")}</div>
      <table class="compact">
        <thead>
          <tr>
            <th>状态</th>
            <th>项目</th>
            <th>策略</th>
            <th>已加入</th>
            <th>已克隆</th>
            <th>警告</th>
            <th>错误</th>
          </tr>
        </thead>
        <tbody>
          ${
            items.length
              ? items
                  .map(
                    (item) => `
            <tr>
              <td>${pill(safeTaskValue(item.status || "unknown", "status"))}</td>
              <td><strong>${escapeHtml(safeTaskValue(item.project_name || "-", "project_name"))}</strong><br><span class="muted">${escapeHtml(safeTaskValue(item.project_id || "-", "project_id"))}</span></td>
              <td>${escapeHtml(formatTaskKey(safeTaskValue(item.strategy || "unknown", "strategy")))}</td>
              <td>${escapeHtml(safeTaskValue(item.joined_project_id || "-", "project_id"))}</td>
              <td>${escapeHtml(safeTaskValue(item.cloned_project_id || "-", "project_id"))}</td>
              <td>${escapeHtml(taskWarningsText(item.warnings))}</td>
              <td>${escapeHtml(safeTaskValue(item.error_message || "-", "error"))}</td>
            </tr>
          `,
                  )
                  .join("")
              : `
            <tr>
              <td colspan="7" class="muted">没有返回项目明细。</td>
            </tr>
          `
          }
        </tbody>
      </table>
      ${
        missing.length
          ? `
        <div class="task-recovery-hint">核对后缺失：${missing.map((item) => escapeHtml(item.project_name || item.project_id || "-")).join(", ")}</div>
      `
          : ""
      }
    </div>
  `;
}

export function taskWarningsText(warnings) {
  if (!Array.isArray(warnings) || !warnings.length) {
    return "-";
  }
  return warnings.map((warning) => safeTaskValue(warning, "warning")).join("; ");
}

export function renderTaskResultItems(title, items) {
  return `
    <div class="task-result-table">
      <div class="muted">${escapeHtml(title)}</div>
      <table class="compact">
        <thead>
          <tr>
            <th>别名</th>
            <th>状态</th>
            <th>详情</th>
          </tr>
        </thead>
        <tbody>
          ${items
            .map(
              (item) => `
            <tr>
              <td>${escapeHtml(taskItemAlias(item))}</td>
              <td>${pill(taskItemStatus(item))}</td>
              <td>${escapeHtml(taskItemDetails(item))}</td>
            </tr>
          `,
            )
            .join("")}
        </tbody>
      </table>
    </div>
  `;
}

export function renderTaskResultErrors(errors) {
  return `
    <div class="task-result-table">
      <div class="muted">后续操作错误</div>
      <ol class="task-log">
        ${errors.map((error) => `<li>${escapeHtml(taskErrorDetails(error))}</li>`).join("")}
      </ol>
    </div>
  `;
}

export function taskItemAlias(item) {
  if (!isPlainObject(item)) return "-";
  return safeTaskValue(
    item.alias || item.resolved_alias || item.target_alias || item.source_alias || "-",
    "alias",
  );
}

export function taskItemStatus(item) {
  if (!isPlainObject(item)) return "unknown";
  if (item.error) return "failed";
  if (isPlainObject(item.report) && typeof item.report.eligibility === "string") {
    return safeTaskValue(item.report.eligibility, "eligibility");
  }
  if (item.status) return safeTaskValue(item.status, "status");
  if (item.updated || item.changed || item.refreshed || item.opened || item.removed || item.executed)
    return "completed";
  return "unknown";
}

export function taskItemDetails(item) {
  if (!isPlainObject(item)) {
    return safeTaskValue(item, "");
  }
  if (item.error) {
    return taskErrorDetails(item.error);
  }
  if (isPlainObject(item.report)) {
    if (typeof item.report.eligibility === "string") {
      const details = [
        `试用资格：${statusText(item.report.eligibility)}`,
        item.report.subscription_label
          ? `订阅：${safeTaskValue(item.report.subscription_label, "subscription_label")}`
          : "",
      ].filter(Boolean);
      return details.join("，");
    }
    const report = taskObjectSummary(item.report);
    if (report) return report;
  }
  return (
    taskObjectSummary(item, [
      "alias",
      "resolved_alias",
      "target_alias",
      "source_alias",
      "status",
      "error",
      "report",
    ]) || "-"
  );
}

export function taskErrorDetails(error) {
  if (isPlainObject(error)) {
    return [error.kind && safeTaskValue(error.kind, "kind"), error.message && safeTaskValue(error.message, "message")]
      .filter(Boolean).join(": ") || taskObjectSummary(error) || "未知错误";
  }
  return safeTaskValue(error, "error");
}

export function taskObjectSummary(object, excludeKeys = []) {
  if (!isPlainObject(object)) return "";
  const excluded = new Set(excludeKeys);
  return Object.entries(object)
    .filter(([key]) => !excluded.has(key))
    .slice(0, 6)
    .map(([key, value]) => `${formatTaskKey(key)}: ${safeTaskValue(value, key)}`)
    .join(", ");
}

export function safeTaskValue(value, key = "") {
  if (isSensitiveTaskKey(key)) {
    return "[hidden]";
  }
  if (typeof value === "string" && isSensitiveTaskValue(value)) {
    return "[hidden]";
  }
  if (value === null || value === undefined || value === "") {
    return "-";
  }
  if (typeof value === "boolean") {
    return value ? "yes" : "no";
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (typeof value === "string") {
    return value.length > 140 ? `${value.slice(0, 137)}...` : value;
  }
  if (Array.isArray(value)) {
    return `${value.length} 项`;
  }
  if (isPlainObject(value)) {
    return `{${Object.keys(value).length} 个字段}`;
  }
  return String(value);
}

export function isSensitiveTaskKey(key) {
  const normalized = String(key || "").toLowerCase();
  return taskResultSensitiveKeyRules().some((rule) => {
    if (rule.matcher === "equals") {
      return normalized === rule.value;
    }
    if (rule.matcher === "ends_with") {
      return normalized.endsWith(rule.value);
    }
    if (rule.matcher === "contains") {
      return normalized.includes(rule.value);
    }
    return false;
  });
}

export function isSensitiveTaskValue(value) {
  const normalized = String(value || "")
    .trim()
    .toLowerCase();
  if (!normalized) return false;
  return taskResultSensitiveValueMarkers().some((marker) => {
    if (marker === "git_token_prefix") {
      return normalized.startsWith("olp_");
    }
    if (marker === "overleaf_session_cookie_assignment") {
      return normalized.includes("overleaf_session2=");
    }
    if (marker === "encoded_overleaf_session_prefix") {
      return normalized.startsWith("s%3a");
    }
    if (marker === "raw_overleaf_session_prefix") {
      return normalized.startsWith("s:");
    }
    return false;
  });
}

export function formatTaskKey(key) {
  return String(key || "").replaceAll("_", " ");
}

export function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
