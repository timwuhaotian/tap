(function () {
  if (window.__tapConsole) return;
  window.__tapConsole = true;
  function post(level, text, where) {
    try {
      var handlers = window.webkit && window.webkit.messageHandlers;
      var to = handlers && handlers.tapConsole;
      if (!to) return;
      // A frame names itself only when it isn't the page: the page's own URL is
      // already known, and a cross-origin iframe can't read its parent's anyway.
      var frame = window === window.top ? '' : String(location.href);
      to.postMessage({
        level: level,
        text: String(text).slice(0, 1000),
        where: String(where || '').slice(0, 300),
        frame: frame.slice(0, 300)
      });
    } catch (e) {}
  }
  function one(value) {
    if (typeof value === 'string') return value;
    if (value instanceof Error) return value.stack || String(value);
    try {
      var asJson = JSON.stringify(value);
      return asJson === undefined ? String(value) : asJson;
    } catch (e) {
      return String(value);
    }
  }
  function format(args) {
    var out = [];
    for (var i = 0; i < args.length; i++) {
      try {
        out.push(one(args[i]));
      } catch (e) {
        out.push('[?]');
      }
    }
    return out.join(' ');
  }
  ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
    try {
      var original = console[level];
      if (typeof original !== 'function') return;
      var bound = original.bind(console);
      console[level] = function () {
        post(level, format(arguments), '');
        return bound.apply(null, arguments);
      };
    } catch (e) {}
  });
  window.addEventListener('error', function (event) {
    var detail = event.message || 'Script error';
    if (event.error && event.error.stack) detail += '\n' + event.error.stack;
    var where = event.filename || '';
    if (event.lineno) where += ':' + event.lineno;
    post('error', detail, where);
  });
  window.addEventListener('unhandledrejection', function (event) {
    var reason = event.reason;
    post(
      'error',
      'Unhandled rejection: ' + (reason && reason.stack ? reason.stack : one(reason)),
      ''
    );
  });
})();
