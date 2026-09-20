function responseError(path, response, data, message) {
  return Object.assign(new Error(data?.error || message || `${response.status} ${response.statusText}`), {
    route: path,
    status: response.status,
    kind: typeof data?.kind === "string" ? data.kind : null,
    payload: data,
  });
}

export async function request(path, { method = "GET", payload, timeoutMs, text = false } = {}) {
  const controller = timeoutMs === undefined ? null : new AbortController();
  const timer = controller ? setTimeout(() => controller.abort(), timeoutMs) : null;
  try {
    const response = await fetch(path, {
      method,
      headers: method === "POST"
        ? { Accept: "application/json", "Content-Type": "application/json" }
        : { Accept: "application/json" },
      cache: "no-store",
      signal: controller?.signal,
      body: method === "POST" ? JSON.stringify(payload) : undefined,
    });
    const body = await response.text();
    if (response.ok && text) return body;
    let data = null;
    try {
      data = body ? JSON.parse(body) : null;
    } catch {
      throw responseError(path, response, null, response.ok ? "服务返回了无效的 JSON" : "");
    }
    if (!response.ok) throw responseError(path, response, data);
    return data;
  } catch (cause) {
    const error = controller?.signal.aborted ? new Error("读取服务超时，请检查服务状态") : cause;
    error.route = path;
    throw error;
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

export function fetchJson(path, timeoutMs = 15000) {
  return request(path, { timeoutMs });
}

export async function fetchOptionalJson(path) {
  try {
    return await fetchJson(path);
  } catch {
    return null;
  }
}

export function postText(path, payload) {
  return request(path, { method: "POST", payload, text: true });
}
