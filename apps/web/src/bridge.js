function desktopCommand(name) {
  const command = window.__OVERLEAF_DESKTOP__?.[name];
  return typeof command === "function" ? command : null;
}

export function taskBrowserUrl(config, baseUrl = window.location.href) {
  const port = config?.browser_viewer_port;
  if (!Number.isInteger(port) || port < 1 || port > 65535) return "";
  const url = new URL(baseUrl);
  if (!["http:", "https:"].includes(url.protocol)) return "";
  url.port = String(port);
  url.pathname = "/vnc.html";
  url.search = "autoconnect=1&resize=scale";
  url.hash = "";
  return url.href;
}

export function resolveTauriDialogOpen() {
  return desktopCommand("openDialog");
}

export function resolveTauriDialogSave() {
  return desktopCommand("saveDialog");
}

export function resolveTauriWriteTextFile() {
  return desktopCommand("writeTextFile");
}

export function resolveTauriWriteClipboardText() {
  const write = desktopCommand("writeClipboardText");
  return write ? (text) => write(String(text ?? "")) : null;
}

export function resolveDesktopFocusMainWindow() {
  return desktopCommand("focusMainWindow");
}

export function desktopBridgeStatus() {
  return {
    openDialog: Boolean(resolveTauriDialogOpen()),
    saveDialog: Boolean(resolveTauriDialogSave()),
    writeTextFile: Boolean(resolveTauriWriteTextFile()),
    clipboardWrite: Boolean(resolveTauriWriteClipboardText()),
  };
}

export function selectedDialogPath(selection) {
  const value = Array.isArray(selection) ? selection[0] : selection;
  return typeof value === "string" && value.trim() ? value : "";
}
