//! Bring things over from Chrome or a Safari export, a profile, or a passwords CSV.
//! Chrome is counted without decrypting passwords; its key is requested only on import.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    io::Read as _,
    path::{Path, PathBuf},
};

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSButton, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSFontWeightSemibold, NSMenuItem, NSModalResponseOK,
    NSOpenPanel, NSPopUpButton, NSResponder, NSSwitch, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use serde_json::Value;
use tap_core::{history, identity};
use tap_import::{
    Source,
    chromium::{self, Pick},
    passwords,
    profile::Incoming,
    safari,
};

use crate::{keychain, palette, profile, rt, store, vault, window};

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

/// Chrome when installed, and Safari's export-file import.
pub fn found() -> Vec<Source> {
    let mut sources: Vec<_> = tap_import::found(&base())
        .into_iter()
        .filter(|source| matches!(source, Source::Chromium(browser) if browser.name == "Chrome"))
        .collect();
    sources.push(Source::Safari);
    sources
}

fn named(name: &str) -> Option<Source> {
    found().into_iter().find(|s| s.name() == name)
}

/// The addresses open in this window now.
fn open_here() -> HashSet<String> {
    rt::read(|b| b.tabs().iter().map(|t| t.url().to_owned()).collect()).unwrap_or_default()
}

/// The profile chosen: `None` the one used last, `Some("")` all of them, else a folder.
fn pick_of(chosen: Option<&str>) -> Pick<'_> {
    match chosen {
        None => Pick::Usual,
        Some("") => Pick::All,
        Some(folder) => Pick::One(folder),
    }
}

/// Everything `source` has to bring but its passwords, less the tabs open here already.
fn read(source: Source, open: &HashSet<String>, chosen: Option<&str>) -> Result<Incoming, String> {
    let base = base();
    let mut incoming = match source {
        Source::Safari => return Err("Choose a Safari export file".to_owned()),
        Source::Chromium(browser) => {
            chromium::read(browser, &base, history::now(), pick_of(chosen))
                .ok_or_else(|| format!("{} has no profile on this Mac", browser.name))?
                .incoming
        }
    };
    incoming.leave_out(open);
    Ok(incoming)
}

/// How many passwords `source` has, from its files alone: no key, no question.
fn password_count(source: Source, chosen: Option<&str>) -> usize {
    match source {
        Source::Chromium(browser) if browser.safe_storage().is_some() => {
            chromium::password_count(browser, &base(), pick_of(chosen))
        }
        Source::Safari | Source::Chromium(_) => 0,
    }
}

/// A Chromium browser's key: the keychain's to hand over, after macOS asks. A test run's
/// made-up browser keeps its key beside its profiles (Search's rule).
fn passphrase(browser: &chromium::Browser) -> Option<String> {
    if probing() {
        let file = browser.root(&base()).join("Safe Storage");
        return std::fs::read_to_string(file)
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
    }
    let (service, account) = browser.safe_storage()?;
    keychain::generic(service, account)
}

/// What came of the passwords, read off the main thread.
enum Passwords {
    Chromium(chromium::Passwords),
    Export(Vec<passwords::Login>, usize),
    Trouble(String),
}

fn read_passwords(source: Source, chosen: Option<&str>) -> Passwords {
    match source {
        Source::Safari => Passwords::Trouble("Choose a Safari export file".to_owned()),
        Source::Chromium(browser) => {
            if browser.safe_storage().is_none() {
                return Passwords::Trouble(format!(
                    "Passwords aren't supported for {}",
                    browser.name
                ));
            }
            // The one moment macOS asks, for the browser's key.
            let Some(key) = passphrase(browser) else {
                return Passwords::Trouble(format!(
                    "macOS didn't hand over {}'s key — allow it and try again",
                    browser.name
                ));
            };
            match chromium::passwords(browser, &base(), &key, pick_of(chosen)) {
                Some(found) => Passwords::Chromium(found),
                None => Passwords::Trouble(format!("No passwords readable in {}", browser.name)),
            }
        }
    }
}

/// What to bring, as the sheet's switches say.
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // one switch each, as the sheet has them
pub struct Wants {
    pub passwords: bool,
    pub bookmarks: bool,
    pub history: bool,
    pub tabs: bool,
    pub settings: bool,
}

impl Default for Wants {
    fn default() -> Wants {
        Wants {
            passwords: true,
            bookmarks: true,
            history: true,
            tabs: true,
            settings: true,
        }
    }
}

/// One line of what happened: whether it went well, and what it says.
type Said = (bool, String);

fn n(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn counted(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", n(count))
    }
}

/// What was read, brought in on the main thread and said, line by line (Search's
/// `bring(from:)`).
#[allow(clippy::too_many_lines)] // one report covering each imported category
fn land(
    source: Source,
    incoming: Result<Incoming, String>,
    found: Option<Passwords>,
    wants: Wants,
) -> Vec<Said> {
    let name = source.name();
    let mut said: Vec<Said> = Vec::new();
    match found {
        Some(Passwords::Chromium(found)) => {
            let kept = vault::keep(&found.logins, found.never);
            let skipped = if found.skipped > 0 {
                format!(" ({} skipped: no address)", n(found.skipped))
            } else {
                String::new()
            };
            // Ones that didn't decrypt: said, so an import that brought nothing says why.
            let locked = if found.undecrypted > 0 {
                format!(
                    " — {} couldn't be decrypted with {name}'s key",
                    n(found.undecrypted)
                )
            } else {
                String::new()
            };
            let failed = found.logins.len() - kept;
            let unsaved = if failed > 0 {
                format!(" — {} couldn't be saved", n(failed))
            } else {
                String::new()
            };
            said.push((
                failed == 0 && found.undecrypted == 0,
                format!(
                    "{}{skipped}{locked}{unsaved}",
                    counted(kept, "password", "passwords")
                ),
            ));
        }
        Some(Passwords::Export(found, skipped)) => {
            let kept = vault::keep(&found, Vec::new());
            let failed = found.len() - kept;
            let mut text = counted(kept, "password", "passwords");
            if skipped > 0 {
                let _ = write!(text, ", {} skipped", n(skipped));
            }
            if failed > 0 {
                let _ = write!(text, " — {} couldn't be saved", n(failed));
            }
            said.push((failed == 0, text));
        }
        Some(Passwords::Trouble(why)) => said.push((false, why)),
        None => {}
    }
    let mut incoming = match incoming {
        Ok(incoming) => incoming,
        Err(why) => {
            said.push((false, why));
            return said;
        }
    };
    if !wants.tabs {
        incoming.sessions.clear();
    }
    if !wants.history {
        incoming.histories.clear();
    }
    if !wants.bookmarks {
        incoming.bookmarks.clear();
    }
    if !wants.settings {
        incoming.prefs.clear();
    }
    let had_marks = !incoming.bookmarks.is_empty();
    let had_tabs = incoming.tabs() > 0;
    let taken = profile::bring_in(incoming, name);
    if wants.bookmarks && had_marks {
        said.push((
            true,
            if taken.bookmarks == 0 {
                format!("{name}'s bookmarks were all here already")
            } else {
                format!(
                    "{} — kept for when Tap shows them",
                    counted(taken.bookmarks, "bookmark", "bookmarks")
                )
            },
        ));
    }
    if wants.history && taken.places > 0 {
        said.push((true, counted(taken.places, "place", "places")));
    }
    if wants.tabs && had_tabs {
        said.push((
            true,
            if taken.tabs == 0 {
                format!("{name}'s open tabs were all here already")
            } else if taken.groups > 0 {
                format!(
                    "{} in {}",
                    counted(taken.tabs, "tab", "tabs"),
                    counted(taken.groups, "group", "groups")
                )
            } else {
                counted(taken.tabs, "tab", "tabs")
            },
        ));
    }
    if wants.settings && taken.settings > 0 {
        said.push((true, counted(taken.settings, "setting", "settings")));
    }
    said
}

/// The bench's way in: `name` brought over at once, unasked, passwords and all.
///
/// # Errors
/// If no such browser is here.
pub fn take(name: &str, wants: Wants) -> Result<Vec<Said>, String> {
    let source = named(name).ok_or_else(|| format!("{name} isn't on this Mac"))?;
    if source == Source::Safari {
        return Err("Safari requires an export file selected in the import sheet".to_owned());
    }
    let incoming = read(source, &open_here(), None);
    let found = wants.passwords.then(|| read_passwords(source, None));
    Ok(land(source, incoming, found, wants))
}

/// Settings' words for what was found.
pub fn listed(found: &[Source]) -> Option<String> {
    let (last, rest) = found.split_last()?;
    Some(if rest.is_empty() {
        last.name().to_owned()
    } else {
        let rest: Vec<&str> = rest.iter().map(|s| s.name()).collect();
        format!("{} and {}", rest.join(", "), last.name())
    })
}

/// A passwords CSV another browser exported (Search's `Vault.take(csv:)`): read once, put
/// in the keychain, and the file is yours to delete.
pub fn take_csv(path: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(path).map_err(|_| "That file couldn't be read".to_owned())?;
    let found = passwords::csv(&text);
    if found.logins.is_empty() && found.skipped == 0 {
        return Err("That file has no passwords in a shape Tap knows".to_owned());
    }
    let kept = vault::keep(&found.logins, Vec::new());
    if kept < found.logins.len() {
        return Err(format!(
            "{} — {} couldn't be saved; the exported file holds your passwords in the clear: delete it when no longer needed",
            counted(kept, "password", "passwords"),
            n(found.logins.len() - kept)
        ));
    }
    let mut said = counted(kept, "password", "passwords");
    if found.skipped > 0 {
        let _ = write!(said, ", {} skipped", n(found.skipped));
    }
    said.push_str(" — the exported file holds your passwords in the clear: delete it now");
    Ok(said)
}

// MARK: - the sheet

/// A browser's profiles, and the folder of the one used last.
type Profiles = (Vec<chromium::Profile>, Option<String>);

/// What one browser holds, counted from its files.
#[derive(Clone, Copy, Default)]
struct Preview {
    tabs: usize,
    groups: usize,
    places: usize,
    bookmarks: usize,
    passwords: usize,
}

struct Sheet {
    window: Retained<NSWindow>,
    root: Retained<Flipped>,
    target: Retained<Target>,
    sources: Vec<Source>,
    looking: bool,
    pick: Option<Source>,
    /// Each browser's profiles, and the one used last.
    profiles: HashMap<&'static str, Profiles>,
    /// The profile chosen, where it isn't the usual one: a folder, or "" for all.
    chosen: HashMap<&'static str, String>,
    previews: HashMap<String, Preview>,
    wants: Wants,
    bringing: bool,
    brought: Option<Vec<Said>>,
}

thread_local! {
    static SHEET: RefCell<Option<Sheet>> = const { RefCell::new(None) };
}

const WIDTH: f64 = 580.0;
const INSET: f64 = 24.0;
const CARD: f64 = WIDTH - 2.0 * INSET;

/// The profile to read for `source`: the one chosen, or the one used last.
fn chosen_of(sheet: &Sheet, source: Source) -> Option<String> {
    sheet.chosen.get(source.name()).cloned()
}

fn key(source: Source, chosen: Option<&str>) -> String {
    format!("{}/{}", source.name(), chosen.unwrap_or("usual"))
}

/// Settings' Bring Over: the sheet, on the window.
pub fn open() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(main) = window::main_window() else {
        return;
    };
    if SHEET.with_borrow(Option::is_some) {
        return;
    }
    let sheet = {
        // SAFETY: a fresh allocation, initialised once, on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 200.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: kept in `SHEET` until it is closed; AppKit must not release it too.
        unsafe { window.setReleasedWhenClosed(false) };
        window
    };
    let root = Flipped::new(mtm);
    sheet.setContentView(Some(&root));
    SHEET.with_borrow_mut(|slot| {
        *slot = Some(Sheet {
            window: sheet.clone(),
            root,
            target: Target::new(mtm),
            sources: Vec::new(),
            looking: true,
            pick: None,
            profiles: HashMap::new(),
            chosen: HashMap::new(),
            previews: HashMap::new(),
            wants: Wants::default(),
            bringing: false,
            brought: None,
        });
    });
    render();
    main.beginSheet_completionHandler(&sheet, None);
    look();
}

fn close() {
    let Some(sheet) = SHEET.with_borrow_mut(Option::take) else {
        return;
    };
    if let Some(main) = window::main_window() {
        main.endSheet(&sheet.window);
    }
    sheet.window.orderOut(None);
}

/// Every browser on this Mac and its profiles, found off the main thread.
fn look() {
    std::thread::spawn(|| {
        let base = base();
        let sources = found();
        let profiles: Vec<(&'static str, Profiles)> = sources
            .iter()
            .filter_map(|s| match s {
                Source::Chromium(b) => Some((b.name, chromium::profile_list(b, &base))),
                Source::Safari => None,
            })
            .collect();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let pick = sources.first().copied();
            let found = sources.clone();
            let live = SHEET.with_borrow_mut(|slot| {
                let Some(sheet) = slot.as_mut() else {
                    return false;
                };
                sheet.sources = sources;
                sheet.profiles = profiles.into_iter().collect();
                sheet.pick = pick;
                sheet.looking = false;
                true
            });
            if !live {
                return;
            }
            render();
            for source in found {
                count(source);
            }
        });
    });
}

/// What a browser holds in the profile chosen for it, unless counted already.
fn count(source: Source) {
    if source == Source::Safari {
        return;
    }
    let Some((chosen, open)) = SHEET.with_borrow(|slot| {
        let sheet = slot.as_ref()?;
        let chosen = chosen_of(sheet, source);
        (!sheet.previews.contains_key(&key(source, chosen.as_deref())))
            .then(|| (chosen, open_here()))
    }) else {
        return;
    };
    std::thread::spawn(move || {
        let incoming = read(source, &open, chosen.as_deref());
        let preview = Preview {
            tabs: incoming.as_ref().map_or(0, Incoming::tabs),
            groups: incoming.as_ref().map_or(0, Incoming::groups),
            places: incoming.as_ref().map_or(0, Incoming::places),
            bookmarks: incoming.as_ref().map_or(0, Incoming::bookmark_count),
            passwords: password_count(source, chosen.as_deref()),
        };
        let key = key(source, chosen.as_deref());
        dispatch2::DispatchQueue::main().exec_async(move || {
            let live = SHEET.with_borrow_mut(|slot| {
                slot.as_mut()
                    .map(|sheet| sheet.previews.insert(key, preview))
                    .is_some()
            });
            if live {
                render();
            }
        });
    });
}

fn preview_of(sheet: &Sheet, source: Source) -> Option<Preview> {
    sheet
        .previews
        .get(&key(source, chosen_of(sheet, source).as_deref()))
        .copied()
}

/// The row's line under a browser's name: "2 profiles · 368 bookmarks · 2,000 places".
fn detail(sheet: &Sheet, source: Source) -> String {
    if source == Source::Safari {
        return "Choose a Safari export file".to_owned();
    }
    let Some(preview) = preview_of(sheet, source) else {
        return "Counting…".to_owned();
    };
    let mut parts = Vec::new();
    if let Some((list, _)) = sheet.profiles.get(source.name())
        && list.len() > 1
    {
        parts.push(format!("{} profiles", list.len()));
    }
    for (count, one, many) in [
        (preview.bookmarks, "bookmark", "bookmarks"),
        (preview.places, "place", "places"),
        (preview.passwords, "password", "passwords"),
        (preview.tabs, "tab", "tabs"),
    ] {
        if count > 0 {
            parts.push(counted(count, one, many));
        }
    }
    if parts.is_empty() {
        "Nothing to bring in".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn bring() {
    if SHEET.with_borrow(|slot| slot.as_ref().and_then(|sheet| sheet.pick) == Some(Source::Safari))
    {
        choose_safari();
        return;
    }
    let Some((source, chosen, wants, open, preview)) = SHEET.with_borrow_mut(|slot| {
        let sheet = slot.as_mut()?;
        let source = sheet.pick?;
        if sheet.bringing {
            return None;
        }
        sheet.bringing = true;
        let chosen = chosen_of(sheet, source);
        let preview = preview_of(sheet, source).unwrap_or_default();
        Some((source, chosen, sheet.wants, open_here(), preview))
    }) else {
        return;
    };
    render();
    // A kind it has none of isn't read at all: no keychain question for a browser with
    // no passwords.
    let passwords = wants.passwords && preview.passwords > 0;
    std::thread::spawn(move || {
        let incoming = read(source, &open, chosen.as_deref());
        let found = passwords.then(|| read_passwords(source, chosen.as_deref()));
        dispatch2::DispatchQueue::main().exec_async(move || {
            let said = land(source, incoming, found, wants);
            SHEET.with_borrow_mut(|slot| {
                if let Some(sheet) = slot.as_mut() {
                    sheet.bringing = false;
                    sheet.brought = Some(said);
                }
            });
            render();
        });
    });
}

fn choose_safari() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
        "zip",
    )])));
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseDirectories(false);
    panel.setPrompt(Some(&NSString::from_str("Bring In")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose the file exported from Safari.",
    )));
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    let Some(path) = panel.URL().and_then(|u| u.path()) else {
        return;
    };
    let path = PathBuf::from(path.to_string());
    let open = open_here();
    let Some(wants) = SHEET.with_borrow_mut(|slot| {
        let sheet = slot.as_mut()?;
        if sheet.bringing {
            return None;
        }
        sheet.bringing = true;
        Some(sheet.wants)
    }) else {
        return;
    };
    render();
    std::thread::spawn(move || {
        const ZIP_LIMIT: u64 = 256 << 20;
        let result = (|| {
            let file =
                std::fs::File::open(path).map_err(|_| "That file couldn't be read".to_owned())?;
            if file
                .metadata()
                .map_err(|_| "That file couldn't be read".to_owned())?
                .len()
                > ZIP_LIMIT
            {
                return Err("Safari export exceeds compressed size limit".to_owned());
            }
            let mut bytes = Vec::new();
            file.take(ZIP_LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "That file couldn't be read".to_owned())?;
            if bytes.len() as u64 > ZIP_LIMIT {
                return Err("Safari export exceeds compressed size limit".to_owned());
            }
            safari::read(&bytes)
        })();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let mut said = match result {
                Ok(mut export) => {
                    export.incoming.leave_out(&open);
                    let found = (wants.passwords
                        && (!export.passwords.is_empty() || export.password_skipped > 0))
                        .then_some(Passwords::Export(export.passwords, export.password_skipped));
                    let mut said = land(Source::Safari, Ok(export.incoming), found, wants);
                    if said.is_empty() {
                        said.push((false, "That file has nothing Tap can bring in".to_owned()));
                    }
                    said
                }
                Err(why) => vec![(false, why)],
            };
            said.push((
                false,
                "The export may hold passwords in the clear: delete it when no longer needed"
                    .to_owned(),
            ));
            SHEET.with_borrow_mut(|slot| {
                if let Some(sheet) = slot.as_mut() {
                    sheet.bringing = false;
                    sheet.brought = Some(said);
                }
            });
            render();
        });
    });
}

/// File…: a passwords CSV another browser exported, or a Tap profile.
fn choose_file() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    #[allow(deprecated)] // Tap's profile extension is custom
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[
        NSString::from_str("csv"),
        NSString::from_str(tap_import::profile::EXTENSION),
        NSString::from_str("zip"),
    ])));
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseDirectories(false);
    panel.setPrompt(Some(&NSString::from_str("Bring In")));
    panel.setMessage(Some(&NSString::from_str(
        "A passwords CSV or a Tap profile.",
    )));
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    let Some(path) = panel.URL().and_then(|u| u.path()) else {
        return;
    };
    let path = PathBuf::from(path.to_string());
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    {
        let said = match take_csv(&path) {
            Ok(said) => (true, said),
            Err(why) => (false, why),
        };
        SHEET.with_borrow_mut(|slot| {
            if let Some(sheet) = slot.as_mut() {
                sheet.brought = Some(vec![said]);
            }
        });
        render();
    } else {
        close();
        profile::take_and_tell(&path);
    }
}

// MARK: - drawing the sheet

fn label(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    bold: bool,
    color: &NSColor,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    let font = if bold {
        // SAFETY: a system font at a size and the system's own weight constant.
        NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    label.setTextColor(Some(color));
    label
}

/// Puts `view` at `x, y`, `width` wide and as tall as its text there. Answers its height.
fn put(holder: &NSView, view: &NSTextField, x: f64, y: f64, width: f64) -> f64 {
    view.setPreferredMaxLayoutWidth(width);
    let height = view.cell().map_or(18.0, |cell| {
        cell.cellSizeForBounds(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, 10_000.0),
        ))
        .height
        .ceil()
    });
    view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
    holder.addSubview(view);
    height
}

#[derive(Clone, Copy)]
#[repr(isize)]
enum Kind {
    Passwords = 1,
    Bookmarks,
    History,
    Tabs,
    Settings,
}

impl Kind {
    fn from(tag: isize) -> Option<Kind> {
        [
            Kind::Passwords,
            Kind::Bookmarks,
            Kind::History,
            Kind::Tabs,
            Kind::Settings,
        ]
        .into_iter()
        .find(|k| *k as isize == tag)
    }
}

#[allow(clippy::too_many_lines)] // one sheet, laid out top to bottom
fn render() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    SHEET.with_borrow(|slot| {
        let Some(sheet) = slot.as_ref() else { return };
        let root = &sheet.root;
        for view in &root.subviews() {
            view.removeFromSuperview();
        }
        let ink = palette::INK.dynamic();
        let muted = palette::MUTED.dynamic();
        let mut y = 22.0;
        y += put(
            root,
            &label(mtm, "Bring things over", 17.0, true, &ink),
            INSET,
            y,
            CARD,
        ) + 6.0;
        y += put(
            root,
            &label(
                mtm,
                "Nothing in the other browser changes. Passwords go into your keychain.",
                12.5,
                false,
                &muted,
            ),
            INSET,
            y,
            CARD,
        ) + 16.0;
        if sheet.looking {
            y += put(
                root,
                &label(mtm, "Looking on this Mac…", 12.0, false, &muted),
                INSET,
                y,
                CARD,
            ) + 14.0;
        } else if sheet.sources.is_empty() {
            y += put(
                root,
                &label(
                    mtm,
                    "No other browser found on this Mac.",
                    13.0,
                    false,
                    &muted,
                ),
                INSET,
                y,
                CARD,
            ) + 14.0;
        } else {
            y += put(
                root,
                &label(mtm, "ON THIS MAC", 11.0, true, &muted),
                INSET + 2.0,
                y,
                CARD,
            ) + 6.0;
            let card = Card::new(mtm);
            let mut inner = 6.0;
            for (index, source) in sheet.sources.iter().enumerate() {
                // SAFETY: the target lives as long as the sheet, and the action takes the sender.
                let radio = unsafe {
                    NSButton::radioButtonWithTitle_target_action(
                        &NSString::from_str(source.name()),
                        Some(&sheet.target),
                        Some(sel!(picked:)),
                        mtm,
                    )
                };
                radio.setTag(index.try_into().unwrap_or(0));
                radio.setState(if sheet.pick == Some(*source) {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                radio.setEnabled(!sheet.bringing);
                radio.sizeToFit();
                radio.setFrameOrigin(NSPoint::new(14.0, inner + 6.0));
                card.addSubview(&radio);
                let line = label(mtm, &detail(sheet, *source), 11.5, false, &muted);
                inner += 6.0 + radio.frame().size.height + 1.0;
                inner += put(&card, &line, 34.0, inner, CARD - 48.0) + 8.0;
            }
            card.setFrame(NSRect::new(
                NSPoint::new(INSET, y),
                NSSize::new(CARD, inner + 2.0),
            ));
            root.addSubview(&card);
            y += inner + 2.0 + 16.0;
            if let Some(source) = sheet.pick {
                y = options(mtm, sheet, source, y) + 16.0;
            }
        }
        // The foot: what happened, then the buttons.
        if let Some(brought) = &sheet.brought {
            for (ok, text) in brought {
                let mark = if *ok { "✓" } else { "!" };
                let color = if *ok { &ink } else { &muted };
                y += put(
                    root,
                    &label(mtm, &format!("{mark}  {text}"), 12.5, false, color),
                    INSET,
                    y,
                    CARD,
                ) + 4.0;
            }
            y += 10.0;
        }
        let button = |title: &str, action: objc2::runtime::Sel| {
            // SAFETY: the target lives as long as the sheet, and the action takes the sender.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(&sheet.target),
                    Some(action),
                    mtm,
                )
            };
            button.sizeToFit();
            button
        };
        let file = button("File…", sel!(file:));
        file.setEnabled(!sheet.bringing);
        file.setFrameOrigin(NSPoint::new(INSET - 6.0, y));
        root.addSubview(&file);
        let mut right = WIDTH - INSET + 6.0;
        if sheet.brought.is_some() && !sheet.bringing {
            let done = button("Done", sel!(close:));
            done.setKeyEquivalent(&NSString::from_str("\r"));
            right -= done.frame().size.width;
            done.setFrameOrigin(NSPoint::new(right, y));
            root.addSubview(&done);
        } else {
            let wants = sheet.wants;
            let preview = sheet.pick.and_then(|s| preview_of(sheet, s));
            let something = (sheet.pick == Some(Source::Safari)
                && (wants.passwords || wants.bookmarks || wants.history))
                || preview.is_some_and(|p| {
                    (wants.passwords && p.passwords > 0)
                        || (wants.bookmarks && p.bookmarks > 0)
                        || (wants.history && p.places > 0)
                        || (wants.tabs && p.tabs > 0)
                });
            let go = button(
                if sheet.bringing {
                    "Bringing…"
                } else {
                    "Bring them in"
                },
                sel!(bring:),
            );
            go.setKeyEquivalent(&NSString::from_str("\r"));
            go.setEnabled(!sheet.bringing && something);
            right -= go.frame().size.width;
            go.setFrameOrigin(NSPoint::new(right, y));
            root.addSubview(&go);
            let cancel = button("Cancel", sel!(close:));
            cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
            cancel.setEnabled(!sheet.bringing);
            right -= cancel.frame().size.width + 4.0;
            cancel.setFrameOrigin(NSPoint::new(right, y));
            root.addSubview(&cancel);
        }
        y += file.frame().size.height + 18.0;
        sheet.window.setContentSize(NSSize::new(WIDTH, y));
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, y)));
    });
}

/// The card of what to bring from the browser picked. Answers where it ends.
#[allow(clippy::too_many_lines)] // one card, its lines top to bottom
fn options(mtm: MainThreadMarker, sheet: &Sheet, source: Source, top: f64) -> f64 {
    let ink = palette::INK.dynamic();
    let muted = palette::MUTED.dynamic();
    let name = source.name();
    let preview = preview_of(sheet, source);
    let card = Card::new(mtm);
    let mut y = 4.0;
    let rule = |card: &Card, y: f64| {
        let line = Rule::new(mtm);
        line.setFrame(NSRect::new(
            NSPoint::new(14.0, y),
            NSSize::new(CARD - 28.0, 1.0),
        ));
        card.addSubview(&line);
    };
    let mut first = true;
    // The profile, when there is more than one to choose from.
    if let Some((list, usual)) = sheet.profiles.get(name)
        && list.len() > 1
    {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        let current = sheet
            .chosen
            .get(name)
            .cloned()
            .or_else(|| usual.clone())
            .unwrap_or_default();
        let mut selected = 0;
        for (index, profile) in list.iter().enumerate() {
            let title = if Some(&profile.folder) == usual.as_ref() {
                format!("Most recent: {}", profile.name)
            } else {
                profile.name.clone()
            };
            popup.addItemWithTitle(&NSString::from_str(&title));
            if profile.folder == current {
                selected = index;
            }
        }
        if let Some(menu) = popup.menu() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        popup.addItemWithTitle(&NSString::from_str("All profiles"));
        let all = popup.numberOfItems() - 1;
        popup.selectItemAtIndex(if current.is_empty() && sheet.chosen.contains_key(name) {
            all
        } else {
            selected.try_into().unwrap_or(0)
        });
        // SAFETY: the target lives as long as the sheet, and the action takes the sender.
        unsafe {
            popup.setTarget(Some(&sheet.target));
            popup.setAction(Some(sel!(profile:)));
        }
        popup.setEnabled(!sheet.bringing);
        popup.sizeToFit();
        let size = popup.frame().size;
        let title = label(mtm, "Profile", 13.0, false, &ink);
        put(&card, &title, 14.0, y + 12.0, 200.0);
        popup.setFrameOrigin(NSPoint::new(CARD - 14.0 - size.width, y + 8.0));
        card.addSubview(&popup);
        y += size.height + 16.0;
        first = false;
    }
    let none = |count: Option<usize>| count == Some(0);
    let password_detail = match source {
        Source::Chromium(b) if b.safe_storage().is_none() => format!("Not supported for {name}"),
        _ if none(preview.map(|p| p.passwords)) => format!("None in {name}"),
        Source::Safari => "From a Safari export file".to_owned(),
        Source::Chromium(_) => format!("macOS asks once for {name}'s keychain key"),
    };
    let unsupported = matches!(source, Source::Chromium(b) if b.safe_storage().is_none());
    let mut lines = vec![
        (
            Kind::Passwords,
            "Passwords",
            password_detail,
            sheet.wants.passwords,
            unsupported || none(preview.map(|p| p.passwords)),
        ),
        (
            Kind::Bookmarks,
            "Bookmarks",
            if none(preview.map(|p| p.bookmarks)) {
                format!("None in {name}")
            } else {
                format!("In a “{name}” folder")
            },
            sheet.wants.bookmarks,
            none(preview.map(|p| p.bookmarks)),
        ),
        (
            Kind::History,
            "History",
            match preview.map(|p| p.places) {
                Some(0) => format!("None in {name}"),
                Some(places) => format!("The last {} places", n(places)),
                None => "Counting…".to_owned(),
            },
            sheet.wants.history,
            none(preview.map(|p| p.places)),
        ),
        (
            Kind::Tabs,
            "Tabs & tab groups",
            match preview {
                Some(p) if p.tabs > 0 => {
                    let groups = if p.groups > 0 {
                        format!(" in {}", counted(p.groups, "group", "groups"))
                    } else {
                        String::new()
                    };
                    format!(
                        "{}{groups} — asleep at the end of this window",
                        counted(p.tabs, "tab", "tabs")
                    )
                }
                Some(_) => format!("None open in {name}"),
                None => "Counting…".to_owned(),
            },
            sheet.wants.tabs,
            none(preview.map(|p| p.tabs)),
        ),
    ];
    if source == Source::Safari {
        lines.pop();
        "From a Safari export file".clone_into(&mut lines[1].2);
        "From a Safari export file".clone_into(&mut lines[2].2);
    }
    for (kind, title, detail, on, disabled) in lines {
        if !first {
            rule(&card, y);
        }
        first = false;
        let switch = NSSwitch::initWithFrame(NSSwitch::alloc(mtm), NSRect::ZERO);
        switch.setState(if on && !disabled {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        switch.setEnabled(!disabled && !sheet.bringing);
        switch.setTag(kind as isize);
        // SAFETY: the target lives as long as the sheet, and the action takes the sender.
        unsafe {
            switch.setTarget(Some(&sheet.target));
            switch.setAction(Some(sel!(toggled:)));
        }
        switch.sizeToFit();
        let size = switch.frame().size;
        let words = CARD - 28.0 - size.width - 16.0;
        let mut h = 10.0;
        h += put(
            &card,
            &label(mtm, title, 13.0, false, &ink),
            14.0,
            y + h,
            words,
        ) + 2.0;
        h += put(
            &card,
            &label(mtm, &detail, 11.5, false, &muted),
            14.0,
            y + h,
            words,
        ) + 10.0;
        switch.setFrameOrigin(NSPoint::new(
            CARD - 14.0 - size.width,
            y + (h - size.height) / 2.0,
        ));
        card.addSubview(&switch);
        y += h;
    }
    card.setFrame(NSRect::new(
        NSPoint::new(INSET, top),
        NSSize::new(CARD, y + 4.0),
    ));
    sheet.root.addSubview(&card);
    top + y + 4.0
}

define_class!(
    /// The sheet's controls' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBringTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(picked:))]
        fn picked(&self, sender: Option<&NSButton>) {
            let Some(at) = sender.map(|b| b.tag()) else { return };
            SHEET.with_borrow_mut(|slot| {
                if let Some(sheet) = slot.as_mut()
                    && !sheet.bringing
                {
                    sheet.pick = usize::try_from(at).ok().and_then(|i| sheet.sources.get(i).copied());
                    sheet.brought = None;
                }
            });
            render();
        }

        #[unsafe(method(profile:))]
        fn profile(&self, sender: Option<&NSPopUpButton>) {
            let Some(popup) = sender else { return };
            let at = popup.indexOfSelectedItem();
            let source = SHEET.with_borrow_mut(|slot| {
                let sheet = slot.as_mut()?;
                let source = sheet.pick?;
                let (list, _) = sheet.profiles.get(source.name())?;
                let folder = usize::try_from(at)
                    .ok()
                    .and_then(|i| list.get(i))
                    .map_or_else(String::new, |p| p.folder.clone());
                sheet.chosen.insert(source.name(), folder);
                sheet.brought = None;
                Some(source)
            });
            render();
            if let Some(source) = source {
                count(source);
            }
        }

        #[unsafe(method(toggled:))]
        fn toggled(&self, sender: Option<&NSSwitch>) {
            let Some(switch) = sender else { return };
            let on = switch.state() == NSControlStateValueOn;
            SHEET.with_borrow_mut(|slot| {
                let Some(sheet) = slot.as_mut() else { return };
                match Kind::from(switch.tag()) {
                    Some(Kind::Passwords) => sheet.wants.passwords = on,
                    Some(Kind::Bookmarks) => sheet.wants.bookmarks = on,
                    Some(Kind::History) => sheet.wants.history = on,
                    Some(Kind::Tabs) => sheet.wants.tabs = on,
                    Some(Kind::Settings) => sheet.wants.settings = on,
                    None => {}
                }
            });
            render();
        }

        #[unsafe(method(bring:))]
        fn bring(&self, _sender: Option<&AnyObject>) {
            bring();
        }

        #[unsafe(method(file:))]
        fn file(&self, _sender: Option<&AnyObject>) {
            choose_file();
        }

        #[unsafe(method(close:))]
        fn close(&self, _sender: Option<&AnyObject>) {
            close();
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// The sheet's content: flipped, laid out from the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBringRoot"]
    struct Flipped;

    impl Flipped {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl Flipped {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    /// A white card with a hairline, as Search's.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBringCard"]
    struct Card;

    impl Card {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            let bounds = self.bounds();
            let inset = NSRect::new(
                NSPoint::new(0.5, 0.5),
                NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0),
            );
            let path = palette::card_path(inset);
            palette::GROUND.resolve(dark).setFill();
            path.fill();
            palette::stroke_hairline(&path, dark);
        }
    }
);

impl Card {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    /// A hairline between a card's lines.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBringRule"]
    struct Rule;

    impl Rule {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            palette::HAIRLINE.resolve(palette::is_dark(self)).setFill();
            NSBezierPath::fillRect(self.bounds());
        }
    }
);

impl Rule {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

/// For the bench: what the sheet shows, if it is up.
pub fn snapshot() -> Value {
    SHEET.with_borrow(|slot| {
        let Some(sheet) = slot.as_ref() else {
            return Value::Null;
        };
        serde_json::json!({
            "looking": sheet.looking,
            "sources": sheet.sources.iter().map(|s| serde_json::json!({
                "name": s.name(), "detail": detail(sheet, *s),
            })).collect::<Vec<_>>(),
            "pick": sheet.pick.map(Source::name),
            "bringing": sheet.bringing,
            "brought": sheet.brought.as_ref().map(|b| b.iter().map(|(ok, t)| serde_json::json!({"ok": ok, "text": t})).collect::<Vec<_>>()),
        })
    })
}

/// For the bench: the sheet's Bring them in, pressed, for `name`.
pub fn bench_bring(name: &str) {
    let picked = SHEET.with_borrow_mut(|slot| {
        let sheet = slot.as_mut()?;
        let source = sheet.sources.iter().copied().find(|s| s.name() == name)?;
        sheet.pick = Some(source);
        Some(())
    });
    if picked.is_some() {
        bring();
    }
}

/// For the bench: the sheet closed.
pub fn bench_close() {
    close();
}
