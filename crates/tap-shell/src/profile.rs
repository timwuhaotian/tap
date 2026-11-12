//! File › Export Tap Profile… and Import Profile…: this profile as one offline file for
//! your other Mac, and one brought in from it. The format is `tap_import::profile`'s; this is the Mac side:
//! panels, the share picker, and where what comes in goes.
//!
//! What comes in: the tabs of each of its spaces with their groups, folded or not, after
//! the ones here (Tap has one row so far, so spaces come in one after another); its
//! history, a place already here keeping the larger count; its bookmarks, kept beside
//! these without doubles for when Tap shows bookmarks; its settings, allowlisted again.
//! Sign-ins, passwords and cookies are never in the file. Not yet: pins as pins (they
//! come as tabs), spaces and profiles of their own, and reinstalling its extensions.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use block2::RcBlock;
use objc2::{AnyThread, MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSModalResponseOK, NSOpenPanel, NSSavePanel,
    NSSharingServicePicker, NSWindow,
};
use objc2_foundation::{NSArray, NSDate, NSDateFormatter, NSLocale, NSRectEdge, NSString, NSURL};
use tap_core::history;
use tap_import::profile::{self, Incoming, Outgoing};

use crate::{oslog, prefs, rt, settings, store, update, window};

/// What an export wrote.
pub struct Written {
    pub tabs: usize,
    pub groups: usize,
    pub settings: usize,
}

/// The profile as it is now, written to `path`.
///
/// # Errors
/// If it can't be packed or written.
pub fn write(path: &Path) -> Result<Written, String> {
    let (session, history, tabs, groups) = rt::read(|b| {
        let shape = b.shape();
        let groups = shape.groups.as_ref().map_or(0, Vec::len);
        let tabs = shape.tabs.len();
        (
            shape.encode(),
            b.history().encode(history::now()),
            tabs,
            groups,
        )
    })
    .ok_or("Tap is busy")?;
    let prefs = prefs::for_profile();
    let settings = prefs.len();
    let read = |name: &str| std::fs::read(store::file(name)).ok();
    let bytes = profile::pack(&Outgoing {
        session,
        history,
        read: &read,
        prefs,
        extensions: Vec::new(),
        app_version: update::current(),
        now: history::now(),
    })?;
    let temporary = path.with_extension("partial");
    std::fs::write(&temporary, &bytes)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    oslog::note(&format!(
        "exported a profile: {tabs} tabs, {groups} groups, {settings} settings"
    ));
    Ok(Written {
        tabs,
        groups,
        settings,
    })
}

/// What an import brought in, and the sentence that says so.
pub struct Taken {
    pub tabs: usize,
    pub groups: usize,
    pub places: usize,
    pub bookmarks: usize,
    pub settings: usize,
    pub said: String,
}

/// Brings in the profile at `path`. Nothing in this window is overwritten: its tabs come
/// after the ones here, in groups of their own.
///
/// # Errors
/// If it isn't a profile this version reads, said as it is to be shown.
pub fn take(path: &Path) -> Result<Taken, String> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("searchprofile"))
    {
        return Err("Search profiles are not supported".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|_| "That file couldn't be read".to_owned())?;
    let incoming = profile::unpack(&bytes).map_err(|p| p.said())?;
    Ok(bring_in(incoming, "the profile"))
}

/// What came in, from a file or from another browser here (`bring`), merged into this
/// window and said.
pub(crate) fn bring_in(incoming: Incoming, from: &str) -> Taken {
    let Incoming {
        sessions,
        histories,
        bookmarks,
        prefs: settings,
        extensions,
        ..
    } = incoming;
    let (tabs, groups) = rt::update(|b| b.take_tabs(sessions)).unwrap_or_default();
    let places = histories
        .iter()
        .map(|data| rt::update(|b| b.take_history(data)).unwrap_or(0))
        .sum();
    let bookmarks = if bookmarks.is_empty() {
        0
    } else {
        let file = store::file("bookmarks.json");
        let kept = std::fs::read(&file).ok();
        let (data, added, _) = profile::merge_bookmarks(kept.as_deref(), bookmarks);
        // The model keeps what is kept: the file has just changed under it.
        let merged = tap_core::bookmarks::Bookmarks::load(&data).unwrap_or_default();
        store::save(file, data);
        rt::update(|b| b.set_bookmarks(merged));
        added
    };
    let set = prefs::from_profile(&settings);
    rt::settle();
    if let Some(mtm) = MainThreadMarker::new() {
        prefs::apply(mtm);
        settings::changed();
    }
    let mut parts = Vec::new();
    if tabs > 0 {
        parts.push(match groups {
            0 => count(tabs, "tab", "tabs"),
            _ => format!(
                "{} in {}",
                count(tabs, "tab", "tabs"),
                count(groups, "group", "groups")
            ),
        });
    }
    if places > 0 {
        parts.push(count(places, "place", "places"));
    }
    if bookmarks > 0 {
        parts.push(format!(
            "{} kept for when Tap shows them",
            count(bookmarks, "bookmark", "bookmarks")
        ));
    }
    if set > 0 {
        parts.push(count(set, "setting", "settings"));
    }
    let mut said = if parts.is_empty() {
        format!("Nothing to bring in from {from}")
    } else {
        format!("{} from {from}", parts.join(", "))
    };
    if !parts.is_empty() {
        said.push_str(". Sign-ins, passwords and cookies aren't carried over");
        if !extensions.is_empty() {
            let _ = write!(
                said,
                "; its {} aren't in Tap yet",
                count(extensions.len(), "extension", "extensions")
            );
        }
        said.push('.');
    }
    oslog::note(&format!("imported a profile: {said}"));
    Taken {
        tabs,
        groups,
        places,
        bookmarks,
        settings: set,
        said,
    }
}

pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Whether a file is one to bring in rather than show: by its extension, or a `.zip`
/// carrying a profile's manifest.
pub fn is_profile_path(path: &str) -> bool {
    let path = Path::new(path);
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        profile::EXTENSION => true,
        "zip" => std::fs::read(path).is_ok_and(|bytes| profile::is_profile(&bytes)),
        _ => false,
    }
}

/// File › Export Tap Profile…: a save sheet, the file, then the share picker, so it can
/// go straight to the other Mac by AirDrop. Local only: nothing reaches the network
/// unless AirDrop or a share destination does.
pub fn export() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSSavePanel::savePanel(mtm);
    #[allow(deprecated)] // the extension is Tap's own: no system type covers it
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
        profile::EXTENSION,
    )])));
    panel.setAllowsOtherFileTypes(true);
    panel.setCanCreateDirectories(true);
    panel.setExtensionHidden(false);
    panel.setNameFieldStringValue(&NSString::from_str(&format!(
        "Tap-profile-{}.{}",
        today(),
        profile::EXTENSION
    )));
    panel.setMessage(Some(&NSString::from_str(
        "This profile's tabs and groups, history, bookmarks and settings — without sign-ins, passwords or cookies.",
    )));
    let chosen = panel.clone();
    let done = RcBlock::new(move |answer: isize| {
        if answer != NSModalResponseOK {
            return;
        }
        let Some(path) = chosen.URL().and_then(|u| u.path()) else {
            return;
        };
        let path = PathBuf::from(path.to_string());
        match write(&path) {
            Ok(_) => share(&path),
            Err(why) => tell("Couldn't export the profile", &why),
        }
    });
    match window::main_window() {
        Some(window) => panel.beginSheetModalForWindow_completionHandler(&window, &done),
        None => done.call((panel.runModal(),)),
    }
}

/// The day, for the export's name, so two exports on two days don't collide.
fn today() -> String {
    let formatter = NSDateFormatter::new();
    formatter.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(
        &NSString::from_str("en_US_POSIX"),
    )));
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd")));
    formatter.stringFromDate(&NSDate::now()).to_string()
}

/// The file offered wherever the Mac would send it: AirDrop, Mail, Messages.
fn share(path: &Path) {
    let Some(window) = window::main_window() else {
        return;
    };
    let Some(view) = window.contentView() else {
        return;
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: an array of one NSURL is an array of objects, and an NSURL is an item the
    // picker takes.
    let picker = unsafe {
        let items: Retained<NSArray> =
            Retained::cast_unchecked(NSArray::from_retained_slice(&[url]));
        NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items)
    };
    // A few points in from the top right corner, the picker hanging below it.
    let bounds = view.bounds();
    let inset = 12.0;
    let y = if view.isFlipped() {
        inset
    } else {
        bounds.size.height - inset
    };
    let anchor = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(bounds.size.width - inset, y),
        objc2_foundation::NSSize::new(1.0, 1.0),
    );
    let edge = if view.isFlipped() {
        NSRectEdge::MaxY
    } else {
        NSRectEdge::MinY
    };
    picker.showRelativeToRect_ofView_preferredEdge(anchor, &view, edge);
}

/// File › Import Profile…: a Tap profile, or a `.zip` that is one.
pub fn choose_and_import() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    #[allow(deprecated)] // Tap's extension has no system type
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[
        NSString::from_str(profile::EXTENSION),
        NSString::from_str("zip"),
    ])));
    panel.setAllowsOtherFileTypes(true);
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseDirectories(false);
    panel.setPrompt(Some(&NSString::from_str("Bring In")));
    panel.setMessage(Some(&NSString::from_str(
        "A profile exported from Tap (.tapprofile).",
    )));
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    let Some(path) = panel.URL().and_then(|u| u.path()) else {
        return;
    };
    take_and_tell(&PathBuf::from(path.to_string()));
}

/// A profile file opened from the Finder or AirDrop: asked first, since its settings
/// will be set here.
pub fn open_file(path: &Path) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    window::bring_forward();
    crate::links::bring_app_forward(mtm);
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!("Bring in “{name}”?")));
    alert.setInformativeText(&NSString::from_str(
        "Its tabs and their groups join this window, its history and bookmarks are added, and its settings are set here. Sign-ins, passwords and cookies aren't in it.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Bring In"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    if alert.runModal() == NSAlertFirstButtonReturn {
        take_and_tell(path);
    }
}

pub(crate) fn take_and_tell(path: &Path) {
    match take(path) {
        Ok(taken) => tell("Brought in", &taken.said),
        Err(why) => tell("Couldn't bring that in", &why),
    }
}

/// Says how it went, on the window.
pub(crate) fn tell(title: &str, said: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if std::env::var_os(tap_core::identity::PROBE_ENV).is_some() {
        oslog::note(&format!("{title}: {said}"));
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(said));
    match window::main_window() {
        Some(window) => sheet(&alert, &window),
        None => {
            alert.runModal();
        }
    }
}

fn sheet(alert: &Retained<NSAlert>, window: &NSWindow) {
    alert.beginSheetModalForWindow_completionHandler(window, None);
}
