import {
  recordClientRuntimeError,
  recordClientRuntimeInfo,
  reportUiOperationFailure,
  reportUiOperationStart,
  reportUiOperationSuccess,
  showConfirmDialog,
  showToast,
} from "./feedback.js";
import { ADDRESS_HISTORY_DISPLAY_LIMIT, endpoints, state } from "./state.js";
import { downloadTextFile, escapeAttr, escapeHtml, pill, statusText, writeClipboard } from "./format.js";
import { resolveTauriDialogSave, resolveTauriWriteTextFile, selectedDialogPath } from "./bridge.js";
import {
  addressActionButtons,
  addressCollectionActions,
  addressSelectionActions,
  cardActionButtons,
  cardCollectionActions,
  cardMutationActions,
  syncCardSelectionControls,
} from "./capabilities.js";
import { iconSvg } from "./icons.js";
import { fetchJson, fetchOptionalJson, postText } from "./http.js";
import { postJson, refresh } from "./sync.js";


export function renderCards(cards) {
  const normalizedCards = Array.isArray(cards) ? cards : [];
  state.cards = normalizedCards;
  const availableIds = new Set(normalizedCards.map((card) => card.id));
  state.selectedCardIds.forEach((id) => { if (!availableIds.has(id)) state.selectedCardIds.delete(id); });
  syncCardSelectionControls();
  syncRegistrationCardBinOptions(normalizedCards);
  const groups = groupCardsByBin(normalizedCards);
  const validGroupKeys = new Set(groups.map((group) => group.key));
  for (const key of Array.from(state.expandedCardBins)) {
    if (!validGroupKeys.has(key)) state.expandedCardBins.delete(key);
  }

  document.getElementById("card-count").textContent = `${normalizedCards.length} 张卡`;
  const body = document.getElementById("cards-body");
  if (!normalizedCards.length) {
    body.innerHTML = `<tr><td colspan="4"><div class="resource-empty-state">没有卡片。添加后会显示脱敏卡号、有效期和使用状态。</div></td></tr>`;
    return;
  }

  body.innerHTML = groups
    .map((group) => {
      const expanded = state.expandedCardBins.has(group.key);
      const summaryCard = group.cards.find((card) => card && card.masked_number) || group.cards[0];
      const summary = summaryCard && summaryCard.masked_number ? summaryCard.masked_number : "卡号摘要不可用";
      const status = summaryCard ? pill(summaryCard.status || "unknown") : "";
      const detailRows = expanded
        ? group.cards
            .map(
              (card) => `
            <tr class="resource-table-row card-bin-detail-row${state.selectedCardIds.has(card.id) ? " selected" : ""}" data-card-id="${escapeAttr(card.id)}">
              <td><label class="check-row"><input class="ui-checkbox" type="checkbox" aria-label="选择 ${escapeAttr(card.masked_number)}" ${state.selectedCardIds.has(card.id) ? "checked" : ""}><strong>${escapeHtml(card.masked_number || "-")}</strong></label></td>
              <td>${escapeHtml(card.exp_month || "--")}/${escapeHtml(card.exp_year || "----")}</td>
              <td>${pill(card.status || "unknown")}${card.last_error ? `<br><span class="muted">${escapeHtml(card.last_error)}</span>` : ""}</td>
              <td><div class="actions resource-row-actions">${cardActionButtons(card)}</div></td>
            </tr>
          `,
            )
            .join("")
        : "";
      return `
        <tr class="card-bin-group-row">
          <td colspan="4">
            <button
              class="card-bin-toggle"
              type="button"
              data-card-bin-toggle="${escapeAttr(group.key)}"
              aria-expanded="${expanded}"
              aria-label="${escapeHtml(`${expanded ? "收起" : "展开"}${cardBinLabel(group.key)}卡片`)}"
            >
              <span class="card-bin-toggle-icon">${iconSvg(expanded ? "chevron-down" : "chevron-right")}</span>
              <span class="card-bin-toggle-main">
                <strong>${escapeHtml(cardBinLabel(group.key))}</strong>
                <span>${group.cards.length} 张</span>
              </span>
              <span class="card-bin-toggle-summary">${escapeHtml(summary)}</span>
              <span class="card-bin-toggle-status">${status}</span>
            </button>
          </td>
        </tr>
        ${detailRows}
      `;
    })
    .join("");
}

export function groupCardsByBin(cards) {
  const groups = new Map();
  (Array.isArray(cards) ? cards : []).forEach((card, index) => {
    const rawBin = String((card && card.bin) || "").trim();
    const bin = /^\d{6}$/.test(rawBin)
      ? rawBin
      : `unknown-${card && card.index != null ? card.index : index}`;
    if (!groups.has(bin)) groups.set(bin, { key: bin, cards: [] });
    groups.get(bin).cards.push(card);
  });
  return Array.from(groups.values());
}

export function selectCardRow(event) {
  if (event.target.closest("button, a")) return;
  const row = event.target.closest("[data-card-id]");
  if (!row) return;
  if (event.target.closest("label") && !event.target.matches("input")) return;
  const id = row.dataset.cardId;
  const selected = event.target.matches("input") ? event.target.checked : !state.selectedCardIds.has(id);
  if (selected) state.selectedCardIds.add(id);
  else state.selectedCardIds.delete(id);
  row.classList.toggle("selected", selected);
  row.querySelector("input").checked = selected;
  syncCardSelectionControls();
}

export async function exportSelectedCards() {
  const ids = Array.from(state.selectedCardIds);
  if (!ids.length) return;
  const button = document.getElementById("card-export");
  button.disabled = true;
  try {
    const save = resolveTauriDialogSave();
    const write = resolveTauriWriteTextFile();
    let path = "";
    if (save && write) {
      path = selectedDialogPath(await save({ filters: [{ name: "Text", extensions: ["txt"] }] }));
      if (!path) return;
    }
    const text = await postText(endpoints.exportCards, { ids });
    if (path) await write(path, text);
    else downloadTextFile("cards.txt", text, "text/plain;charset=utf-8");
    reportUiOperationSuccess({ status: null, shortMessage: "", title: "银行卡已导出", scope: "银行卡导出",
      route: endpoints.exportCards, message: `已导出 ${ids.length} 张银行卡。` });
  } catch (error) {
    reportUiOperationFailure({ status: null, shortMessage: "", title: "银行卡导出失败", scope: "银行卡导出", route: endpoints.exportCards, error });
  } finally {
    syncCardSelectionControls();
  }
}

export async function deleteSelectedCards() {
  const ids = Array.from(state.selectedCardIds);
  if (!ids.length || state.deletingCards || !cardMutationActions().includes("remove")) return;
  state.deletingCards = true;
  renderCards(state.cards);
  let removed = 0;
  const status = document.getElementById("card-action-status");
  try {
    if (!await showConfirmDialog({
      title: "删除选中的银行卡",
      message: `将从本地卡片库删除 ${ids.length} 张银行卡及其保存的卡号和安全码。此操作不可撤销。`,
      confirmText: "删除",
    })) return;
    reportUiOperationStart({ status, shortMessage: "正在删除", scope: "银行卡批量删除",
      route: endpoints.removeCard, message: `正在删除 ${ids.length} 张银行卡。` });
    for (const id of ids) {
      await postJson(endpoints.removeCard, { number_or_suffix: id });
      state.selectedCardIds.delete(id);
      state.cards = state.cards.filter((card) => card.id !== id);
      removed += 1;
    }
    reportUiOperationSuccess({ status, shortMessage: `已删除 ${removed} 张`, title: "银行卡已删除",
      scope: "银行卡批量删除", route: endpoints.removeCard, message: `已删除 ${removed} 张银行卡。` });
  } catch (error) {
    reportUiOperationFailure({ status, shortMessage: `已删除 ${removed} 张，剩余 ${ids.length - removed} 张未删除`,
      title: "银行卡删除未完成", scope: "银行卡批量删除", route: endpoints.removeCard, error });
  } finally {
    state.deletingCards = false;
    renderCards(state.cards);
    if (removed) await refresh({ detectBrowser: false });
  }
}

export function cardBinLabel(key) {
  return String(key || "").startsWith("unknown-") ? "BIN 未知" : `BIN ${key}`;
}

export function syncRegistrationCardBinOptions(cards = state.cards) {
  const select = document.getElementById("registration-card-bin");
  if (!select) return;
  const current = String(select.value || "").trim();
  const bins = Array.from(
    new Set(
      (Array.isArray(cards) ? cards : [])
        .map((card) => String((card && card.bin) || "").trim())
        .filter((bin) => /^\d{6}$/.test(bin)),
    ),
  ).sort();
  select.innerHTML = [
    '<option value="">全部 BIN</option>',
    ...bins.map((bin) => `<option value="${bin}">${bin}</option>`),
  ].join("");
  select.value = bins.includes(current) ? current : "";
}

export function toggleCardBin(key) {
  const normalized = String(key || "").trim();
  if (!normalized) return;
  if (state.expandedCardBins.has(normalized)) {
    state.expandedCardBins.delete(normalized);
  } else {
    state.expandedCardBins.add(normalized);
  }
  renderCards(state.cards);
}

export function renderAddresses(addresses) {
  renderAddressSourceTag(state.selectedAddress);
  const list = document.getElementById("addresses-list");
  if (!addresses.length) {
    list.hidden = true;
    list.innerHTML = "";
    updateAddressDisplay(state.selectedAddress);
    return;
  }

  const currentLine1 = state.selectedAddress && state.selectedAddress.line1;
  const visibleAddresses = addresses
    .filter((address) => !currentLine1 || address.line1 !== currentLine1)
    .slice(-ADDRESS_HISTORY_DISPLAY_LIMIT);
  list.hidden = visibleAddresses.length === 0;
  list.innerHTML = visibleAddresses
    .map(
      (address) => `
      <div class="resource-address-item">
        <div class="resource-address-main">
          <strong>${escapeHtml(address.name || `地址 ${address.index + 1}`)}</strong>
          <span>${escapeHtml(address.line1)}, ${escapeHtml(address.city)}, ${escapeHtml(address.state)} ${escapeHtml(address.zip)}</span>
        </div>
        <div class="resource-address-state">${address.complete ? pill("present") : pill("missing")}</div>
        <div class="actions resource-address-actions">${addressActionButtons(address)}</div>
      </div>
    `,
    )
    .join("");
  updateAddressDisplay(state.selectedAddress);
}

export function stopAddressPrefetchPolling() {
  if (state.addressPrefetchTimer) {
    window.clearTimeout(state.addressPrefetchTimer);
    state.addressPrefetchTimer = null;
  }
}

export function syncCurrentAddressDisplay(selection) {
  const status = document.getElementById("address-action-status");
  if (selection) {
    stopAddressPrefetchPolling();
    state.selectedAddress = addressSummaryFromSelection(selection);
    updateAddressDisplay(state.selectedAddress);
    if (status) status.textContent = "";
    return;
  }

  state.selectedAddress = null;
  updateAddressDisplay(null);
  if (status) status.textContent = "";
  scheduleAddressPrefetchPoll(0);
}

export function scheduleAddressPrefetchPoll(attempt) {
  stopAddressPrefetchPolling();
  if (attempt >= 20) {
    const status = document.getElementById("address-action-status");
    if (status) status.textContent = "注册地址尚未准备，可点击获取地址";
    return;
  }

  state.addressPrefetchTimer = window.setTimeout(
    async () => {
      state.addressPrefetchTimer = null;
      try {
        const selection = await fetchOptionalJson(endpoints.currentAddress);
        if (selection) {
          syncCurrentAddressDisplay(selection);
          const addresses = await fetchJson(endpoints.addresses);
          renderAddresses(addresses);
          return;
        }
      } catch (_) {
        // 启动预取仍在后台进行，保持有界轮询即可。
      }
      scheduleAddressPrefetchPoll(attempt + 1);
    },
    attempt === 0 ? 750 : 1000,
  );
}

export async function addCard(event) {
  event.preventDefault();
  const lineInput = document.getElementById("card-line");
  const status = document.getElementById("card-action-status");
  const submit = document.getElementById("card-submit");
  const lines = lineInput.value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (!lines.length || submit.disabled) return;
  let added = 0;
  let duplicate = 0;
  let processed = 0;
  if (!cardCollectionActions().includes("add")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "添加银行卡不可用",
      scope: "添加银行卡",
      route: endpoints.addCard,
      error: new Error("当前后端不支持添加银行卡"),
    });
    return;
  }

  submit.disabled = true;
  lineInput.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在添加",
    scope: "添加银行卡",
    route: endpoints.addCard,
    message: `正在校验并保存 ${lines.length} 张银行卡。`,
  });

  try {
    for (const line of lines) {
      const report = await postJson(endpoints.addCard, { line });
      if (report.status === "duplicate") duplicate += 1;
      else added += 1;
      processed += 1;
    }
    reportUiOperationSuccess({
      status,
      shortMessage: added ? "卡片已添加" : "卡片已存在",
      title: added ? "银行卡已添加" : "银行卡未重复添加",
      scope: "添加银行卡",
      route: endpoints.addCard,
      message: `已添加 ${added} 张，跳过重复 ${duplicate} 张。`,
      tone: added ? "success" : "warning",
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "卡片添加未完成，请查看运行日志",
      title: "卡片添加未完成",
      scope: "添加银行卡",
      route: endpoints.addCard,
      error: new Error(`已添加 ${added} 张，跳过重复 ${duplicate} 张，剩余 ${lines.length - processed} 张未添加：${error.message}`),
    });
  } finally {
    lineInput.value = lines.slice(processed).join("\n");
    lineInput.disabled = false;
    submit.disabled = false;
    if (processed) await refresh({ detectBrowser: false });
  }
}

export async function handleCardAction(button) {
  const action = button.dataset.cardAction;
  const key = button.dataset.cardKey;
  if (!action || !key || button.disabled || state.deletingCards) return;

  const originalText = button.textContent;
  const status = document.getElementById("card-action-status");
  if (!cardMutationActions().includes(action)) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "银行卡操作不可用",
      scope: "银行卡状态操作",
      route: action === "remove" ? endpoints.removeCard : endpoints.cardStatus,
      error: new Error("当前后端不支持该卡片操作"),
    });
    return;
  }
  button.disabled = true;
  button.textContent = "处理中";
  reportUiOperationStart({
    status,
    shortMessage: "处理中",
    scope: `银行卡${cardActionText(action)}`,
    route: action === "remove" ? endpoints.removeCard : endpoints.cardStatus,
    message: "已提交银行卡状态操作。",
  });

  try {
    let shortMessage = "操作完成";
    let detail = "银行卡状态已更新。";
    if (action === "remove") {
      await postJson(endpoints.removeCard, { number_or_suffix: key });
      shortMessage = "卡片已移除";
      detail = "银行卡已从本地卡片库移除。";
    } else {
      const payload = { number_or_suffix: key, status: action };
      if (action === "failed") {
        payload.error = "manual mark failed";
      }
      await postJson(endpoints.cardStatus, payload);
      shortMessage = `已标记为${cardActionText(action)}`;
      detail = `银行卡状态已标记为${cardActionText(action)}。`;
    }
    reportUiOperationSuccess({
      status,
      shortMessage,
      title: "银行卡操作完成",
      scope: `银行卡${cardActionText(action)}`,
      route: action === "remove" ? endpoints.removeCard : endpoints.cardStatus,
      message: detail,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "卡片操作失败，请查看运行日志",
      title: "卡片操作失败",
      scope: `银行卡${cardActionText(action)}`,
      route: action === "remove" ? endpoints.removeCard : endpoints.cardStatus,
      error,
    });
  } finally {
    button.disabled = false;
    button.textContent = originalText;
  }
}

export async function fetchAddress(event) {
  event.preventDefault();
  const status = document.getElementById("address-action-status");
  const submit = document.getElementById("address-submit");
  if (!addressCollectionActions().includes("fetch")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "获取地址不可用",
      scope: "获取注册地址",
      route: endpoints.fetchAddress,
      error: new Error("当前后端不支持获取地址"),
    });
    return;
  }

  submit.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在获取",
    scope: "获取注册地址",
    route: endpoints.fetchAddress,
    message: "正在读取 API 地址，失败时自动回退备用地址。",
  });

  try {
    const report = await postJson(endpoints.fetchAddress, {});
    stopAddressPrefetchPolling();
    state.selectedAddress = addressSummaryFromSelection(report);
    updateAddressDisplay(state.selectedAddress);
    reportUiOperationSuccess({
      status,
      shortMessage: report.source === "fallback" ? "已使用备用地址" : "地址已获取",
      title: report.source === "fallback" ? "已回退备用地址" : "API 地址已获取",
      scope: "获取注册地址",
      route: endpoints.fetchAddress,
      message: `${report.source === "fallback" ? "API 获取失败，使用备用地址" : "API 地址已准备"}; ${selectedAddressText(report)}`,
    });
    await refresh();
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "地址获取失败，请查看运行日志",
      title: "地址获取失败",
      scope: "获取注册地址",
      route: endpoints.fetchAddress,
      error,
    });
  } finally {
    submit.disabled = false;
  }
}

export async function handleAddressAction(button) {
  const index = Number(button.dataset.addressIndex);
  if (!Number.isInteger(index) || button.disabled) return;

  const originalText = button.textContent;
  const status = document.getElementById("address-action-status");
  if (!addressSelectionActions().includes("select")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "地址选择不可用",
      scope: "选择注册地址",
      route: endpoints.addressSelect,
      error: new Error("当前后端不支持选择地址"),
    });
    return;
  }
  button.disabled = true;
  button.textContent = "选择中";
  reportUiOperationStart({
    status,
    shortMessage: "正在选择",
    scope: "选择注册地址",
    route: endpoints.addressSelect,
    message: "正在切换当前注册地址。",
  });

  try {
    const selection = await postJson(endpoints.addressSelect, { index });
    stopAddressPrefetchPolling();
    state.selectedAddress = { ...addressSummaryFromSelection(selection), source: "fallback" };
    updateAddressDisplay(state.selectedAddress);
    reportUiOperationSuccess({
      status,
      shortMessage: "地址已选择",
      title: "注册地址已选择",
      scope: "选择注册地址",
      route: endpoints.addressSelect,
      message: selectedAddressText(selection),
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "地址选择失败，请查看运行日志",
      title: "地址选择失败",
      scope: "选择注册地址",
      route: endpoints.addressSelect,
      error,
    });
  } finally {
    button.disabled = false;
    button.textContent = originalText;
  }
}

export async function selectFallbackAddress() {
  const button = document.getElementById("address-fallback");
  const status = document.getElementById("address-action-status");
  if (!addressSelectionActions().includes("fallback")) {
    reportUiOperationFailure({
      status,
      shortMessage: "操作不可用",
      title: "备用地址不可用",
      scope: "选择备用地址",
      route: endpoints.addressFallback,
      error: new Error("当前后端不支持备用地址选择"),
    });
    return;
  }
  button.disabled = true;
  reportUiOperationStart({
    status,
    shortMessage: "正在选择",
    scope: "选择备用地址",
    route: endpoints.addressFallback,
    message: "正在选择本地备用注册地址。",
  });

  try {
    const selection = await postJson(endpoints.addressFallback, {
      selector: Math.floor(Date.now() / 1000),
    });
    stopAddressPrefetchPolling();
    state.selectedAddress = { ...addressSummaryFromSelection(selection), source: "fallback" };
    updateAddressDisplay(state.selectedAddress);
    reportUiOperationSuccess({
      status,
      shortMessage: "备用地址已选择",
      title: "备用地址已选择",
      scope: "选择备用地址",
      route: endpoints.addressFallback,
      message: selectedAddressText(selection),
    });
  } catch (error) {
    reportUiOperationFailure({
      status,
      shortMessage: "备用地址不可用，请查看运行日志",
      title: "备用地址不可用",
      scope: "选择备用地址",
      route: endpoints.addressFallback,
      error,
    });
  } finally {
    button.disabled = false;
  }
}

export function addressSummaryFromSelection(selection) {
  const address = selection && selection.address ? selection.address : selection;
  if (!address) return null;
  return {
    index: Number.isInteger(selection && selection.index) ? selection.index : address.index,
    name: address.name || "",
    line1: address.line1 || "",
    city: address.city || "",
    state: address.state || "",
    zip: address.zip || "",
    phone: address.phone || "",
    source: selection && selection.source ? selection.source : "",
    complete: Boolean(
      address.complete || (address.name && address.line1 && address.city && address.state && address.zip),
    ),
  };
}

export function updateAddressDisplay(address) {
  renderAddressSourceTag(address);
  const fields = {
    name: address && address.name,
    line1: address && address.line1,
    city: address && address.city,
    state: address && address.state,
    zip: address && address.zip,
    phone: address && address.phone,
  };
  Object.entries(fields).forEach(([key, value]) => {
    const element = document.getElementById(`address-${key}`);
    const button = document.querySelector(`[data-address-copy-field="${key}"]`);
    const text = value || "待获取";
    if (element) element.textContent = text;
    if (button) {
      button.dataset.copyValue = value || "";
      button.disabled = !value;
    }
  });
}

export function renderAddressSourceTag(address) {
  const tag = document.getElementById("address-count");
  if (!tag) return;
  tag.textContent = address ? "地址已准备" : "等待地址";
}

export async function handleAddressDisplayClick(event) {
  const button = event.target.closest("[data-address-copy-field]");
  if (!button || button.disabled) return;
  const value = button.dataset.copyValue || "";
  if (!value) return;
  const field = button.dataset.addressCopyField || "unknown";
  const originalTitle = button.title;
  try {
    await writeClipboard(value);
    button.classList.add("copied");
    button.title = "已复制";
    recordClientRuntimeInfo({
      scope: "复制地址字段",
      route: "clipboard",
      message: `已复制地址字段：${field}。`,
    });
    showToast({ tone: "success", title: "地址字段已复制", message: "内容已复制到剪贴板。" });
  } catch (error) {
    button.title = "复制失败";
    recordClientRuntimeError({
      scope: "复制地址字段",
      route: "clipboard",
      error,
    });
    showToast({ tone: "error", title: "复制失败", message: "详细原因已写入运行日志。" });
  } finally {
    window.setTimeout(() => {
      button.classList.remove("copied");
      button.title = originalTitle || "点击复制";
    }, 1200);
  }
}

export function selectedAddressText(selection) {
  const address = selection.address || {};
  return `已选择 ${selection.index}: ${address.line1 || ""}, ${address.city || ""}`;
}

export function cardActionText(action) {
  return (
    {
      fresh: "新卡",
      used: "已使用",
      failed: "失败",
      remove: "移除",
    }[action] || statusText(action || "unknown")
  );
}
