//! Reading mode (Search's `Reader.swift` and `Markdown.swift`, in their smallest form):
//! ⇧⌘R puts the article alone on the page, in clean type. When the site publishes a
//! Markdown copy of the page (`reader.twins`, off unless asked for, Search's
//! `Prefs.readsTwins`) that copy is drawn instead of the guess at the article, and
//! nothing in it can run: it is turned into HTML here, with every piece of text
//! escaped and only web and mail addresses kept. The look follows the window
//! (`reader.follows`) only when asked for; otherwise the article is light, as
//! Search's was before the switch existed.
//!
//! The scripts run through `web::evaluate_in_world`, Tap's own content world: the DOM
//! is shared with the page, so the reading page can be built there, and the page's own
//! scripts cannot see or swap the functions used. What is missing in Tap for Search's
//! whole Reader is the corner light/dark switch (`ReaderLook`): it is a view over the
//! stage, and `window.rs` has no hook for putting one there, so the kept `reader.look`
//! is read and honoured but cannot be changed in the article. Leaving reading mode
//! reloads, as in Search: putting the old markup back would give a page that looks
//! right and does nothing.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fmt::Write as _,
    ptr::{NonNull, null_mut},
};

use block2::{DynBlock, RcBlock};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
use objc2_web_kit::WKWebView;
use serde_json::Value;
use tap_core::{browser::TabId, settings_index as index};

use crate::{prefs, rt, web};

thread_local! {
    /// Tabs asked for a twin that hasn't answered yet: a second ⇧⌘R meanwhile is the
    /// same request, not another (Search's `seekingTwin`).
    static SEEKING: RefCell<HashSet<TabId>> = RefCell::new(HashSet::new());
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Puts the ⇧⌘R monitor in place, once. Tap's menus have no Reading Mode item yet
/// (that is `menu.rs`, which this module may not touch), so the key is taken here,
/// the way Search's menu takes it, and swallowed when it is ours.
pub fn install() {
    if INSTALLED.replace(true) {
        return;
    }
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> =
        RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: AppKit hands a live event to the monitor, for the call.
            let event = unsafe { event.as_ref() };
            let flags = event.modifierFlags();
            let ours = flags.contains(NSEventModifierFlags::Command)
                && flags.contains(NSEventModifierFlags::Shift)
                && !event.isARepeat()
                && event
                    .charactersIgnoringModifiers()
                    .is_some_and(|keys| keys.to_string().eq_ignore_ascii_case("r"));
            if ours {
                toggle_active();
                return null_mut();
            }
            std::ptr::from_ref(event).cast_mut()
        });
    let block: &DynBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> = &block;
    // SAFETY: a key-down monitor for this app's own events; the returned monitor is
    // forgotten on purpose, so it lives as long as the app does.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, block)
    };
    std::mem::forget(monitor);
}

/// ⇧⌘R on the tab in front.
pub fn toggle_active() {
    let Some(tab) = rt::read(tap_core::browser::Browser::active) else {
        return;
    };
    let Some(tab) = tab else { return };
    toggle(tab);
}

/// Reading mode on or off for one tab (Search's `Tab.toggleReader`).
pub fn toggle(tab: TabId) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    if SEEKING.with_borrow(|seeking| seeking.contains(&tab)) {
        return;
    }
    eval(
        &web,
        "(!!document.getElementById('office-reader'))",
        move |value| {
            if value == Some(Value::Bool(true)) {
                // Leaving reloads rather than putting the old markup back (see above).
                let Some(web) = web::existing(tab) else {
                    return;
                };
                // SAFETY: a plain call on a live web view.
                unsafe { web.reload() };
                return;
            }
            let kept = prefs::Kept::new();
            if index::flag(&kept, "reader.twins") {
                seek_twin(tab);
            } else {
                guess(tab);
            }
        },
    );
}

/// The site's own Markdown for the page, if it has some (Search's `Reader.twin`):
/// the page itself, a `<link rel="alternate" type="text/markdown">` it names, or the
/// same address with `.md` on the end — only from its own origin, only when the
/// answer says it is Markdown, and only the named one may be plain text.
fn seek_twin(tab: TabId) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    SEEKING.with_borrow_mut(|seeking| seeking.insert(tab));
    let before = web::url_of(&web);
    eval(
        &web,
        &format!("(async () => {{\n{TWIN}\n}})()"),
        move |value| {
            SEEKING.with_borrow_mut(|seeking| seeking.remove(&tab));
            let Some(web) = web::existing(tab) else {
                return;
            };
            // Gone somewhere else while the twin was on its way: it is not this page's.
            if web::url_of(&web) != before {
                return;
            }
            let drawn = value.as_ref().is_some_and(|found| {
                let (Some(text), Some(address)) = (
                    found.get("text").and_then(Value::as_str),
                    found.get("url").and_then(Value::as_str),
                ) else {
                    return false;
                };
                let page = markdown_page(text, address);
                if page.html.is_empty() {
                    return false;
                }
                let host = found
                    .get("from")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let from = if host.is_empty() {
                    "markdown".to_owned()
                } else {
                    format!("{host} · markdown")
                };
                let title = page
                    .title
                    .or_else(|| {
                        found
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                show(tab, &page.html, &title, &from);
                true
            });
            if !drawn {
                guess(tab);
            }
        },
    );
}

/// The twin, drawn in the same page the guess makes (Search's `Reader.show`).
fn show(tab: TabId, html: &str, title: &str, from: &str) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    let script = format!(
        "(async () => {{\n var readerLook = {};\n var sheet = {};\n var html = {};\n var title = {};\n var from = {};\n{PRESENT}\n var wrap = document.createElement('div');\n wrap.innerHTML = html;\n present(wrap, title, from, sheet);\n return 'read';\n}})()",
        json(&look()),
        json(SHEET),
        json(html),
        json(title),
        json(from),
    );
    eval(&web, &script, |_| {});
}

/// The guess, for a page with no twin (Search's `Reader.script`, shortened: the
/// prose-over-links scoring, the cleanup, and pictures resolved to the address the
/// page actually loaded; Search's byline, lead-picture and chart-frame hunting are
/// not ported).
fn guess(tab: TabId) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    let script = format!(
        "(async () => {{\n var readerLook = {};\n var sheet = {};\n{PRESENT}\n{GUESS}\n}})()",
        json(&look()),
        json(SHEET),
    );
    eval(&web, &script, |_| {});
}

/// Light, dark, or as the window is, for the next article (Search's `Reader.look`):
/// light whenever reading mode doesn't follow the window; otherwise the look kept
/// under `reader.look`, or `system` when none was ever kept.
fn look() -> String {
    let kept = prefs::Kept::new();
    let stored =
        index::Prefs::get(&kept, "reader.look").and_then(|v| v.as_str().map(str::to_owned));
    look_of(index::flag(&kept, "reader.follows"), stored.as_deref()).to_owned()
}

fn look_of(follows: bool, stored: Option<&str>) -> &'static str {
    if !follows {
        return "light";
    }
    match stored {
        Some("light") => "light",
        Some("dark") => "dark",
        _ => "system",
    }
}

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

fn eval(web: &WKWebView, script: &str, then: impl Fn(Option<Value>) + 'static) {
    web::evaluate_in_world(web, script, then);
}

/// Search's sheet, shortened to the same look: a 38em serif column, the window's own
/// dark ground when dark, pictures a touch dimmer there.
const SHEET: &str = concat!(
    ":root{color-scheme:light;--r-ground:#fff;--r-ink:#171717;--r-quiet:#a3a3a3;",
    "--r-aside:#555;--r-rule:#e8e8e8;--r-code:#f5f5f5;--r-picture:none}",
    "@media (prefers-color-scheme:dark){:root:not([data-office-look=light]){",
    "color-scheme:dark;--r-ground:#1c1c1c;--r-ink:#e0e0e0;--r-quiet:#8a8a8a;",
    "--r-aside:#a8a8a8;--r-rule:#333;--r-code:#2a2a2a;--r-picture:brightness(.92)}}",
    ":root[data-office-look=dark]{color-scheme:dark;--r-ground:#1c1c1c;--r-ink:#e0e0e0;",
    "--r-quiet:#8a8a8a;--r-aside:#a8a8a8;--r-rule:#333;--r-code:#2a2a2a;--r-picture:brightness(.92)}",
    "html,body{background:var(--r-ground) !important;margin:0 !important;padding:0 !important}",
    "#office-reader{max-width:38em;margin:0 auto;padding:72px 24px 160px;",
    "font:400 18px/1.72 ui-serif,Georgia,serif;color:var(--r-ink)}",
    "#office-reader h1{font:600 30px/1.24 -apple-system,sans-serif;margin:0 0 8px}",
    "#office-reader .office-from{font:400 12px/1 -apple-system,sans-serif;color:var(--r-quiet);",
    "margin:0 0 40px;text-transform:uppercase;letter-spacing:.06em}",
    "#office-reader p{margin:0 0 1.35em}",
    "#office-reader img,#office-reader video,#office-reader iframe{max-width:100%;height:auto;",
    "border-radius:6px;margin:1.6em 0;display:block}",
    "#office-reader img{filter:var(--r-picture)}",
    "#office-reader a{color:var(--r-ink);text-underline-offset:3px}",
    "#office-reader h2,#office-reader h3{font:600 20px/1.3 -apple-system,sans-serif;margin:2em 0 .6em}",
    "#office-reader pre,#office-reader code{font-family:ui-monospace,monospace;font-size:14px}",
    "#office-reader pre{background:var(--r-code);padding:14px;border-radius:8px;overflow:auto}",
    "#office-reader blockquote{margin:1.6em 0;padding-left:1.2em;border-left:2px solid var(--r-rule);color:var(--r-aside)}",
    "#office-reader ul,#office-reader ol{margin:0 0 1.35em;padding-left:1.4em}",
    "#office-reader table{border-collapse:collapse;font:400 15px/1.5 -apple-system,sans-serif}",
    "#office-reader th,#office-reader td{border-bottom:1px solid var(--r-rule);padding:8px 12px 8px 0;text-align:left}",
);

/// Puts the reading page in place of the page (Search's `Reader.present`): the page's
/// own stylesheets go, the title and where it is from go on top.
const PRESENT: &str = r#"
function present(wrap, title, from, sheet) {
  var chosen = typeof readerLook === 'string' ? readerLook : 'light';
  if (chosen === 'system') document.documentElement.removeAttribute('data-office-look');
  else document.documentElement.setAttribute('data-office-look', chosen);
  var theirs = document.querySelectorAll('style, link[rel~="stylesheet" i]');
  for (var i = 0; i < theirs.length; i++) {
    if ((theirs[i].id || '').indexOf('office-') !== 0) theirs[i].remove();
  }
  document.documentElement.removeAttribute('style');
  document.body.removeAttribute('style');
  var style = document.createElement('style');
  style.id = 'office-reader-sheet';
  style.textContent = sheet;
  var top = document.createElement('h1');
  top.textContent = title;
  var line = document.createElement('p');
  line.className = 'office-from';
  line.textContent = from;
  wrap.id = 'office-reader';
  document.body.innerHTML = '';
  (document.head || document.documentElement).appendChild(style);
  wrap.insertBefore(line, wrap.firstChild);
  wrap.insertBefore(top, wrap.firstChild);
  document.body.appendChild(wrap);
  window.scrollTo(0, 0);
}
"#;

const GUESS: &str = r#"
function prose(el) {
  var paragraphs = el.querySelectorAll('p');
  if (paragraphs.length < 2) return 0;
  var letters = 0;
  for (var i = 0; i < paragraphs.length; i++) letters += (paragraphs[i].innerText || '').length;
  if (letters < 400) return 0;
  return letters / (1 + el.querySelectorAll('a').length * 14);
}
var candidates = document.querySelectorAll('article, main, [role="main"], .post, .entry, .article, .content, #content, div, section');
var article = null, mark = 0;
for (var i = 0; i < candidates.length; i++) {
  var score = prose(candidates[i]);
  if (score > mark) { mark = score; article = candidates[i]; }
}
if (!article) return 'none';
var pictures = article.querySelectorAll('img');
for (var p = 0; p < pictures.length; p++) {
  var real = pictures[p].currentSrc || pictures[p].getAttribute('src') || '';
  if (real) pictures[p].setAttribute('src', real);
  pictures[p].setAttribute('loading', 'eager');
}
var heading = document.querySelector('h1');
var title = (heading && heading.innerText.trim()) || document.title;
var wrap = document.createElement('div');
wrap.innerHTML = article.innerHTML;
var clutter = wrap.querySelectorAll('script,style,noscript,form,nav,aside,footer,button,input,select,textarea,[role="complementary"],[role="navigation"],[role="banner"],[aria-hidden="true"]');
for (var c = 0; c < clutter.length; c++) clutter[c].remove();
var styled = wrap.querySelectorAll('[style]');
for (var y = 0; y < styled.length; y++) styled[y].removeAttribute('style');
var kept = wrap.querySelectorAll('img');
for (var g = 0; g < kept.length; g++) {
  var src = kept[g].getAttribute('src') || '';
  if (!src || src.indexOf('data:image') === 0) kept[g].remove();
}
var own = wrap.querySelector('h1');
if (own && own.textContent.replace(/\s+/g, ' ').trim().toLowerCase() === title.replace(/\s+/g, ' ').trim().toLowerCase()) own.remove();
present(wrap, title, location.host.replace(/^www\./, ''), sheet);
return 'read';
"#;

/// Search's `Reader.twin`, verbatim in behaviour (see `seek_twin`).
const TWIN: &str = r#"
var markdown = ['text/markdown', 'text/x-markdown'];
function kind(type) { return (type || '').toLowerCase().split(';')[0].trim(); }
var heading = document.querySelector('h1');
var about = {
  title: (heading && heading.innerText.trim()) || document.title,
  from: location.host.replace(/^www\./, '')
};
var own = kind(document.contentType);
if (markdown.indexOf(own) >= 0 ||
    (own === 'text/plain' && /\.(md|markdown)$/i.test(location.pathname))) {
  return { url: location.href, text: document.body ? document.body.textContent : '',
           title: about.title, from: about.from };
}
if (!/^https?:$/.test(location.protocol)) return null;
function home(address) {
  try { return new URL(address, location.href).origin === location.origin; } catch (e) { return false; }
}
async function read(address, patience, named) {
  if (!home(address)) return null;
  var stop = new AbortController();
  var timer = setTimeout(function () { stop.abort(); }, patience);
  try {
    var answer = await fetch(address, {
      headers: { Accept: 'text/markdown, text/plain;q=0.5' },
      cache: 'no-store', credentials: 'same-origin', signal: stop.signal
    });
    var type = kind(answer.headers.get('content-type'));
    var fits = markdown.indexOf(type) >= 0 || (named && type === 'text/plain');
    if (!answer.ok || !fits || (answer.url && !home(answer.url))) {
      if (answer.body) answer.body.cancel();
      return null;
    }
    var text = await answer.text();
    if (text.length > 4000000 || /^\s*<(!doctype|html)/i.test(text)) return null;
    return { url: answer.url || address, text: text, title: about.title, from: about.from };
  } catch (e) {
    return null;
  } finally {
    clearTimeout(timer);
  }
}
var links = document.querySelectorAll('link[rel~="alternate" i][type]');
for (var i = 0; i < links.length; i++) {
  if (markdown.indexOf(kind(links[i].type)) < 0 || !/^https?:/.test(links[i].href) || !home(links[i].href)) continue;
  var named = await read(links[i].href, 5000, true);
  if (named) return named;
  break;
}
var guess = new URL(location.href);
guess.hash = '';
if (/\.(md|markdown)$/i.test(guess.pathname)) return null;
guess.pathname += /\/$/.test(guess.pathname) ? 'index.html.md' : '.md';
return await read(guess.href, 1500, false);
"#;

// MARK: - Markdown, in its smallest form (Search's Markdown.swift)

/// A twin turned into HTML: its first heading lifted out as the title.
#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub title: Option<String>,
    pub html: String,
}

/// Markdown to the HTML reading mode shows. `CommonMark`'s everyday shapes and the
/// GitHub additions people actually write (tables, strikethrough, task lists);
/// four spaces is not a code block, as in the MDX most twins are written in.
/// Every piece of text is escaped, and an address has to be a web, mail or
/// on-page one (`javascript:` and its kind never get through).
#[must_use]
pub fn markdown_page(text: &str, base: &str) -> Page {
    let mut lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .trim_start_matches('\u{feff}')
        .split('\n')
        .map(str::to_owned)
        .collect();
    let mut title = front_matter(&mut lines);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut html = String::new();
    render_blocks(&refs, base, true, &mut title, &mut html);
    Page {
        title: title.filter(|t| !t.is_empty()),
        html,
    }
}

/// YAML at the very top: only its title is of any use here.
fn front_matter(lines: &mut Vec<String>) -> Option<String> {
    if lines.first().map(|l| l.trim()) != Some("---") || lines.len() < 2 || !lines[1].contains(':')
    {
        return None;
    }
    let end = lines
        .iter()
        .skip(1)
        .position(|l| matches!(l.trim(), "---" | "..."))?
        + 1;
    let mut title = None;
    for line in &lines[1..end] {
        if let Some(value) = line.strip_prefix("title:") {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            title = Some(value.to_owned());
        }
    }
    lines.drain(0..=end);
    title
}

fn render_blocks(
    lines: &[&str],
    base: &str,
    top: bool,
    title: &mut Option<String>,
    out: &mut String,
) {
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if let Some(fence) = fence_of(trimmed) {
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1; // the closing fence, or the end
            out.push_str("<pre><code>");
            out.push_str(&escape(&body.join("\n")));
            out.push_str("</code></pre>\n");
            continue;
        }
        if trimmed.starts_with("<!--") {
            while i < lines.len() && !lines[i].contains("-->") {
                i += 1;
            }
            i += 1;
            continue;
        }
        if let Some((level, text)) = atx(trimmed) {
            // The first top-level heading names the page, as Search lifts it out.
            if top && level == 1 && title.is_none() {
                *title = Some(plain(text));
                i += 1;
                continue;
            }
            let _ = writeln!(out, "<h{level}>{}</h{level}>", inline(text, base));
            i += 1;
            continue;
        }
        if is_rule(trimmed) {
            out.push_str("<hr>\n");
            i += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            let mut inner: Vec<String> = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let rest = lines[i].trim_start().strip_prefix('>').unwrap_or_default();
                inner.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
                i += 1;
            }
            // A note left at the top for agents says nothing to a person reading.
            if inner.iter().any(|l| l.contains("llms.txt")) {
                continue;
            }
            let refs: Vec<&str> = inner.iter().map(String::as_str).collect();
            let mut quoted = String::new();
            render_blocks(&refs, base, false, title, &mut quoted);
            let _ = writeln!(out, "<blockquote>{quoted}</blockquote>");
            continue;
        }
        if marker(line).is_some() {
            i = render_list(lines, i, base, out);
            continue;
        }
        if trimmed.contains('|') && i + 1 < lines.len() && delimiters(lines[i + 1]).is_some() {
            i = render_table(lines, i, base, out);
            continue;
        }
        // A paragraph, until a blank line or something of its own begins.
        let mut text = vec![line.trim_start().to_owned()];
        i += 1;
        while i < lines.len() {
            let next = lines[i].trim();
            if next.is_empty()
                || fence_of(next).is_some()
                || atx(next).is_some()
                || is_rule(next)
                || next.starts_with('>')
                || marker(lines[i]).is_some()
            {
                break;
            }
            text.push(lines[i].trim_start().to_owned());
            i += 1;
        }
        let html = inline(&text.join("\n"), base);
        if !html.trim().is_empty() {
            let _ = writeln!(out, "<p>{html}</p>");
        }
    }
}

fn fence_of(bare: &str) -> Option<&'static str> {
    ["```", "~~~"]
        .into_iter()
        .find(|mark| bare.starts_with(mark))
}

fn atx(bare: &str) -> Option<(usize, &str)> {
    let level = bare.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &bare[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

fn is_rule(bare: &str) -> bool {
    let Some(mark) = bare.chars().next() else {
        return false;
    };
    if !"-*_".contains(mark) {
        return false;
    }
    let marks: Vec<char> = bare.chars().filter(|c| *c != ' ').collect();
    marks.len() >= 3 && marks.iter().all(|c| *c == mark)
}

struct Marker {
    ordered: bool,
    start: u64,
    content: usize,
}

fn marker(line: &str) -> Option<Marker> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if "-*+".contains(first) {
        let after = &rest[1..];
        if !after.is_empty() && !after.starts_with(' ') {
            return None;
        }
        let spaces = after.len() - after.trim_start_matches(' ').len();
        return Some(Marker {
            ordered: false,
            start: 1,
            content: indent + 1 + spaces.max(1),
        });
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let after = &rest[digits.len()..];
    let delimiter = after.chars().next()?;
    if delimiter != '.' && delimiter != ')' {
        return None;
    }
    let after = &after[1..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let spaces = after.len() - after.trim_start_matches(' ').len();
    Some(Marker {
        ordered: true,
        start: digits.parse().unwrap_or(1),
        content: indent + digits.len() + 1 + spaces.max(1),
    })
}

fn render_list(lines: &[&str], begin: usize, base: &str, out: &mut String) -> usize {
    let Some(first) = marker(lines[begin]) else {
        return begin;
    };
    let mut items: Vec<(Option<bool>, String)> = Vec::new();
    let mut i = begin;
    while i < lines.len() {
        let Some(item) = marker(lines[i]) else { break };
        if item.ordered != first.ordered {
            break;
        }
        let mut text = lines[i][item.content.min(lines[i].len())..].to_owned();
        let mut check = None;
        for (box_, state) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(rest) = text.strip_prefix(box_) {
                check = Some(state);
                // `rest` is the text after the box: what goes is the box.
                let box_at = text.len() - rest.len();
                text.drain(..box_at);
            }
        }
        items.push((check, text));
        i += 1;
    }
    let tag = if first.ordered { "ol" } else { "ul" };
    let from = if first.ordered && first.start != 1 {
        format!(" start=\"{}\"", first.start)
    } else {
        String::new()
    };
    let _ = write!(out, "<{tag}{from}>");
    for (check, text) in items {
        let box_ = match check {
            Some(true) => "<input type=\"checkbox\" checked disabled> ",
            Some(false) => "<input type=\"checkbox\" disabled> ",
            None => "",
        };
        let _ = write!(out, "<li>{box_}{}</li>", inline(&text, base));
    }
    let _ = writeln!(out, "</{tag}>");
    i
}

fn delimiters(line: &str) -> Option<Vec<Option<&'static str>>> {
    if !line.contains('|') {
        return None;
    }
    let cells = row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::new();
    for cell in &cells {
        let bare = cell.trim();
        if bare.is_empty() || !bare.chars().all(|c| matches!(c, ':' | '-')) || !bare.contains('-') {
            return None;
        }
        aligns.push(match (bare.starts_with(':'), bare.ends_with(':')) {
            (true, true) => Some("center"),
            (false, true) => Some("right"),
            (true, false) => Some("left"),
            _ => None,
        });
    }
    Some(aligns)
}

fn row(line: &str) -> Vec<String> {
    let bare = line.trim().trim_start_matches('|').trim_end_matches('|');
    bare.split('|').map(|cell| cell.trim().to_owned()).collect()
}

fn render_table(lines: &[&str], begin: usize, base: &str, out: &mut String) -> usize {
    let head = row(lines[begin]);
    let aligns = delimiters(lines[begin + 1]).unwrap_or_default();
    let cells = |values: &[String], tag: &str| -> String {
        let mut out = String::new();
        for (at, align) in aligns.iter().enumerate() {
            let style = align
                .map(|a| format!(" style=\"text-align:{a}\""))
                .unwrap_or_default();
            let value = values.get(at).map(String::as_str).unwrap_or_default();
            let _ = write!(out, "<{tag}{style}>{}</{tag}>", inline(value, base));
        }
        out
    };
    let mut body = String::new();
    let mut i = begin + 2;
    while i < lines.len() && !lines[i].trim().is_empty() {
        let _ = write!(body, "<tr>{}</tr>", cells(&row(lines[i]), "td"));
        i += 1;
    }
    let _ = writeln!(
        out,
        "<div class=\"office-wide\"><table><thead><tr>{}</tr></thead><tbody>{body}</tbody></table></div>",
        cells(&head, "th")
    );
    i
}

/// Text with the markup taken off, for a title.
fn plain(text: &str) -> String {
    let html = inline(text, "");
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").trim().to_owned()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// An address from the Markdown, made whole against the twin's own and kept only
/// if it goes somewhere a link should (Search's `Markdown.address`).
fn address(raw: &str, base: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('#') {
        return Some(raw.to_owned());
    }
    let lowered = raw.to_lowercase();
    for scheme in ["http://", "https://", "mailto:"] {
        if lowered.starts_with(scheme) {
            return Some(raw.to_owned());
        }
    }
    if raw.contains(':') {
        return None; // javascript:, data:, and every other scheme
    }
    // Relative to the twin: against its folder, its root, or its scheme.
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    if let Some(stripped) = raw.strip_prefix("//") {
        return Some(format!("{scheme}://{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix('/') {
        return Some(format!("{scheme}://{host}/{stripped}"));
    }
    let folder = path.rsplit_once('/').map_or("", |(head, _)| head);
    let folder = if folder.is_empty() {
        folder.to_owned()
    } else {
        format!("{folder}/")
    };
    Some(format!("{scheme}://{host}/{folder}{raw}"))
}

/// Inline markup: code spans first (held aside, so a mark inside one is never read
/// as one), then pictures, links, and emphasis over what is left.
fn inline(text: &str, base: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut i = 0;
    let keep = |kept: &mut Vec<String>, html: String| -> String {
        kept.push(html);
        format!("\u{e000}{}\u{e001}", kept.len() - 1)
    };
    while i < chars.len() {
        match chars[i] {
            '`' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '`' {
                    j += 1;
                }
                if j < chars.len() {
                    let code: String = chars[i + 1..j].iter().collect();
                    out.push_str(&keep(&mut kept, format!("<code>{}</code>", escape(&code))));
                    i = j + 1;
                } else {
                    out.push('`');
                    i += 1;
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((alt, href, next)) = link_at(&chars, i + 1) {
                    if let Some(source) = address(&href, base) {
                        out.push_str(&keep(
                            &mut kept,
                            format!(
                                "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
                                escape(&source),
                                escape(&plain(&alt))
                            ),
                        ));
                    }
                    i = next;
                } else {
                    out.push('!');
                    i += 1;
                }
            }
            '[' => {
                if let Some((label, href, next)) = link_at(&chars, i) {
                    let inner = inline(&label, base);
                    match address(&href, base) {
                        Some(target) => out.push_str(&keep(
                            &mut kept,
                            format!("<a href=\"{}\">{inner}</a>", escape(&target)),
                        )),
                        None => out.push_str(&keep(&mut kept, inner)),
                    }
                    i = next;
                } else {
                    out.push('[');
                    i += 1;
                }
            }
            '\n' => {
                out.push('\n');
                i += 1;
            }
            c => {
                out.push_str(&escape(&c.to_string()));
                i += 1;
            }
        }
    }
    let mut out = emphasis(&out);
    for (at, html) in kept.iter().enumerate().rev() {
        out = out.replace(&format!("\u{e000}{at}\u{e001}"), html);
    }
    out
}

/// `[text](address)` at the opening bracket: the label, the address, and where the
/// scanner stands after it.
fn link_at(chars: &[char], at: usize) -> Option<(String, String, usize)> {
    if chars.get(at) != Some(&'[') {
        return None;
    }
    let mut depth = 1;
    let mut label = String::new();
    let mut i = at + 1;
    while i < chars.len() {
        match chars[i] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        if depth > 0 {
            label.push(chars[i]);
        }
        i += 1;
    }
    if depth != 0 || chars.get(i + 1) != Some(&'(') {
        return None;
    }
    let mut href = String::new();
    let mut j = i + 2;
    while j < chars.len() && chars[j] != ')' {
        href.push(chars[j]);
        j += 1;
    }
    if j >= chars.len() {
        return None;
    }
    // A title after the address is not the address.
    let href = href
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|c| c == '<' || c == '>')
        .to_owned();
    Some((label, href, j + 1))
}

fn emphasis(text: &str) -> String {
    let mut out = text.to_owned();
    for (mark, open, close) in [
        ("**", "<strong>", "</strong>"),
        ("~~", "<del>", "</del>"),
        ("*", "<em>", "</em>"),
        ("_", "<em>", "</em>"),
    ] {
        out = emphasise(&out, mark, open, close);
    }
    out
}

fn emphasise(text: &str, mark: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(mark) {
        let after = &rest[start + mark.len()..];
        let Some(end) = after.find(mark).filter(|e| *e > 0) else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(open);
        out.push_str(&after[..end]);
        out.push_str(close);
        rest = &after[end + mark.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_follows_only_when_asked() {
        assert_eq!(look_of(false, Some("dark")), "light");
        assert_eq!(look_of(true, None), "system");
        assert_eq!(look_of(true, Some("dark")), "dark");
        assert_eq!(look_of(true, Some("bogus")), "system");
    }

    #[test]
    fn markdown_lifts_the_first_heading_as_title() {
        let page = markdown_page(
            "# Hello *there*\n\nSome **bold** text.\n",
            "https://ex.com/a/b.md",
        );
        assert_eq!(page.title.as_deref(), Some("Hello there"));
        assert!(page.html.contains("<strong>bold</strong>"), "{}", page.html);
        assert!(!page.html.contains("<h1>"), "{}", page.html);
    }

    #[test]
    fn markdown_escapes_text_and_drops_bad_addresses() {
        let page = markdown_page(
            "<script>x</script> [click](javascript:alert(1)) [ok](/guide)",
            "https://ex.com/a/b.md",
        );
        assert!(!page.html.contains("<script>"), "{}", page.html);
        assert!(page.html.contains("&lt;script&gt;"), "{}", page.html);
        assert!(!page.html.contains("javascript:"), "{}", page.html);
        assert!(
            page.html.contains("href=\"https://ex.com/guide\""),
            "{}",
            page.html
        );
    }

    #[test]
    fn markdown_code_list_and_table() {
        let page = markdown_page(
            "```\na < b\n```\n\n- [x] done\n- two\n\n| A | B |\n| :- | -: |\n| 1 | 2 |\n",
            "https://ex.com/x.md",
        );
        assert!(
            page.html.contains("<pre><code>a &lt; b</code></pre>"),
            "{}",
            page.html
        );
        assert!(page.html.contains("checkbox\" checked"), "{}", page.html);
        assert!(
            page.html.contains("<td style=\"text-align:right\">2</td>"),
            "{}",
            page.html
        );
    }

    #[test]
    fn front_matter_gives_the_title() {
        let page = markdown_page(
            "---\ntitle: \"From YAML\"\n---\n\nBody.\n",
            "https://ex.com/x.md",
        );
        assert_eq!(page.title.as_deref(), Some("From YAML"));
    }

    #[test]
    fn relative_addresses_resolve_against_the_twin() {
        assert_eq!(
            address("pic.png", "https://ex.com/docs/page.md").as_deref(),
            Some("https://ex.com/docs/pic.png")
        );
        assert_eq!(
            address("#part", "https://ex.com/docs/page.md").as_deref(),
            Some("#part")
        );
        assert_eq!(address("data:text/html,x", "https://ex.com/"), None);
    }
}
