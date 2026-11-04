//! The web views: one per tab that has been shown, made from one shared configuration,
//! with one delegate and one observer for all of them (Search's `Tab.swift` and the
//! `WKNavigationDelegate`/`WKUIDelegate` half of `Browser.swift`).

use std::{cell::RefCell, collections::HashMap, ffi::c_void, ptr, time::Instant};

use block2::{DynBlock, RcBlock};
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{NSEventModifierFlags, NSWindow, NSWindowStyleMask, NSWorkspace};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSHTTPURLResponse, NSInteger, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect, NSSize,
    NSString, NSURL, NSURLAuthenticationChallenge, NSURLCredential, NSURLRequest,
    NSURLSessionAuthChallengeDisposition,
};
#[allow(deprecated)]
use objc2_web_kit::WKProcessPool;
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKContentWorld, WKDownload, WKFrameInfo, WKNavigation,
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKNavigationType, WKOpenPanelParameters, WKPreferences,
    WKSecurityOrigin, WKUIDelegate, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    WKWindowFeatures,
};
use serde_json::Value;
use tap_core::{
    browser::{Effect, TabId},
    crash::PageEnd,
    fetching,
    fnd::Url,
    identity,
};

use crate::{dialogs, downloads, forms, icons, oslog, page_find, rt, signin, sleep, store, window};

#[path = "shield.rs"]
mod shield;

/// What the observer watches on every web view; each change re-reads the lot.
const OBSERVED: [&str; 7] = [
    "title",
    "URL",
    "loading",
    "estimatedProgress",
    "canGoBack",
    "canGoForward",
    "hasOnlySecureContent",
];

thread_local! {
    static PAGES_WAS_FAST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static WEBS: RefCell<HashMap<TabId, Retained<WKWebView>>> = RefCell::new(HashMap::new());
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
    static PAGES: std::cell::Cell<Pages> = const { std::cell::Cell::new(Pages::DEFAULT) };
}

/// What Settings says every page does (Search's `Web.configuration` and `Privacy.swift`):
/// set at launch and whenever a setting changes.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)] // one bool per Settings switch, read at a glance
struct Pages {
    /// `pageZoom`: where every site starts.
    zoom: f64,
    /// `media.click`: videos wait for a click, with sound or without.
    waits: bool,
    /// `privacy.clickIDs`: WebKit takes gclid, fbclid and the like off an address.
    click_ids: bool,
    /// `privacy.gpc`: Global Privacy Control with every request.
    gpc: bool,
    /// `tabs.popupWindows`: a page asking for a window at its own size gets one.
    popups: bool,
    /// `pages.120`: pages drawn past 60 frames a second right now (mode and power).
    fast: bool,
}

impl Pages {
    const DEFAULT: Pages = Pages {
        zoom: 1.0,
        waits: false,
        click_ids: true,
        gpc: true,
        popups: false,
        fast: false,
    };
}

/// Settings changed: every page made from now on follows them, and the open ones take
/// the zoom and the click-ID switch at once (WebKit reads that one on each navigation).
/// Autoplay and GPC are fixed when a page's view is made: a tab woken or opened from now
/// on has them.
pub fn apply_prefs(kept: &crate::prefs::Kept) {
    use tap_core::settings_index as index;
    let pages = Pages {
        zoom: index::value(kept, "pageZoom")
            .as_f64()
            .filter(|z| *z > 0.0)
            .unwrap_or(1.0),
        waits: index::flag(kept, "media.click"),
        click_ids: index::flag(kept, "privacy.clickIDs"),
        gpc: index::flag(kept, "privacy.gpc"),
        popups: index::flag(kept, "tabs.popupWindows"),
        fast: fast_pages(kept),
    };
    let was = PAGES.get();
    PAGES_WAS_FAST.with(|w| w.set(was.fast));
    PAGES.set(pages);
    let dusk = index::flag(kept, "pages.dusk");
    let autoscroll = index::flag(kept, "autoscroll");
    let shield_on = index::flag(kept, "shield");
    let paused = paused_sites(kept);
    shield::set(shield_on, paused.clone());
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    apply_sites_keep(mtm, index::flag(kept, "sites.keep"));
    apply_autocorrect(kept);
    escape::apply(index::flag(kept, "fullscreen.escapeTwice"));
    if pages.fast && index::text(kept, "pages.120.mode") == "onPower" {
        power::watch();
    }
    if shield_on {
        // Compiled once; when it lands, the open pages get it (below, and in
        // `shield_ready`). While it compiles, new pages are tuned as they register
        // and again when it is ready.
        shield::compile(mtm, shield_ready);
    }
    WEBS.with_borrow(|webs| {
        for web in webs.values() {
            take_prefs(web, pages);
            sync_scripts(mtm, web, dusk, shield_on, &paused, autoscroll);
            shield::tune(&controller_of(web), host_of(web).as_deref());
            if !dusk {
                // A page already darkened by the script lets go at once; pages
                // opened from now on simply never get the script.
                evaluate(web, "window.__tapDusk && window.__tapDusk.off()");
            }
            // Autoscroll reaches a page already up at once, and leaves it the
            // same way; its script in the next document follows the switch.
            evaluate(
                web,
                if autoscroll {
                    AUTOSCROLL_SCRIPT
                } else {
                    AUTOSCROLL_OFF
                },
            );
        }
    });
    if was.fast != pages.fast {
        nudge();
    }
}

/// The shield's list landed: every open page gets it for the site it is on.
fn shield_ready() {
    WEBS.with_borrow(|webs| {
        for web in webs.values() {
            shield::tune(&controller_of(web), host_of(web).as_deref());
        }
    });
}

/// A page's content controller: its scripts and Shield's list live there.
fn controller_of(web: &WKWebView) -> Retained<WKUserContentController> {
    // SAFETY: plain property reads on a live web view and its configuration.
    unsafe { web.configuration().userContentController() }
}

/// The sites Shield is paused on, as Settings keeps them.
fn paused_sites(kept: &crate::prefs::Kept) -> Vec<String> {
    use tap_core::settings_index::Prefs as _;
    match kept.get("shield.paused") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// This page's host, as sites are kept everywhere else: no `www.`.
fn host_of(web: &WKWebView) -> Option<String> {
    url_of(web)
        .and_then(|address| Url::parse(&address))
        .and_then(|url| url.host())
        .map(|host| shield::host_key(&host))
}

/// Settings › Privacy › Prevent cross-site tracking, turned off (`sites.keep` on):
/// WebKit's tracking prevention off and cookies always accepted, as Safari's same
/// switch does (Search's `Store.followSignIns`). Both live on the data store every
/// tab shares, so open pages follow from their next request. The statistics switch
/// is not in the public framework, so it is asked for first.
fn apply_sites_keep(mtm: MainThreadMarker, keeps: bool) {
    // SAFETY: the default store Tap's pages use, on the main thread.
    let store = unsafe { WKWebsiteDataStore::defaultDataStore(mtm) };
    let statistics = objc2::sel!(_setResourceLoadStatisticsEnabled:);
    // SAFETY: asked first whether the store answers the selector, which takes a BOOL.
    unsafe {
        let answers: bool = msg_send![&*store, respondsToSelector: statistics];
        if answers {
            // Preventing cross-site tracking is the switch the Settings row shows:
            // on (the row lit) is statistics on, which is `sites.keep` off.
            let _: () = msg_send![&*store, _setResourceLoadStatisticsEnabled: Bool::new(!keeps)];
        }
    }
    // Off, framed sites get their cookies again, as in Chrome; on, only the page's
    // own site's are taken (Search's `followCookies`).
    // SAFETY: a plain property read on the store.
    let jar = unsafe { store.httpCookieStore() };
    let policy = objc2::sel!(_setCookieAcceptPolicy:completionHandler:);
    let done = RcBlock::new(|| {});
    // HTTPCookie's own numbers: always 0, only-from-main-document-domain 2.
    let accept: NSInteger = if keeps { 0 } else { 2 };
    // SAFETY: asked first; the selector takes an integer policy and a block.
    unsafe {
        let answers: bool = msg_send![&*jar, respondsToSelector: policy];
        if answers {
            let _: () = msg_send![&*jar, _setCookieAcceptPolicy: accept, completionHandler: &*done];
        }
    }
}

/// Correct spelling as you type (the find-only `autocorrect` row, now in Edit ›
/// Spelling and Grammar as in Safari): WebKit's text checker takes its orders from
/// the app's standard defaults — the real ones, not a test suite, because it is
/// WebKit reading them (Search's `Preferences.tellWebKit`). Written at launch,
/// before the first web view exists, and whenever Settings moves.
fn apply_autocorrect(kept: &crate::prefs::Kept) {
    use tap_core::settings_index::Prefs as _;
    let on = match kept.get("autocorrect") {
        Some(value) => value.as_bool().unwrap_or(true),
        // Never set: as the Mac itself has it, as Search starts from.
        None => system_autocorrect(),
    };
    objc2_foundation::NSUserDefaults::standardUserDefaults().setBool_forKey(
        on,
        &NSString::from_str("WebAutomaticSpellingCorrectionEnabled"),
    );
}

/// Whether macOS itself corrects spelling, until the switch is used.
fn system_autocorrect() -> bool {
    let checker = objc2::class!(NSSpellChecker);
    // SAFETY: a class property asking the shared checker's own setting.
    unsafe {
        let answers: bool = msg_send![checker, respondsToSelector: objc2::sel!(isAutomaticSpellingCorrectionEnabled)];
        answers && msg_send![checker, isAutomaticSpellingCorrectionEnabled]
    }
}

/// Pages at 120 Hz right now: Settings' three choices (`pages.120.mode`, or the
/// older `pages.120` switch a profile may still carry), and, for "on power",
/// whether the Mac is on its adapter (Search's `FrameRate`).
fn fast_pages(kept: &crate::prefs::Kept) -> bool {
    use tap_core::settings_index::{self as index, Prefs as _};
    if kept.get("pages.120.mode").is_none()
        && let Some(legacy) = kept.get("pages.120").and_then(|v| v.as_bool())
    {
        return legacy;
    }
    match index::text(kept, "pages.120.mode").as_str() {
        "always" => true,
        "never" => false,
        _ => power::on_power(),
    }
}

/// WebKit works a page's rate out again only when the page is hidden or shown, so
/// the pages on screen are hidden for a turn of the run loop and shown again — too
/// short to be seen (Search's `FrameRate.nudge`). Whoever held the keys in a page
/// gets them back: a hidden key view hands them on and doesn't reclaim them.
fn nudge() {
    let shown: Vec<Retained<WKWebView>> = WEBS.with_borrow(|webs| {
        webs.values()
            .filter(|web| web.window().is_some() && !web.isHiddenOrHasHiddenAncestor())
            .cloned()
            .collect()
    });
    if shown.is_empty() {
        return;
    }
    let focus: Vec<(
        Retained<objc2_app_kit::NSWindow>,
        Retained<objc2_app_kit::NSView>,
    )> = shown
        .iter()
        .filter_map(|web| {
            let window = web.window()?;
            let held = window.firstResponder()?;
            let held = held.downcast::<objc2_app_kit::NSView>().ok()?;
            let inside = held.isDescendantOf(web);
            inside.then_some((window, held))
        })
        .collect();
    for web in &shown {
        web.setHidden(true);
    }
    // Back on in a turn of the run loop: a timer, whose block stays on this
    // thread with the views it holds, rather than a queue that would send them.
    let back = RcBlock::new(
        move |_timer: std::ptr::NonNull<objc2_foundation::NSTimer>| {
            for web in &shown {
                web.setHidden(false);
            }
            for (window, held) in &focus {
                let now = window.firstResponder();
                let same = now.as_ref().is_some_and(|responder| {
                    ptr::eq(
                        Retained::as_ptr(responder).cast::<std::ffi::c_void>(),
                        Retained::as_ptr(held).cast::<std::ffi::c_void>(),
                    )
                });
                if !same {
                    window.makeFirstResponder(Some(held));
                }
            }
        },
    );
    // SAFETY: a one-shot timer on the main run loop, fired a millisecond on.
    unsafe {
        objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            0.001, false, &back,
        );
    }
}

/// Whether this Mac's WebKit can do what a row of Settings asks (Search hides the switch
/// otherwise): Global Privacy Control and WebKit's click-ID filter come with macOS 27.
pub fn offers(row: &str) -> bool {
    match row {
        "privacy.gpc" => {
            let gpc = objc2::sel!(setGlobalPrivacyControlEnabled:);
            // SAFETY: a class method asking whether instances answer a selector.
            unsafe {
                msg_send![
                    <objc2_web_kit::WKWebpagePreferences as objc2::ClassType>::class(),
                    instancesRespondToSelector: gpc
                ]
            }
        }
        "privacy.clickIDs" => {
            let at_least = objc2_foundation::NSOperatingSystemVersion {
                majorVersion: 27,
                minorVersion: 0,
                patchVersion: 0,
            };
            feature("FilterLinkDecorationByDefaultEnabled").is_some()
                && objc2_foundation::NSProcessInfo::processInfo()
                    .isOperatingSystemAtLeastVersion(at_least)
        }
        // Pages past 60 need WebKit's near-60 flag to turn off; without it the
        // choice would promise what this WebKit can't do.
        "pages.120" => feature("PreferPageRenderingUpdatesNear60FPSEnabled").is_some(),
        // Preventing cross-site tracking needs the store's private statistics
        // switch; without it the row would move and nothing would follow.
        "sites.keep" => {
            let Some(mtm) = MainThreadMarker::new() else {
                return true;
            };
            // SAFETY: the default store, asked whether it answers the selector.
            unsafe {
                let store = WKWebsiteDataStore::defaultDataStore(mtm);
                msg_send![&*store, respondsToSelector: objc2::sel!(_setResourceLoadStatisticsEnabled:)]
            }
        }
        _ => true,
    }
}

/// The settings a view takes after it is made.
fn take_prefs(web: &WKWebView, pages: Pages) {
    // SAFETY: plain property calls on a live web view and its own preferences.
    unsafe {
        if (web.pageZoom() - pages.zoom).abs() > 0.004 {
            web.setPageZoom(pages.zoom);
        }
        set_feature(
            &web.configuration().preferences(),
            "FilterLinkDecorationByDefaultEnabled",
            pages.click_ids,
        );
        // WebKit holds a page near 60 frames a second unless told otherwise at
        // the preferences (Search's FrameRate): fast takes the hold off. "Never"
        // leaves the flag untouched, as Search does, until a page that was fast
        // needs WebKit's own rate back.
        if pages.fast || PAGES_WAS_FAST.get() {
            set_feature(
                &web.configuration().preferences(),
                "PreferPageRenderingUpdatesNear60FPSEnabled",
                !pages.fast,
            );
        }
    }
}

/// The settings a view has to be made with, put on the configuration it is made from.
fn make_with(configuration: &WKWebViewConfiguration, pages: Pages) {
    // SAFETY: plain property setters on a configuration not yet used to make a view.
    unsafe {
        configuration.setMediaTypesRequiringUserActionForPlayback(if pages.waits {
            WKAudiovisualMediaTypes::All
        } else {
            WKAudiovisualMediaTypes::Audio
        });
        let webpage = configuration.defaultWebpagePreferences();
        let gpc = objc2::sel!(setGlobalPrivacyControlEnabled:);
        // From macOS 27; on a WebKit without it nothing is sent.
        let answers: bool = msg_send![&*webpage, respondsToSelector: gpc];
        if answers {
            let _: () = msg_send![&*webpage, setGlobalPrivacyControlEnabled: Bool::new(pages.gpc)];
        }
    }
}

/// One of WebKit's feature flags, by its key, on `preferences` (Search's `ClickIDs.set`):
/// not in the public framework, so each step is asked first.
fn set_feature(preferences: &WKPreferences, key: &str, on: bool) {
    let Some(feature) = feature(key) else {
        return;
    };
    let set = objc2::sel!(_setEnabled:forFeature:);
    // SAFETY: asked first whether the preferences answer the selector, which takes a BOOL
    // and one of WebKit's own feature objects.
    unsafe {
        let answers: bool = msg_send![preferences, respondsToSelector: set];
        if answers {
            let _: () = msg_send![preferences, _setEnabled: Bool::new(on), forFeature: &*feature];
        }
    }
}

/// WebKit's feature object for a key, looked up once.
fn feature(key: &str) -> Option<Retained<AnyObject>> {
    thread_local! {
        static FEATURES: RefCell<HashMap<String, Option<Retained<AnyObject>>>> = RefCell::new(HashMap::new());
    }
    FEATURES.with_borrow_mut(|known| {
        known
            .entry(key.to_owned())
            .or_insert_with(|| {
                let list = objc2::sel!(_features);
                let class = <WKPreferences as objc2::ClassType>::class();
                // SAFETY: asked first whether the class answers `_features`, an array of
                // `_WKFeature`s, each with a string `key`.
                unsafe {
                    let answers: bool = msg_send![class, respondsToSelector: list];
                    if !answers {
                        return None;
                    }
                    let all: Option<Retained<NSArray<AnyObject>>> = msg_send![class, _features];
                    all?.iter().find(|f| {
                        let name: Option<Retained<NSString>> = msg_send![&**f, key];
                        name.is_some_and(|n| n.to_string() == key)
                    })
                }
            })
            .clone()
    })
}

struct Shared {
    configuration: Retained<WKWebViewConfiguration>,
    delegate: Retained<WebDelegate>,
    observer: Retained<Observer>,
}

fn shared<R>(mtm: MainThreadMarker, f: impl FnOnce(&Shared) -> R) -> R {
    SHARED.with_borrow_mut(|slot| {
        let shared = slot.get_or_insert_with(|| Shared {
            configuration: configuration(mtm),
            delegate: WebDelegate::new(mtm),
            observer: Observer::new(mtm),
        });
        f(shared)
    })
}

/// One configuration for every tab, as Search's `Web.configuration`. WebKit copies it
/// into each view it makes.
fn configuration(mtm: MainThreadMarker) -> Retained<WKWebViewConfiguration> {
    // SAFETY: plain property setters on a fresh configuration, on the main thread.
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        // One pool for every tab: a configuration without it gets a pool of its own,
        // and every new tab then started a web process from cold (41-59 ms against
        // 9-10 ms, measured in Search, Tab.swift).
        // Deprecated, and said to do nothing now, but measured in Search to matter.
        #[allow(deprecated)]
        configuration.setProcessPool(&WKProcessPool::new(mtm));
        // Left alone, WKWebView names no browser and no version, and Google serves the
        // stripped-back page from a decade ago. The Safari this Mac has is the WebKit
        // every tab runs on, so its version is the honest one to give.
        configuration.setApplicationNameForUserAgent(Some(&NSString::from_str(&format!(
            "Version/{} Safari/605.1.15",
            safari_version()
        ))));
        configuration.setAllowsAirPlayForMediaPlayback(true);
        // Off by default on macOS: a video's full-screen button did nothing at all.
        configuration
            .preferences()
            .setElementFullscreenEnabled(true);
        // The sign-in script's ear, on the controller every page shares.
        forms::install(mtm, &configuration.userContentController());
        configuration
    }
}

/// The installed Safari's version, or the one this macOS shipped with.
fn safari_version() -> String {
    for path in [
        "/System/Cryptexes/App/System/Applications/Safari.app",
        "/Applications/Safari.app",
    ] {
        let version = NSBundle::bundleWithPath(&NSString::from_str(path))
            .and_then(|bundle| {
                bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
            })
            .and_then(|value| value.downcast::<NSString>().ok());
        if let Some(version) = version {
            return version.to_string();
        }
    }
    "26.0".to_owned()
}

/// The tab's web view, made the first time it is asked for.
fn web_view(mtm: MainThreadMarker, tab: TabId) -> Retained<WKWebView> {
    if let Some(web) = existing(tab) {
        return web;
    }
    let web = shared(mtm, |shared| {
        make_with(&shared.configuration, PAGES.get());
        // SAFETY: a fresh allocation, initialised once with a valid configuration.
        unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                NSRect::ZERO,
                &shared.configuration,
            )
        }
    });
    register(mtm, tab, &web);
    web
}

/// Takes a web view into Tap's care: its delegates and observers, under its tab.
fn register(mtm: MainThreadMarker, tab: TabId, web: &Retained<WKWebView>) {
    shared(mtm, |shared| {
        // SAFETY: the delegate and the observer live for the app's whole life in
        // `SHARED`, longer than any web view; observers are removed in `discard`.
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*shared.delegate)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(&*shared.delegate)));
            web.setAllowsBackForwardNavigationGestures(true);
            web.setAllowsMagnification(true);
            web.setInspectable(true);
            for key in OBSERVED {
                web.addObserver_forKeyPath_options_context(
                    &shared.observer,
                    &NSString::from_str(key),
                    NSKeyValueObservingOptions::New,
                    ptr::null_mut(),
                );
            }
        }
    });
    take_prefs(web, PAGES.get());
    {
        use tap_core::settings_index as index;
        let kept = crate::prefs::Kept::new();
        let paused = paused_sites(&kept);
        // A page made after Settings moved still starts with what Settings says:
        // its scripts for its next document, and Shield for its first request.
        sync_scripts(
            mtm,
            web,
            index::flag(&kept, "pages.dusk"),
            index::flag(&kept, "shield"),
            &paused,
            index::flag(&kept, "autoscroll"),
        );
        shield::tune(&controller_of(web), host_of(web).as_deref());
    }
    WEBS.with_borrow_mut(|webs| webs.insert(tab, web.clone()));
}

/// The user scripts a page carries into its next document, rebuilt whenever
/// Settings moves so a switch reaches pages already open at their next load and
/// new pages at their first. Tap puts no other user scripts on a tab, so the
/// controller's list is rebuilt whole rather than picked at.
fn sync_scripts(
    mtm: MainThreadMarker,
    web: &WKWebView,
    dusk: bool,
    shield_on: bool,
    paused: &[String],
    autoscroll: bool,
) {
    let controller = controller_of(web);
    // SAFETY: plain calls on a live page's own controller.
    unsafe {
        controller.removeAllUserScripts();
        if shield_on {
            controller.addUserScript(&user_script(
                mtm,
                &shield::youtube_page_script(paused),
                true,
                false,
            ));
            controller.addUserScript(&user_script(mtm, shield::YOUTUBE_SKIPPER, true, true));
        }
        if dusk {
            controller.addUserScript(&user_script(mtm, DUSK_SCRIPT, false, false));
        }
        if autoscroll {
            controller.addUserScript(&user_script(mtm, AUTOSCROLL_SCRIPT, false, false));
        }
        // Sign-ins, always: Search's script at the document's end, in Tap's world, the
        // main frame only (Tab.swift).
        controller.addUserScript(
            &WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &NSString::from_str(forms::SCRIPT),
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
                &world(mtm),
            ),
        );
    }
}

/// One script: at the document's start, ahead of the page's own. Tap's own world
/// is where a page can't see or replace what runs (the YouTube skipper); the page
/// world is for the ones that must stand in the page's way (its player data, its
/// colours), visible there as Search's are.
fn user_script(
    mtm: MainThreadMarker,
    source: &str,
    all_frames: bool,
    own_world: bool,
) -> Retained<WKUserScript> {
    // SAFETY: a script body and WebKit's own injection choices.
    unsafe {
        if own_world {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &NSString::from_str(source),
                WKUserScriptInjectionTime::AtDocumentStart,
                !all_frames,
                &world(mtm),
            )
        } else {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(source),
                WKUserScriptInjectionTime::AtDocumentStart,
                !all_frames,
            )
        }
    }
}

/// Runs a snippet in the page's own world and keeps the answer to itself: dusk
/// letting go, where the script's own `off` is the whole conversation.
fn evaluate(web: &WKWebView, source: &str) {
    let done = RcBlock::new(|_value: *mut AnyObject, _error: *mut NSError| {});
    // SAFETY: a live web view and a block that outlives the call.
    unsafe {
        web.evaluateJavaScript_completionHandler(&NSString::from_str(source), Some(&done));
    }
}

/// Dark pages for sites with no dark look of their own (Search's `Dusk.swift`, in
/// its smallest working shape): WebKit has no switch for it, so while the frame
/// is dark (`prefers-color-scheme: dark` in the page) and the page itself
/// measures light, its colours are turned over with the filter Search falls back
/// to, and its pictures are turned back. A site already dark, or a frame not
/// dark, is left exactly as its site made it. The measuring reads only the
/// grounds the page paints its root and body with; deeper sampling is Search's
/// full engine, not this.
const DUSK_SCRIPT: &str = r"(function () {
  if (window.__tapDusk) { window.__tapDusk.refresh(); return; }
  var style = null;
  var scheme = matchMedia('(prefers-color-scheme: dark)');
  function luminance(colour) {
    var m = /rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)/.exec(colour || '');
    if (!m || (m[4] !== undefined && parseFloat(m[4]) < 0.5)) return null;
    function lin(c) { c /= 255; return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); }
    return 0.2126 * lin(+m[1]) + 0.7152 * lin(+m[2]) + 0.0722 * lin(+m[3]);
  }
  function light() {
    if (!document.body) return false;
    var top = luminance(getComputedStyle(document.documentElement).backgroundColor);
    if (top !== null) return top > 0.35;
    var body = luminance(getComputedStyle(document.body).backgroundColor);
    return body === null ? true : body > 0.35;
  }
  function apply() {
    if (!scheme.matches || !document.body) { off(); return; }
    if (!light()) { off(); return; }
    if (style) return;
    style = document.createElement('style');
    style.textContent = '@media screen {'
      + 'html { filter: invert(1) hue-rotate(180deg) contrast(.85) !important; }'
      + 'img, video, picture, canvas, embed, object { filter: contrast(1.1765) hue-rotate(180deg) invert(1) !important; }'
      + 'html:has(:fullscreen), html:has(:fullscreen) img, html:has(:fullscreen) video { filter: none !important; }'
      + '}';
    (document.head || document.documentElement).appendChild(style);
  }
  function off() { if (style) { style.remove(); style = null; } }
  window.__tapDusk = { refresh: apply, off: off };
  document.addEventListener('DOMContentLoaded', apply);
  addEventListener('load', apply);
  if (scheme.addEventListener) scheme.addEventListener('change', apply);
  apply();
})();";

/// Scroll with the middle button, as on Windows (Search's `AutoScroll.swift`,
/// renamed to Tap's mark): a click of the wheel on a page (not on a link, which
/// the middle button still opens beside this tab) leaves a mark, and the page
/// scrolls towards the pointer, faster the further away it is. Another click,
/// Escape or the wheel stops it; held down and dragged, it stops on release.
/// Done in the page, which knows what is under the pointer and what scrolls.
const AUTOSCROLL_SCRIPT: &str = r#"(() => {
  window.__tapAutoScrollOff = false;
  if (window.__tapAutoScroll) return;
  window.__tapAutoScroll = true;
  let active = null;
  let swallow = -1;
  const scroller = (el) => {
    for (; el && el !== document.body && el !== document.documentElement; el = el.parentElement) {
      const s = getComputedStyle(el);
      if (/(auto|scroll|overlay)/.test(s.overflowY + s.overflowX) &&
          (el.scrollHeight > el.clientHeight + 1 || el.scrollWidth > el.clientWidth + 1)) return el;
    }
    return document.scrollingElement || document.documentElement;
  };
  const mark = (x, y) => {
    const host = document.createElement('div');
    host.style.cssText = 'all:initial;position:fixed;z-index:2147483647;pointer-events:none;' +
      `left:${x - 15}px;top:${y - 15}px;width:30px;height:30px;`;
    host.attachShadow({ mode: 'closed' }).innerHTML =
      '<svg viewBox="0 0 30 30" width="30" height="30" style="filter:drop-shadow(0 2px 6px rgba(0,0,0,.25))">' +
      '<circle cx="15" cy="15" r="13.5" fill="rgba(255,255,255,.94)" stroke="rgba(0,0,0,.18)"/>' +
      '<path d="M15 6.5l3.5 4.5h-7zM15 23.5l3.5-4.5h-7z" fill="rgba(0,0,0,.62)"/>' +
      '<circle cx="15" cy="15" r="1.6" fill="rgba(0,0,0,.62)"/></svg>';
    document.documentElement.appendChild(host);
    return host;
  };
  const speed = (d) => {
    const a = Math.abs(d) - 12;
    return a > 0 ? Math.sign(d) * Math.min(60, Math.pow(a / 10, 1.4)) : 0;
  };
  const tick = () => {
    if (!active) return;
    active.target.scrollBy(speed(active.dx), speed(active.dy));
    active.frame = requestAnimationFrame(tick);
  };
  const move = (e) => {
    if (!active) return;
    active.dx = e.clientX - active.x;
    active.dy = e.clientY - active.y;
  };
  const stop = () => {
    if (!active) return;
    cancelAnimationFrame(active.frame);
    active.badge.remove();
    document.documentElement.style.cursor = active.cursor;
    removeEventListener('mousemove', move, true);
    active = null;
  };
  addEventListener('mousedown', (e) => {
    swallow = -1;
    if (active) { e.preventDefault(); e.stopPropagation(); stop(); swallow = e.button; return; }
    if (e.button !== 1 || window.__tapAutoScrollOff) return;
    if (e.target.closest && e.target.closest('a[href], area[href], input, textarea, select, button, video, audio, iframe, [contenteditable=""], [contenteditable="true"]')) return;
    e.preventDefault();
    active = {
      x: e.clientX, y: e.clientY, dx: 0, dy: 0, since: performance.now(),
      target: scroller(e.target), badge: mark(e.clientX, e.clientY),
      cursor: document.documentElement.style.cursor,
    };
    document.documentElement.style.cursor = 'all-scroll';
    addEventListener('mousemove', move, true);
    active.frame = requestAnimationFrame(tick);
  }, true);
  addEventListener('mouseup', (e) => {
    if (active && e.button === 1 && performance.now() - active.since > 250 &&
        (Math.abs(active.dx) > 12 || Math.abs(active.dy) > 12)) { stop(); swallow = 1; }
  }, true);
  const eat = (e) => {
    if (e.button === swallow) { swallow = -1; e.preventDefault(); e.stopPropagation(); }
    else if (e.button === 1 && active) e.preventDefault();
  };
  addEventListener('click', eat, true);
  addEventListener('auxclick', eat, true);
  addEventListener('keydown', (e) => { if (active && e.key === 'Escape') { e.preventDefault(); stop(); } }, true);
  addEventListener('wheel', stop, { capture: true, passive: true });
  addEventListener('blur', stop);
  document.addEventListener('visibilitychange', stop);
})();"#;

/// What turning autoscroll off says to a page already up (Search's `AutoScroll.off`).
const AUTOSCROLL_OFF: &str = "window.__tapAutoScrollOff = true;";

/// The tab's web view, if it has been made.
pub fn existing(tab: TabId) -> Option<Retained<WKWebView>> {
    WEBS.with_borrow(|webs| webs.get(&tab).cloned())
}

/// Tap's own content world: the page can't see into it, so it can't read the word looked
/// for or the list the count comes from, or swap the functions the search uses.
///
/// Kept for the app's life, as Search keeps its `Web.world`: WebKit lets a world go with
/// the last object naming it, and the next call got a fresh one, with none of the find's
/// state in it. Every Next then started again from the first match.
pub fn world(mtm: MainThreadMarker) -> Retained<WKContentWorld> {
    thread_local! {
        static WORLD: std::cell::OnceCell<Retained<WKContentWorld>> = const { std::cell::OnceCell::new() };
    }
    WORLD.with(|world| {
        world
            .get_or_init(|| {
                // SAFETY: a world named once and shared by every page.
                unsafe { WKContentWorld::worldWithName(&NSString::from_str(identity::NAME), mtm) }
            })
            .clone()
    })
}

/// Evaluates an expression in Tap's own content world, where the page can't see it: for
/// sleep's look at what is typed, and for the bench.
pub fn evaluate_in_world(
    web: &WKWebView,
    expression: &str,
    then: impl Fn(Option<Value>) + 'static,
) {
    let body = format!("return JSON.stringify(await (async () => ({expression}))());");
    let block = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
        // SAFETY: WebKit hands a valid object or null, for the call's length.
        let text = unsafe { value.as_ref() }
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(ToString::to_string);
        then(text.and_then(|t| serde_json::from_str(&t).ok()));
    });
    // SAFETY: a live web view, a script body, and a block that outlives the call.
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&body),
            None,
            None,
            &world(MainThreadMarker::from(web)),
            Some(&block),
        );
    }
}

/// Runs a function body in Tap's own world with `arguments` as its named strings: a
/// value never spliced into the source, so a password with a quote in it is just a
/// password. The answer comes back as JSON.
pub fn call_in_world(
    web: &WKWebView,
    body: &str,
    arguments: &[(&str, &str)],
    then: impl Fn(Option<Value>) + 'static,
) {
    let body = format!("return JSON.stringify(await (async () => ({body}))());");
    let keys: Vec<Retained<NSString>> = arguments.iter().map(|a| NSString::from_str(a.0)).collect();
    let values: Vec<Retained<NSString>> =
        arguments.iter().map(|a| NSString::from_str(a.1)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = values.iter().map(AsRef::as_ref).collect();
    let arguments: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&keys, &values);
    let block = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
        // SAFETY: WebKit hands a valid object or null, for the call's length.
        let text = unsafe { value.as_ref() }
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(ToString::to_string);
        then(text.and_then(|t| serde_json::from_str(&t).ok()));
    });
    // SAFETY: a live web view, a script body, its arguments, and a block that outlives
    // the call.
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&body),
            Some(&arguments),
            None,
            &world(MainThreadMarker::from(web)),
            Some(&block),
        );
    }
}

/// Lets the tab's page go (sleep): the view, its observers and its delegates.
pub fn discard_page(tab: TabId) {
    if let Some(mtm) = MainThreadMarker::new() {
        discard(mtm, tab);
    }
}

/// The address a web view is on.
pub fn url_of(web: &WKWebView) -> Option<String> {
    // SAFETY: a plain property read on a live web view.
    unsafe { web.URL() }
        .and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
}

/// The page's WebContent process, through WebKit's private `_webProcessIdentifier`; 0
/// when this WebKit doesn't say.
pub fn process_id(web: &WKWebView) -> i32 {
    let selector = objc2::sel!(_webProcessIdentifier);
    // SAFETY: asked first whether the web view answers the selector; it returns a pid_t.
    unsafe {
        let answers: bool = msg_send![web, respondsToSelector: selector];
        if answers {
            msg_send![web, _webProcessIdentifier]
        } else {
            0
        }
    }
}

pub(crate) fn tab_of(web: &WKWebView) -> Option<TabId> {
    WEBS.with_borrow(|webs| {
        webs.iter()
            .find(|(_, w)| ptr::eq(Retained::as_ptr(w), web))
            .map(|(id, _)| *id)
    })
}

fn discard(mtm: MainThreadMarker, tab: TabId) {
    dialogs::drop_held(tab);
    forms::forget_tab(tab);
    signin::lower_now(tab);
    let Some(web) = WEBS.with_borrow_mut(|webs| webs.remove(&tab)) else {
        return;
    };
    shared(mtm, |shared| {
        for key in OBSERVED {
            // SAFETY: each key was added exactly once, in `register`.
            unsafe { web.removeObserver_forKeyPath(&shared.observer, &NSString::from_str(key)) };
        }
    });
    // SAFETY: plain calls on a live web view; the delegates are cleared so a page
    // still unloading can't call into a tab that is gone.
    unsafe {
        web.stopLoading();
        web.setNavigationDelegate(None);
        web.setUIDelegate(None);
    }
    web.removeFromSuperview();
}

/// Carries out one thing the model asked for.
pub fn perform(effect: Effect) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    match effect {
        Effect::Load { tab, url } => {
            let page = web_view(mtm, tab);
            // A tab that slept has its own history to go back to: the page, its back
            // list and its scroll position, in one. A file starts from its address, as
            // its history comes back without the folder it may read.
            match sleep::take_memory(tab).filter(|_| !url.starts_with("file:")) {
                // SAFETY: the state WebKit gave this tab's view as it slept.
                Some(state) => unsafe { page.setInteractionState(Some(&state)) },
                None => load(&page, &url),
            }
        }
        Effect::Reload(tab) => {
            // SAFETY: plain call on a live web view.
            let _ = existing(tab).map(|web| unsafe { web.reload() });
        }
        Effect::Back(tab) => {
            // SAFETY: plain call on a live web view.
            let _ = existing(tab).map(|web| unsafe { web.goBack() });
        }
        Effect::Forward(tab) => {
            // SAFETY: plain call on a live web view.
            let _ = existing(tab).map(|web| unsafe { web.goForward() });
        }
        Effect::Stop(tab) => {
            // SAFETY: plain call on a live web view.
            let _ = existing(tab).map(|web| unsafe { web.stopLoading() });
        }
        Effect::Discard(tab) => {
            sleep::take_memory(tab);
            discard(mtm, tab);
        }
        // The only tab was already new: the window goes, as Search closes it.
        Effect::CloseWindow => {
            if let Some(window) = window::main_window() {
                window.performClose(None);
            }
        }
        Effect::Sleep(tab) => sleep::try_sleep(tab, |_| {}),
        Effect::Retire { tab, since } => sleep::try_retire(tab, since),
        Effect::FocusPage(tab) => window::focus_page(existing(tab).as_deref()),
        Effect::Save(shape) => store::save_session(shape),
        Effect::SaveHistory(history, now) => store::save_history(history, now),
        Effect::Find(ask) => page_find::ask(&ask),
        Effect::ClearFind { tab, generation } => page_find::clear(tab, generation),
    }
}

fn load(web: &WKWebView, address: &str) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(address)) else {
        return;
    };
    // SAFETY: plain calls on a live web view with a valid URL.
    unsafe {
        if url.isFileURL() {
            // A local file may read the folder it is in, as Safari lets it.
            let folder = url
                .URLByDeletingLastPathComponent()
                .unwrap_or_else(|| url.clone());
            web.loadFileURL_allowingReadAccessToURL(&url, &folder);
        } else {
            web.loadRequest(&NSURLRequest::requestWithURL(&url));
        }
    }
}

/// Reads what the observer watches and hands it to the model. For callers without a
/// key to go by; the observer itself takes the cheaper road below.
fn sync(web: &WKWebView) {
    let Some(tab) = tab_of(web) else { return };
    // SAFETY: plain property reads on a live web view.
    let (title, url, loading, progress, back, forward, secure) = unsafe {
        (
            web.title().map(|t| t.to_string()).unwrap_or_default(),
            web.URL()
                .and_then(|u| u.absoluteString())
                .map(|u| u.to_string())
                .unwrap_or_default(),
            web.isLoading(),
            web.estimatedProgress(),
            web.canGoBack(),
            web.canGoForward(),
            web.hasOnlySecureContent(),
        )
    };
    rt::dispatch(move |browser| {
        browser.page_url(tab, &url);
        browser.page_security(tab, secure);
        browser.page_title(tab, &title);
        browser.page_loading(tab, loading, progress);
        browser.page_history(tab, back, forward);
    });
}

/// Reads one watched property and hands it to the model. A load fires the observer
/// many times over, and re-reading all seven properties — five model calls each time
/// — for every one of them cost the main thread dearly. Loading and progress are read
/// together: the model takes them as one.
fn sync_one(web: &WKWebView, key: &str) {
    let Some(tab) = tab_of(web) else { return };
    // SAFETY: plain property reads on a live web view, whichever key moved.
    let reads = || unsafe {
        match key {
            "title" => Reads::Title(web.title().map(|t| t.to_string()).unwrap_or_default()),
            "URL" => Reads::Address(
                web.URL()
                    .and_then(|u| u.absoluteString())
                    .map(|u| u.to_string())
                    .unwrap_or_default(),
            ),
            "loading" | "estimatedProgress" => {
                Reads::Load(web.isLoading(), web.estimatedProgress())
            }
            "canGoBack" | "canGoForward" => Reads::History(web.canGoBack(), web.canGoForward()),
            "hasOnlySecureContent" => Reads::Secure(web.hasOnlySecureContent()),
            _ => Reads::All,
        }
    };
    match reads() {
        Reads::Title(title) => rt::dispatch(move |b| b.page_title(tab, &title)),
        Reads::Address(url) => rt::dispatch(move |b| b.page_url(tab, &url)),
        Reads::Load(loading, progress) => {
            rt::dispatch(move |b| b.page_loading(tab, loading, progress));
        }
        Reads::History(back, forward) => rt::dispatch(move |b| b.page_history(tab, back, forward)),
        Reads::Secure(secure) => rt::dispatch(move |b| b.page_security(tab, secure)),
        Reads::All => sync(web),
    }
}

/// What one key's read brought back.
enum Reads {
    Title(String),
    Address(String),
    Load(bool, f64),
    History(bool, bool),
    Secure(bool),
    All,
}

fn process_ended(web: &WKWebView, end: PageEnd) {
    let Some(tab) = tab_of(web) else { return };
    // What the page asked is answered as dismissed: there is no page left to answer.
    dialogs::drop_held(tab);
    rt::dispatch(move |browser| {
        let site = browser
            .tab(tab)
            .and_then(|t| Url::parse(t.url()))
            .and_then(|u| u.host())
            .unwrap_or_default();
        let done = browser.process_ended(tab, end, Instant::now());
        // The reason and what was done are public; the site is not (Search #849).
        oslog::page_ended(end.name(), &site, done.said());
    });
}

define_class!(
    /// Every web view's navigation and UI delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapWebDelegate"]
    struct WebDelegate;

    unsafe impl NSObjectProtocol for WebDelegate {}

    unsafe impl WKNavigationDelegate for WebDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            web: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            handler.call((decide(web, action),));
        }

        #[unsafe(method(webView:didReceiveAuthenticationChallenge:completionHandler:))]
        fn challenged(
            &self,
            web: &WKWebView,
            challenge: &NSURLAuthenticationChallenge,
            handler: &DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
        ) {
            dialogs::challenge(web, tab_of(web), challenge, handler);
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _web: &WKWebView,
            response: &WKNavigationResponse,
            handler: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            handler.call((decide_response(response),));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn committed(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            // Shield is tuned for the site this page is now on, before its
            // requests gather: off for this site means off for the whole page.
            shield::tune(&controller_of(web), host_of(web).as_deref());
            if let Some(tab) = tab_of(web) {
                rt::dispatch(move |browser| browser.page_committed(tab));
            }
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn action_download(&self, web: &WKWebView, _action: &WKNavigationAction, download: &WKDownload) {
            became_download(web, download);
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn response_download(&self, web: &WKWebView, _response: &WKNavigationResponse, download: &WKDownload) {
            became_download(web, download);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            if let Some(tab) = tab_of(web) {
                rt::dispatch(move |browser| browser.page_finished(tab));
                icons::fetch(tab, web);
                forms::page_finished(tab);
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_early(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            failed(web, error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed_late(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            failed(web, error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn ended(&self, web: &WKWebView) {
            process_ended(web, PageEnd::Unknown);
        }
    }

    unsafe impl WKUIDelegate for WebDelegate {
        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(&self, web: &WKWebView, message: &NSString, frame: &WKFrameInfo, handler: &DynBlock<dyn Fn()>) {
            dialogs::alert_panel(web, tab_of(web), message, frame, handler);
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(&self, web: &WKWebView, message: &NSString, frame: &WKFrameInfo, handler: &DynBlock<dyn Fn(Bool)>) {
            dialogs::confirm_panel(web, tab_of(web), message, frame, handler);
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt(
            &self,
            web: &WKWebView,
            message: &NSString,
            default_text: Option<&NSString>,
            frame: &WKFrameInfo,
            handler: &DynBlock<dyn Fn(*mut NSString)>,
        ) {
            dialogs::prompt_panel(web, tab_of(web), message, default_text, frame, handler);
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn choose_file(
            &self,
            web: &WKWebView,
            parameters: &WKOpenPanelParameters,
            frame: &WKFrameInfo,
            handler: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            dialogs::open_panel(web, tab_of(web), parameters, frame, handler);
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create(
            &self,
            web: &WKWebView,
            configuration: &WKWebViewConfiguration,
            _action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // A page asking for a window at its own size — a call, a sign-in, a
            // chat — gets a small window of that size while Settings says so
            // (`tabs.popupWindows`, Search's sized pop-ups); every other new
            // window is a tab, as before.
            // SAFETY: plain property reads on the features WebKit hands us.
            let sized = unsafe { features.width().is_some() || features.height().is_some() };
            if sized && PAGES.get().popups {
                Some(create_popup(self.mtm(), configuration, features))
            } else {
                create_tab(self.mtm(), web, configuration)
            }
        }

        #[unsafe(method(webViewDidClose:))]
        fn closed(&self, web: &WKWebView) {
            if close_popup(web) {
                return;
            }
            if let Some(tab) = tab_of(web) {
                rt::dispatch(move |browser| browser.close(tab));
            }
        }
    }

    impl WebDelegate {
        /// WebKit's private reason for a process ending: implemented, WebKit calls this
        /// *instead of* the public callback (Search #849, Dialogs.swift).
        #[unsafe(method(_webView:webContentProcessDidTerminateWithReason:))]
        fn ended_with(&self, web: &WKWebView, reason: NSInteger) {
            process_ended(web, PageEnd::from_webkit(reason));
        }

        /// A page asking to send notifications (Search's `askedForNotifications`):
        /// not in the public delegate, so WebKit calls it by this name. Only while
        /// Settings lets sites ask, only for the page's own origin, and the answer
        /// is kept per origin under Search's `capture.` keys, so a site is asked
        /// about once. What a site may then *send* is WebKit's own posting; this
        /// is only ever the asking.
        #[unsafe(method(_webView:requestNotificationPermissionForSecurityOrigin:decisionHandler:))]
        fn notify_permission(
            &self,
            web: &WKWebView,
            origin: &WKSecurityOrigin,
            handler: &DynBlock<dyn Fn(Bool)>,
        ) {
            notify_permission(web, origin, handler);
        }
    }
);

impl WebDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// A page asked for a new window: it becomes a tab in front, beside the page that asked,
/// with the web view WebKit configured for it.
fn create_tab(
    mtm: MainThreadMarker,
    web: &WKWebView,
    configuration: &WKWebViewConfiguration,
) -> Option<Retained<WKWebView>> {
    let opener = tab_of(web);
    // SAFETY: WebKit's own configuration for this window, used once, as it asks.
    let fresh = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, configuration)
    };
    let id = rt::update(|browser| browser.adopt(opener, true))?;
    register(mtm, id, &fresh);
    rt::settle();
    Some(fresh)
}

thread_local! {
    /// Sized pop-up windows and the page in each, kept until the page closes
    /// itself (`window.close`, answered in `closed`) — a pop-up is no tab, so
    /// nothing else holds its window up.
    static POPUPS: RefCell<Vec<(Retained<WKWebView>, Retained<NSWindow>)>> = const { RefCell::new(Vec::new()) };
}

/// A sized pop-up's small window: WebKit's configured view in a plain window at
/// the size the page asked for, clamped to what a screen can hold. It is not a
/// tab — the model never hears of it — and it goes when the page closes itself
/// or the window is closed and its page with it.
fn create_popup(
    mtm: MainThreadMarker,
    configuration: &WKWebViewConfiguration,
    features: &WKWindowFeatures,
) -> Retained<WKWebView> {
    // SAFETY: WebKit's own configuration for this window, used once, as it asks.
    let fresh = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, configuration)
    };
    shared(mtm, |shared| {
        // SAFETY: the delegates live for the app's whole life in `SHARED`, longer
        // than any pop-up's view.
        unsafe {
            fresh.setNavigationDelegate(Some(ProtocolObject::from_ref(&*shared.delegate)));
            fresh.setUIDelegate(Some(ProtocolObject::from_ref(&*shared.delegate)));
            fresh.setAllowsBackForwardNavigationGestures(true);
            fresh.setInspectable(true);
        }
    });
    take_prefs(&fresh, PAGES.get());
    shield::tune(&controller_of(&fresh), None);
    // SAFETY: plain property reads on the features WebKit hands us.
    let (width, height) = unsafe {
        (
            features.width().map_or(640.0, |n| n.doubleValue()),
            features.height().map_or(480.0, |n| n.doubleValue()),
        )
    };
    let frame = NSRect::new(
        NSPoint::new(80.0, 80.0),
        NSSize::new(width.clamp(280.0, 1280.0), height.clamp(200.0, 960.0)),
    );
    // SAFETY: a fresh window on the main thread, kept in `POPUPS` below.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            objc2_app_kit::NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: a fresh window on the main thread, before it is shown.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setContentView(Some(&fresh));
    window.center();
    window.makeKeyAndOrderFront(None);
    POPUPS.with_borrow_mut(|popups| popups.push((fresh.clone(), window)));
    fresh
}

/// A pop-up's page closed itself: its window goes with it.
fn close_popup(web: &WKWebView) -> bool {
    let found = POPUPS.with_borrow_mut(|popups| {
        let at = popups
            .iter()
            .position(|(page, _)| ptr::eq(Retained::as_ptr(page), web));
        at.map(|at| popups.remove(at))
    });
    let Some((_page, window)) = found else {
        return false;
    };
    window.close();
    true
}

/// `"scheme://host[:port]"`, the way an origin is kept for its answers (Search's
/// `Browser.origin`): a standard port is left off.
fn origin_string(scheme: &str, host: &str, port: NSInteger) -> String {
    let scheme = scheme.to_lowercase();
    let host = host.to_lowercase();
    let standard = (scheme == "https" && port == 443) || (scheme == "http" && port == 80);
    if port == 0 || standard {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// What a notification ask is answered without asking you: the switch off, or an
/// answer already kept for the origin. `None` means ask.
fn notification_decision(ask_on: bool, remembered: Option<bool>) -> Option<bool> {
    if !ask_on {
        return Some(false);
    }
    remembered
}

/// The asking itself: a sheet over the page's window naming the site, kept under
/// `capture.<origin>|notifications` as Search keeps it. A test run puts no window
/// up and answers no, as every question in `dialogs` does there.
fn notify_permission(web: &WKWebView, origin: &WKSecurityOrigin, handler: &DynBlock<dyn Fn(Bool)>) {
    use objc2::Message as _;
    use tap_core::settings_index::{self as index, Prefs as _};
    // SAFETY: plain reads of the origin and the address WebKit hands us.
    let (scheme, host, port) = unsafe {
        (
            origin.protocol().to_string(),
            origin.host().to_string(),
            origin.port(),
        )
    };
    let site = origin_string(&scheme, &host, port);
    // Only the page's own origin may ask: a frame asking for another site is
    // refused without a sheet, as in Search.
    // SAFETY: a plain property read on a live web view.
    let page_matches = unsafe { web.URL() }.is_some_and(|url| {
        let page = origin_string(
            &url.scheme().map(|s| s.to_string()).unwrap_or_default(),
            &url.host().map(|s| s.to_string()).unwrap_or_default(),
            url.port().map_or(0, |n| n.integerValue()),
        );
        page == site
    });
    let kept = crate::prefs::Kept::new();
    let key = format!("capture.{site}|notifications");
    let remembered = kept.get(&key).and_then(|v| v.as_bool());
    let decided = notification_decision(index::flag(&kept, "notifications.ask"), remembered);
    if host.is_empty() || !page_matches {
        handler.call((Bool::NO,));
        return;
    }
    if let Some(allowed) = decided {
        handler.call((Bool::new(allowed),));
        return;
    }
    if std::env::var_os(identity::PROBE_ENV).is_some() {
        handler.call((Bool::NO,));
        return;
    }
    let handler = handler.copy();
    let web = web.retain();
    let question = move || {
        let mtm = MainThreadMarker::from(&*web);
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "{host} asks to send notifications"
        )));
        alert.setInformativeText(&NSString::from_str(
            "Only a site you allow reaches your Mac's notifications.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Allow"));
        alert.addButtonWithTitle(&NSString::from_str("Don't Allow"));
        let once = std::cell::Cell::new(Some(handler));
        let block = RcBlock::new(move |answer: objc2_app_kit::NSModalResponse| {
            let Some(handler) = once.take() else { return };
            let allowed = answer == objc2_app_kit::NSAlertFirstButtonReturn;
            crate::prefs::Kept::new().set(&key, allowed.into());
            handler.call((Bool::new(allowed),));
        });
        match web.window() {
            Some(window) => alert.beginSheetModalForWindow_completionHandler(&window, Some(&block)),
            None => block.call((alert.runModal(),)),
        }
    };
    question();
}

/// ⌘-click opens a link behind the page; a link to another app (mailto:, an app's own
/// scheme) goes to that app.
fn decide(web: &WKWebView, action: &WKNavigationAction) -> WKNavigationActionPolicy {
    // A link that says it is a file to keep (`<a download>`): left to load, nothing
    // happens and nothing says why.
    // SAFETY: a plain property read on the action WebKit hands us.
    if unsafe { action.shouldPerformDownload() } {
        return WKNavigationActionPolicy::Download;
    }
    // SAFETY: plain property reads on the action WebKit hands us.
    let (request_url, kind, modifiers, main_frame) = unsafe {
        (
            action.request().URL(),
            action.navigationType(),
            action.modifierFlags(),
            action
                .targetFrame()
                .is_some_and(|frame| frame.isMainFrame()),
        )
    };
    let Some(request_url) = request_url else {
        return WKNavigationActionPolicy::Allow;
    };
    let scheme = request_url
        .scheme()
        .map(|s| s.to_string().to_lowercase())
        .unwrap_or_default();
    if !matches!(
        scheme.as_str(),
        "http" | "https" | "file" | "about" | "data" | "blob" | ""
    ) {
        NSWorkspace::sharedWorkspace().openURL(&request_url);
        return WKNavigationActionPolicy::Cancel;
    }
    if kind == WKNavigationType::LinkActivated
        && modifiers.contains(NSEventModifierFlags::Command)
        && main_frame
    {
        let opener = tab_of(web);
        let address = request_url
            .absoluteString()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let shift = modifiers.contains(NSEventModifierFlags::Shift);
        rt::dispatch(move |browser| {
            if let Some(url) = Url::parse(&address) {
                browser.open(Some(&url), shift, opener);
            }
        });
        return WKNavigationActionPolicy::Cancel;
    }
    WKNavigationActionPolicy::Allow
}

/// What arrives is shown if WebKit can show it, and kept as a file if not, or if the
/// server says it is an attachment (Search's response policy).
fn decide_response(response: &WKNavigationResponse) -> WKNavigationResponsePolicy {
    // SAFETY: plain property reads on the response WebKit hands us.
    let (showable, answer) = unsafe { (response.canShowMIMEType(), response.response()) };
    let attachment = answer
        .downcast::<NSHTTPURLResponse>()
        .ok()
        .and_then(|http| http.valueForHTTPHeaderField(&NSString::from_str("Content-Disposition")))
        .is_some_and(|disposition| fetching::is_attachment(&disposition.to_string()));
    if attachment || !showable {
        WKNavigationResponsePolicy::Download
    } else {
        WKNavigationResponsePolicy::Allow
    }
}

/// The navigation became a download: it is kept until it ends, and a tab opened for it
/// alone goes.
fn became_download(web: &WKWebView, download: &WKDownload) {
    downloads::keep(MainThreadMarker::from(web), download, tab_of(web));
    if let Some(tab) = tab_of(web) {
        rt::dispatch(move |browser| browser.drop_empty(tab));
    }
}

/// A load that never arrived is said in place of the page. A load stopped on purpose
/// (another one started, Esc) is not a failure.
fn failed(web: &WKWebView, error: &NSError) {
    const CANCELLED: NSInteger = -999;
    // WebKit's own "frame load interrupted", which a download or a policy change gives.
    const INTERRUPTED: NSInteger = 102;
    let code = error.code();
    if code == CANCELLED || code == INTERRUPTED {
        return;
    }
    let Some(tab) = tab_of(web) else { return };
    let message = error.localizedDescription().to_string();
    rt::dispatch(move |browser| browser.page_failed(tab, &message));
}

/// Press Esc twice to leave full screen (`fullscreen.escapeTwice`, Search's
/// `FullScreenEscape.swift`): WebKit leaves on the first Escape it is given, so
/// the first press in a full screen — a page's own, or the window's macOS full
/// screen — is kept from it and a reminder goes up; a second within two seconds
/// goes on and leaves. The count is by the keys' own clock, and a held key's
/// repeats count for nothing. Search counts a press in its key monitor once its
/// own Escape jobs have passed it over; here the monitor is installed from this
/// file and only ever speaks for a full screen holding a page, so a find field
/// or a text field keeps its Escape.
mod escape {
    use std::{cell::Cell, ptr};

    use block2::RcBlock;
    use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{
        NSEvent, NSEventMask, NSPanel, NSTextField, NSWindow, NSWindowOrderingMode,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::WKFullscreenState;

    use super::{POPUPS, WEBS};

    /// How long after the first press a second one still leaves.
    const WITHIN: f64 = 2.0;
    /// Escape's key code, as every Mac keyboard gives it.
    const ESCAPE: u16 = 53;

    thread_local! {
        static ON: Cell<bool> = const { Cell::new(false) };
        static TOKEN: std::cell::RefCell<Option<Retained<AnyObject>>> = const { std::cell::RefCell::new(None) };
        /// The window waiting for its second press (by address), and when the
        /// wait ends, on the key events' own clock.
        static WAITING: Cell<(usize, f64)> = const { Cell::new((0, 0.0)) };
        static KEEPS_KEYUP: Cell<bool> = const { Cell::new(false) };
        static PANEL: std::cell::RefCell<Option<Retained<NSPanel>>> = const { std::cell::RefCell::new(None) };
        static ROUND: Cell<u64> = const { Cell::new(0) };
    }

    /// Settings moved: the monitor is put up with the switch, and taken down
    /// without it, so off costs a page nothing at all.
    pub fn apply(on: bool) {
        ON.set(on);
        let installed = TOKEN.with_borrow(Option::is_some);
        if on && !installed {
            let block = RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
                // SAFETY: AppKit hands a live event, for the call's length.
                let event = unsafe { event.as_ref() };
                if decided(event) {
                    std::ptr::NonNull::from(event).as_ptr()
                } else {
                    ptr::null_mut()
                }
            });
            // SAFETY: a local monitor for this app's own key events; the token
            // is kept in `TOKEN` for as long as it should live.
            let token = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                    NSEventMask::KeyDown | NSEventMask::KeyUp,
                    &block,
                )
            };
            TOKEN.with_borrow_mut(|slot| *slot = token);
        } else if !on && installed {
            disarm();
            TOKEN.with_borrow_mut(|slot| {
                if let Some(token) = slot.take() {
                    // SAFETY: the token this monitor was given, removed once.
                    unsafe { NSEvent::removeMonitor(&token) };
                }
            });
        }
    }

    /// True lets the event go on its way; false keeps it.
    fn decided(event: &NSEvent) -> bool {
        if !ON.get() {
            return true;
        }
        let (kind, code) = (event.r#type(), event.keyCode());
        if code != ESCAPE {
            return true;
        }
        if kind == objc2_app_kit::NSEventType::KeyUp {
            if KEEPS_KEYUP.get() {
                KEEPS_KEYUP.set(false);
                return false;
            }
            return true;
        }
        if kind != objc2_app_kit::NSEventType::KeyDown {
            return true;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return true;
        };
        let Some(window) = event.window(mtm) else {
            return true;
        };
        // A text field's Escape is its own (a find field closing, a form giving
        // up): only a page holding the keys is guarded.
        if let Some(held) = window.firstResponder()
            && held.downcast_ref::<NSTextField>().is_some()
        {
            return true;
        }
        let at = Retained::as_ptr(&window) as usize;
        // Which full screen, if any: a page WebKit has in its own window, or
        // this window itself in macOS full screen with a page in it.
        // (This page is in the event's window, it fills it full screen.)
        let look = |web: &objc2_web_kit::WKWebView| -> (bool, bool) {
            // SAFETY: plain property reads on a live web view.
            unsafe {
                if web
                    .window()
                    .is_some_and(|w| Retained::as_ptr(&w) as usize == at)
                {
                    let state = web.fullscreenState();
                    let full = state == WKFullscreenState::InFullscreen
                        || state == WKFullscreenState::EnteringFullscreen;
                    return (true, full);
                }
            }
            (false, false)
        };
        let mut page_full = false;
        let mut has_page = false;
        WEBS.with_borrow(|webs| {
            for web in webs.values() {
                let (has, full) = look(web);
                has_page |= has;
                page_full |= full;
            }
        });
        if !page_full {
            POPUPS.with_borrow(|popups| {
                for (web, _) in popups {
                    let (has, full) = look(web);
                    has_page |= has;
                    page_full |= full;
                }
            });
        }
        let window_full = window.styleMask().contains(NSWindowStyleMask::FullScreen);
        if !(page_full || (window_full && has_page)) {
            return true;
        }
        // The event's own timestamp: the clock the count is kept by.
        let now = event.timestamp();
        if event.isARepeat() {
            // A held first press repeating: kept, and counts for nothing.
            return WAITING.with(|w| w.get().0 != at);
        }
        let (waiting_at, deadline) = WAITING.get();
        if waiting_at == at && now < deadline {
            disarm();
            if page_full {
                // WebKit's own way out: the press goes on to it.
                return true;
            }
            window.toggleFullScreen(None);
            return false;
        }
        WAITING.set((at, now + WITHIN));
        KEEPS_KEYUP.set(true);
        show(&window);
        let round = ROUND.get() + 1;
        ROUND.set(round);
        // The wait, on the wall clock, for the reminder's sake; the count
        // itself is by the events' timestamps above.
        let _ = dispatch2::DispatchQueue::main().after(
            dispatch2::DispatchTime::NOW.time(2_000_000_000),
            move || {
                if ROUND.get() == round {
                    WAITING.set((0, 0.0));
                    hide();
                }
            },
        );
        false
    }

    fn disarm() {
        WAITING.set((0, 0.0));
        ROUND.set(ROUND.get() + 1);
        hide();
    }

    /// The reminder: a small pill over the full screen, never key, the pointer
    /// through it, saying what the second press does.
    fn show(window: &NSWindow) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let panel = PANEL.with_borrow_mut(|slot| {
            if let Some(panel) = slot {
                return panel.clone();
            }
            let panel = {
                NSPanel::initWithContentRect_styleMask_backing_defer(
                    NSPanel::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 36.0)),
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    objc2_app_kit::NSBackingStoreType::Buffered,
                    false,
                )
            };
            // SAFETY: a fresh panel on the main thread, before it is shown.
            unsafe { panel.setReleasedWhenClosed(false) };
            panel.setIgnoresMouseEvents(true);
            panel.setOpaque(false);
            let ground =
                objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.72);
            panel.setBackgroundColor(Some(&ground));
            let words = NSTextField::labelWithString(
                &NSString::from_str("Press Esc again to exit full screen"),
                mtm,
            );
            let ink = objc2_app_kit::NSColor::whiteColor();
            words.setTextColor(Some(&ink));
            words.setAlignment(objc2_app_kit::NSTextAlignment::Center);
            words.setFrame(NSRect::new(
                NSPoint::new(0.0, 9.0),
                NSSize::new(300.0, 18.0),
            ));
            panel.setContentView(Some(&words));
            *slot = Some(panel.clone());
            panel
        });
        if let Some(parent) = panel.parentWindow()
            && !ptr::eq(parent.as_ref(), window)
        {
            parent.removeChildWindow(&panel);
        }
        let over = window.frame();
        panel.setFrame_display(
            NSRect::new(
                NSPoint::new(
                    over.origin.x + (over.size.width - 300.0) / 2.0,
                    over.origin.y + over.size.height - 80.0,
                ),
                NSSize::new(300.0, 36.0),
            ),
            false,
        );
        if panel.parentWindow().is_none() {
            // SAFETY: both windows live, on the main thread.
            unsafe { window.addChildWindow_ordered(&panel, NSWindowOrderingMode::Above) };
        }
        panel.orderFront(None);
    }

    fn hide() {
        PANEL.with_borrow(|slot| {
            if let Some(panel) = slot {
                if let Some(parent) = panel.parentWindow() {
                    parent.removeChildWindow(panel);
                }
                panel.orderOut(None);
            }
        });
    }
}

/// Whether the Mac is on its adapter, and word when that changes (Search's
/// `FrameRate` power watch): `IOKit` is not among Tap's linked frameworks, so its
/// few functions are found by name at run time, and a Mac where they can't be
/// found is taken to be on power, as Search takes it.
mod power {
    use std::{
        cell::Cell,
        ffi::{CString, c_char, c_void},
    };

    type Info = unsafe extern "C" fn() -> *mut c_void;
    type Providing = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type Release = unsafe extern "C" fn(*mut c_void);
    type Source = unsafe extern "C" fn(extern "C" fn(*mut c_void), *mut c_void) -> *mut c_void;
    type RunLoop = unsafe extern "C" fn() -> *mut c_void;
    type AddSource = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

    struct Fns {
        copy_info: Info,
        providing: Providing,
        release: Release,
        create_source: Source,
        run_loop: RunLoop,
        add_source: AddSource,
        /// `kCFRunLoopCommonModes`, by address: a `CFString` kept by CoreFoundation.
        common_modes: usize,
    }

    thread_local! {
        static WATCHING: Cell<bool> = const { Cell::new(false) };
    }

    fn fns() -> Option<&'static Fns> {
        static FNS: std::sync::OnceLock<Option<Fns>> = std::sync::OnceLock::new();
        FNS.get_or_init(||
            // SAFETY: `load` only finds framework entry points by name.
            unsafe { load() })
            .as_ref()
    }

    /// Finds `IOKit`'s and CoreFoundation's entry points by name.
    ///
    /// # Safety
    /// The names are `IOKit`'s and CoreFoundation's own, with the signatures
    /// their headers give; a missing one loads nothing.
    unsafe fn load() -> Option<Fns> {
        // SAFETY: framework paths that ship with macOS; handles kept for the
        // app's life by never being closed.
        let iokit = unsafe {
            libc::dlopen(
                c"/System/Library/Frameworks/IOKit.framework/IOKit".as_ptr(),
                libc::RTLD_NOW,
            )
        };
        // SAFETY: as above, for CoreFoundation's framework path.
        let core = unsafe {
            libc::dlopen(
                c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation".as_ptr(),
                libc::RTLD_NOW,
            )
        };
        if iokit.is_null() || core.is_null() {
            return None;
        }
        // SAFETY: as `load` says; each symbol is asked for before it is used.
        unsafe {
            let sym = |handle: *mut c_void, name: &str| -> *mut c_void {
                let Ok(name) = CString::new(name) else {
                    return ptr_null();
                };
                libc::dlsym(handle, name.as_ptr().cast::<c_char>())
            };
            let copy_info = sym(iokit, "IOPSCopyPowerSourcesInfo");
            let providing = sym(iokit, "IOPSGetProvidingPowerSourceType");
            let create_source = sym(iokit, "IOPSNotificationCreateRunLoopSource");
            let release = sym(core, "CFRelease");
            let run_loop = sym(core, "CFRunLoopGetMain");
            let add_source = sym(core, "CFRunLoopAddSource");
            let modes = sym(core, "kCFRunLoopCommonModes");
            if [
                copy_info,
                providing,
                create_source,
                release,
                run_loop,
                add_source,
                modes,
            ]
            .iter()
            .any(|p| p.is_null())
            {
                return None;
            }
            // SAFETY: the global is a CFStringRef; its address is what was found.
            let common_modes = *(modes.cast::<*mut c_void>()) as usize;
            Some(Fns {
                copy_info: std::mem::transmute::<*mut c_void, Info>(copy_info),
                providing: std::mem::transmute::<*mut c_void, Providing>(providing),
                release: std::mem::transmute::<*mut c_void, Release>(release),
                create_source: std::mem::transmute::<*mut c_void, Source>(create_source),
                run_loop: std::mem::transmute::<*mut c_void, RunLoop>(run_loop),
                add_source: std::mem::transmute::<*mut c_void, AddSource>(add_source),
                common_modes,
            })
        }
    }

    fn ptr_null() -> *mut c_void {
        std::ptr::null_mut()
    }

    /// On the adapter, or no battery to spare: only a laptop running on its
    /// battery says otherwise.
    pub fn on_power() -> bool {
        let Some(f) = fns() else { return true };
        // SAFETY: IOKit's own calls with the signatures above; the info is
        // released with CoreFoundation's, as its header asks.
        unsafe {
            let info = (f.copy_info)();
            if info.is_null() {
                return true;
            }
            let kind = (f.providing)(info);
            let battery = if kind.is_null() {
                false
            } else {
                // A CFString and an NSString are the same object told two
                // ways; read, never kept.
                let text = &*kind.cast::<objc2_foundation::NSString>();
                text.to_string() == "Battery Power"
            };
            (f.release)(info);
            !battery
        }
    }

    extern "C" fn changed(_context: *mut c_void) {
        dispatch2::DispatchQueue::main().exec_async(|| {
            super::apply_prefs(&crate::prefs::Kept::new());
        });
    }

    /// Told when the adapter is plugged or pulled, from now on. Made once.
    pub fn watch() {
        if WATCHING.get() {
            return;
        }
        let Some(f) = fns() else { return };
        // SAFETY: IOKit's run-loop source for power changes, added to the main
        // run loop and kept for the app's life, as Search keeps its.
        unsafe {
            let source = (f.create_source)(changed, ptr_null());
            if source.is_null() {
                return;
            }
            (f.add_source)((f.run_loop)(), source, f.common_modes as *mut c_void);
        }
        WATCHING.set(true);
    }
}

define_class!(
    /// Hears every web view's title, address, loading, progress and history change.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapWebObserver"]
    struct Observer;

    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            key: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let Some(web) = object.and_then(|o| o.downcast_ref::<WKWebView>()) else {
                return;
            };
            match key {
                Some(key) => sync_one(web, &key.to_string()),
                None => sync(web),
            }
        }
    }
);

impl Observer {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::{notification_decision, origin_string};

    #[test]
    fn origins_leave_standard_ports_off() {
        assert_eq!(
            origin_string("HTTPS", "Example.COM", 443),
            "https://example.com"
        );
        assert_eq!(
            origin_string("http", "example.com", 80),
            "http://example.com"
        );
        assert_eq!(
            origin_string("https", "example.com", 8443),
            "https://example.com:8443"
        );
        assert_eq!(
            origin_string("https", "example.com", 0),
            "https://example.com"
        );
    }

    #[test]
    fn notifications_are_decided_only_when_they_can_be() {
        // The switch off refuses without asking, whatever was kept before.
        assert_eq!(notification_decision(false, Some(true)), Some(false));
        assert_eq!(notification_decision(false, None), Some(false));
        // A kept answer is the answer.
        assert_eq!(notification_decision(true, Some(true)), Some(true));
        assert_eq!(notification_decision(true, Some(false)), Some(false));
        // Nothing kept: ask.
        assert_eq!(notification_decision(true, None), None);
    }
}
