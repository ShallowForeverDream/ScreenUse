const state = document.getElementById('state');
const title = document.getElementById('title');
const url = document.getElementById('url');
const dot = document.getElementById('dot');
const syncButton = document.getElementById('sync');

function render(status = {}) {
  const hasError = Boolean(status.lastError);
  dot.className = `dot ${hasError ? 'bad' : status.lastSync ? 'ok' : ''}`;
  state.textContent = hasError
    ? `未连接：${status.lastError}`
    : status.lastSync
      ? `已同步 ${new Date(status.lastSync).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`
      : '尚未同步';
  title.textContent = status.lastTitle || '暂无活动标题';
  url.textContent = status.lastUrl || '';
}

async function readStatus() {
  try {
    const status = await chrome.runtime.sendMessage({ type: 'screenuse-status' });
    render(status);
  } catch (error) {
    render({ lastError: error instanceof Error ? error.message : String(error) });
  }
}

syncButton.addEventListener('click', async () => {
  syncButton.disabled = true;
  state.textContent = '正在同步…';
  try {
    await chrome.runtime.sendMessage({ type: 'screenuse-sync-now' });
  } finally {
    syncButton.disabled = false;
    await readStatus();
  }
});

void readStatus();
