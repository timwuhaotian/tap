//! The Web Inspector: WebKit's own, on the tab in front — from the View menu
//! and its keys, as from Inspect Element in a page's right-click menu (Search's
//! `Inspector.swift`). The keys are Chrome's and Arc's: ⌥⌘I for the inspector,
//! ⌥⌘J for the console, ⌥⌘C to pick an element on the page.
//!
//! WebKit answers for its inspector only through names outside the public
//! framework, so each is asked for before it is used; a WebKit without them
//! leaves the menu doing nothing rather than the app falling over.

use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool},
    sel,
};
use objc2_web_kit::{WKPreferences, WKWebView};
use tap_core::browser::TabId;

use crate::{rt, web};

/// WebKit's "developer extras": Inspect Element in a page's right-click menu,
/// and the Web Inspector the View menu opens. `isInspectable` alone only lets
/// Safari's Develop menu reach the page. The name is outside the public
/// framework, so it is asked first (Search's `Web.inspector`, `Tab.swift`).
pub fn developer_extras(preferences: &WKPreferences) {
    let set = sel!(_setDeveloperExtrasEnabled:);
    // SAFETY: asked first whether the preferences answer the selector, which
    // takes the one BOOL its name says.
    unsafe {
        let answers: bool = msg_send![preferences, respondsToSelector: set];
        if answers {
            let _: () = msg_send![preferences, _setDeveloperExtrasEnabled: Bool::new(true)];
        }
    }
}

/// A page the Web Inspector reaches: Safari's Develop menu and the View
/// menu's inspector both (Search's `Web.inspectable`).
pub fn inspectable(web: &WKWebView) {
    // SAFETY: plain property accessors on a live web view.
    unsafe {
        web.setInspectable(true);
        developer_extras(&web.configuration().preferences());
    }
}

/// One of the inspector's no-argument commands, sent only when it answers to
/// the name (Search's `send`).
macro_rules! send {
    ($inspector:expr, $name:ident) => {{
        let inspector = $inspector;
        // SAFETY: asked first whether the inspector answers the selector,
        // which takes no argument and answers nothing.
        unsafe {
            let answers: bool = msg_send![inspector, respondsToSelector: sel!($name)];
            if answers {
                let _: () = msg_send![inspector, $name];
            }
        }
    }};
}

/// One of the inspector's no-argument questions, answered `false` when it
/// doesn't answer to the name (Search's `asks`).
macro_rules! asks {
    ($inspector:expr, $name:ident) => {{
        let inspector = $inspector;
        // SAFETY: asked first whether the inspector answers the selector,
        // which takes no argument and answers a BOOL.
        unsafe {
            let answers: bool = msg_send![inspector, respondsToSelector: sel!($name)];
            answers && {
                let said: Bool = msg_send![inspector, $name];
                said.as_bool()
            }
        }
    }};
}

/// ⌥⌘I: the inspector, or put away if it is up.
pub fn toggle() {
    let Some(inspector) = front() else { return };
    if asks!(&inspector, isVisible) {
        send!(&inspector, close);
    } else {
        send!(&inspector, show);
    }
}

/// ⌥⌘J: the inspector, at its console.
pub fn show_console() {
    let Some(inspector) = front() else { return };
    send!(&inspector, showConsole);
}

/// ⌥⌘C: the inspector, and the next click on the page picks what it shows.
/// Again, and the picking stops.
pub fn inspect_element() {
    let Some(inspector) = front() else { return };
    if !asks!(&inspector, isVisible) {
        send!(&inspector, show);
    }
    send!(&inspector, toggleElementSelection);
}

/// How a tab's inspector stands, for the bench: `(visible, picking)`; `None`
/// for a tab with no page to look at.
#[must_use]
pub fn state(tab: TabId) -> Option<(bool, bool)> {
    if !has_page(tab) {
        return None;
    }
    let view = web::existing(tab)?;
    let inspector = inspector_of(&view)?;
    Some((
        asks!(&inspector, isVisible),
        asks!(&inspector, isElementSelectionActive),
    ))
}

/// Whether a tab has a page to look at: awake, and neither a blank tab nor
/// one of Tap's own pages.
fn has_page(id: TabId) -> bool {
    rt::read(|browser| {
        browser
            .tab(id)
            .is_some_and(|tab| tab.awake() && !tab.is_blank() && !tab.is_internal())
    })
    .unwrap_or(false)
}

/// The inspector of the page in front — a pop-up window's page while that
/// window is key, else the tab in front; none for a blank tab or one of Tap's
/// own pages, which have no page to look at.
fn front() -> Option<Retained<AnyObject>> {
    if let Some(popup) = web::key_popup() {
        return inspector_of(&popup);
    }
    let tab = rt::read(tap_core::browser::Browser::active).flatten()?;
    if !has_page(tab) {
        return None;
    }
    let view = web::existing(tab)?;
    inspector_of(&view)
}

fn inspector_of(view: &WKWebView) -> Option<Retained<AnyObject>> {
    // SAFETY: asked first whether the web view answers `_inspector`, which
    // hands back WebKit's own inspector object for this page.
    unsafe {
        let answers: bool = msg_send![view, respondsToSelector: sel!(_inspector)];
        if !answers {
            return None;
        }
        msg_send![view, _inspector]
    }
}
