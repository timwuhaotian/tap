//! Bring things over from another browser on this Mac (Search's `ImportPanel`, cut to
//! what Tap holds): the browsers found here, Search and the Chromium ones, then one of
//! them brought in. What there is to bring is read off the main thread and said before
//! anything changes; then it is merged as a profile file is (`profile::bring_in`).
//!
//! Nothing in the other browser changes. Its tabs already open here aren't opened again,
//! so bringing the same browser over twice adds only what is new. Not yet: choosing a
//! profile (the one used last comes), Firefox and Safari, passwords and extensions.

use std::{collections::HashSet, path::PathBuf};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn};
use objc2_foundation::NSString;
use serde_json::{Map, Value};
use tap_core::{history, identity};
use tap_import::{Source, chromium, from_search, profile::Incoming};

use crate::{prefs, profile, rt, store};

/// What one browser has to bring, and the profile it comes from when it has several.
type Read = Result<(Incoming, Option<String>), String>;

fn probing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// Where browsers keep their data: `~/Library/Application Support`. A test run reads
/// made-up ones from its own folder instead, never a real browser's (as Search's
/// `Chromium.base`).
fn base() -> PathBuf {
    if probing() {
        return store::folder().join("Import");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    home.join("Library/Application Support")
}

/// The browsers here with something to bring, Search first.
pub fn found() -> Vec<Source> {
    tap_import::found(&base())
}

fn named(name: &str) -> Option<Source> {
    found().into_iter().find(|s| s.name() == name)
}

/// The addresses open in this window now.
fn open() -> HashSet<String> {
    rt::read(|b| b.tabs().iter().map(|t| t.url().to_owned()).collect()).unwrap_or_default()
}

/// Search's settings under the profile allowlist, read on the main thread. A test run
/// takes none: they would be the real Search's.
fn settings_of(source: Source) -> Map<String, Value> {
    match source {
        Source::Search if !probing() => prefs::for_profile_of(from_search::DOMAIN),
        _ => Map::new(),
    }
}

/// Everything `source` has to bring, less the tabs open here already.
fn read(source: Source, settings: Map<String, Value>, open: &HashSet<String>) -> Read {
    let base = base();
    let (mut incoming, profile) = match source {
        Source::Search => (
            from_search::read(&base.join(from_search::FOLDER), settings, history::now())?,
            None,
        ),
        Source::Chromium(browser) => {
            let found = chromium::read(browser, &base, history::now())
                .ok_or_else(|| format!("{} has no profile on this Mac", browser.name))?;
            (found.incoming, found.profile)
        }
    };
    incoming.leave_out(open);
    Ok((incoming, profile))
}

/// Settings' Bring Over menu: `name`'s things read off the main thread, then said, and
/// brought in once you say so.
pub fn offer(name: &str) {
    let Some(source) = named(name) else {
        return profile::tell(
            "Couldn't bring that over",
            &format!("{name} has nothing to bring on this Mac any more."),
        );
    };
    let settings = settings_of(source);
    let open = open();
    std::thread::spawn(move || {
        let read = read(source, settings, &open);
        dispatch2::DispatchQueue::main().exec_async(move || ask(source, read));
    });
}

fn ask(source: Source, read: Read) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let name = source.name();
    let (incoming, from) = match read {
        Ok(read) => read,
        Err(why) => return profile::tell("Couldn't bring that over", &why),
    };
    let Some(what) = summary(&incoming) else {
        return profile::tell(
            &format!("Nothing to bring over from {name}"),
            "Its open tabs are open here already, and it has no history or bookmarks to bring.",
        );
    };
    if !probing() {
        let from = from
            .map(|p| format!(", from its profile “{p}”"))
            .unwrap_or_default();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!("Bring over from {name}?")));
        alert.setInformativeText(&NSString::from_str(&format!(
            "{what}{from}. Its tabs join this window, asleep. Nothing in {name} changes, and its passwords, cookies and extensions stay there."
        )));
        alert.addButtonWithTitle(&NSString::from_str("Bring In"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return;
        }
    }
    let taken = profile::bring_in(incoming, name);
    profile::tell("Brought over", &taken.said);
}

/// What there is to bring, in a few words, or nothing.
fn summary(incoming: &Incoming) -> Option<String> {
    let (tabs, groups) = (incoming.tabs(), incoming.groups());
    let mut parts = Vec::new();
    if tabs > 0 {
        let open = profile::count(tabs, "open tab", "open tabs");
        parts.push(match groups {
            0 => open,
            _ => format!("{open} in {}", profile::count(groups, "group", "groups")),
        });
    }
    let places = incoming.places();
    if places > 0 {
        parts.push(profile::count(
            places,
            "place in its history",
            "places in its history",
        ));
    }
    let bookmarks = incoming.bookmark_count();
    if bookmarks > 0 {
        parts.push(profile::count(bookmarks, "bookmark", "bookmarks"));
    }
    if !incoming.prefs.is_empty() {
        parts.push(profile::count(incoming.prefs.len(), "setting", "settings"));
    }
    let last = parts.pop()?;
    Some(if parts.is_empty() {
        last
    } else {
        format!("{} and {last}", parts.join(", "))
    })
}

/// The bench's way in: `name` brought over at once, unasked.
///
/// # Errors
/// If no such browser is here, or what it has couldn't be read.
pub fn take(name: &str) -> Result<profile::Taken, String> {
    let source = named(name).ok_or_else(|| format!("{name} isn't on this Mac"))?;
    let (incoming, _) = read(source, settings_of(source), &open())?;
    Ok(profile::bring_in(incoming, name))
}

/// Settings' words for what was found: "Search, Chrome and Arc".
pub fn listed(found: &[Source]) -> Option<String> {
    let (last, rest) = found.split_last()?;
    Some(if rest.is_empty() {
        last.name().to_owned()
    } else {
        let rest: Vec<&str> = rest.iter().map(|s| s.name()).collect();
        format!("{} and {}", rest.join(", "), last.name())
    })
}
