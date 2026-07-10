document.getElementById('sync').addEventListener('click', async () => {
  await chrome.runtime.getBackgroundPage?.();
  chrome.storage.local.get(['lastSync','lastError'], data => {
    document.getElementById('state').textContent = data.lastError || `上次同步：${data.lastSync || '暂无'}`;
  });
});
chrome.storage.local.get(['lastSync','lastError'], data => { document.getElementById('state').textContent = data.lastError || `上次同步：${data.lastSync || '暂无'}`; });
