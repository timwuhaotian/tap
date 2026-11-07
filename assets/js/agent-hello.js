// Every frame introduces itself to Tap (tap-shell/src/agent.rs), in Tap's own
// content world where the page can't see or forge it: snapshot, act and text then
// reach controls and words in cross-origin iframes the main frame can't see into
// (an Apple sign-in lives entirely in one). At each document's start, every frame;
// a frame that never says hello is never asked.
(function () {
  if (window.__tapAgentHello) return;
  window.__tapAgentHello = true;
  // A document's own mark, echoed back in its snapshot answer: a frame that says
  // hello twice (one document written over another) answers twice, and only the
  // answer wearing the latest mark counts.
  window.__tapAgentToken = Math.random().toString(36).slice(2) +
    Date.now().toString(36);
  try {
    var handlers = window.webkit && window.webkit.messageHandlers;
    var to = handlers && handlers.tapAgent;
    if (to) {
      to.postMessage({
        kind: 'hello',
        token: window.__tapAgentToken,
        url: String(location.href).slice(0, 500),
      });
    }
  } catch (e) {}
})();
