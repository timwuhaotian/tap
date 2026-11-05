//! SVG site icons, drawn where WebKit draws every SVG a page shows: in a web
//! process of its own, sandboxed, out of this one (Search's `SVGIcons`, #680).
//!
//! A site's SVG is put in an `<img>` there (an SVG as an image runs no script
//! and loads nothing), drawn on a canvas at a tab's size and handed back as a
//! PNG, which only the PNG decoder then reads. No SVG parser runs in Tap's
//! process. One small view, not in any window, with a store that keeps nothing
//! and page scripts off; it is let go after a few seconds without work, and
//! with it its process. What it draws is kept as the PNGs every icon is.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use block2::{DynBlock, RcBlock};
use objc2::{
    AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{
    NSData, NSDataBase64DecodingOptions, NSDictionary, NSError, NSRect, NSString, NSTimer,
};
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKContentWorld, WKNavigation, WKNavigationAction,
    WKNavigationActionPolicy, WKNavigationDelegate, WKWebView, WKWebViewConfiguration,
    WKWebsiteDataStore,
};

/// The largest SVG drawn, in bytes. Icons are a few kilobytes.
const LIMIT: usize = 256 * 1024;
/// Longer than this, the drawing is given up on, and its process with it.
const PATIENCE: f64 = 4.0;
/// Without work for this long, the view and its process go.
const IDLE: f64 = 10.0;
/// A PNG back from the drawing, and no larger: icons are small.
const LARGEST_PNG: usize = 2_000_000;

/// The empty page the drawing view starts on: nothing can be loaded into it
/// but `data:` images.
const EMPTY: &str = "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:\">";

/// The SVG drawn into a square PNG, in the web process; only the PNG it makes
/// comes back. The SVG arrives as `svg`; kept to its proportions, centred.
const DRAW: &str = r"const side = 128;
const img = new Image();
img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
await img.decode();
const w = img.naturalWidth || side, h = img.naturalHeight || side;
const scale = Math.min(side / w, side / h);
const canvas = document.createElement('canvas');
canvas.width = side; canvas.height = side;
canvas.getContext('2d').drawImage(img, (side - w * scale) / 2, (side - h * scale) / 2, w * scale, h * scale);
return canvas.toDataURL('image/png');";

/// The prefix every drawing comes back with.
const PNG_PREFIX: &str = "data:image/png;base64,";

/// A drawing waiting for the view's empty page: the view, or nothing when it failed.
type Waiting = Box<dyn FnOnce(Option<Retained<WKWebView>>)>;
/// A drawing's answer: the PNG, or nothing when it can't be drawn.
type Answer = Box<dyn FnOnce(Option<Vec<u8>>)>;

struct Shared {
    view: Retained<WKWebView>,
    loaded: bool,
    idle: Option<Retained<NSTimer>>,
}

thread_local! {
    /// The drawing view, if one is kept; the delegate below outlives it.
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
    /// The delegate, kept for the app's whole life so a view still unloading
    /// can't call into a delegate that is gone.
    static DELEGATE: RefCell<Option<Retained<SvgDelegate>>> = const { RefCell::new(None) };
    /// Draws waiting for the view's empty page to finish loading.
    static WAITING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
}

/// An SVG by what the server says it is, or by its first bytes.
#[must_use]
pub fn looks_like_svg(bytes: &[u8], mime: Option<&str>) -> bool {
    if bytes.len() > LIMIT {
        return false;
    }
    if mime.is_some_and(|mime| mime.eq_ignore_ascii_case("image/svg+xml")) {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    head.contains("<svg")
}

/// The SVG drawn into a square PNG, or `None` if it can't be. On the main
/// thread, where the drawing view lives; anywhere else answers `None` at once.
pub fn draw(svg: Vec<u8>, then: impl FnOnce(Option<Vec<u8>>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        then(None);
        return;
    };
    SHARED.with_borrow_mut(|slot| {
        if let Some(shared) = slot.as_mut()
            && let Some(idle) = shared.idle.take()
        {
            idle.invalidate();
        }
    });
    let view = SHARED.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(make(mtm));
        }
        slot.as_ref()
            .filter(|shared| shared.loaded)
            .map(|shared| shared.view.clone())
    });
    match view {
        Some(view) => run(&view, &svg, then),
        None => WAITING.with_borrow_mut(|waiting| {
            waiting.push(Box::new(move |view| match view {
                Some(view) => run(&view, &svg, then),
                None => then(None),
            }));
        }),
    }
}

/// Draws now, in the loaded view.
fn run(view: &WKWebView, svg: &[u8], then: impl FnOnce(Option<Vec<u8>>) + 'static) {
    let Ok(source) = std::str::from_utf8(svg) else {
        then(None);
        return;
    };
    let keys = [NSString::from_str("svg")];
    let values = [NSString::from_str(source)];
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let values: Vec<&AnyObject> = values.iter().map(AsRef::as_ref).collect();
    let arguments: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&keys, &values);
    // Whichever answers first — the drawing or the patience — owns the answer.
    let done = Rc::new(Cell::new(false));
    let then: Rc<RefCell<Option<Answer>>> = Rc::new(RefCell::new(Some(Box::new(then))));
    let stop = RcBlock::new({
        let done = done.clone();
        let then = then.clone();
        move |_timer: std::ptr::NonNull<NSTimer>| {
            if done.get() {
                return;
            }
            done.set(true);
            if let Some(then) = then.borrow_mut().take() {
                then(None);
            }
            let_go();
        }
    });
    // SAFETY: a one-shot timer on the main run loop, invalidated with the answer.
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(PATIENCE, false, &stop) };
    let block = RcBlock::new({
        let then = then.clone();
        move |value: *mut AnyObject, _error: *mut NSError| {
            if done.get() {
                return;
            }
            done.set(true);
            timer.invalidate();
            // SAFETY: WebKit hands a valid object or null, for the call's length.
            let text = unsafe { value.as_ref() }
                .and_then(|value| value.downcast_ref::<NSString>())
                .map(ToString::to_string);
            let png = text
                .as_deref()
                .and_then(|text| text.strip_prefix(PNG_PREFIX))
                .and_then(|encoded| {
                    NSData::initWithBase64EncodedString_options(
                        NSData::alloc(),
                        &NSString::from_str(encoded),
                        NSDataBase64DecodingOptions::empty(),
                    )
                })
                .map(|data| data.to_vec())
                .filter(|png| !png.is_empty() && png.len() < LARGEST_PNG);
            if let Some(then) = then.borrow_mut().take() {
                then(png);
            }
            let_go_later();
        }
    });
    // SAFETY: a live web view showing the empty page, its arguments, and blocks
    // that outlive the call; the page's own world, as Search draws in.
    unsafe {
        let world = WKContentWorld::defaultClientWorld(MainThreadMarker::from(view));
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(DRAW),
            Some(&arguments),
            None,
            &world,
            Some(&block),
        );
    }
}

/// The shared view, loading its empty page.
fn make(mtm: MainThreadMarker) -> Shared {
    let delegate =
        DELEGATE.with_borrow_mut(|slot| slot.get_or_insert_with(|| SvgDelegate::new(mtm)).clone());
    // SAFETY: plain property setters on a fresh configuration, on the main thread.
    let config = unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
        config
            .defaultWebpagePreferences()
            .setAllowsContentJavaScript(false);
        config.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::All);
        config
    };
    // SAFETY: a fresh allocation, initialised once with a valid configuration.
    let view = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config)
    };
    // SAFETY: the delegate lives for the app's whole life in `DELEGATE`, longer
    // than any drawing view; cleared again in `let_go`.
    unsafe {
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        view.loadHTMLString_baseURL(&NSString::from_str(EMPTY), None);
    }
    Shared {
        view,
        loaded: false,
        idle: None,
    }
}

/// The drawing view may go after a while without work, and with it its process.
fn let_go_later() {
    SHARED.with_borrow_mut(|slot| {
        let Some(shared) = slot.as_mut() else {
            return;
        };
        if let Some(idle) = shared.idle.take() {
            idle.invalidate();
        }
        let rest = RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| {
            let_go();
        });
        // SAFETY: a one-shot timer on the main run loop, invalidated with the
        // next drawing.
        shared.idle = Some(unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(IDLE, false, &rest)
        });
    });
}

/// Lets the drawing view go, with its process. A drawing in flight answers
/// through its own patience, so nothing hangs.
fn let_go() {
    SHARED.with_borrow_mut(|slot| {
        let Some(shared) = slot.take() else {
            return;
        };
        // SAFETY: plain calls on a live web view; the delegate is cleared so a
        // view still unloading can't call back into a view that is gone.
        unsafe {
            shared.view.stopLoading();
            shared.view.setNavigationDelegate(None);
        }
    });
}

/// The empty page finished loading: waiting drawings run.
fn did_finish(web: &WKWebView) {
    let view = SHARED.with_borrow_mut(|slot| {
        let same = slot
            .as_ref()
            .is_some_and(|shared| std::ptr::eq(Retained::as_ptr(&shared.view), web));
        if !same {
            return None;
        }
        if let Some(shared) = slot.as_mut() {
            shared.loaded = true;
            Some(shared.view.clone())
        } else {
            None
        }
    });
    let waiting = WAITING.with_borrow_mut(std::mem::take);
    for waiting in waiting {
        waiting(view.clone());
    }
}

/// The empty page failed, or the drawing process ended: waiting drawings give
/// up, and the view goes so the next drawing starts fresh.
fn did_fail() {
    let_go();
    let waiting = WAITING.with_borrow_mut(std::mem::take);
    for waiting in waiting {
        waiting(None);
    }
}

define_class!(
    /// The drawing view's navigation delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSvgDelegate"]
    struct SvgDelegate;

    unsafe impl NSObjectProtocol for SvgDelegate {}

    unsafe impl WKNavigationDelegate for SvgDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            web: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: plain property reads on the action WebKit hands us.
            let blank = unsafe {
                action
                    .request()
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .is_some_and(|url| url.to_string() == "about:blank")
            };
            let _ = web;
            handler.call((if blank {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            did_finish(web);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_early(
            &self,
            _web: &WKWebView,
            _navigation: Option<&WKNavigation>,
            _error: &NSError,
        ) {
            did_fail();
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed_late(
            &self,
            _web: &WKWebView,
            _navigation: Option<&WKNavigation>,
            _error: &NSError,
        ) {
            did_fail();
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn ended(&self, _web: &WKWebView) {
            did_fail();
        }
    }
);

impl SvgDelegate {
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
    fn an_svg_by_its_mime_or_its_first_bytes() {
        let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        assert!(looks_like_svg(svg, None));
        assert!(looks_like_svg(svg, Some("image/svg+xml")));
        assert!(looks_like_svg(b"<SVG viewBox=\"0 0 28 28\"></SVG>", None));
        assert!(looks_like_svg(b"not an image", Some("IMAGE/SVG+XML")));
    }

    #[test]
    fn anything_else_is_no_svg() {
        let png = b"\x89PNG\r\n\x1a\nnot really, but not an svg";
        assert!(!looks_like_svg(png, Some("image/png")));
        assert!(!looks_like_svg(b"", None));
        assert!(!looks_like_svg(
            b"<html><body>no svg here",
            Some("text/html")
        ));
        let mut big = vec![0u8; LIMIT + 1];
        big[..5].copy_from_slice(b"<svg ");
        assert!(!looks_like_svg(&big, Some("image/svg+xml")));
    }
}
