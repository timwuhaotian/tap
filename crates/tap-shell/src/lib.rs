//! Tap's Mac side: the app, its window and views, and the web views, through objc2
//! (docs/PLAN.md §5). The model is `tap-core`'s; this crate shows it and carries out what
//! it asks for.
//!
//! Split into `tap-ui`, `tap-web` and `tap-shell` crates once each part has the weight to
//! make incremental builds worth it (plan §4); the modules already follow those lines.

mod bench;
mod crash;
mod menu;
mod offers;
mod palette;
mod rt;
mod store;
mod strip;
mod web;
mod window;

use objc2::{MainThreadMarker, runtime::ProtocolObject};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use tap_core::browser::{Browser, Tab};

/// Runs Tap until it quits.
pub fn run() {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("tap: must start on the main thread");
        std::process::exit(1);
    };
    crash::install();

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let commands = menu::Commands::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*commands)));
    app.setMainMenu(Some(&menu::main_menu(mtm, &commands)));

    let mut browser = Browser::restored(store::read_session());
    let (history, rewrite) = store::read_history();
    browser.set_history(history, rewrite);
    rt::install(browser);
    window::open(mtm, &commands);
    bench::start();
    // A window that opens on a new tab opens with the keys in its field.
    rt::dispatch(|browser| {
        if browser.active_tab().is_some_and(Tab::is_blank) && browser.editing().is_none() {
            browser.begin_editing(false);
        }
        browser.focus_page();
    });

    app.activate();
    app.run();
    drop(commands);
}
