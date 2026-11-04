//! The floating video (Search's `Float.swift`, `NativePiP.swift` and `Isolate`):
//! a video that keeps playing after you have gone somewhere else, in a small window
//! above everything.
//!
//! Which window is Settings › The floating video (`float.style`): Tap's own
//! (`search`, the default), or macOS's picture in picture (`system`), the one Safari
//! uses. Tap's own is the page itself moved — everything but the video hidden, the
//! web view lifted out of its tab into a small floating panel — so the video never
//! stops, as in Search. It is a smaller Float than Search's: no still cut from a
//! snapshot of the page while it settles, no flick-to-a-corner or docking, no
//! progress line; two buttons (play/pause, back to its tab) and ⇧⌘P to put it back.
//! Picture in picture is handed to WebKit (`webkitSetPresentationMode`), which
//! needs the private `_setAllowsPictureInPictureMediaPlayback:` preference turned
//! on while it is the choice, and may still refuse a page: then nothing happens,
//! where Search announces why (Tap has no announce hook this module may use).
//!
//! On its own (`float.own`, over `float.leave`/`float.away` through
//! `settings_index::merged(FloatOwn)`): leaving the tab floats a playing video
//! from a site people go to watch (Search's `Players`), leaving the app does too
//! when the choice says tabs-or-apps, and coming back to the tab or the app puts
//! it back. Tap has no tab-switch or app-activation hook this module may plug
//! into (`window.rs`/`web.rs` are not its to change), so it watches: a half-second
//! timer notices the tab in front change through `rt::read`, and AppKit's own
//! resign/become-active notifications say the app was left and came back.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ptr::{NonNull, null_mut},
};

use block2::{DynBlock, RcBlock};
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationDidBecomeActiveNotification,
    NSApplicationDidResignActiveNotification, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSButton, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSFloatingWindowLevel, NSPanel,
    NSResponder, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use objc2_web_kit::WKWebView;
use serde_json::Value;
use tap_core::{
    browser::TabId,
    settings_index::{self as index, Merged},
};

use crate::{prefs, rt, web};

/// Why the video is out: only one lifted on its own comes home on its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    ByHand,
    LeftTab,
    LeftApp,
}

/// Where a page lived in its tab: its stage, its frame there, and its mask.
type Home = (Retained<NSView>, NSRect, NSAutoresizingMaskOptions);

struct Out {
    panel: Retained<Panel>,
    origin: TabId,
    /// Where the page lived, and how, so landing puts it back exactly there: the
    /// stage takes it again on its next layout in Search; here the stage is the
    /// same view for the app's life, kept and given back.
    home: Option<Home>,
    why: Why,
}

thread_local! {
    static OUT: RefCell<Option<Out>> = const { RefCell::new(None) };
    /// The tab whose video is in macOS's picture in picture, to bring back.
    static PICTURED: RefCell<Option<TabId>> = const { RefCell::new(None) };
    /// Set when leaving the app lifted the video: only that one lands on return.
    static LIFTED_AWAY: Cell<bool> = const { Cell::new(false) };
    static LAST_ACTIVE: RefCell<Option<TabId>> = const { RefCell::new(None) };
    /// Where each shown page last lived in its tab, kept as it is seen: by the
    /// time leaving a tab is noticed, the stage has already taken the page out,
    /// and landing needs the stage it came from (see `Out::home`).
    static HOMES: RefCell<HashMap<TabId, Home>> = RefCell::new(HashMap::new());
    static LAST_STYLE: RefCell<String> = const { RefCell::new(String::new()) };
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Starts the watching, once, at launch: ⇧⌘P by hand, the timer that notices a
/// tab change, and the app's own leaving and returning.
pub fn install(mtm: MainThreadMarker) {
    if INSTALLED.replace(true) {
        return;
    }
    install_key_monitor();
    install_app_notes();
    LAST_ACTIVE.set(rt::read(tap_core::browser::Browser::active).flatten());
    apply_style(true);
    let block = RcBlock::new(|_timer: NonNull<NSTimer>| poll());
    // SAFETY: a repeating timer on the main run loop, its block kept by the timer,
    // and the timer kept here for the app's life.
    let timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.5, true, &block) };
    timer.setTolerance(0.1);
    TIMER.with_borrow_mut(|slot| *slot = Some(timer));
    let _ = mtm;
}

/// ⇧⌘P, as Search's menu has it. Tap's menus have no such item yet (`menu.rs` is
/// not this module's to change), so the key is taken here and swallowed.
fn install_key_monitor() {
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
                    .is_some_and(|keys| keys.to_string().eq_ignore_ascii_case("p"));
            if ours {
                toggle_active();
                return null_mut();
            }
            std::ptr::from_ref(event).cast_mut()
        });
    let block: &DynBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> = &block;
    // SAFETY: a key-down monitor for this app's own events; the returned monitor
    // is forgotten on purpose, so it lives as long as the app does.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, block)
    };
    std::mem::forget(monitor);
}

fn install_app_notes() {
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: AppKit's own notification names, read as the constants they are.
    for (name, work) in unsafe {
        [
            (NSApplicationDidResignActiveNotification, app_left as fn()),
            (NSApplicationDidBecomeActiveNotification, app_back as fn()),
        ]
    } {
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| work());
        // SAFETY: the centre keeps the observer for the app's life; the token is
        // forgotten on purpose, as in window.rs's lights.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        std::mem::forget(token);
    }
}

/// Every half second: the style as Settings now says it, and the tab in front.
fn poll() {
    apply_style(false);
    // Its tab went: the window goes too, rather than outlive its page.
    if OUT.with_borrow(|out| {
        out.as_ref()
            .is_some_and(|o| rt::read(|b| b.tab(o.origin).is_none()).unwrap_or(false))
    }) {
        land();
    }
    let active = rt::read(tap_core::browser::Browser::active).flatten();
    if let Some(page) = active.and_then(web::existing) {
        // SAFETY: plain reads of a live view and its superview.
        if let Some(stage) = unsafe { page.superview() } {
            HOMES.with_borrow_mut(|homes| {
                if let Some(tab) = active {
                    homes.insert(tab, (stage, page.frame(), page.autoresizingMask()));
                }
            });
        }
    }
    let previous = LAST_ACTIVE.replace(active);
    if previous == active {
        return;
    }
    let Some(left) = previous else { return };
    // Back on the tab it came from: in it goes again, unless you lifted it by hand.
    if OUT.with_borrow(|out| out.as_ref().is_some_and(|o| Some(o.origin) == active)) {
        if OUT.with_borrow(|out| out.as_ref().is_some_and(|o| o.why != Why::ByHand)) {
            land();
        }
        return;
    }
    if PICTURED.with_borrow(|p| *p == active) {
        unpicture();
        return;
    }
    if showing() || PICTURED.with_borrow(Option::is_some) {
        return;
    }
    let kept = prefs::Kept::new();
    if index::flag(&kept, "float.leave") {
        lift(left, true, Why::LeftTab);
    }
}

/// Another app in front (Search's `Browser.appLeft`).
fn app_left() {
    let kept = prefs::Kept::new();
    if index::merged(&kept, Merged::FloatOwn) != "tabsOrApps" {
        return;
    }
    let Some(active) = rt::read(tap_core::browser::Browser::active).flatten() else {
        return;
    };
    LIFTED_AWAY.set(!showing() && PICTURED.with_borrow(Option::is_none));
    lift(active, true, Why::LeftApp);
}

/// Back, and still on the tab it came from: into the tab again (Search's `appBack`).
fn app_back() {
    let was = LIFTED_AWAY.replace(false);
    if !was {
        return;
    }
    let active = rt::read(tap_core::browser::Browser::active).flatten();
    if OUT.with_borrow(|out| out.as_ref().is_some_and(|o| Some(o.origin) == active)) {
        land();
    }
    if PICTURED.with_borrow(|p| *p == active && active.is_some()) {
        unpicture();
    }
}

/// ⇧⌘P on the tab in front: out, or back in if it is out.
pub fn toggle_active() {
    if showing() {
        land();
        return;
    }
    let Some(active) = rt::read(tap_core::browser::Browser::active).flatten() else {
        return;
    };
    if PICTURED.with_borrow(|p| *p == Some(active)) {
        unpicture();
        return;
    }
    lift(active, false, Why::ByHand);
}

fn showing() -> bool {
    OUT.with_borrow(Option::is_some)
}

/// The floating video's style as Settings says it now.
fn style() -> String {
    index::text(&prefs::Kept::new(), "float.style")
}

/// Picture in picture is the choice: WebKit's private preference goes on for
/// every page there is (Search's `NativePiP.allowEverywhere`), and off again
/// when Tap's window is, so with Tap's window nothing changes for any page.
fn apply_style(force: bool) {
    let now = style();
    if !force && LAST_STYLE.with_borrow(|last| *last == now) {
        return;
    }
    LAST_STYLE.set(now.clone());
    let on = now == "system";
    let tabs: Vec<TabId> =
        rt::read(|browser| browser.tabs().iter().map(|t| t.id).collect()).unwrap_or_default();
    for tab in tabs {
        if let Some(web) = web::existing(tab) {
            allow_pip(&web, on);
        }
    }
}

fn allow_pip(web: &WKWebView, on: bool) {
    let set = sel!(_setAllowsPictureInPictureMediaPlayback:);
    // SAFETY: asked first whether the preferences answer the private selector,
    // which takes a BOOL; a WebKit without it is simply left alone.
    unsafe {
        let preferences = web.configuration().preferences();
        let answers: bool = msg_send![&*preferences, respondsToSelector: set];
        if answers {
            let _: () =
                msg_send![&*preferences, _setAllowsPictureInPictureMediaPlayback: Bool::new(on)];
        }
    }
}

/// The video playing in `tab`, out. Quietly (on its own) only from a site whose
/// video is the point of the site, and never from one grounded in its site card.
fn lift(tab: TabId, quietly: bool, why: Why) {
    if style() == "system" {
        picture(tab, quietly, why == Why::ByHand);
        return;
    }
    if showing() {
        return;
    }
    let Some(page) = web::existing(tab) else {
        return;
    };
    let Some((url, blank, own_page)) = rt::read(|browser| {
        browser
            .tab(tab)
            .map(|t| (t.url().to_owned(), t.is_blank(), t.page().is_some()))
    })
    .flatten() else {
        return;
    };
    if blank || own_page {
        return;
    }
    if quietly && (!knows_player(&url) || grounded(&url)) {
        return;
    }
    apply_style(false);
    eval(&page, ISOLATE_ON, move |value| {
        let floated = value
            .as_ref()
            .and_then(|v| v.get("floating"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if floated {
            move_page(tab, why);
        }
    });
}

/// Everything but the video out of the way, and the page into the little window.
fn move_page(tab: TabId, why: Why) {
    if showing() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(page) = web::existing(tab) else {
        return;
    };
    let size = NSSize::new(440.0, 247.0);
    let screen = NSScreen::mainScreen(mtm).map_or(NSRect::ZERO, |s| s.visibleFrame());
    let spot = NSRect::new(
        NSPoint::new(
            screen.origin.x + screen.size.width - size.width - 24.0,
            screen.origin.y + 24.0,
        ),
        size,
    );
    let panel = Panel::new(mtm, spot);
    panel.setLevel(NSFloatingWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    panel.setMovableByWindowBackground(true);
    panel.setBackgroundColor(Some(&NSColor::blackColor()));
    panel.setOpaque(true);
    panel.setHasShadow(false);
    panel.setMinSize(NSSize::new(260.0, 146.0));
    panel.setAspectRatio(size);

    let ground = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), size),
    );
    ground.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    // Kept, so landing can put the page back where it was (see Out::home).
    // SAFETY: plain reads of a live view and its superview.
    let home = unsafe { page.superview() }
        .map(|stage| (stage, page.frame(), page.autoresizingMask()))
        .or_else(|| HOMES.with_borrow(|homes| homes.get(&tab).cloned()));
    page.removeFromSuperview();
    // SAFETY: plain calls on a live web view; magnification comes back on landing,
    // as in Search, whose own pinch on the web view would eat the window's.
    unsafe { page.setAllowsMagnification(false) };
    page.setFrame(ground.bounds());
    page.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    ground.addSubview(&page);
    add_controls(mtm, &ground, size);
    panel.setContentView(Some(&ground));
    panel.orderFrontRegardless();
    OUT.with_borrow_mut(|slot| {
        *slot = Some(Out {
            panel,
            origin: tab,
            home,
            why,
        });
    });
}

/// Two quiet buttons over the picture: pause, and back to its tab. Shown always —
/// Search's fade in on hover, over a layer it draws; a small window whose only
/// job is the picture does not need the fade to be usable.
fn add_controls(mtm: MainThreadMarker, ground: &NSView, size: NSSize) {
    let target = target(mtm);
    for (title, action, x) in [
        ("❚❚", sel!(play_pause:), size.width - 76.0),
        ("↗", sel!(go_back:), size.width - 40.0),
    ] {
        // SAFETY: the target lives in TARGET for the app's life, and each action
        // takes the sender, as an action must.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&*target),
                Some(action),
                mtm,
            )
        };
        button.setBordered(false);
        button.setFrame(NSRect::new(
            NSPoint::new(x, size.height - 34.0),
            NSSize::new(30.0, 26.0),
        ));
        button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        ground.addSubview(&button);
    }
}

fn target(mtm: MainThreadMarker) -> Retained<Target> {
    TARGET.with_borrow_mut(|slot| slot.get_or_insert_with(|| Target::new(mtm)).clone())
}

/// Back into its tab: the isolation off, the page home, the window gone.
pub fn land() {
    let Some(out) = OUT.with_borrow_mut(Option::take) else {
        return;
    };
    if let Some(page) = web::existing(out.origin) {
        eval(&page, ISOLATE_OFF, |_| {});
        // SAFETY: a plain call on a live web view (see move_page).
        unsafe { page.setAllowsMagnification(true) };
        page.removeFromSuperview();
        if let Some((stage, frame, mask)) = &out.home {
            page.setFrame(*frame);
            page.setAutoresizingMask(*mask);
            stage.addSubview_positioned_relativeTo(
                &page,
                objc2_app_kit::NSWindowOrderingMode::Below,
                None,
            );
        }
    }
    out.panel.orderOut(None);
    out.panel.close();
}

/// Back to the tab the video came from, in front.
fn go_back() {
    let origin = OUT.with_borrow(|out| out.as_ref().map(|o| o.origin));
    land();
    if let Some(origin) = origin {
        rt::dispatch(move |browser| browser.select(origin));
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).activate();
        }
        crate::window::bring_forward();
    }
}

fn toggle_play() {
    let origin = OUT.with_borrow(|out| out.as_ref().map(|o| o.origin));
    let Some(page) = origin.and_then(web::existing) else {
        return;
    };
    eval(&page, TOGGLE, |_| {});
}

/// The video playing in `tab` into macOS's picture in picture (Search's
/// `NativePiP.script`); by hand, a paused one can go too, as in Safari.
fn picture(tab: TabId, quietly: bool, by_hand: bool) {
    let Some(page) = web::existing(tab) else {
        return;
    };
    let Some((url, blank, own_page)) = rt::read(|browser| {
        browser
            .tab(tab)
            .map(|t| (t.url().to_owned(), t.is_blank(), t.page().is_some()))
    })
    .flatten() else {
        return;
    };
    if blank || own_page {
        return;
    }
    if quietly && (!knows_player(&url) || grounded(&url)) {
        return;
    }
    allow_pip(&page, true);
    eval(&page, &pip_script(by_hand), move |value| {
        match value.as_ref().and_then(Value::as_str) {
            Some("in") => PICTURED.with_borrow_mut(|p| *p = Some(tab)),
            Some("out") => PICTURED.with_borrow_mut(|p| {
                if *p == Some(tab) {
                    *p = None;
                }
            }),
            // "none" and "refused": nothing to remember, and nothing said (above).
            _ => {}
        }
    });
}

/// Back into its page, whatever is in picture in picture there.
fn unpicture() {
    let Some(tab) = PICTURED.with_borrow_mut(Option::take) else {
        return;
    };
    if let Some(page) = web::existing(tab) {
        eval(&page, PIP_OFF, |_| {});
    }
}

fn eval(web: &WKWebView, script: &str, then: impl Fn(Option<Value>) + 'static) {
    web::evaluate_in_world(web, script, then);
}

/// Search's `NativePiP.script`, with `toggle` fixed by who asked: by hand it
/// toggles, on its own it only ever puts one in.
fn pip_script(toggle: bool) -> String {
    format!(
        r"(async () => {{
          var toggle = {};
          var videos = document.querySelectorAll('video');
          for (var i = 0; i < videos.length; i++) {{
            var v = videos[i];
            if (v.webkitPresentationMode === 'picture-in-picture') {{
              if (!toggle) return 'in';
              try {{ v.webkitSetPresentationMode('inline'); return 'out'; }} catch (e) {{ return 'refused'; }}
            }}
          }}
          var best = null, area = 0;
          for (var j = 0; j < videos.length; j++) {{
            var w = videos[j];
            if (w.paused || w.ended || w.readyState < 2) continue;
            var box = w.getBoundingClientRect();
            if (box.width * box.height >= area) {{ area = box.width * box.height; best = w; }}
          }}
          if (!best && toggle) {{
            for (var k = 0; k < videos.length; k++) {{
              var u = videos[k];
              if (u.readyState < 2) continue;
              var b = u.getBoundingClientRect();
              if (b.width * b.height >= area) {{ area = b.width * b.height; best = u; }}
            }}
          }}
          if (!best) return 'none';
          if (!best.webkitSupportsPresentationMode || !best.webkitSupportsPresentationMode('picture-in-picture')) return 'refused';
          try {{ best.webkitSetPresentationMode('picture-in-picture'); return 'in'; }} catch (e) {{ return 'refused'; }}
        }})()",
        if toggle { "true" } else { "false" }
    )
}

const PIP_OFF: &str = r"(async () => {
  var videos = document.querySelectorAll('video'), out = false;
  for (var i = 0; i < videos.length; i++) {
    if (videos[i].webkitPresentationMode === 'picture-in-picture') {
      try { videos[i].webkitSetPresentationMode('inline'); out = true; } catch (e) {}
    }
  }
  return out ? 'out' : 'none';
})()";

/// Search's `Isolate.on`, shortened (no snapshot geometry, no scale-to-fit trick):
/// everything but the largest playing video hidden, the video filling the window,
/// and the mark defended four times a second against a player rebuilding its DOM.
const ISOLATE_ON: &str = r"(async () => {
  var videos = document.querySelectorAll('video');
  var best = null, area = 0;
  for (var i = 0; i < videos.length; i++) {
    var v = videos[i];
    if (v.paused || v.ended || v.readyState < 2) continue;
    var box = v.getBoundingClientRect();
    if (box.width * box.height >= area) { area = box.width * box.height; best = v; }
  }
  if (!best) return { floating: false };
  best.setAttribute('data-office-float', '');
  var sheet = document.getElementById('office-float');
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = 'office-float';
    (document.head || document.documentElement).appendChild(sheet);
  }
  sheet.textContent = [
    'html.office-floating, html.office-floating body {background:#000 !important; overflow:hidden !important; margin:0 !important}',
    'html.office-floating body > * { visibility:hidden !important }',
    'html.office-floating [data-office-float] {',
    'visibility:visible !important; position:fixed !important;',
    'left:0 !important; top:0 !important; right:0 !important; bottom:0 !important;',
    'width:100vw !important; height:100vh !important;',
    'max-width:none !important; max-height:none !important;',
    'margin:0 !important; padding:0 !important; border:0 !important;',
    'transform:none !important; opacity:1 !important; object-fit:contain !important; z-index:2147483647 !important}',
    'html.office-floating body :has([data-office-float]) {',
    'overflow:visible !important; transform:none !important; filter:none !important;',
    'contain:none !important; content-visibility:visible !important; opacity:1 !important}',
    'html.office-floating [data-office-float]::-webkit-media-controls {display:none !important}'
  ].join('');
  document.documentElement.classList.add('office-floating');
  clearInterval(window.__officeFloatWatch);
  window.__officeFloatWatch = setInterval(function () {
    if (document.querySelector('[data-office-float]')) return;
    var again = null, most = 0;
    var all = document.querySelectorAll('video');
    for (var j = 0; j < all.length; j++) {
      var one = all[j];
      if (one.paused || one.ended || one.readyState < 2) continue;
      var shape = one.getBoundingClientRect();
      if (shape.width * shape.height >= most) { most = shape.width * shape.height; again = one; }
    }
    if (again) again.setAttribute('data-office-float', '');
  }, 250);
  return { floating: true };
})()";

/// Everything back as it was (Search's `Isolate.off`, without its settling watch).
const ISOLATE_OFF: &str = r"(async () => {
  try {
    var out = document.querySelector('video[data-office-float]') || document.querySelector('video');
    if (out) {
      if (out.webkitPresentationMode === 'picture-in-picture') out.webkitSetPresentationMode('inline');
      if (document.pictureInPictureElement && document.exitPictureInPicture) document.exitPictureInPicture();
    }
  } catch (e) {}
  clearInterval(window.__officeFloatWatch);
  window.__officeFloatWatch = null;
  document.documentElement.classList.remove('office-floating');
  var sheet = document.getElementById('office-float');
  if (sheet) sheet.textContent = '';
  var video = document.querySelector('[data-office-float]');
  if (video) video.removeAttribute('data-office-float');
  return 'landed';
})()";

const TOGGLE: &str = r"(async () => {
  var video = document.querySelector('[data-office-float]') || document.querySelector('video');
  if (!video) return true;
  if (video.paused) { video.play(); } else { video.pause(); }
  return !video.paused;
})()";

/// Sites with a player worth following into the little window (Search's `Players`,
/// shortened): anywhere else, a playing video is as likely to be a background as
/// a film. A host suffix, and for shops that also stream, the path that separates
/// the film from the product page.
const PLAYERS: &[(&str, Option<&str>)] = &[
    ("youtube.com", None),
    ("youtu.be", None),
    ("netflix.com", None),
    ("primevideo.com", None),
    ("amazon.com", Some("/gp/video")),
    ("amazon.co.uk", Some("/gp/video")),
    ("amazon.de", Some("/gp/video")),
    ("amazon.fr", Some("/gp/video")),
    ("disneyplus.com", None),
    ("tv.apple.com", None),
    ("twitch.tv", None),
    ("vimeo.com", None),
    ("dailymotion.com", None),
    ("max.com", None),
    ("hbomax.com", None),
    ("arte.tv", None),
    ("france.tv", None),
    ("crunchyroll.com", None),
    ("plex.tv", None),
    ("peacocktv.com", None),
    ("hulu.com", None),
    ("paramountplus.com", None),
    ("mubi.com", None),
    ("ted.com", None),
    ("nebula.tv", None),
];

#[must_use]
pub fn knows_player(url: &str) -> bool {
    let Some((host, path)) = host_path(url) else {
        return false;
    };
    PLAYERS.iter().any(|(known, needle)| {
        (host == *known || host.ends_with(&format!(".{known}")))
            && needle.is_none_or(|n| path.starts_with(n))
    })
}

fn host_path(url: &str) -> Option<(String, String)> {
    let rest = url.split_once("://")?.1;
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, String::new()),
    };
    let host = host.split([':', '@']).next_back()?.to_lowercase();
    Some((host, path.to_lowercase()))
}

/// A site whose videos never float out on their own (Search's `Grounded`): kept
/// by host under `nofloat.<host>`, as a site's other choices are.
fn grounded(url: &str) -> bool {
    let Some((host, _)) = host_path(url) else {
        return false;
    };
    index::flag(&prefs::Kept::new(), &format!("nofloat.{host}"))
}

define_class!(
    /// A panel that takes key status without bringing the whole app forward
    /// (Search's `Panel`): borderless windows refuse to become key by default,
    /// and a window that never becomes key stops getting its clicks.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFloatPanel"]
    struct Panel;

    impl Panel {
        #[unsafe(method(canBecomeKey))]
        fn can_become_key(&self) -> Bool {
            Bool::YES
        }
    }
);

impl Panel {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSPanel's designated initialiser, once, on a fresh allocation.
        unsafe {
            msg_send![
                super(this),
                initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        }
    }
}

define_class!(
    /// The little window's buttons' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFloatTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(play_pause:))]
        fn play_pause_action(&self, _sender: Option<&AnyObject>) {
            toggle_play();
        }

        #[unsafe(method(go_back:))]
        fn go_back_action(&self, _sender: Option<&AnyObject>) {
            go_back();
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_are_known_by_host_and_path() {
        assert!(knows_player("https://www.youtube.com/watch?v=1"));
        assert!(knows_player("https://music.youtube.com/watch"));
        assert!(knows_player("https://www.amazon.com/gp/video/detail/B0"));
        assert!(!knows_player("https://www.amazon.com/dp/B0"));
        assert!(!knows_player("https://example.com/watch"));
        assert!(!knows_player("not a url"));
    }

    #[test]
    fn pip_script_says_whether_it_toggles() {
        assert!(pip_script(true).contains("var toggle = true;"));
        assert!(pip_script(false).contains("var toggle = false;"));
    }
}
