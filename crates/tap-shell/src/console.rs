//! The page's console: what its scripts threw, one right-click away (the page menu's
//! Console) or one `tap console` away for an agent. No Search equivalent: Search
//! leans on the Web Inspector, which Tap has too (View menu, `inspector.rs`); this
//! is the small one that is always there, errors included from frames the page's
//! own snapshot can't see into.
//!
//! A page-world script at each document's start hears `error` and
//! `unhandledrejection` in every frame, and posts to the relay below. It wraps
//! nothing: a wrapped native shows up in `Function.prototype.toString`, and Apple's
//! bot check refuses browsers that patch natives (its sign-in widget then never
//! boots). Entries live per tab, oldest first, capped; a new document clears them.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
};

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSFont, NSScrollView, NSTextView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController};
use tap_core::browser::TabId;

use crate::{prefs, rt, web};

/// The collector, run at each document's start in the page's world, every frame:
/// it must stand where the page's own `console` calls land.
pub const SCRIPT: &str = include_str!("console.js");
/// The name the script posts to.
const HANDLER: &str = "tapConsole";
/// Entries kept per tab; a chatty page's first lines fall off.
const KEPT: usize = 200;

/// One line the page said: its level, its text, and where (a script with its line,
/// or a frame's address when the call came from one).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    level: String,
    text: String,
    place: String,
}

impl Entry {
    fn line(&self) -> String {
        if self.place.is_empty() {
            format!("[{}] {}", self.level, self.text)
        } else {
            format!("[{}] {} (@{})", self.level, self.text, self.place)
        }
    }
}

thread_local! {
    static ENTRIES: RefCell<HashMap<TabId, VecDeque<Entry>>> = RefCell::new(HashMap::new());
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

struct Shown {
    window: Retained<NSWindow>,
    view: Retained<NSTextView>,
    tab: TabId,
}

define_class!(
    /// Hears the console script: one for every page, as Search's relays are.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapConsoleRelay"]
    struct Relay;

    unsafe impl NSObjectProtocol for Relay {}

    unsafe impl WKScriptMessageHandler for Relay {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            received(message);
        }
    }
);

impl Relay {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Puts the relay on the controller every page shares, in the page's world: the
/// collector stands there, not in Tap's. Once, as the configuration is made:
/// WebKit refuses a second handler of the same name.
pub fn install(mtm: MainThreadMarker, controller: &WKUserContentController) {
    let relay = Relay::new(mtm);
    // SAFETY: the controller keeps the relay for as long as it lives.
    unsafe {
        controller.addScriptMessageHandler_name(
            ProtocolObject::from_ref(&*relay),
            &NSString::from_str(HANDLER),
        );
    }
}

fn received(message: &WKScriptMessage) {
    // SAFETY: plain property reads on a message WebKit hands over for the call's length.
    let (body, web) = unsafe { (message.body(), message.webView()) };
    let Some(web) = web else { return };
    let Some(tab) = web::tab_of(&web) else { return };
    let Some(body) = prefs::to_json(&body) else {
        return;
    };
    let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
    let level = text("level");
    let level = match level.as_str() {
        "log" | "info" | "warn" | "error" | "debug" => level,
        _ => "log".to_owned(),
    };
    let place = {
        let detailed = text("where");
        if detailed.is_empty() {
            text("frame")
        } else {
            detailed
        }
    };
    ENTRIES.with_borrow_mut(|all| {
        let entries = all.entry(tab).or_default();
        entries.push_back(Entry {
            level,
            text: text("text"),
            place,
        });
        while entries.len() > KEPT {
            entries.pop_front();
        }
    });
    SHOWN.with_borrow(|shown| {
        if shown.as_ref().is_some_and(|shown| shown.tab == tab) {
            render(shown.as_ref());
        }
    });
}

/// The tab's console as text, oldest first; what the panel shows and `tap console`
/// prints.
#[must_use]
pub fn text(tab: TabId) -> String {
    let lines: Vec<String> = ENTRIES.with_borrow(|all| {
        all.get(&tab)
            .map(|entries| entries.iter().map(Entry::line).collect())
            .unwrap_or_default()
    });
    if lines.is_empty() {
        "(no console entries for this tab)".to_owned()
    } else {
        lines.join("\n")
    }
}

/// A new document: the last page's lines go with it.
pub fn clear(tab: TabId) {
    ENTRIES.with_borrow_mut(|all| all.remove(&tab));
    SHOWN.with_borrow(|shown| {
        if shown.as_ref().is_some_and(|shown| shown.tab == tab) {
            render(shown.as_ref());
        }
    });
}

/// The tab's page went: its lines go with it, and the panel if it showed them.
pub fn forget(tab: TabId) {
    ENTRIES.with_borrow_mut(|all| all.remove(&tab));
    let showing = SHOWN.with_borrow(|shown| shown.as_ref().is_some_and(|shown| shown.tab == tab));
    if showing {
        SHOWN.with_borrow_mut(|shown| {
            if let Some(shown) = shown.take() {
                shown.window.orderOut(None);
            }
        });
    }
}

/// Opens the tab's console, or brings it forward, following new lines while open.
pub fn show(tab: TabId) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let made = SHOWN.with_borrow(Option::is_some);
    if !made {
        let (window, view) = make(mtm);
        SHOWN.with_borrow_mut(|slot| *slot = Some(Shown { window, view, tab }));
    }
    SHOWN.with_borrow_mut(|shown| {
        if let Some(shown) = shown {
            shown.tab = tab;
            let label =
                rt::read(|browser| browser.tab(tab).map(tap_core::browser::Tab::label)).flatten();
            shown.window.setTitle(&NSString::from_str(&match label {
                Some(label) if !label.is_empty() => format!("Console — {label}"),
                _ => "Console".to_owned(),
            }));
            render(Some(shown));
            shown.window.makeKeyAndOrderFront(None);
        }
    });
}

fn render(shown: Option<&Shown>) {
    let Some(shown) = shown else { return };
    shown.view.setString(&NSString::from_str(&text(shown.tab)));
    // The tail is what matters: the newest line stays in sight.
    // SAFETY: NSResponder's own scroller, with no sender.
    unsafe {
        let _: () = msg_send![&shown.view, scrollToEndOfDocument: Option::<&AnyObject>::None];
    }
}

fn make(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<NSTextView>) {
    // SAFETY: a fresh allocation, initialised once, on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 420.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: kept in `SHOWN` while it may come back; AppKit must not also release it.
    unsafe { window.setReleasedWhenClosed(false) };
    let frame = window.contentView().map_or_else(
        || NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 420.0)),
        |content| content.bounds(),
    );
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // SAFETY: NSTextView's designated initialiser, once, on a fresh allocation.
    let view: Retained<NSTextView> = unsafe {
        msg_send![
            NSTextView::alloc(mtm),
            initWithFrame: frame,
        ]
    };
    view.setEditable(false);
    view.setSelectable(true);
    view.setVerticallyResizable(true);
    view.setHorizontallyResizable(false);
    view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    // SAFETY: a plain getter on a live text view; the container outlives the call.
    if let Some(container) = unsafe { view.textContainer() } {
        container.setWidthTracksTextView(true);
        container.setContainerSize(NSSize::new(frame.size.width, f64::MAX));
    }
    view.setFont(NSFont::userFixedPitchFontOfSize(12.0).as_deref());
    scroll.setDocumentView(Some(&view));
    window.setContentView(Some(&scroll));
    window.center();
    (window, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_their_level_and_where() {
        let plain = Entry {
            level: "log".to_owned(),
            text: "hello".to_owned(),
            place: String::new(),
        };
        assert_eq!(plain.line(), "[log] hello");
        let thrown = Entry {
            level: "error".to_owned(),
            text: "boom".to_owned(),
            place: "https://x.example/app.js:42".to_owned(),
        };
        assert_eq!(thrown.line(), "[error] boom (@https://x.example/app.js:42)");
    }

    #[test]
    fn an_empty_console_says_so() {
        assert_eq!(text(TabId(999_999)), "(no console entries for this tab)");
    }
}
