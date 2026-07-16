const ENDPOINT = 'http://127.0.0.1:51247/integrations/browser/tabs';
const HEARTBEAT_ALARM = 'screenuse-active-tab-heartbeat';
const DEBOUNCE_MS = 650;

let debounceTimer;
let lastSignature = '';
let lastSentAt = 0;

function browserName() {
  const userAgent = navigator.userAgent;
  if (userAgent.includes('Edg/')) return 'Microsoft Edge';
  if (userAgent.includes('OPR/')) return 'Opera';
  if (userAgent.includes('Vivaldi/')) return 'Vivaldi';
  if (userAgent.includes('Tabbit')) return 'Tabbit Browser';
  if (userAgent.includes('Arc/')) return 'Arc';
  if (userAgent.includes('Brave') || navigator.brave) return 'Brave';
  if (userAgent.includes('Firefox/')) return 'Mozilla Firefox';
  if (userAgent.includes('Chrome/')) return 'Google Chrome';
  return 'Chromium';
}

function compactUrl(rawUrl) {
  if (!rawUrl) return null;
  try {
    const url = new URL(rawUrl);
    url.search = '';
    url.hash = '';
    return url.toString();
  } catch {
    return String(rawUrl).slice(0, 1200);
  }
}

async function readActiveContext() {
  const currentWindow = await chrome.windows.getLastFocused({ windowTypes: ['normal'] });
  if (!currentWindow?.focused || currentWindow.id == null) return null;
  const tabs = await chrome.tabs.query({ active: true, windowId: currentWindow.id });
  const tab = tabs[0];
  if (!tab || tab.id == null) return null;

  const url = compactUrl(tab.url);
  let videoPlaying = false;
  let contextTitle = null;
  let contextType = null;
  try {
    const mediaState = await chrome.tabs.sendMessage(tab.id, { type: 'screenuse-media-state' });
    videoPlaying = Boolean(mediaState?.videoPlaying);
    contextTitle = mediaState?.contextTitle || null;
    contextType = mediaState?.contextType || null;
  } catch {
    // Browser-internal pages do not run content scripts.
  }
  const title = contextTitle || tab.title || null;
  return {
    source: 'chromium-extension',
    capturedAt: new Date().toISOString(),
    eventId: `${currentWindow.id}:${tab.id}:${url || title || ''}`,
    browser: browserName(),
    windowId: currentWindow.id,
    tabId: tab.id,
    title,
    tabTitle: tab.title || null,
    contextTitle,
    contextType,
    url,
    audible: Boolean(tab.audible),
    videoPlaying,
  };
}

async function syncActiveContext(reason = 'event', force = false) {
  try {
    const payload = await readActiveContext();
    if (!payload) return { ok: false, inactive: true };

    payload.reason = reason;
    const signature = `${payload.eventId}|${payload.title || ''}|${payload.audible}|${payload.videoPlaying}`;
    const now = Date.now();
    if (!force && signature === lastSignature && now - lastSentAt < 55_000) {
      return { ok: true, skipped: true };
    }

    const response = await fetch(ENDPOINT, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload),
    });
    if (!response.ok) throw new Error(`ScreenUse 返回 ${response.status}`);

    lastSignature = signature;
    lastSentAt = now;
    await chrome.storage.local.set({
      lastSync: payload.capturedAt,
      lastError: '',
      lastTitle: payload.title || '',
      lastUrl: payload.url || '',
    });
    return { ok: true };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await chrome.storage.local.set({ lastError: message });
    return { ok: false, error: message };
  }
}

function scheduleSync(reason, force = false) {
  clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => {
    void syncActiveContext(reason, force);
  }, DEBOUNCE_MS);
}

chrome.runtime.onInstalled.addListener(() => {
  chrome.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: 1 });
  scheduleSync('installed', true);
});

chrome.runtime.onStartup.addListener(() => {
  chrome.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: 1 });
  scheduleSync('startup', true);
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === HEARTBEAT_ALARM) void syncActiveContext('heartbeat', true);
});

chrome.tabs.onActivated.addListener(() => scheduleSync('tab-activated', true));
chrome.tabs.onUpdated.addListener((_tabId, changeInfo, tab) => {
  if (tab.active && (changeInfo.title || changeInfo.url || changeInfo.audible !== undefined || changeInfo.status === 'complete')) {
    scheduleSync('tab-updated');
  }
});
chrome.windows.onFocusChanged.addListener((windowId) => {
  if (windowId !== chrome.windows.WINDOW_ID_NONE) scheduleSync('window-focused', true);
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'screenuse-media-changed') {
    scheduleSync('media-state-changed', true);
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === 'screenuse-page-context-changed') {
    scheduleSync('page-context-changed', true);
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === 'screenuse-sync-now') {
    void syncActiveContext('manual', true).then(sendResponse);
    return true;
  }
  if (message?.type === 'screenuse-status') {
    void chrome.storage.local
      .get(['lastSync', 'lastError', 'lastTitle', 'lastUrl'])
      .then((status) => sendResponse(status));
    return true;
  }
  return false;
});

chrome.alarms.create(HEARTBEAT_ALARM, { periodInMinutes: 1 });
scheduleSync('service-worker-start', true);
