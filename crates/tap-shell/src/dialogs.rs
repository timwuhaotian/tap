//! The questions a page is allowed to ask, and the answers it gets (Search's
//! `Dialogs.swift`).
//!
//! WebKit does nothing with `alert()`, `confirm()`, `prompt()`, a file input or a site
//! behind a name and password unless somebody answers for them, and "nothing" means
//! `confirm()` is always false and a file picker never opens. Each one here is the
//! system's own sheet on the window, as every other browser on this Mac shows.
//!
//! A question from a tab behind the one in front waits for its own tab: over the page in
//! front, a `prompt()` asking for a password would pass for that page's. Every question
//! is answered exactly once; one let go unanswered (its tab closed, its process gone)
//! answers as dismissed. A test run never puts a window on the screen: what it would have
//! asked is written down for the bench, and it is answered as cancelled.
//!
//! Not yet: a site whose certificate can't be trusted (WebKit's own error is shown in the
//! failure view) and this Mac's own servers taken on trust.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ptr,
};

use block2::{DynBlock, RcBlock};
use objc2::{
    MainThreadMarker, MainThreadOnly, Message, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSModalResponse, NSModalResponseOK,
    NSOpenPanel, NSSecureTextField, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSCopying, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLAuthenticationChallenge,
    NSURLAuthenticationMethodHTTPBasic, NSURLAuthenticationMethodHTTPDigest,
    NSURLAuthenticationMethodNTLM, NSURLCredential, NSURLCredentialPersistence,
    NSURLSessionAuthChallengeDisposition,
};
use objc2_web_kit::{WKFrameInfo, WKOpenPanelParameters, WKWebView};
use tap_core::{browser::TabId, identity};

use crate::rt;

/// A question held for a tab behind the one in front. Dropped without being shown, it
/// answers as dismissed.
struct Held {
    show: Option<Box<dyn FnOnce()>>,
    dismiss: Option<Box<dyn FnOnce()>>,
}

impl Held {
    fn present(mut self) {
        self.dismiss = None;
        if let Some(show) = self.show.take() {
            show();
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(dismiss) = self.dismiss.take() {
            dismiss();
        }
    }
}

thread_local! {
    static HELD: RefCell<HashMap<TabId, Vec<Held>>> = RefCell::new(HashMap::new());
    /// A test run's questions, in the order they would have been shown (bench `probe`).
    static ASKED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn testing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// What a test run would have asked so far.
pub fn asked() -> Vec<String> {
    ASKED.with_borrow(Clone::clone)
}

/// Shows the question now if its tab is in front, or holds it for that tab.
fn ask(tab: Option<TabId>, show: impl FnOnce() + 'static, dismiss: impl FnOnce() + 'static) {
    let active = rt::read(tap_core::browser::Browser::active).flatten();
    match tab {
        Some(tab) if Some(tab) != active => HELD.with_borrow_mut(|held| {
            held.entry(tab).or_default().push(Held {
                show: Some(Box::new(show)),
                dismiss: Some(Box::new(dismiss)),
            });
        }),
        _ => show(),
    }
}

/// The tab came to the front: what it asked meanwhile is shown now, in order.
pub fn present_held(tab: TabId) {
    let waiting = HELD
        .with_borrow_mut(|held| held.remove(&tab))
        .unwrap_or_default();
    for question in waiting {
        question.present();
    }
}

/// The tab is gone, or its page went away: what it asked is answered as dismissed.
pub fn drop_held(tab: TabId) {
    let dropped = HELD.with_borrow_mut(|held| held.remove(&tab));
    drop(dropped);
}

/// An alert whose title is the asking frame's own site, so a page can't dress its
/// message up as one from the system or from Tap.
fn alert(mtm: MainThreadMarker, frame: &WKFrameInfo, message: &NSString) -> Retained<NSAlert> {
    // SAFETY: plain reads of the frame WebKit hands us.
    let host = unsafe { frame.securityOrigin().host() }.to_string();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(if host.is_empty() {
        "This page says"
    } else {
        &host
    }));
    alert.setInformativeText(message);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert
}

/// The sheet over the page's window, or a test run's note of it, answered as cancelled.
fn show(alert: &NSAlert, web: &WKWebView, finish: impl FnOnce(NSModalResponse) + 'static) {
    if testing() {
        ASKED.with_borrow_mut(|asked| asked.push(alert.messageText().to_string()));
        finish(0);
        return;
    }
    let once = Cell::new(Some(finish));
    let block = RcBlock::new(move |answer: NSModalResponse| {
        if let Some(finish) = once.take() {
            finish(answer);
        }
    });
    match web.window() {
        Some(window) => alert.beginSheetModalForWindow_completionHandler(&window, Some(&block)),
        None => block.call((alert.runModal(),)),
    }
}

/// `alert()`.
pub fn alert_panel(
    web: &WKWebView,
    tab: Option<TabId>,
    message: &NSString,
    frame: &WKFrameInfo,
    handler: &DynBlock<dyn Fn()>,
) {
    let (handler, again) = (handler.copy(), handler.copy());
    let (web, frame, message) = (web.retain(), frame.retain(), message.copy());
    ask(
        tab,
        move || {
            let alert = alert(MainThreadMarker::from(&*web), &frame, &message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            show(&alert, &web, move |_| handler.call(()));
        },
        move || again.call(()),
    );
}

/// `confirm()`: true only for OK.
pub fn confirm_panel(
    web: &WKWebView,
    tab: Option<TabId>,
    message: &NSString,
    frame: &WKFrameInfo,
    handler: &DynBlock<dyn Fn(Bool)>,
) {
    let (handler, again) = (handler.copy(), handler.copy());
    let (web, frame, message) = (web.retain(), frame.retain(), message.copy());
    ask(
        tab,
        move || {
            let alert = alert(MainThreadMarker::from(&*web), &frame, &message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            show(&alert, &web, move |answer| {
                handler.call((Bool::new(answer == NSAlertFirstButtonReturn),));
            });
        },
        move || again.call((Bool::NO,)),
    );
}

/// `prompt()`: the text for OK, nothing for Cancel.
pub fn prompt_panel(
    web: &WKWebView,
    tab: Option<TabId>,
    message: &NSString,
    default_text: Option<&NSString>,
    frame: &WKFrameInfo,
    handler: &DynBlock<dyn Fn(*mut NSString)>,
) {
    let (handler, again) = (handler.copy(), handler.copy());
    let (web, frame, message) = (web.retain(), frame.retain(), message.copy());
    let default_text = default_text.map(ToString::to_string).unwrap_or_default();
    ask(
        tab,
        move || {
            let mtm = MainThreadMarker::from(&*web);
            let alert = alert(mtm, &frame, &message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSTextField::textFieldWithString(&NSString::from_str(&default_text), mtm);
            field.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(260.0, 24.0),
            ));
            alert.setAccessoryView(Some(&field));
            alert.window().setInitialFirstResponder(Some(&field));
            show(&alert, &web, move |answer| {
                if answer == NSAlertFirstButtonReturn {
                    let text = field.stringValue();
                    handler.call((Retained::as_ptr(&text).cast_mut(),));
                } else {
                    handler.call((ptr::null_mut(),));
                }
            });
        },
        move || again.call((ptr::null_mut(),)),
    );
}

/// A file input: the system's panel, naming the site the file goes to.
pub fn open_panel(
    web: &WKWebView,
    tab: Option<TabId>,
    parameters: &WKOpenPanelParameters,
    frame: &WKFrameInfo,
    handler: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
) {
    // SAFETY: plain reads of what WebKit hands us.
    let (host, directories, many) = unsafe {
        (
            frame.securityOrigin().host().to_string(),
            parameters.allowsDirectories(),
            parameters.allowsMultipleSelection(),
        )
    };
    if testing() {
        ASKED.with_borrow_mut(|asked| asked.push(format!("Choose a file ({host})")));
        handler.call((ptr::null_mut(),));
        return;
    }
    let (handler, again) = (handler.copy(), handler.copy());
    let web = web.retain();
    ask(
        tab,
        move || {
            let panel = NSOpenPanel::openPanel(MainThreadMarker::from(&*web));
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(directories);
            panel.setAllowsMultipleSelection(many);
            panel.setResolvesAliases(true);
            let message = if host.is_empty() {
                "Choose a file for this page".to_owned()
            } else {
                format!("Choose a file for {host}")
            };
            panel.setMessage(Some(&NSString::from_str(&message)));
            let chosen = panel.clone();
            let once = Cell::new(Some(handler));
            let block = RcBlock::new(move |answer: NSModalResponse| {
                let Some(handler) = once.take() else { return };
                if answer == NSModalResponseOK {
                    let urls = chosen.URLs();
                    handler.call((Retained::as_ptr(&urls).cast_mut(),));
                } else {
                    handler.call((ptr::null_mut(),));
                }
            });
            match web.window() {
                Some(window) => panel.beginSheetModalForWindow_completionHandler(&window, &block),
                None => block.call((panel.runModal(),)),
            }
        },
        move || again.call((ptr::null_mut(),)),
    );
}

/// The challenge itself. WebKit hands over a wrapper (`WKNSURLAuthenticationChallenge`)
/// that answers by forwarding each message to the challenge it wraps; objc2's debug
/// checks look for the methods on the wrapper's own class, don't find them, and stop.
/// The wrapper names its target through `NSObject`'s `forwardingTargetForSelector:`; a
/// plain challenge names none and is its own.
fn unwrapped(challenge: &NSURLAuthenticationChallenge) -> Retained<NSURLAuthenticationChallenge> {
    // SAFETY: `forwardingTargetForSelector:` is NSObject's, takes a selector and returns
    // an object or nil.
    let target: Option<Retained<AnyObject>> =
        unsafe { msg_send![challenge, forwardingTargetForSelector: sel!(protectionSpace)] };
    target
        .and_then(|t| t.downcast::<NSURLAuthenticationChallenge>().ok())
        .unwrap_or_else(|| challenge.retain())
}

/// A site behind a name and a password: a staging server, a router. One wrong answer
/// gets another go; the second is taken as "not for me". Anything else WebKit decides.
pub fn challenge(
    web: &WKWebView,
    tab: Option<TabId>,
    challenge: &NSURLAuthenticationChallenge,
    handler: &DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
) {
    let challenge = &*unwrapped(challenge);
    let space = challenge.protectionSpace();
    let method = space.authenticationMethod();
    // SAFETY: Foundation's own constants, read as the constants they are.
    let asks = unsafe {
        [
            NSURLAuthenticationMethodHTTPBasic,
            NSURLAuthenticationMethodHTTPDigest,
            NSURLAuthenticationMethodNTLM,
        ]
        .iter()
        .any(|m| **m == *method)
    };
    if !asks {
        handler.call((
            NSURLSessionAuthChallengeDisposition::PerformDefaultHandling,
            ptr::null_mut(),
        ));
        return;
    }
    if challenge.previousFailureCount() >= 2 {
        handler.call((
            NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge,
            ptr::null_mut(),
        ));
        return;
    }
    let host = space.host().to_string();
    let realm = space.realm().map(|r| r.to_string());
    let retry = challenge.previousFailureCount() > 0;
    let (handler, again) = (handler.copy(), handler.copy());
    let web = web.retain();
    ask(
        tab,
        move || {
            let mtm = MainThreadMarker::from(&*web);
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str(&format!("{host} asks you to sign in")));
            let mut said = realm.map_or_else(
                || "The site wants a name and a password.".to_owned(),
                |r| format!("“{r}”"),
            );
            if retry {
                said.push_str("\nThat wasn't accepted — try again.");
            }
            alert.setInformativeText(&NSString::from_str(&said));
            alert.addButtonWithTitle(&NSString::from_str("Sign In"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let holder = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 56.0)),
            );
            let name = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
            name.setFrame(NSRect::new(
                NSPoint::new(0.0, 32.0),
                NSSize::new(260.0, 24.0),
            ));
            name.setPlaceholderString(Some(&NSString::from_str("Name")));
            let password = NSSecureTextField::new(mtm);
            password.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(260.0, 24.0),
            ));
            password.setPlaceholderString(Some(&NSString::from_str("Password")));
            // SAFETY: both fields live in the alert's accessory view for as long as it is up.
            unsafe { name.setNextKeyView(Some(&password)) };
            holder.addSubview(&name);
            holder.addSubview(&password);
            alert.setAccessoryView(Some(&holder));
            alert.window().setInitialFirstResponder(Some(&name));
            show(&alert, &web, move |answer| {
                if answer != NSAlertFirstButtonReturn {
                    handler.call((
                        NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge,
                        ptr::null_mut(),
                    ));
                    return;
                }
                let credential = NSURLCredential::credentialWithUser_password_persistence(
                    &name.stringValue(),
                    &password.stringValue(),
                    NSURLCredentialPersistence::ForSession,
                );
                handler.call((
                    NSURLSessionAuthChallengeDisposition::UseCredential,
                    Retained::as_ptr(&credential).cast_mut(),
                ));
            });
        },
        move || {
            again.call((
                NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge,
                ptr::null_mut(),
            ));
        },
    );
}
