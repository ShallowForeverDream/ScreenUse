function videoPlaying() {
  return [...document.querySelectorAll('video')].some(
    (video) => !video.paused && !video.ended && video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA,
  );
}

let lastState = videoPlaying();

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

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== 'screenuse-media-state') return false;
  sendResponse({ videoPlaying: videoPlaying() });
  return false;
});
