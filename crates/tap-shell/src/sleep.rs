//! Tabs you aren't using, put to sleep (Search's `Sleep.swift`).
//!
//! A page open in a tab keeps its whole content process, a hundred to three hundred
//! megabytes, for as long as the tab exists. So a tab left alone for half an hour gives
//! its page back, and keeps what it takes to come back where it was: its history and its
//! scroll position (WebKit's `interactionState`). Half an hour is Normal; Search's
//! `tabs.sleep.pace` makes it two hours (later) or ten minutes (sooner), and `sleep.after`
//! sets it in seconds for the bench. When macOS says memory is short, the wait shrinks:
//! to five minutes on a warning, to nothing when it is critical.
//!
//! Some tabs never sleep, because waking them couldn't give back what they were doing:
//! the one on screen, one still loading, the page that opened the one on screen (the
//! model's part, `Browser::awake_because`), and one playing sound, on a call, sending a
//! download, waiting on a question, or holding something typed (this module's).
//!
//! Not yet: the picture shown while a page wakes, pins and spaces, keeping chosen sites
//! awake, and Search's own "typed" check, which comes with its form script (M5); a
//! smaller one stands in until then.

use std::{cell::RefCell, collections::HashMap, sync::OnceLock};

use block2::RcBlock;
use objc2::{MainThreadMarker, msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSString, NSTimer};
use objc2_web_kit::{WKMediaCaptureState, WKWebView};
use serde_json::Value;
use tap_core::{browser::TabId, history};

use crate::{dialogs, downloads, rt, web};

thread_local! {
    /// What each sleeping tab needs to come back where it was.
    static MEMORY: RefCell<HashMap<TabId, Retained<AnyObject>>> = RefCell::new(HashMap::new());
    static TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
}

/// How long a tab has to go without being looked at.
fn wait() -> f64 {
    let defaults = crate::prefs::defaults();
    // Settings › Tabs › Sleep tabs you aren't using › Never.
    if !tap_core::settings_index::flag(&crate::prefs::Kept::new(), "tabs.sleep") {
        return f64::INFINITY;
    }
    let set = defaults.doubleForKey(&NSString::from_str("sleep.after"));
    if set > 0.0 {
        return set;
    }
    match defaults
        .stringForKey(&NSString::from_str("tabs.sleep.pace"))
        .map(|p| p.to_string())
        .as_deref()
    {
        Some("later") => 2.0 * 60.0 * 60.0,
        Some("sooner") => 10.0 * 60.0,
        _ => 30.0 * 60.0,
    }
}

/// Starts looking, once, at launch.
pub fn start(mtm: MainThreadMarker) {
    let every = (wait() / 4.0).clamp(5.0, 60.0);
    let block = RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| {
        let wait = wait();
        rt::dispatch(move |browser| browser.sleep_idle(history::now(), wait));
    });
    // SAFETY: a repeating timer on the main run loop, its block kept by the timer.
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(every, true, &block) };
    timer.setTolerance(every / 4.0);
    TIMER.with_borrow_mut(|slot| *slot = Some(timer));
    watch_memory();
    let _ = mtm;
}

/// macOS saying memory is short: the wait shrinks to five minutes on a warning and to
/// nothing when it is critical, whatever was chosen.
fn watch_memory() {
    const WARN: usize = 0x2;
    const CRITICAL: usize = 0x4;
    struct Source(dispatch2::DispatchRetained<dispatch2::DispatchSource>);
    // SAFETY: a dispatch source is a thread-safe object; it is only read from its handler.
    unsafe impl Send for Source {}
    // SAFETY: as above.
    unsafe impl Sync for Source {}
    static SOURCE: OnceLock<Source> = OnceLock::new();

    extern "C" fn pressed(_context: *mut std::ffi::c_void) {
        let critical = SOURCE.get().is_some_and(|s| s.0.data() & CRITICAL != 0);
        let wait = if critical { 0.0 } else { 5.0 * 60.0 };
        rt::dispatch(move |browser| browser.sleep_idle(history::now(), wait));
    }

    // SAFETY: the memory-pressure source type, its warning and critical flags, and the
    // main queue, where the handler then runs.
    let source = unsafe {
        dispatch2::DispatchSource::new(
            (&raw const dispatch2::_dispatch_source_type_memorypressure).cast_mut(),
            0,
            WARN | CRITICAL,
            Some(dispatch2::DispatchQueue::main()),
        )
    };
    source.set_event_handler_f(pressed);
    dispatch2::DispatchObject::resume(&*source);
    let _ = SOURCE.set(Source(source));
}

/// What the page is doing that letting it go would cut off (Search's `busy`).
fn busy(tab: TabId, web: &WKWebView) -> Option<&'static str> {
    if playing(web) {
        return Some("playing sound");
    }
    // SAFETY: plain property reads on a live web view.
    let (camera, microphone) = unsafe { (web.cameraCaptureState(), web.microphoneCaptureState()) };
    if camera != WKMediaCaptureState::None || microphone != WKMediaCaptureState::None {
        return Some("on a call");
    }
    if downloads::sending(tab) {
        return Some("downloading");
    }
    if dialogs::waiting(tab) {
        return Some("a question waiting");
    }
    None
}

/// WebKit's private `_isPlayingAudio`, asked first whether it answers.
fn playing(web: &WKWebView) -> bool {
    let selector = objc2::sel!(_isPlayingAudio);
    // SAFETY: asked first whether the web view answers the selector; it returns a BOOL.
    unsafe {
        let answers: bool = msg_send![web, respondsToSelector: selector];
        answers && msg_send![web, _isPlayingAudio]
    }
}

/// Whether a field holds something typed and not sent. Interim: Search asks its form
/// script, which comes with M5.
const UNSAVED: &str = r#"(() => {
  const skipped = ["hidden", "password", "submit", "button", "reset", "image", "file", "checkbox", "radio", "range", "color"];
  const edited = field => {
    if (field.matches("textarea")) return field.value !== field.defaultValue;
    return !skipped.includes((field.type || "text").toLowerCase()) && field.value !== field.defaultValue;
  };
  if ([...document.querySelectorAll("input, textarea")].some(edited)) return true;
  const active = document.activeElement;
  return !!(active && active.isContentEditable && active.textContent.trim());
})()"#;

/// Puts a tab to sleep now if nothing keeps it awake, and says what happened: "asleep",
/// or why it stays awake.
pub fn try_sleep(tab: TabId, done: impl FnOnce(String) + 'static) {
    if let Some(reason) = rt::read(|b| b.awake_because(tab)).flatten() {
        return done(reason.to_owned());
    }
    let Some(page) = web::existing(tab) else {
        return done("no page".to_owned());
    };
    if let Some(reason) = busy(tab, &page) {
        return done(reason.to_owned());
    }
    let done = RefCell::new(Some(done));
    web::evaluate_in_world(&page, UNSAVED, move |value| {
        let Some(done) = done.borrow_mut().take() else {
            return;
        };
        if value == Some(Value::Bool(true)) {
            return done("holding something typed".to_owned());
        }
        // Looked at again: you may have gone back to the tab in the meantime.
        if let Some(reason) = rt::read(|b| b.awake_because(tab)).flatten() {
            return done(reason.to_owned());
        }
        let Some(page) = web::existing(tab) else {
            return done("no page".to_owned());
        };
        if let Some(reason) = busy(tab, &page) {
            return done(reason.to_owned());
        }
        // SAFETY: a plain property read on a live web view.
        if let Some(state) = unsafe { page.interactionState() } {
            MEMORY.with_borrow_mut(|memory| memory.insert(tab, state));
        }
        web::discard_page(tab);
        rt::dispatch(move |browser| browser.slept(tab));
        done("asleep".to_owned());
    });
}

/// What a sleeping tab needs to come back where it was, taken as it wakes.
pub fn take_memory(tab: TabId) -> Option<Retained<AnyObject>> {
    MEMORY.with_borrow_mut(|memory| memory.remove(&tab))
}
