const ENDPOINT = 'http://127.0.0.1:51247/integrations/browser/tabs';

async function collectTabs() {
  const windows = await chrome.windows.getAll({ populate: true, windowTypes: ['normal'] });
  const groups = chrome.tabGroups ? await chrome.tabGroups.query({}) : [];
  const groupMap = new Map(groups.map(group => [group.id, group]));
  const payload = {
    source: 'chromium-extension',
    capturedAt: new Date().toISOString(),
    windows: windows.map(win => ({
      id: win.id,
      focused: win.focused,
      tabs: (win.tabs || []).map(tab => ({
        id: tab.id,
        active: tab.active,
        highlighted: tab.highlighted,
        title: tab.title,
        url: tab.url,
        group: tab.groupId && tab.groupId > -1 ? groupMap.get(tab.groupId)?.title || `group:${tab.groupId}` : null,
      })),
    })),
  };
  try {
    await fetch(ENDPOINT, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload) });
    await chrome.storage.local.set({ lastSync: payload.capturedAt, lastError: '' });
  } catch (error) {
    await chrome.storage.local.set({ lastError: String(error) });
  }
}

chrome.alarms.create('screenuse-sync-tabs', { periodInMinutes: 1 });
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'screenuse-sync-tabs') collectTabs(); });
chrome.tabs.onActivated.addListener(() => collectTabs());
chrome.tabs.onUpdated.addListener((_tabId, changeInfo) => { if (changeInfo.title || changeInfo.url || changeInfo.status === 'complete') collectTabs(); });
collectTabs();
