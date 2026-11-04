//! Tap's Mac side: the app, its window and views, and the web views, through objc2
//! (docs/PLAN.md §5). The model is `tap-core`'s; this crate shows it and carries out what
//! it asks for.
//!
//! Split into `tap-ui`, `tap-web` and `tap-shell` crates once each part has the weight to
//! make incremental builds worth it (plan §4); the modules already follow those lines.

mod address_bar;
mod agents;
mod bench;
mod blank;
mod bring;
mod crash;
mod dialogs;
mod downloads;
mod findbar;
mod float;
mod forms;
mod icons;
mod keychain;
mod links;
mod menu;
mod motion;
mod offers;
mod oslog;
mod page_find;
mod palette;
mod prefs;
mod profile;
mod reader;
mod rt;
mod settings;
mod signin;
mod sleep;
mod store;
mod strip;
mod tabmenu;
mod update;
mod update_window;
mod vault;
mod web;
mod window;

use objc2::{MainThreadMarker, runtime::ProtocolObject};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use tap_core::browser::{Browser, Tab};

/// Search's profile on this Mac written as a Tap profile file (`cargo xtask
/// search-profile`); answers what went in.
///
/// # Errors
/// If there is no Search store here, or the file can't be written.
pub fn write_search_profile(path: &std::path::Path) -> Result<String, String> {
    profile::write_from_search(path)
}

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
    let (bar, update_item) = menu::main_menu(mtm, &commands);
    app.setMainMenu(Some(&bar));
    update_window::install(commands.clone());

    // Settings › Start with a fresh window: only what you said to keep comes back.
    let session = store::read_session();
    let session = if prefs::starts_fresh() {
        session.fresh()
    } else {
        session
    };
    let mut browser = Browser::restored(session);
    let (history, rewrite) = store::read_history();
    browser.set_history(history, rewrite);
    rt::install(browser);
    prefs::apply(mtm);
    window::open(mtm, &commands);
    reader::install();
    float::install(mtm);
    bench::start();
    sleep::start(mtm);
    if let Some(item) = update_item {
        update::start(item);
    }
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
