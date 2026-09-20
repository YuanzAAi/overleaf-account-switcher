/**
 * Overleaf Account Switcher Bridge - 后台服务
 *
 * 功能：通过 WebSocket 连接本地 Rust service，支持读取和设置 Cookie
 * 通信协议：WebSocket (ws://localhost:9876)
 */

const WS_URL = 'ws://localhost:9876';
const RECONNECT_INTERVAL = 500; // 500ms快速重连
const KEEPALIVE_INTERVAL = 15000; // 15秒发送一次心跳

let ws = null;
let reconnectTimer = null;
let keepaliveTimer = null;
let currentSessionId = 'unknown'; // 保存当前的session_id
let extensionClientId = null; // 每个扩展安装实例的稳定ID，用于Profile静默识别

async function getExtensionClientId() {
  if (extensionClientId) {
    return extensionClientId;
  }

  const storageKey = 'oas_client_id';
  const stored = await chrome.storage.local.get(storageKey);
  if (stored && stored[storageKey]) {
    extensionClientId = stored[storageKey];
    return extensionClientId;
  }

  extensionClientId =
    (typeof crypto !== 'undefined' && crypto.randomUUID)
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  await chrome.storage.local.set({ [storageKey]: extensionClientId });
  return extensionClientId;
}

async function sendHandshake(reason) {
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    return;
  }

  try {
    const clientId = await getExtensionClientId();
    const handshake = {
      action: 'handshake',
      session_id: currentSessionId,
      client_id: clientId,
      timestamp: Date.now()
    };
    ws.send(JSON.stringify(handshake));
    console.log('已发送握手消息，Session ID:', currentSessionId.substring(0, 8) + '...', reason || '');
  } catch (error) {
    console.error('发送握手信息失败:', error);
  }
}

// 使用chrome.alarms保持Service Worker活跃（缩短到5秒）
chrome.alarms.create('keepalive', { periodInMinutes: 0.08 }); // 约5秒触发一次

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === 'keepalive') {
    // 保持Service Worker活跃
    console.log('Service Worker keepalive');
    // 如果WebSocket断开，尝试重连
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      connectWebSocket();
    }
  }
});

// 监听标签页更新，捕获session_id
chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.url && changeInfo.url.includes('overleaf.com') && changeInfo.url.includes('oas_session=')) {
    try {
      const url = new URL(changeInfo.url);
      const sessionId = url.searchParams.get('oas_session');
      if (sessionId) {
        currentSessionId = sessionId;
        console.log('捕获到Session ID:', sessionId.substring(0, 8) + '...');

        // 如果WebSocket已连接，立即发送握手
        sendHandshake('标签页更新触发');
      }
    } catch (error) {
      console.error('解析URL失败:', error);
    }
  }
});

// 监听标签页激活事件，立即尝试连接
chrome.tabs.onActivated.addListener(() => {
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    connectWebSocket();
  }
});

// 监听窗口焦点变化，立即尝试连接
chrome.windows.onFocusChanged.addListener((windowId) => {
  if (windowId !== chrome.windows.WINDOW_ID_NONE) {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      connectWebSocket();
    }
  }
});

// 保持Service Worker活跃
function keepAlive() {
  if (keepaliveTimer) {
    clearInterval(keepaliveTimer);
  }
  keepaliveTimer = setInterval(() => {
    // 发送心跳保持连接
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ action: 'ping' }));
    }
  }, KEEPALIVE_INTERVAL);
}

// 获取Cookie
async function getCookie(cookieName) {
  try {
    const cookie = await chrome.cookies.get({
      url: 'https://www.overleaf.com',
      name: cookieName
    });

    if (cookie) {
      console.log('Cookie读取成功:', cookie.name);
      return {
        success: true,
        cookie: {
          name: cookie.name,
          value: cookie.value,
          domain: cookie.domain,
          path: cookie.path,
          secure: cookie.secure,
          httpOnly: cookie.httpOnly,
          sameSite: cookie.sameSite,
          expirationDate: cookie.expirationDate
        }
      };
    } else {
      console.log('Cookie不存在:', cookieName);
      return { success: false, message: 'Cookie不存在' };
    }
  } catch (error) {
    console.error('Cookie读取失败:', error);
    return { success: false, message: error.message };
  }
}

// 设置Cookie
async function setCookie(cookieData) {
  try {
    const cookie = {
      url: 'https://www.overleaf.com',
      name: cookieData.name,
      value: cookieData.value,
      domain: cookieData.domain || '.overleaf.com',
      path: cookieData.path || '/',
      secure: cookieData.secure !== false,
      httpOnly: cookieData.httpOnly !== false,
      sameSite: cookieData.sameSite || 'no_restriction'
    };

    // 已验证的会话可能仍有效，过期的本地元数据不能让 Chrome 立即删除 Cookie。
    if (Number.isFinite(cookieData.expirationDate) && cookieData.expirationDate > Date.now() / 1000) {
      cookie.expirationDate = cookieData.expirationDate;
    }

    const saved = await chrome.cookies.set(cookie);
    if (!saved) throw new Error('Cookie未保存，请检查扩展权限');
    console.log('Cookie设置成功:', cookie.name);
    return { success: true, message: 'Cookie设置成功' };
  } catch (error) {
    console.error('Cookie设置失败:', error);
    return { success: false, message: error.message };
  }
}

// 刷新所有 Overleaf 标签页
async function refreshOverleafTabs() {
  try {
    const tabs = await chrome.tabs.query({
      url: ['https://*.overleaf.com/*', 'https://overleaf.com/*']
    });
    let refreshed = 0;
    for (const tab of tabs) {
      if (tab.id !== undefined) {
        chrome.tabs.reload(tab.id);
        refreshed += 1;
      }
    }
    console.log('刷新了', refreshed, '个标签页');
    return { success: true, refreshed };
  } catch (error) {
    console.error('刷新标签页失败:', error);
    return { success: false, message: error.message };
  }
}

// 获取 Cookie 过期时间
async function getCookieExpiry() {
  try {
    const result = await getCookie('overleaf_session2');
    if (!result.success) return result;
    if (!result.cookie) {
      return { success: false, message: 'Cookie不存在' };
    }
    return {
      success: true,
      expirationDate: result.cookie.expirationDate || null
    };
  } catch (error) {
    console.error('获取Cookie过期时间失败:', error);
    return { success: false, message: error.message };
  }
}

// 处理指令
async function handleCommand(command) {
  console.log('收到指令:', command.action);
  try {
    let result;
    switch (command.action) {
      case 'get_cookie':
        result = await getCookie(command.name);
        break;
      case 'set_cookie':
        result = await setCookie(command.cookie);
        break;
      case 'refresh_tabs':
        result = await refreshOverleafTabs();
        break;
      case 'get_cookie_expiry':
        result = await getCookieExpiry();
        break;
      case 'ping':
        return { success: true, message: 'pong' };
      default:
        console.error('未知指令:', command.action);
        return { success: false, message: '未知指令' };
    }

    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ ...result, request_id: command.request_id, action: command.action }));
    } else {
      console.error('WebSocket未连接，无法发送响应');
    }
    return result;
  } catch (error) {
    console.error('处理指令时出错:', error);
    return { success: false, message: error.message };
  }
}

// 连接WebSocket
function connectWebSocket() {
  // 如果已经在连接中，不要重复连接
  if (ws && (ws.readyState === WebSocket.CONNECTING || ws.readyState === WebSocket.OPEN)) {
    console.log('WebSocket已连接或正在连接中');
    return;
  }

  try {
    console.log('尝试连接WebSocket...');
    ws = new WebSocket(WS_URL);

    ws.onopen = async () => {
      console.log('WebSocket连接成功');
      // 清除重连定时器
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }

      // 主动查询所有Overleaf标签页，找到带有session_id的标签页
      try {
        const tabs = await chrome.tabs.query({ url: 'https://*.overleaf.com/*' });
        let foundSessionId = false;

        for (const tab of tabs) {
          if (tab.url && tab.url.includes('oas_session=')) {
            try {
              const url = new URL(tab.url);
              const sessionId = url.searchParams.get('oas_session');
              if (sessionId) {
                currentSessionId = sessionId;
                foundSessionId = true;
                console.log('从标签页URL读取到Session ID:', sessionId.substring(0, 8) + '...');
                break;
              }
            } catch (error) {
              console.error('解析URL失败:', error);
            }
          }
        }

        if (!foundSessionId) {
          console.log('未找到带有session_id的标签页，使用默认值:', currentSessionId);
        }
      } catch (error) {
        console.error('查询标签页失败:', error);
      }

      // 发送握手消息
      await sendHandshake('WebSocket连接成功');

      // 启动心跳保持连接
      keepAlive();
    };

    ws.onmessage = async (event) => {
      try {
        const command = JSON.parse(event.data);
        await handleCommand(command);
      } catch (error) {
        console.error('处理消息失败:', error);
      }
    };

    ws.onerror = (error) => {
      console.error('WebSocket错误:', error);
    };

    ws.onclose = () => {
      console.log('WebSocket连接关闭，准备重连...');
      ws = null;
      // 清除心跳定时器
      if (keepaliveTimer) {
        clearInterval(keepaliveTimer);
        keepaliveTimer = null;
      }
      // 自动重连
      if (!reconnectTimer) {
        reconnectTimer = setTimeout(connectWebSocket, RECONNECT_INTERVAL);
      }
    };
  } catch (error) {
    console.error('WebSocket连接失败:', error);
    // 自动重连
    if (!reconnectTimer) {
      reconnectTimer = setTimeout(connectWebSocket, RECONNECT_INTERVAL);
    }
  }
}

// 扩展安装时初始化
chrome.runtime.onInstalled.addListener(() => {
  console.log('Overleaf Account Switcher Bridge 已安装');
  connectWebSocket();
});

// 扩展启动时连接WebSocket
console.log('Service Worker启动，开始连接WebSocket');
connectWebSocket();

// 监听来自content script的消息（保持Service Worker活跃）
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.action === 'keepalive') {
    sendResponse({ status: 'alive' });
  }
  return true;
});
