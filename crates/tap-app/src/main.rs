//! Tap's app. For now it is M0's first window (docs/PLAN.md §8): one window with one
//! `WKWebView`, enough to prove the build, signing, notarization and update path end to
//! end. The browser is ported into it, milestone by milestone, from Search.

use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSMenu, NSMenuItem, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

/// What the window shows until the browser is in.
const PAGE: &str = include_str!("placeholder.html");

fn main() {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("tap: must start on the main thread");
        std::process::exit(1);
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.setMainMenu(Some(&main_menu(mtm)));

    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(960.0, 640.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // SAFETY: a fresh allocation, initialised once, on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: `window` is owned by this function's `Retained` for the app's whole life, so
    // AppKit must not also release it when it closes.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Tap"));

    // SAFETY: a fresh configuration, made and used on the main thread.
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    // SAFETY: a fresh allocation, initialised once, with a valid configuration.
    let web = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
    };
    // SAFETY: a static, script-free page with no base URL, so it can reach nothing.
    unsafe { web.loadHTMLString_baseURL(&NSString::from_str(PAGE), None) };
    window.setContentView(Some(&web));
    window.center();
    window.makeKeyAndOrderFront(None);

    app.activate();
    app.run();
}

/// The app menu: About, Hide and Quit, sent to `NSApplication`'s standard actions.
fn main_menu(mtm: MainThreadMarker) -> objc2::rc::Retained<NSMenu> {
    let menu_bar = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    let app_menu = NSMenu::new(mtm);
    for (title, action, key) in [
        ("About Tap", sel!(orderFrontStandardAboutPanel:), ""),
        ("Hide Tap", sel!(hide:), "h"),
        ("Quit Tap", sel!(terminate:), "q"),
    ] {
        // SAFETY: a fresh allocation; each action is a standard NSApplication action that
        // takes the sender, and the target is found through the responder chain.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        if title == "Quit Tap" {
            app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        app_menu.addItem(&item);
    }
    app_item.setSubmenu(Some(&app_menu));
    menu_bar.addItem(&app_item);
    menu_bar
}
