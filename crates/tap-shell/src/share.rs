//! Share and copy a page (docs/PLAN.md "Share and copy a page"): the page menu's and
//! the menu bar's Copy Page as Markdown, Copy as Markdown Link, Copy Clean Link
//! (Search's `CleanLink.swift`), Copy Link to Highlight, Share… (Search's
//! `Sharing.swift`) and Show QR Code.
//!
//! What gets written is `tap_core::share` and `tap_core::clean_link`'s; this side reads
//! the page (a script in Tap's own world, only when an item is chosen), puts the text on
//! the pasteboard and shows the system's share picker or the code's popover.

use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

use block2::RcBlock;
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSBeep, NSBitmapImageRep, NSColor, NSColorSpace, NSFont, NSImage, NSImageView,
    NSLineBreakMode, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeHTML,
    NSPasteboardTypeString, NSPasteboardTypeURL, NSPopover, NSPopoverBehavior, NSRectFill,
    NSSharingServicePicker, NSTextAlignment, NSTextField, NSUserInterfaceItemIdentification,
    NSView, NSViewController,
};
use objc2_core_image::CIFilter;
use objc2_foundation::{
    NSArray, NSData, NSObjectNSKeyValueCoding, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};
use objc2_web_kit::WKWebView;
use serde_json::Value;
use tap_core::{
    browser::{Browser, TabId},
    clean_link,
    share::{self, Quote},
};

use crate::{agent, bench, menu::Command, reader, rt, web};

thread_local! {
    /// The page menu that is up: its tab, and where it opened in the web view.
    static POINTED: RefCell<Option<(TabId, NSPoint)>> = const { RefCell::new(None) };
    /// WebKit's own Copy Link in that menu, for Copy Clean Link to run first.
    static COPY_LINK: RefCell<Option<(Option<Retained<AnyObject>>, Sel)>> =
        const { RefCell::new(None) };
    /// The code's popover while it is up.
    static POPOVER: RefCell<Option<Retained<NSPopover>>> = const { RefCell::new(None) };
    static TARGET: std::cell::OnceCell<Retained<Actions>> = const { std::cell::OnceCell::new() };
}

/// The tab's address and label, when it shows a page that can be passed on: not a
/// blank tab, not one of Tap's own pages.
fn page(browser: &Browser, tab: TabId) -> Option<(String, String)> {
    let tab = browser.tab(tab)?;
    (!tab.is_blank() && !tab.is_internal() && !tab.url().is_empty())
        .then(|| (tab.url().to_owned(), tab.label()))
}

fn page_of(tab: TabId) -> Option<(String, String)> {
    rt::read(|browser| page(browser, tab)).flatten()
}

fn is_web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// Whether a menu bar command can act on the tab in front.
pub fn can_run(tag: isize) -> Option<bool> {
    let front = || rt::read(|browser| browser.active().and_then(|id| page(browser, id))).flatten();
    match tag {
        t if t == Command::CopyCleanLink as isize
            || t == Command::CopyMarkdownLink as isize
            || t == Command::CopyPageMarkdown as isize
            || t == Command::Share as isize =>
        {
            Some(front().is_some())
        }
        t if t == Command::CopyHighlightLink as isize || t == Command::QrCode as isize => {
            Some(front().is_some_and(|(url, _)| is_web(&url)))
        }
        _ => None,
    }
}

/// Runs a menu bar command on the tab in front; whether the tag was one.
pub fn run(tag: isize) -> bool {
    let commands = [
        Command::CopyCleanLink,
        Command::CopyMarkdownLink,
        Command::CopyPageMarkdown,
        Command::CopyHighlightLink,
        Command::Share,
        Command::QrCode,
    ];
    let Some(&command) = commands.iter().find(|c| **c as isize == tag) else {
        return false;
    };
    let Some(tab) = rt::read(Browser::active).flatten() else {
        return true;
    };
    match command {
        Command::CopyCleanLink => copy_clean_link(tab),
        Command::CopyMarkdownLink => copy_markdown_link(tab),
        Command::CopyPageMarkdown => copy_page_markdown(tab),
        Command::CopyHighlightLink => copy_highlight_link(tab),
        Command::Share | Command::QrCode => {
            if let Some((web, at)) = front_anchor(tab) {
                if command == Command::Share {
                    share(tab, web.as_ref(), at);
                } else {
                    show_qr(tab, web.as_ref(), at);
                }
            }
        }
        _ => {}
    }
    true
}

/// With no button to open under, a picker from the menu bar opens from the top right
/// corner of the page, about where Safari keeps its Share button (Search's `share`).
fn front_anchor(tab: TabId) -> Option<(Retained<WKWebView>, NSRect)> {
    let web = web::existing(tab)?;
    let view: &NSView = web.as_ref();
    view.window()?;
    // A few points in from the corner, so the arrow points at the page rather than at
    // its edge.
    let inset = 12.0;
    let bounds = view.bounds();
    let y = if view.isFlipped() {
        inset
    } else {
        bounds.size.height - inset
    };
    let at = NSRect::new(
        NSPoint::new(bounds.size.width - inset, y),
        NSSize::new(1.0, 1.0),
    );
    Some((web, at))
}

// MARK: - The pasteboard

fn put(text: &str, html: Option<&str>) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: the system's own pasteboard type constants, valid for the app's life.
    let (plain, rich) = unsafe { (NSPasteboardTypeString, NSPasteboardTypeHTML) };
    board.setString_forType(&NSString::from_str(text), plain);
    if let Some(html) = html {
        board.setString_forType(&NSString::from_str(html), rich);
    }
}

/// ⌥⇧⌘C, and the page menu's item when the page's address has something to take out.
pub fn copy_clean_link(tab: TabId) {
    if let Some((url, _)) = page_of(tab) {
        put(&clean_link::clean(&url), None);
    }
}

/// The page as `[title](address)`, and as the title linked for apps that paste rich
/// text.
pub fn copy_markdown_link(tab: TabId) {
    if let Some((url, label)) = page_of(tab) {
        put(
            &share::markdown_link(&label, &url),
            Some(&share::html_link(&label, &url)),
        );
    }
}

/// What WebKit's own Copy Link just put on the pasteboard, cleaned (Search's
/// `CleanLink.rewrite`). Only WebKit knows which link the right-click was on, so its
/// item runs first; it may write a moment later, so this waits for the pasteboard to
/// change, for a second at most.
fn rewrite(since: isize, tries: u8) {
    let board = NSPasteboard::generalPasteboard();
    if board.changeCount() == since {
        if tries > 0 {
            bench::later(0.05, move || rewrite(since, tries - 1));
        }
        return;
    }
    // SAFETY: the system's own pasteboard type constants, valid for the app's life.
    let (url, plain) = unsafe { (NSPasteboardTypeURL, NSPasteboardTypeString) };
    let text = board
        .stringForType(url)
        .or_else(|| board.stringForType(plain))
        .map(|s| s.to_string());
    if let Some(text) = text.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
        put(&clean_link::clean(&text), None);
    }
}

// MARK: - Reading the page

/// Copy Page as Markdown: the article reading mode would show (or the whole page, when
/// none stands out), with its headings, lists, links, pictures, code and tables.
pub fn copy_page_markdown(tab: TabId) {
    let (Some((url, label)), Some(web)) = (page_of(tab), web::existing(tab)) else {
        return;
    };
    let script = format!("(async () => {{\n{}\n{TREE}\n}})()", reader::ARTICLE);
    web::evaluate_in_world(&web, &script, move |answer| {
        let Some(tree) = answer.as_ref().and_then(|a| a.get("tree")) else {
            return NSBeep();
        };
        let title = answer
            .as_ref()
            .and_then(|a| a.get("title"))
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map_or_else(|| label.clone(), str::to_owned);
        put(
            &share::page_markdown(&title, &clean_link::clean(&url), tree),
            None,
        );
    });
}

/// Copy Link to Highlight: the address of the page the words were picked on, with a
/// text fragment for them, so the link opens on them, marked (Chrome's and Safari's
/// item of the name). The words can be in a frame (the link is then the frame's own
/// page: a browser looks for a link's words in that page, never in the frames it
/// holds) or in a text field. Every frame is asked; the one with the focus answers.
pub fn copy_highlight_link(tab: TabId) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    let frames = agent::frames(tab);
    let shared = Arc::new(Mutex::new(Asking {
        answers: vec![None; frames.len() + 1],
        left: frames.len() + 1,
        done: false,
    }));
    let frames = std::iter::once(None).chain(frames.iter().map(Some));
    for (slot, frame) in frames.enumerate() {
        let shared = Arc::clone(&shared);
        web::call_in_frame(&web, frame.map(|f| &**f), QUOTE, &[], move |answer| {
            let last = {
                let mut asking = shared.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(at) = asking.answers.get_mut(slot) {
                    *at = answer;
                }
                asking.left = asking.left.saturating_sub(1);
                asking.left == 0
            };
            if last {
                finish_highlight(&shared);
            }
        });
    }
    // A frame that is gone or stuck doesn't keep the copy waiting.
    bench::later(PATIENCE, move || finish_highlight(&shared));
}

/// How long Copy Link to Highlight waits for the frames, as the agent verbs do.
const PATIENCE: f64 = 2.0;

/// Copy Link to Highlight's answers coming in: the main frame's first, then each
/// subframe's.
struct Asking {
    answers: Vec<Option<Value>>,
    left: usize,
    done: bool,
}

fn finish_highlight(shared: &Mutex<Asking>) {
    let answers = {
        let mut asking = shared.lock().unwrap_or_else(PoisonError::into_inner);
        if asking.done {
            return;
        }
        asking.done = true;
        std::mem::take(&mut asking.answers)
    };
    let Some(picked) = picked(answers.into_iter().flatten().collect()) else {
        return NSBeep();
    };
    let text = |key: &str| {
        picked
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let quote = Quote {
        prefix: text("prefix"),
        start: text("start"),
        end: text("end"),
        suffix: text("suffix"),
        anchor: text("anchor"),
    };
    match share::highlight_link(&clean_link::clean(&text("url")), &quote) {
        Some(link) => put(&link, None),
        None => NSBeep(),
    }
}

/// The frame whose words a highlight link is for: of the frames with words picked,
/// not counting one whose focus is in a frame it holds (its selection is an old one),
/// the one with the focus, else the first, the main frame before its subframes.
fn picked(answers: Vec<Value>) -> Option<Value> {
    let mut candidates: Vec<Value> = answers
        .into_iter()
        .filter(|a| {
            a.get("start")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        .collect();
    let focused = candidates
        .iter()
        .position(|a| a.get("focused").and_then(Value::as_bool) == Some(true))
        .unwrap_or(0);
    (focused < candidates.len()).then(|| candidates.swap_remove(focused))
}

/// The page as a tree for `share::page_markdown`: what is hidden, the page's chrome
/// (navigation, forms, buttons) and what can't be text are left out. After
/// `reader::ARTICLE`.
const TREE: &str = r"
var skip = /^(script|style|noscript|template|svg|canvas|iframe|object|embed|form|nav|aside|footer|button|input|select|textarea|dialog|video|audio|map|head|link|meta)$/;
var chrome = /^(navigation|banner|complementary|contentinfo|dialog|alertdialog|search|menu|menubar|toolbar)$/;
var budget = 60000;
function tree(node) {
  if (--budget < 0) return null;
  if (node.nodeType === 3) return node.nodeValue;
  if (node.nodeType !== 1) return null;
  var t = node.localName;
  if (skip.test(t) || node.hidden || node.getAttribute('aria-hidden') === 'true') return null;
  if (chrome.test(node.getAttribute('role') || '')) return null;
  if (node.checkVisibility && !node.checkVisibility()) return null;
  var out = { t: t };
  if (t === 'pre') {
    out.x = node.innerText;
    var code = node.querySelector('code');
    var lang = ((code && code.className) || node.className || '').match(/(?:language|lang)-([\w+#.-]+)/);
    if (lang) out.l = lang[1];
    return out;
  }
  if (t === 'code') { out.x = node.textContent; return out; }
  if (t === 'img') { out.s = node.currentSrc || node.src || ''; out.a = node.alt || ''; return out; }
  if (t === 'a') out.h = node.href || '';
  if (t === 'ol' && node.start > 1) out.n = node.start;
  var kids = [];
  for (var c = node.firstChild; c; c = c.nextSibling) {
    var kid = tree(c);
    if (kid !== null && kid !== '') kids.push(kid);
  }
  out.c = kids;
  return out;
}
var heading = document.querySelector('h1');
var title = (heading && heading.innerText.trim()) || document.title;
var root = findArticle() || document.body;
if (!root) return null;
return { title: title, tree: tree(root) };
";

/// The words picked, as a text directive's terms (`share::Quote`): whole words only, as
/// browsers match them; a short run in one block as it is, a longer one as its first
/// and last words; and the words around it when its start is found elsewhere too.
const QUOTE: &str = r"(async () => {
  var focus = document.activeElement;
  // The focus is in a frame this page holds: its words are that frame's to give.
  if (focus && /^(iframe|frame)$/.test(focus.localName)) return null;
  var seg = (typeof Intl !== 'undefined' && Intl.Segmenter) ? new Intl.Segmenter(undefined, { granularity: 'word' }) : null;
  function squash(s) { return s.replace(/\s+/g, ' ').trim(); }
  function words(s) {
    var out = [];
    if (seg) { for (var x of seg.segment(s)) if (x.isWordLike) out.push([x.index, x.index + x.segment.length]); }
    else { var re = /\S+/g, m; while ((m = re.exec(s))) out.push([m.index, m.index + m[0].length]); }
    return out;
  }
  function head(s, n) { var w = words(s); return w.length <= n ? s : s.slice(0, w[n - 1][1]); }
  function tail(s, n) { var w = words(s); return w.length <= n ? s : s.slice(w[w.length - n][0]); }
  function block(node) {
    var el = node.nodeType === 1 ? node : node.parentElement;
    while (el && el !== document.body) {
      var shown = getComputedStyle(el).display || '';
      if (shown.indexOf('inline') !== 0 && shown !== 'contents') return el;
      el = el.parentElement;
    }
    return document.body;
  }
  // The words picked and the text either side of them in their block: a text field's
  // value, or the page's text.
  var lead, chosen, trail, anchor = '', page = '';
  var from = null, to = null;
  if (focus && /^(input|textarea)$/.test(focus.localName)) {
    try { from = focus.selectionStart; to = focus.selectionEnd; } catch (e) {}
  }
  // A password is never put in a link, not even through the page's selection.
  if (focus && focus.type === 'password') return null;
  if (from !== null && to !== null && from < to) {
    var value = focus.value;
    lead = value.slice(0, from).split('\n').pop();
    chosen = value.slice(from, to);
    trail = value.slice(to).split('\n')[0];
    page = value;
    for (var el = focus; el && el !== document.body; el = el.parentElement) {
      if (el.id) { anchor = el.id; break; }
    }
  } else {
    var picked = getSelection();
    if (!picked || picked.isCollapsed || !picked.rangeCount) return null;
    var range = picked.getRangeAt(0);
    var before = document.createRange();
    before.setStart(block(range.startContainer), 0);
    before.setEnd(range.startContainer, range.startOffset);
    var after = document.createRange();
    var last = block(range.endContainer);
    after.setStart(range.endContainer, range.endOffset);
    after.setEnd(last, last.childNodes.length);
    lead = before.toString().split('\n').pop();
    chosen = picked.toString();
    trail = after.toString().split('\n')[0];
  }
  // A word cut by the selection is a whole word in the link.
  function cut(text, at) {
    if (!seg) return null;
    for (var x of seg.segment(text)) {
      if (x.index < at && x.index + x.segment.length > at && x.isWordLike) return [x.index, x.index + x.segment.length];
    }
    return null;
  }
  var lines = chosen.split('\n');
  var whole = cut(lead + lines[0], lead.length);
  if (whole) { lines[0] = lead.slice(whole[0]) + lines[0]; lead = lead.slice(0, whole[0]); }
  var end = lines.length - 1;
  whole = cut(lines[end] + trail, lines[end].length);
  if (whole) { var more = whole[1] - lines[end].length; lines[end] += trail.slice(0, more); trail = trail.slice(more); }
  lines = lines.map(squash).filter(Boolean);
  if (!lines.length) return null;
  var quote = { prefix: '', start: '', end: '', suffix: '', anchor: anchor, url: location.href, focused: document.hasFocus() };
  if (lines.length === 1 && words(lines[0]).length <= 10) {
    quote.start = lines[0];
  } else {
    quote.start = head(lines[0], 5);
    quote.end = tail(lines[lines.length - 1], 5);
  }
  page = squash((document.body ? document.body.innerText : '') + ' ' + page).toLowerCase();
  var term = quote.start.toLowerCase();
  var first = page.indexOf(term);
  if (first >= 0 && page.indexOf(term, first + 1) >= 0) {
    quote.prefix = tail(squash(lead), 3);
    quote.suffix = head(squash(trail), 3);
  }
  return quote;
})()";

// MARK: - Share… and the code

/// The system's share picker for the page (Search's `share`): Mail, Messages, AirDrop,
/// Notes and whatever else is registered.
pub fn share(tab: TabId, view: &NSView, at: NSRect) {
    let Some(url) =
        page_of(tab).and_then(|(url, _)| NSURL::URLWithString(&NSString::from_str(&url)))
    else {
        return;
    };
    let items = NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(url))]);
    // SAFETY: a fresh picker for an array of objects that can be shared.
    let picker =
        unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    let edge = if view.isFlipped() {
        NSRectEdge::MaxY
    } else {
        NSRectEdge::MinY
    };
    picker.showRelativeToRect_ofView_preferredEdge(at, view, edge);
}

/// Show QR Code: the page's address (its clean link) as a code a phone can open, in a
/// popover beside where it was asked for.
pub fn show_qr(tab: TabId, view: &NSView, at: NSRect) {
    let Some((url, _)) = page_of(tab).filter(|(url, _)| is_web(url)) else {
        return;
    };
    let url = clean_link::clean(&url);
    let side = 220.0;
    let Some(image) = qr_image(&url, side) else {
        return NSBeep();
    };
    let mtm = MainThreadMarker::from(view);
    let (wide, high) = (side + 32.0, side + 62.0);
    let content = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(wide, high)),
    );
    let picture = NSImageView::imageViewWithImage(&image, mtm);
    picture.setFrame(NSRect::new(
        NSPoint::new(16.0, 46.0),
        NSSize::new(side, side),
    ));
    content.addSubview(&picture);
    let shown = url
        .split_once("://")
        .map_or(url.as_str(), |(_, rest)| rest)
        .to_owned();
    let label = NSTextField::labelWithString(&NSString::from_str(&shown), mtm);
    label.setFrame(NSRect::new(
        NSPoint::new(16.0, 16.0),
        NSSize::new(side, 18.0),
    ));
    label.setAlignment(NSTextAlignment::Center);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    label.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setToolTip(Some(&NSString::from_str(&url)));
    content.addSubview(&label);
    let controller = NSViewController::new(mtm);
    controller.setView(&content);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setContentViewController(Some(&controller));
    popover.setContentSize(NSSize::new(wide, high));
    if let Some(old) = POPOVER.replace(Some(popover.clone())) {
        old.close();
    }
    let edge = if view.isFlipped() {
        NSRectEdge::MaxY
    } else {
        NSRectEdge::MinY
    };
    popover.showRelativeToRect_ofView_preferredEdge(at, view, edge);
}

/// `text` as a QR code `side` points square, its modules sharp at any scale, on white
/// with the quiet zone readers need.
fn qr_image(text: &str, side: f64) -> Option<Retained<NSImage>> {
    let modules = qr_modules(text)?;
    let count = modules.len();
    let quiet = 2.0;
    #[allow(clippy::cast_precision_loss)] // a code is at most 177 modules wide
    let unit = side / (count as f64 + quiet * 2.0);
    let draw = RcBlock::new(move |bounds: NSRect| -> Bool {
        NSColor::whiteColor().setFill();
        NSRectFill(bounds);
        NSColor::blackColor().setFill();
        for (y, row) in modules.iter().enumerate() {
            for (x, dark) in row.iter().enumerate() {
                if *dark {
                    #[allow(clippy::cast_precision_loss)] // as above
                    let origin = NSPoint::new((x as f64 + quiet) * unit, (y as f64 + quiet) * unit);
                    NSRectFill(NSRect::new(origin, NSSize::new(unit, unit)));
                }
            }
        }
        Bool::YES
    });
    // Flipped, so the first row of modules is the top one, as read from the bitmap.
    Some(NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(side, side),
        true,
        &draw,
    ))
}

/// The code's modules, top row first, from Core Image's generator: one pixel each.
fn qr_modules(text: &str) -> Option<Vec<Vec<bool>>> {
    // SAFETY: Core Image's own generator, set through its documented input keys.
    let image = unsafe {
        let filter = CIFilter::filterWithName(&NSString::from_str("CIQRCodeGenerator"))?;
        let data = NSData::with_bytes(text.as_bytes());
        filter.setValue_forKey(Some(&data), &NSString::from_str("inputMessage"));
        filter.setValue_forKey(
            Some(&NSString::from_str("M")),
            &NSString::from_str("inputCorrectionLevel"),
        );
        filter.outputImage()?
    };
    let rep = NSBitmapImageRep::initWithCIImage(NSBitmapImageRep::alloc(), &image);
    let (wide, high) = (rep.pixelsWide(), rep.pixelsHigh());
    if wide <= 0 || wide != high {
        return None;
    }
    let space = NSColorSpace::deviceRGBColorSpace();
    let rows = (0..high)
        .map(|y| {
            (0..wide)
                .map(|x| {
                    rep.colorAtX_y(x, y)
                        .and_then(|c| c.colorUsingColorSpace(&space))
                        .is_some_and(|c| c.brightnessComponent() < 0.5)
                })
                .collect()
        })
        .collect();
    Some(rows)
}

// MARK: - The page menu

define_class!(
    /// The page menu's share items' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapShareActions"]
    struct Actions;

    unsafe impl NSObjectProtocol for Actions {}

    impl Actions {
        #[unsafe(method(copyPageImage:))]
        fn copy_page_image(&self, _sender: Option<&AnyObject>) {
            if let Some(web) = pointed().and_then(|(id, _)| web::existing(id)) {
                crate::page_image::copy(&web);
            }
        }

        #[unsafe(method(copyPageMarkdown:))]
        fn copy_page_markdown(&self, _sender: Option<&AnyObject>) {
            if let Some((id, _)) = pointed() {
                copy_page_markdown(id);
            }
        }

        #[unsafe(method(copyMarkdownLink:))]
        fn copy_markdown_link(&self, _sender: Option<&AnyObject>) {
            if let Some((id, _)) = pointed() {
                copy_markdown_link(id);
            }
        }

        #[unsafe(method(copyCleanPage:))]
        fn copy_clean_page(&self, _sender: Option<&AnyObject>) {
            if let Some((id, _)) = pointed() {
                copy_clean_link(id);
            }
        }

        /// WebKit's Copy Link, then what it copied, cleaned.
        #[unsafe(method(copyCleanLink:))]
        fn copy_clean_link(&self, sender: Option<&AnyObject>) {
            let Some((target, action)) = COPY_LINK.with_borrow(Clone::clone) else {
                return;
            };
            let since = NSPasteboard::generalPasteboard().changeCount();
            // SAFETY: WebKit's own item's action and target, sent from the copy of it
            // that carries the same represented object, as AppKit would send it.
            unsafe {
                NSApplication::sharedApplication(self.mtm()).sendAction_to_from(
                    action,
                    target.as_deref(),
                    sender,
                );
            }
            rewrite(since, 20);
        }

        #[unsafe(method(copyHighlightLink:))]
        fn copy_highlight_link(&self, _sender: Option<&AnyObject>) {
            if let Some((id, _)) = pointed() {
                copy_highlight_link(id);
            }
        }

        #[unsafe(method(sharePage:))]
        fn share_page(&self, _sender: Option<&AnyObject>) {
            if let Some((id, web, at)) = pointed_view() {
                share(id, web.as_ref(), at);
            }
        }

        #[unsafe(method(showQrCode:))]
        fn show_qr_code(&self, _sender: Option<&AnyObject>) {
            if let Some((id, web, at)) = pointed_view() {
                show_qr(id, web.as_ref(), at);
            }
        }
    }
);

fn pointed() -> Option<(TabId, NSPoint)> {
    POINTED.with_borrow(Clone::clone)
}

/// The page menu's tab, its web view, and a point-sized rect where the menu opened.
fn pointed_view() -> Option<(TabId, Retained<WKWebView>, NSRect)> {
    let (id, at) = pointed()?;
    let web = web::existing(id)?;
    Some((id, web, NSRect::new(at, NSSize::new(1.0, 1.0))))
}

fn actions(mtm: MainThreadMarker) -> Retained<Actions> {
    TARGET.with(|target| {
        target
            .get_or_init(|| {
                let this = Actions::alloc(mtm).set_ivars(());
                // SAFETY: NSObject's designated initialiser.
                unsafe { msg_send![super(this), init] }
            })
            .clone()
    })
}

fn entry(mtm: MainThreadMarker, title: &str, action: Sel) -> Retained<NSMenuItem> {
    // SAFETY: a fresh allocation; the actions take the sender.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    };
    // SAFETY: the target lives for the app's life, longer than the menu.
    unsafe { item.setTarget(Some(&actions(mtm))) };
    item
}

/// The index of WebKit's item with this identifier in the menu.
fn webkit_item(menu: &NSMenu, identifier: &str) -> Option<isize> {
    let items = menu.itemArray();
    (0..items.count()).find_map(|i| {
        let item = items.objectAtIndex(i);
        let id = item.identifier()?;
        (id.to_string() == identifier).then(|| isize::try_from(i).ok())?
    })
}

/// The page menu's share items, for the tab `id`, the menu opened at `at` in `web`.
/// Beside WebKit's own Copy Link, Copy Clean Link (Search's place for it); beside its
/// Copy of a selection, Copy Link to Highlight where WebKit has none of its own; then a
/// group of the page's own.
pub fn add_items(web: &WKWebView, menu: &NSMenu, id: TabId, at: NSPoint) {
    let mtm = MainThreadMarker::from(web);
    POINTED.replace(Some((id, at)));
    COPY_LINK.replace(None);
    let page = page_of(id);
    if let Some(index) = webkit_item(menu, "WKMenuItemIdentifierCopyLink") {
        let original = menu.itemAtIndex(index);
        // SAFETY: NSMenuItem is NSCopying; the copy keeps WebKit's represented object
        // and tag, which its action reads off the sender.
        let clean: Option<Retained<NSMenuItem>> =
            original.map(|item| unsafe { msg_send![&*item, copy] });
        if let (Some(original), Some(clean)) = (menu.itemAtIndex(index), clean) {
            let (target, action) = (original.target(), original.action());
            if let Some(action) = action {
                COPY_LINK.replace(Some((target, action)));
                clean.setTitle(&NSString::from_str("Copy Clean Link"));
                clean.setIdentifier(Some(&NSString::from_str("TapCopyCleanLink")));
                // SAFETY: the target lives for the app's life, longer than the menu.
                unsafe {
                    clean.setTarget(Some(&actions(mtm)));
                    clean.setAction(Some(sel!(copyCleanLink:)));
                }
                menu.insertItem_atIndex(&clean, index + 1);
            }
        }
    }
    // WebKit has its own (Copy Link with Highlight) from macOS 15 on: then that one.
    if page.as_ref().is_some_and(|(url, _)| is_web(url))
        && webkit_item(menu, "WKMenuItemIdentifierCopyLinkWithHighlight").is_none()
        && let Some(index) = webkit_item(menu, "WKMenuItemIdentifierCopy")
    {
        menu.insertItem_atIndex(
            &entry(mtm, "Copy Link to Highlight", sel!(copyHighlightLink:)),
            index + 1,
        );
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&entry(mtm, "Copy Page as Image", sel!(copyPageImage:)));
    let Some((url, _)) = page else { return };
    menu.addItem(&entry(
        mtm,
        "Copy Page as Markdown",
        sel!(copyPageMarkdown:),
    ));
    menu.addItem(&entry(
        mtm,
        "Copy as Markdown Link",
        sel!(copyMarkdownLink:),
    ));
    // On the page itself, only when its own address has something to take out and the
    // menu isn't a link's, as Safari does (Search's `copyCleanPage`).
    if !COPY_LINK.with_borrow(Option::is_some) && clean_link::has_tracking(&url) {
        menu.addItem(&entry(mtm, "Copy Clean Link", sel!(copyCleanPage:)));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // "Page": a link's or a selection's menu has WebKit's own Share… for those.
    menu.addItem(&entry(mtm, "Share Page…", sel!(sharePage:)));
    if is_web(&url) {
        menu.addItem(&entry(mtm, "Show QR Code", sel!(showQrCode:)));
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_image::{CIDetector, CIDetectorTypeQRCode, CIImage, CIQRCodeFeature};

    use super::*;

    /// The code drawn reads back as the address: rows the right way up, modules sharp.
    #[test]
    fn the_code_reads_back_as_the_address() {
        let url = "https://example.com/a/long/path?q=中文&x=1#part";
        let image = qr_image(url, 220.0).expect("a code");
        let tiff = image.TIFFRepresentation().expect("a bitmap");
        // SAFETY: Core Image's QR reader over a picture made just now.
        let read = unsafe {
            let picture = CIImage::imageWithData(&tiff).expect("a picture");
            let detector =
                CIDetector::detectorOfType_context_options(CIDetectorTypeQRCode, None, None)
                    .expect("a reader");
            let features = detector.featuresInImage(&picture);
            assert_eq!(features.count(), 1);
            let feature = features.objectAtIndex(0);
            let code: &CIQRCodeFeature = feature.downcast_ref().expect("a QR code");
            code.messageString().map(|s| s.to_string())
        };
        assert_eq!(read.as_deref(), Some(url));
    }
}
