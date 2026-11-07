// What Copy Tab Details and Copy Element for Agent read from a page (inspect.rs), in
// Tap's own content world, where the page can't see it. Run only when the person
// asks (a menu item) or an agent does (`tap info`): never on a timer, since every
// script the app runs in a page ends its user activation (PLAN 0.2.12).
//
// `verb` is "page" or "element"; the element is the one the last right-click was on,
// kept by the recorder (inspect.rs RECORDER) in this same world.
const clip = (s, n) => {
  s = String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
  return s.length > n ? s.slice(0, n) + '…' : s;
};
const meta = (key) => {
  for (const e of document.querySelectorAll('meta[name], meta[property]')) {
    const k = (e.getAttribute('name') || e.getAttribute('property') || '').toLowerCase();
    if (k === key) return clip(e.getAttribute('content'), 300) || null;
  }
  return null;
};
const link = (rel) => {
  const e = document.querySelector('link[rel~="' + rel + '"][href]');
  return e ? e.href : null;
};

function page() {
  const root = document.documentElement;
  const selection = String(window.getSelection ? getSelection() : '');
  return {
    url: location.href,
    title: document.title,
    lang: (root && root.lang) || null,
    contentType: document.contentType || null,
    charset: document.characterSet || null,
    readyState: document.readyState,
    referrer: document.referrer || null,
    modified: document.lastModified || null,
    canonical: link('canonical'),
    description: meta('description'),
    ogTitle: meta('og:title'),
    ogType: meta('og:type'),
    ogSite: meta('og:site_name'),
    generator: meta('generator'),
    viewport: {
      width: innerWidth,
      height: innerHeight,
      dpr: devicePixelRatio,
      x: Math.round(scrollX),
      y: Math.round(scrollY),
      scrollHeight: root ? root.scrollHeight : 0,
    },
    frames: window.frames.length,
    selection: clip(selection, 1000) || null,
  };
}

// A selector that finds this element and no other, short where the page allows:
// a test id or an id when one is unique, else a path of tag, classes and
// :nth-of-type from the nearest ancestor that is.
function selector(el) {
  const scope = el.getRootNode ? el.getRootNode() : document;
  const unique = (s) => {
    try { return scope.querySelectorAll(s).length === 1; } catch (_) { return false; }
  };
  const attr = (n, v) => '[' + n + '="' + String(v).replace(/(["\\])/g, '\\$1') + '"]';
  const own = (e) => {
    for (const n of ['data-testid', 'data-test', 'data-qa', 'data-cy']) {
      const v = e.getAttribute(n);
      if (v && unique(attr(n, v))) return attr(n, v);
    }
    if (e.id && unique('#' + CSS.escape(e.id))) return '#' + CSS.escape(e.id);
    return null;
  };
  const parts = [];
  for (let e = el; e && e.nodeType === 1; e = e.parentElement) {
    const found = own(e);
    if (found) { parts.unshift(found); break; }
    let part = e.localName;
    const classes = [...e.classList].filter((c) => /^[A-Za-z_-][\w-]*$/.test(c)).slice(0, 2);
    if (classes.length) part += classes.map((c) => '.' + CSS.escape(c)).join('');
    const parent = e.parentElement;
    const same = parent
      ? [...parent.children].filter((c) => c.localName === e.localName) : [];
    // Its place among its kind only when tag and classes don't already tell it apart.
    if (same.length > 1 && !unique([part, ...parts].join(' > '))) {
      part += ':nth-of-type(' + (same.indexOf(e) + 1) + ')';
    }
    parts.unshift(part);
    if (unique(parts.join(' > '))) break;
    if (e.localName === 'body') break;
  }
  return parts.join(' > ');
}

// The name a screen reader would give it, in its smallest form.
function name(el) {
  const label = el.getAttribute('aria-label');
  if (label) return clip(label, 120);
  const by = el.getAttribute('aria-labelledby');
  if (by) {
    const text = by.split(/\s+/).map((id) => document.getElementById(id))
      .filter(Boolean).map((e) => e.textContent).join(' ');
    if (text.trim()) return clip(text, 120);
  }
  if (el.labels && el.labels.length) return clip(el.labels[0].textContent, 120);
  return clip(el.getAttribute('alt') || el.getAttribute('title') ||
    el.getAttribute('placeholder') || '', 120) || null;
}

// The element right-clicked: the one the recorder kept when it was where the menu
// opened (it sees into shadow roots), else whatever stands at that point now (an
// iframe, when the click was inside one: the recorder lives in the main frame only).
function element() {
  const x = Number(pointX), y = Number(pointY);
  const kept = window.__tapPointed;
  let el = kept && kept.target && kept.target.isConnected &&
    Math.abs(kept.x - x) < 4 && Math.abs(kept.y - y) < 4 ? kept.target : null;
  if (!el && isFinite(x) && isFinite(y)) el = document.elementFromPoint(x, y);
  if (!el) return { missing: true };
  if (el.nodeType !== 1) el = el.parentElement;
  if (!el) return { missing: true };
  const box = el.getBoundingClientRect();
  const style = getComputedStyle(el);
  const root = el.getRootNode ? el.getRootNode() : document;
  const tag = el.cloneNode(false).outerHTML.replace(/<\/[^>]+>$/, '');
  const html = el.outerHTML;
  return {
    selector: selector(el),
    shadowHost: root !== document && root.host ? selector(root.host) : null,
    frame: el.localName === 'iframe' || el.localName === 'frame',
    tag: clip(tag, 300),
    role: el.getAttribute('role') || null,
    name: name(el),
    text: clip(el.innerText || el.textContent || '', 300) || null,
    href: el.closest('a[href]') ? el.closest('a[href]').href : null,
    src: el.currentSrc || el.src || null,
    value: 'value' in el && !/^(password|hidden)$/i.test(el.type || '') && typeof el.value === 'string'
      ? clip(el.value, 200) : null,
    box: {
      x: Math.round(box.x), y: Math.round(box.y),
      width: Math.round(box.width), height: Math.round(box.height),
      pageY: Math.round(box.y + scrollY),
    },
    style: {
      display: style.display,
      position: style.position,
      font: style.fontWeight + ' ' + style.fontSize + '/' + style.lineHeight + ' ' +
        clip(style.fontFamily, 60),
      color: style.color,
      background: style.backgroundColor,
      margin: style.margin,
      padding: style.padding,
      zIndex: style.zIndex,
      visibility: style.visibility,
      opacity: style.opacity,
    },
    children: el.children.length,
    html: html.length > 800 ? html.slice(0, 800) + '…' : html,
  };
}

return verb === 'element' ? element() : page();
