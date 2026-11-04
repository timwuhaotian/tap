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
use objc2_app_kit::{NSEventModifierFlags, NSWorkspace};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSHTTPURLResponse, NSInteger, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSRect, NSString, NSURL,
    NSURLAuthenticationChallenge, NSURLCredential, NSURLRequest,
    NSURLSessionAuthChallengeDisposition,
};
#[allow(deprecated)]
use objc2_web_kit::WKProcessPool;
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKContentWorld, WKDownload, WKFrameInfo, WKNavigation,
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKNavigationType, WKOpenPanelParameters, WKPreferences,
    WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use serde_json::Value;
use tap_core::{
    browser::{Effect, TabId},
    crash::PageEnd,
    fetching,
    fnd::Url,
    identity,
};

use crate::{dialogs, downloads, icons, oslog, page_find, rt, sleep, store, window};

/// What the observer watches on every web view; each change re-reads the lot.
const OBSERVED: [&str; 6] = [
    "title",
    "URL",
    "loading",
    "estimatedProgress",
    "canGoBack",
    "canGoForward",
];

thread_local! {
    static WEBS: RefCell<HashMap<TabId, Retained<WKWebView>>> = RefCell::new(HashMap::new());
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
    static PAGES: std::cell::Cell<Pages> = const { std::cell::Cell::new(Pages::DEFAULT) };
}

/// What Settings says every page does (Search's `Web.configuration` and `Privacy.swift`):
/// set at launch and whenever a setting changes.
#[derive(Clone, Copy, Debug)]
struct Pages {
    /// `pageZoom`: where every site starts.
    zoom: f64,
    /// `media.click`: videos wait for a click, with sound or without.
    waits: bool,
    /// `privacy.clickIDs`: WebKit takes gclid, fbclid and the like off an address.
    click_ids: bool,
    /// `privacy.gpc`: Global Privacy Control with every request.
    gpc: bool,
}

impl Pages {
    const DEFAULT: Pages = Pages {
        zoom: 1.0,
        waits: false,
        click_ids: true,
        gpc: true,
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
    };
    PAGES.set(pages);
    WEBS.with_borrow(|webs| {
        for web in webs.values() {
            take_prefs(web, pages);
        }
    });
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
    WEBS.with_borrow_mut(|webs| webs.insert(tab, web.clone()));
}

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

fn tab_of(web: &WKWebView) -> Option<TabId> {
    WEBS.with_borrow(|webs| {
        webs.iter()
            .find(|(_, w)| ptr::eq(Retained::as_ptr(w), web))
            .map(|(id, _)| *id)
    })
}

fn discard(mtm: MainThreadMarker, tab: TabId) {
    dialogs::drop_held(tab);
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
        Effect::Sleep(tab) => sleep::try_sleep(tab, |_| {}),
        Effect::Retire { tab, since } => sleep::try_retire(tab, since),
        Effect::FocusPage(tab) => window::focus_page(existing(tab).as_deref()),
        Effect::Save(shape) => store::save_session(&shape),
        Effect::SaveHistory(data) => store::save_history(data),
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

/// Reads what the observer watches and hands it to the model.
fn sync(web: &WKWebView) {
    let Some(tab) = tab_of(web) else { return };
    // SAFETY: plain property reads on a live web view.
    let (title, url, loading, progress, back, forward) = unsafe {
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
        )
    };
    rt::dispatch(move |browser| {
        browser.page_url(tab, &url);
        browser.page_title(tab, &title);
        browser.page_loading(tab, loading, progress);
        browser.page_history(tab, back, forward);
    });
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
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            create_tab(self.mtm(), web, configuration)
        }

        #[unsafe(method(webViewDidClose:))]
        fn closed(&self, web: &WKWebView) {
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
            _key: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(web) = object.and_then(|o| o.downcast_ref::<WKWebView>()) {
                sync(web);
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
