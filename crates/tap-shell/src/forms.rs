//! What a page says about its sign-in (Search's `Forms.swift` and the sign-in half of
//! `Tab.swift`): where the caret is when it is in a sign-in box, so the accounts kept
//! for the site can hang from it, and when a name and password have just been sent, so
//! the password can be offered a place in the keychain.
//!
//! The script (`forms.js`) is based on Search's, in Tap's own content world and the
//! main frame only, where a page can't see or replace it. Filling
//! goes through the field's own setter and fires the events a keystroke would.
//!
//! Whether a sign-in worked is only known afterwards: a page that comes back without a
//! password box took it, one that still has the box refused it, and only the first is
//! worth remembering.

use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{NSRect, NSString};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView};
use serde_json::Value;
use tap_core::{browser::TabId, fnd::Url};

use crate::{prefs, signin, vault, web};

/// The script, run at each document's end in Tap's world, main frame only.
pub const SCRIPT: &str = include_str!("forms.js");
/// The name the script posts to.
const HANDLER: &str = "tapForms";

/// A sign-in sent and not yet judged: held this long at most.
const HELD: Duration = Duration::from_secs(45);
/// A name sent on a first step, for the password page after it.
const NAMED: Duration = Duration::from_secs(300);

struct Sent {
    host: String,
    user: String,
    password: String,
    clear: bool,
    at: Instant,
}

struct Named {
    site: String,
    user: String,
    at: Instant,
}

/// What one tab's page has said, in memory only, never written down.
#[derive(Default)]
struct Held {
    sent: Option<Sent>,
    named: Option<Named>,
}

thread_local! {
    static HELD_BY_TAB: RefCell<HashMap<TabId, Held>> = RefCell::new(HashMap::new());
}

define_class!(
    /// Hears the form script: one for every page, as Search's `FormRelay`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFormRelay"]
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

/// Puts the relay on the controller every page shares. Once, as the configuration is
/// made: WebKit refuses a second handler of the same name.
pub fn install(mtm: MainThreadMarker, controller: &WKUserContentController) {
    let relay = Relay::new(mtm);
    // SAFETY: the controller keeps the relay for as long as it lives.
    unsafe {
        controller.addScriptMessageHandler_contentWorld_name(
            ProtocolObject::from_ref(&*relay),
            &web::world(mtm),
            &NSString::from_str(HANDLER),
        );
    }
}

/// The page's scheme and host, lower case.
pub fn page(web: &WKWebView) -> Option<(String, String)> {
    let url = Url::parse(&web::url_of(web)?)?;
    Some((url.scheme()?.to_lowercase(), url.host()?.to_lowercase()))
}

fn bare(host: &str) -> String {
    host.strip_prefix("www.").unwrap_or(host).to_owned()
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
    match body["kind"].as_str() {
        Some("submit") => sent(tab, &web, &text("user"), &text("password")),
        Some("name") => gave_name(tab, &web, &text("user")),
        Some("settled") => settle(tab, false),
        Some("reading") => {
            if let (Some(y), Some(ceiling)) = (body["y"].as_f64(), body["max"].as_f64()) {
                crate::strip::page_read(tab, y, ceiling);
            }
        }
        Some("focus") => {
            let rect = body["rect"].as_object().and_then(|r| {
                Some(NSRect::new(
                    objc2_foundation::NSPoint::new(r.get("x")?.as_f64()?, r.get("y")?.as_f64()?),
                    objc2_foundation::NSSize::new(r.get("w")?.as_f64()?, r.get("h")?.as_f64()?),
                ))
            });
            let flag = |key: &str, or: bool| body[key].as_bool().unwrap_or(or);
            signin::field(
                tab,
                &web,
                rect,
                flag("passwords", true),
                flag("name", false),
                flag("wide", true),
            );
        }
        _ => {}
    }
}

/// The name a first step was sent with, for the page after it, which asks for the
/// password alone: a password saved without its name is half a login. This tab only, a
/// few minutes, and only for the same site.
fn gave_name(tab: TabId, web: &WKWebView, user: &str) {
    let Some((_, host)) = page(web) else { return };
    if user.is_empty() || user.chars().count() > 256 {
        return;
    }
    HELD_BY_TAB.with_borrow_mut(|held| {
        held.entry(tab).or_default().named = Some(Named {
            site: vault::registrable(&host),
            user: user.to_owned(),
            at: Instant::now(),
        });
    });
}

/// The name sent on the step before, if it was this site's and recent.
pub fn given_name(tab: TabId, host: &str) -> Option<String> {
    let site = vault::registrable(host);
    HELD_BY_TAB.with_borrow(|held| {
        let named = held.get(&tab)?.named.as_ref()?;
        (named.at.elapsed() < NAMED && named.site == site).then(|| named.user.clone())
    })
}

/// A name and password the page has just sent: held, not yet offered. The host now,
/// while the page is still the sign-in page: a moment later it may be somewhere else
/// entirely, and that is not where the password belongs.
fn sent(tab: TabId, web: &WKWebView, user: &str, password: &str) {
    let Some((scheme, host)) = page(web) else {
        return;
    };
    let user = if user.is_empty() {
        given_name(tab, &host).unwrap_or_default()
    } else {
        user.to_owned()
    };
    HELD_BY_TAB.with_borrow_mut(|held| {
        held.entry(tab).or_default().sent = Some(Sent {
            host: bare(&host),
            user,
            password: password.to_owned(),
            clear: scheme == "http",
            at: Instant::now(),
        });
    });
}

/// A new document has come in: did a sign-in sent before it take?
pub fn page_finished(tab: TabId) {
    settle(tab, true);
}

/// The page has moved on — a new document, or the sign-in boxes gone. If a password went
/// out recently and there is no longer a box for it, that is a sign-in that took.
///
/// A new document is judged at once. Boxes a page removed by itself are given a moment
/// first: a sign-in built into the page closes its form the instant you press the button
/// and puts it back if the server says no, and offering in between is offering a
/// password that may be wrong.
fn settle(tab: TabId, navigated: bool) {
    let stamp = HELD_BY_TAB.with_borrow_mut(|held| {
        let entry = held.get_mut(&tab)?;
        let sent = entry.sent.as_ref()?;
        if sent.at.elapsed() > HELD {
            entry.sent = None;
            return None;
        }
        Some(sent.at)
    });
    let Some(stamp) = stamp else { return };
    if !navigated {
        let later = dispatch2::DispatchTime::NOW.time(1_500_000_000);
        let _ = dispatch2::DispatchQueue::main().after(later, move || {
            // Only if nothing newer went out in the meantime.
            let same = HELD_BY_TAB.with_borrow(|held| {
                held.get(&tab)
                    .and_then(|h| h.sent.as_ref())
                    .is_some_and(|s| s.at == stamp)
            });
            if same {
                settle(tab, true);
            }
        });
        return;
    }
    let Some(web) = web::existing(tab) else {
        return;
    };
    web::evaluate_in_world(
        &web,
        "!!(window.__tapForms && window.__tapForms.hasPassword())",
        move |still| {
            // The box is still there: a refused sign-in, or the second step of one.
            // Kept a moment longer, in case the page is still on its way.
            if still == Some(Value::Bool(true)) {
                return;
            }
            let sent = HELD_BY_TAB.with_borrow_mut(|held| {
                let entry = held.get_mut(&tab)?;
                entry.named = None;
                entry.sent.take()
            });
            if let Some(sent) = sent {
                signin::credentials(tab, sent.host, sent.user, sent.password, sent.clear);
            }
        },
    );
}

/// A tab gone: what it held goes with it.
pub fn forget_tab(tab: TabId) {
    HELD_BY_TAB.with_borrow_mut(|held| held.remove(&tab));
}

/// Puts a remembered name and password where a person would have typed them, if the
/// page is still on `scheme` and `host` when the script runs: a document that changed
/// since gets nothing. `done` hears whether the boxes were there.
pub fn fill(
    web: &WKWebView,
    user: &str,
    password: Option<&str>,
    scheme: &str,
    host: &str,
    done: impl Fn(bool) + 'static,
) {
    let body = match password {
        Some(_) => {
            "location.protocol === scheme + ':' && location.hostname.toLowerCase() === host \
             && !!(window.__tapForms && window.__tapForms.fill(user, password))"
        }
        // A first step: the name alone, the password waits for its page.
        None => {
            "location.protocol === scheme + ':' && location.hostname.toLowerCase() === host \
             && !!(window.__tapForms && window.__tapForms.name(user))"
        }
    };
    let mut arguments = vec![("user", user), ("scheme", scheme), ("host", host)];
    if let Some(password) = password {
        arguments.push(("password", password));
    }
    web::call_in_world(web, body, &arguments, move |answer| {
        done(answer == Some(Value::Bool(true)));
    });
}
