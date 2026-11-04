//! The web views: one per tab that has been shown, made from one shared configuration,
//! with one delegate and one observer for all of them (Search's `Tab.swift` and the
//! `WKNavigationDelegate`/`WKUIDelegate` half of `Browser.swift`).

use std::{cell::RefCell, collections::HashMap, ffi::c_void, ptr, time::Instant};

use block2::DynBlock;
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{NSEventModifierFlags, NSWorkspace};
use objc2_foundation::{
    NSBundle, NSDictionary, NSError, NSInteger, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSRect, NSString, NSURL, NSURLRequest,
};
#[allow(deprecated)]
use objc2_web_kit::WKProcessPool;
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationType, WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use tap_core::{
    browser::{Effect, TabId},
    crash::PageEnd,
    fnd::Url,
};

use crate::{rt, store, window};

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
    WEBS.with_borrow_mut(|webs| webs.insert(tab, web.clone()));
}

/// The tab's web view, if it has been made.
pub fn existing(tab: TabId) -> Option<Retained<WKWebView>> {
    WEBS.with_borrow(|webs| webs.get(&tab).cloned())
}

fn tab_of(web: &WKWebView) -> Option<TabId> {
    WEBS.with_borrow(|webs| {
        webs.iter()
            .find(|(_, w)| ptr::eq(Retained::as_ptr(w), web))
            .map(|(id, _)| *id)
    })
}

fn discard(mtm: MainThreadMarker, tab: TabId) {
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
        Effect::Load { tab, url } => load(&web_view(mtm, tab), &url),
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
        Effect::Discard(tab) => discard(mtm, tab),
        Effect::FocusPage(tab) => window::focus_page(existing(tab).as_deref()),
        Effect::Save(shape) => store::save_session(&shape),
        Effect::SaveHistory(data) => store::save_history(data),
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
    rt::dispatch(move |browser| {
        let again = browser.process_ended(tab, end, Instant::now());
        // The reason and what was done are public; the site is not (Search #849).
        eprintln!(
            "A page's process ended ({}) on <private>: {}",
            end.name(),
            if again {
                "loaded again"
            } else {
                "kept dying, left for Try again"
            }
        );
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

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            if let Some(tab) = tab_of(web) {
                rt::dispatch(move |browser| browser.page_finished(tab));
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
