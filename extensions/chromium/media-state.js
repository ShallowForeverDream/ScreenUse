function videoPlaying() {
  return [...document.querySelectorAll('video')].some(
    (video) => !video.paused && !video.ended && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
  );
}

let lastState = videoPlaying();
let lastContextSignature = '';
let contextCheckScheduled = false;

function pageContext() {
  return globalThis.ScreenUsePageContext?.readPageContext?.() || null;
}

function publishIfChanged() {
  const nextState = videoPlaying();
  if (nextState === lastState) return;
  lastState = nextState;
  void chrome.runtime.sendMessage({ type: 'screenuse-media-changed', videoPlaying: nextState });
}

for (const eventName of ['play', 'playing', 'pause', 'ended', 'emptied', 'stalled']) {
  document.addEventListener(eventName, publishIfChanged, true);
}

document.addEventListener('visibilitychange', publishIfChanged, true);

function publishContextIfChanged() {
  contextCheckScheduled = false;
  const context = pageContext();
  const signature = `${location.pathname}|${context?.type || ''}|${context?.title || ''}`;
  if (signature === lastContextSignature) return;
  lastContextSignature = signature;
  void chrome.runtime.sendMessage({ type: 'screenuse-page-context-changed', context });
}

function scheduleContextCheck() {
  if (contextCheckScheduled) return;
  contextCheckScheduled = true;
  setTimeout(publishContextIfChanged, 600);
}

if (globalThis.ScreenUsePageContext?.isSupportedConversationHost?.(location.hostname)) {
  const observer = new MutationObserver(scheduleContextCheck);
  const roots = [document.querySelector('nav'), document.querySelector('aside'), document.querySelector('title')]
    .filter((root, index, items) => root && items.indexOf(root) === index);
  for (const root of roots) {
    observer.observe(root, { childList: true, subtree: true, characterData: true });
  }
  window.addEventListener('popstate', scheduleContextCheck, true);
  scheduleContextCheck();
}

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== 'screenuse-media-state') return false;
  const context = pageContext();
  sendResponse({
    videoPlaying: videoPlaying(),
    contextTitle: context?.title || null,
    contextType: context?.type || null,
  });
  return false;
});
