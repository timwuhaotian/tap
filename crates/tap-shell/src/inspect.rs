//! Handing a page to an agent (docs/PLAN.md "Hand a page to an agent"): Copy Tab
//! Details and Copy Element Details put on the pasteboard what lets an agent find this
//! exact tab with `tap` (its id, which outlasts a restart) and what it is looking at —
//! the page's own facts, or one element's selector, box, style and markup. `tap info`
//! answers with the same card.
//!
//! The page is read only when someone asks: a menu item, or `tap info`. Nothing here
//! runs on a timer (every script the app runs in a page ends its user activation).
//! The script runs in Tap's own content world; the recorder there keeps the element
//! the last right-click was on, so a menu item can still name it after the menu is up.

use std::{cell::RefCell, collections::HashMap, fmt::Write as _};

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{NSEvent, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSView};
use objc2_foundation::{NSPoint, NSRect, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use serde_json::Value;
use tap_core::browser::{Browser, TabId};

use crate::{rt, web};

/// Kept at each document's start in Tap's world, main frame only: the element a
/// right-click was on and where, read back by the script below. Listening only;
/// nothing is sent anywhere.
pub const RECORDER: &str = "window.addEventListener('contextmenu', function (e) {\n  \
     window.__tapPointed = { target: (e.composedPath && e.composedPath()[0]) || e.target, \
     x: e.clientX, y: e.clientY };\n}, true);";

/// The reading script, as one expression.
const SCRIPT: &str = concat!("(async () => {\n", include_str!("inspect.js"), "\n})()");

thread_local! {
    /// The main document's HTTP status, per tab, from its last navigation's answer.
    static STATUS: RefCell<HashMap<TabId, i64>> = RefCell::new(HashMap::new());
    /// The tab and point (CSS pixels) of the page menu that is up.
    static POINTED: RefCell<Option<(TabId, f64, f64)>> = const { RefCell::new(None) };
    static TARGET: std::cell::OnceCell<Retained<Actions>> = const { std::cell::OnceCell::new() };
}

/// A main-frame answer's status, for the card.
pub fn note_status(tab: TabId, status: i64) {
    STATUS.with_borrow_mut(|map| map.insert(tab, status));
}

/// The tab's page went: its status goes with it.
pub fn forget(tab: TabId) {
    STATUS.with_borrow_mut(|map| map.remove(&tab));
}

/// What the model knows about a tab, read in one borrow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub id: u64,
    /// What the strip shows: the name given, or the page's title.
    pub label: String,
    pub title: String,
    pub url: String,
    pub state: String,
    pub secure: bool,
    pub pinned: bool,
    /// Its group's name and colour.
    pub group: Option<(String, Option<String>)>,
    /// Its place in the row, from 1, and the row's length.
    pub place: usize,
    pub count: usize,
    pub front: bool,
    pub status: Option<i64>,
}

fn facts(browser: &Browser, id: TabId) -> Option<Facts> {
    let tab = browser.tab(id)?;
    let state = if let Some(failure) = tab.failure() {
        format!("failed: {failure}")
    } else if !tab.awake() {
        "asleep (tap text or tap wait wakes it)".to_owned()
    } else if tab.loading() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=100
        let percent = (tab.progress() * 100.0).round() as u32;
        format!("loading {percent}%")
    } else {
        "loaded".to_owned()
    };
    let group = tab.group().and_then(|g| {
        browser
            .groups()
            .iter()
            .find(|x| x.id == g)
            .map(|x| (x.name.clone(), x.colour.clone()))
    });
    Some(Facts {
        id: id.0,
        label: tab.label(),
        title: tab.title().to_owned(),
        url: tab.url().to_owned(),
        state,
        secure: tab.secure(),
        pinned: tab.is_pinned(),
        group,
        place: browser
            .tabs()
            .iter()
            .position(|t| t.id == id)
            .map_or(0, |i| i + 1),
        count: browser.tabs().len(),
        front: browser.active() == Some(id),
        status: STATUS.with_borrow(|map| map.get(&id).copied()),
    })
}

/// A value from the page's answer, as text, when it says anything.
fn said(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The query parameters whose names say they carry a secret: worth a warning before
/// the address is pasted anywhere.
#[must_use]
pub fn secret_params(url: &str) -> Vec<String> {
    const WORDS: [&str; 10] = [
        "token", "code", "key", "secret", "password", "sig", "auth", "session", "jwt", "otp",
    ];
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = query.split('#').next().unwrap_or_default();
    query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            WORDS.iter().any(|w| lower == *w || lower.contains(w))
        })
        .map(str::to_owned)
        .collect()
}

/// A shell word: single-quoted, a quote inside closed and reopened around `\'`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn heading(facts: &Facts) -> String {
    let mut line = format!("Tap tab {} — {}", facts.id, facts.label);
    if facts.label != facts.title && !facts.title.is_empty() {
        let _ = write!(line, " (page title: {})", facts.title);
    }
    line
}

/// Copy Tab Details' card: the tab as the model has it, then what its page says of
/// itself (when it has a page awake to ask), then how to reach it with `tap`.
#[must_use]
pub fn tab_card(facts: &Facts, page: Option<&Value>) -> String {
    let mut lines = vec![heading(facts)];
    lines.push(format!("URL: {}", facts.url));
    let secret = secret_params(&facts.url);
    if !secret.is_empty() {
        lines.push(format!(
            "Careful: the URL carries {} — remove it before sharing further",
            secret.join(", ")
        ));
    }
    let mut state = facts.state.clone();
    if let Some(status) = facts.status {
        let _ = write!(state, " · HTTP {status}");
    }
    if facts.url.starts_with("https:") || facts.url.starts_with("http:") {
        state.push_str(if facts.secure {
            " · secure"
        } else {
            " · not secure"
        });
    }
    lines.push(format!("State: {state}"));
    let mut place = if facts.pinned {
        "pinned".to_owned()
    } else if let Some((name, colour)) = &facts.group {
        colour.as_deref().map_or_else(
            || format!("group \"{name}\""),
            |c| format!("group \"{name}\" ({c})"),
        )
    } else {
        "no group".to_owned()
    };
    let _ = write!(place, " · tab {} of {}", facts.place, facts.count);
    if facts.front {
        place.push_str(" · in front");
    }
    lines.push(format!("Place: {place}"));
    if let Some(page) = page {
        if let Some(url) = said(page, "url").filter(|u| *u != facts.url) {
            lines.push(format!("Page URL: {url}"));
        }
        let kind: Vec<String> = [
            said(page, "lang").map(|l| format!("lang {l}")),
            said(page, "contentType"),
            said(page, "charset"),
            said(page, "readyState").map(|r| format!("readyState {r}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !kind.is_empty() {
            lines.push(format!("Page: {}", kind.join(" · ")));
        }
        for (key, name) in [
            ("canonical", "Canonical"),
            ("description", "Description"),
            ("ogTitle", "og:title"),
            ("ogType", "og:type"),
            ("ogSite", "og:site_name"),
            ("generator", "Generator"),
            ("referrer", "Referrer"),
        ] {
            if let Some(value) = said(page, key) {
                lines.push(format!("{name}: {value}"));
            }
        }
        if let Some(view) = page.get("viewport") {
            let n = |k: &str| view.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            // A tab behind the one in front has no size: its page isn't laid out on screen.
            let size = if n("width") > 0.0 {
                format!("{}×{} @{}x", n("width"), n("height"), n("dpr"))
            } else {
                "not on screen (a tab behind)".to_owned()
            };
            lines.push(format!(
                "Viewport: {size} · scrolled to {},{} of height {}",
                n("x"),
                n("y"),
                n("scrollHeight")
            ));
        }
        if let Some(frames) = page
            .get("frames")
            .and_then(Value::as_u64)
            .filter(|f| *f > 0)
        {
            lines.push(format!("Frames: {frames}"));
        }
        if let Some(selection) = said(page, "selection") {
            lines.push(format!("Selection: \"{selection}\""));
        }
    }
    let id = facts.id;
    lines.push(format!(
        "Reach it with Tap: tap info {id} · tap text {id} · tap snapshot {id} · tap eval {id} '<js>'"
    ));
    lines.join("\n")
}

/// Copy Element Details' card: which tab, then the element.
#[must_use]
pub fn element_card(facts: &Facts, element: &Value) -> String {
    let mut lines = vec![
        format!("Element in {}", heading(facts)),
        format!("URL: {}", facts.url),
    ];
    let selector = said(element, "selector").unwrap_or_default();
    if let Some(host) = said(element, "shadowHost") {
        lines.push(format!("Inside the shadow root of: {host}"));
    }
    lines.push(format!("Selector: {selector}"));
    if element.get("frame").and_then(Value::as_bool) == Some(true) {
        lines.push(
            "Note: this is an iframe; the element clicked is inside it, out of the page's reach"
                .to_owned(),
        );
    }
    for (key, name) in [
        ("tag", "Tag"),
        ("role", "Role"),
        ("name", "Name"),
        ("text", "Text"),
        ("value", "Value"),
        ("href", "Link"),
        ("src", "Source"),
    ] {
        if let Some(value) = said(element, key) {
            lines.push(format!("{name}: {value}"));
        }
    }
    if let Some(b) = element.get("box") {
        let n = |k: &str| b.get(k).and_then(Value::as_i64).unwrap_or(0);
        lines.push(format!(
            "Box: {}×{} at x {}, y {} in the viewport (y {} on the page), CSS px",
            n("width"),
            n("height"),
            n("x"),
            n("y"),
            n("pageY")
        ));
    }
    if let Some(style) = element.get("style") {
        let parts: Vec<String> = [
            "display",
            "position",
            "font",
            "color",
            "background",
            "margin",
            "padding",
            "zIndex",
            "visibility",
            "opacity",
        ]
        .iter()
        .filter_map(|k| said(style, k).map(|v| format!("{k} {v}")))
        .collect();
        lines.push(format!("Style: {}", parts.join(" · ")));
    }
    if let Some(html) = said(element, "html") {
        lines.push(format!("HTML: {html}"));
    }
    if !selector.is_empty() && element.get("shadowHost").is_none_or(Value::is_null) {
        let js = format!(
            "document.querySelector({})?.outerHTML",
            serde_json::to_string(&selector).unwrap_or_default()
        );
        lines.push(format!(
            "Reach it with Tap: tap eval {} {} · tap snapshot {}",
            facts.id,
            shell_quote(&js),
            facts.id
        ));
    }
    lines.join("\n")
}

/// Reads the page in Tap's world and hands its answer on: `None` without one.
fn read_page(
    web: &WKWebView,
    verb: &str,
    point: (f64, f64),
    then: impl FnOnce(Option<Value>) + 'static,
) {
    let then = RefCell::new(Some(then));
    let (x, y) = (point.0.to_string(), point.1.to_string());
    web::call_in_world(
        web,
        SCRIPT,
        &[("verb", verb), ("pointX", &x), ("pointY", &y)],
        move |answer| {
            if let Some(then) = then.borrow_mut().take() {
                then(answer);
            }
        },
    );
}

/// The tab's card, its page asked only if it is awake: Copy Tab Details never wakes a
/// sleeping tab, `tap info` wakes it first (bench).
pub fn tab_details(id: TabId, then: impl FnOnce(Option<String>) + 'static) {
    let Some(facts) = rt::read(|browser| facts(browser, id)).flatten() else {
        return then(None);
    };
    let page = web::existing(id).filter(|_| {
        rt::read(|b| {
            b.tab(id)
                .is_some_and(|t| t.awake() && !t.is_blank() && !t.is_internal())
        })
        .unwrap_or(false)
    });
    match page {
        Some(web) => read_page(&web, "page", (0.0, 0.0), move |answer| {
            then(Some(tab_card(&facts, answer.as_ref())));
        }),
        None => then(Some(tab_card(&facts, None))),
    }
}

fn put_on_pasteboard(text: &str) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: the system's own pasteboard type constant, valid for the app's life.
    let kind = unsafe { NSPasteboardTypeString };
    board.setString_forType(&NSString::from_str(text), kind);
}

/// Copy Tab Details, for the strip's menu and the page's.
pub fn copy_tab(id: TabId) {
    tab_details(id, |card| {
        if let Some(card) = card {
            put_on_pasteboard(&card);
        }
    });
}

/// Copy Element Details: the element under the page menu that was opened.
fn copy_element() {
    let Some((id, x, y)) = POINTED.with_borrow(Clone::clone) else {
        return;
    };
    let (Some(web), Some(facts)) = (
        web::existing(id),
        rt::read(|browser| facts(browser, id)).flatten(),
    ) else {
        return;
    };
    read_page(&web, "element", (x, y), move |answer| {
        let card = match answer {
            Some(element) if element.get("missing").is_none() => element_card(&facts, &element),
            // Nothing there to name: the tab is still worth having.
            _ => tab_card(&facts, None),
        };
        put_on_pasteboard(&card);
    });
}

define_class!(
    /// Every tab's web view: WebKit's own, with the page menu's two items added.
    #[unsafe(super(WKWebView, NSView, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapPageView"]
    pub struct PageView;

    impl PageView {
        #[unsafe(method(willOpenMenu:withEvent:))]
        fn will_open_menu(&self, menu: &NSMenu, event: &NSEvent) {
            // SAFETY: NSView's own hook, handed on as WebKit expects.
            unsafe {
                let _: () = msg_send![super(self), willOpenMenu: menu, withEvent: event];
            }
            add_items(self, menu, event);
        }
    }
);

define_class!(
    /// The page menu items' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapInspectActions"]
    struct Actions;

    unsafe impl NSObjectProtocol for Actions {}

    impl Actions {
        #[unsafe(method(copyTabDetails:))]
        fn copy_tab_details(&self, _sender: Option<&AnyObject>) {
            if let Some((id, ..)) = POINTED.with_borrow(Clone::clone) {
                copy_tab(id);
            }
        }

        #[unsafe(method(copyElementDetails:))]
        fn copy_element_details(&self, _sender: Option<&AnyObject>) {
            copy_element();
        }
    }
);

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

/// A web view for a tab, Tap's kind.
pub fn make(mtm: MainThreadMarker, configuration: &WKWebViewConfiguration) -> Retained<WKWebView> {
    let this = PageView::alloc(mtm).set_ivars(());
    // SAFETY: WKWebView's designated initialiser, with a valid configuration.
    let view: Retained<PageView> = unsafe {
        msg_send![super(this), initWithFrame: NSRect::ZERO, configuration: configuration]
    };
    Retained::into_super(view)
}

/// Where the menu opened, in the page's CSS pixels.
fn css_point(web: &WKWebView, event: &NSEvent) -> (f64, f64) {
    let view: &NSView = web.as_ref();
    let local: NSPoint = view.convertPoint_fromView(event.locationInWindow(), None);
    let y = if view.isFlipped() {
        local.y
    } else {
        view.bounds().size.height - local.y
    };
    // SAFETY: plain property reads on a live web view.
    let scale = unsafe { web.magnification() * web.pageZoom() };
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (local.x / scale, y / scale)
}

fn add_items(view: &PageView, menu: &NSMenu, event: &NSEvent) {
    let web: &WKWebView = view.as_ref();
    let Some(id) = web::tab_of(web) else { return };
    let (x, y) = css_point(web, event);
    POINTED.replace(Some((id, x, y)));
    let mtm = MainThreadMarker::from(view);
    let target = actions(mtm);
    if menu.numberOfItems() > 0 {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    for (title, action) in [
        ("Copy Element Details", sel!(copyElementDetails:)),
        ("Copy Tab Details", sel!(copyTabDetails:)),
    ] {
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
        unsafe { item.setTarget(Some(&target)) };
        menu.addItem(&item);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn facts() -> Facts {
        Facts {
            id: 7,
            label: "Dashboard".to_owned(),
            title: "Dashboard".to_owned(),
            url: "https://app.example.com/dash?range=7d".to_owned(),
            state: "loaded".to_owned(),
            secure: true,
            pinned: false,
            group: Some(("Work".to_owned(), Some("blue".to_owned()))),
            place: 3,
            count: 12,
            front: true,
            status: Some(200),
        }
    }

    #[test]
    fn a_sleeping_tab_s_card_names_it_and_says_how_to_reach_it() {
        let card = tab_card(
            &Facts {
                state: "asleep".to_owned(),
                status: None,
                front: false,
                ..facts()
            },
            None,
        );
        assert_eq!(
            card,
            "Tap tab 7 — Dashboard\n\
             URL: https://app.example.com/dash?range=7d\n\
             State: asleep · secure\n\
             Place: group \"Work\" (blue) · tab 3 of 12\n\
             Reach it with Tap: tap info 7 · tap text 7 · tap snapshot 7 · tap eval 7 '<js>'"
        );
    }

    #[test]
    fn an_awake_tab_s_card_carries_what_the_page_says_of_itself() {
        let page = json!({
            "url": "https://app.example.com/dash?range=7d",
            "title": "Dashboard",
            "lang": "en",
            "contentType": "text/html",
            "charset": "UTF-8",
            "readyState": "complete",
            "canonical": "https://app.example.com/dash",
            "description": "  ",
            "ogTitle": null,
            "viewport": { "width": 1440, "height": 900, "dpr": 2, "x": 0, "y": 1830, "scrollHeight": 5400 },
            "frames": 0,
            "selection": "Revenue dropped 12%",
        });
        let card = tab_card(
            &Facts {
                label: "My board".to_owned(),
                ..facts()
            },
            Some(&page),
        );
        let lines: Vec<&str> = card.lines().collect();
        assert_eq!(lines[0], "Tap tab 7 — My board (page title: Dashboard)");
        assert_eq!(lines[2], "State: loaded · HTTP 200 · secure");
        assert_eq!(
            lines[3],
            "Place: group \"Work\" (blue) · tab 3 of 12 · in front"
        );
        assert_eq!(
            lines[4],
            "Page: lang en · text/html · UTF-8 · readyState complete"
        );
        assert_eq!(lines[5], "Canonical: https://app.example.com/dash");
        assert_eq!(
            lines[6],
            "Viewport: 1440×900 @2x · scrolled to 0,1830 of height 5400"
        );
        assert_eq!(lines[7], "Selection: \"Revenue dropped 12%\"");
        assert!(!card.contains("Description"), "a blank one is left out");
        assert!(
            !card.contains("Page URL"),
            "the same address isn't said twice"
        );
        assert!(!card.contains("Frames"));
    }

    #[test]
    fn a_secret_in_the_address_is_pointed_out() {
        assert_eq!(
            secret_params("https://a.com/cb?code=x&state=y&access_token=z#k=1"),
            ["code", "access_token"]
        );
        assert_eq!(
            secret_params("https://a.com/?q=rust&page=2"),
            Vec::<String>::new()
        );
        let card = tab_card(
            &Facts {
                url: "https://a.com/cb?code=x".to_owned(),
                ..facts()
            },
            None,
        );
        assert!(card.contains("Careful: the URL carries code"));
    }

    #[test]
    fn an_element_s_card_gives_a_selector_tap_can_use() {
        let element = json!({
            "selector": "#app > button.it's",
            "shadowHost": null,
            "frame": false,
            "tag": "<button class=\"it's\">",
            "role": null,
            "name": "Export",
            "text": "Export CSV",
            "box": { "x": 10, "y": 20, "width": 132, "height": 36, "pageY": 1850 },
            "style": { "display": "inline-flex", "position": "relative", "color": "rgb(0, 0, 0)" },
            "html": "<button class=\"it's\">Export CSV</button>",
        });
        let card = element_card(&facts(), &element);
        let lines: Vec<&str> = card.lines().collect();
        assert_eq!(lines[0], "Element in Tap tab 7 — Dashboard");
        assert_eq!(lines[2], "Selector: #app > button.it's");
        assert!(
            card.contains("Box: 132×36 at x 10, y 20 in the viewport (y 1850 on the page), CSS px")
        );
        assert!(
            card.contains("Style: display inline-flex · position relative · color rgb(0, 0, 0)")
        );
        assert_eq!(
            lines.last().copied(),
            Some(
                "Reach it with Tap: tap eval 7 'document.querySelector(\"#app > button.it'\\''s\")?.outerHTML' · tap snapshot 7"
            ),
            "the selector survives the shell's quotes"
        );
        let shadowed = element_card(
            &facts(),
            &json!({ "selector": "b", "shadowHost": "x-card" }),
        );
        assert!(shadowed.contains("Inside the shadow root of: x-card"));
        assert!(
            !shadowed.contains("tap eval"),
            "document can't reach into a shadow root"
        );
    }
}
