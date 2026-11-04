//! Links from elsewhere (Search's `Links.swift`): a click in Mail, in Slack, in a PDF.
//! macOS hands the address to whichever app owns http and https; `Info.plist` says Tap
//! does, and says it can show web pages (`public.html` and `public.xhtml`, which macOS
//! asks of an app before it lists it as a default web browser). This is the other half.
//!
//! The window is made before AppKit starts handing anything over, so nothing has to
//! wait for it. A test run takes nothing from outside: a link from another app would
//! bring its window forward. The bench hands links in itself (`link`).
//!
//! Not yet: Search's small window for links (Little), the Dock menu's New Window,
//! staying open with no window, and Settings' "Make Tap the default browser".

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSURL};
use tap_core::{fnd::Url, identity};

use crate::{rt, window};

fn testing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// What the system opened Tap with: addresses, and pages on this Mac.
pub fn opened(urls: &NSArray<NSURL>) {
    if testing() {
        return;
    }
    for url in urls {
        if let Some(address) = url.absoluteString() {
            take(&address.to_string());
        }
    }
}

/// A link as another app hands one over; the bench's way in on a test run. Answers
/// whether it was one Tap opens.
pub fn take(address: &str) -> bool {
    let Some(url) = Url::parse(address) else {
        return false;
    };
    let scheme = url.scheme().unwrap_or_default().to_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "file") {
        return false;
    }
    rt::dispatch(move |browser| browser.arrive(&url));
    window::bring_forward();
    come_forward();
    true
}

/// In front of the app the link was clicked in, as a browser comes forward for Mail.
/// Since macOS 14 an app comes in front when the app asking hands the front over; not
/// every app does (Electron apps don't), so if Tap still isn't in front a moment later
/// it asks again the old way.
fn come_forward() {
    // Never a test run's: a probe stays off the screen it was started on.
    let Some(mtm) = MainThreadMarker::new().filter(|_| !testing()) else {
        return;
    };
    NSApplication::sharedApplication(mtm).activate();
    let later = dispatch2::DispatchTime::NOW.time(250_000_000);
    // On the main queue, where the marker is taken again.
    let _ = dispatch2::DispatchQueue::main().after(later, || {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if !app.isActive() {
            #[allow(deprecated)] // the pre-macOS 14 way, for apps that don't hand over
            app.activateIgnoringOtherApps(true);
        }
    });
}
