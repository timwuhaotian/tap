//! The menus and their keys (Search's `App.swift` commands and `Shortcuts.swift`), and
//! the one object every menu item and door sends its command to.

use std::cell::Cell;

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationDelegate,
    NSApplicationTerminateReply, NSEventModifierFlags, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSArray, NSInteger, NSNotification, NSString, NSURL};
use tap_core::{
    browser::{Browser, Tab},
    identity,
    internal::Page,
    settings_index,
};

use crate::{bench, findbar, inspector, links, prefs, profile, rt, store, update, update_window};

/// What a menu item or a door asks for, carried as its tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(isize)]
pub enum Command {
    NewTab = 1,
    ReopenTab,
    OpenLocation,
    CloseTab,
    Reload,
    Stop,
    ReloadOrStop,
    Back,
    Forward,
    NextTab,
    PreviousTab,
    Find,
    FindNext,
    FindPrevious,
    CloseFind,
    FindOptions,
    MatchCase,
    WholeWords,
    /// Settings… (⌘,): Tap's own page.
    Settings,
    ImportProfile,
    ExportProfile,
    /// Check for Updates…, or Restart to Update once one is ready.
    CheckForUpdates,
    /// The update window's buttons.
    RestartToUpdate,
    CloseUpdate,
    /// The Web Inspector on the tab in front (inspector): ⌥⌘I, ⌥⌘J, ⌥⌘C.
    Inspector,
    InspectorConsole,
    InspectElement,
    /// ⌘1 to ⌘9 are 101 to 109.
    Tab = 100,
    ReloadFromOrigin = 110,
    /// The tab menu's (tabmenu): they act on the tab under the pointer, whose
    /// identity tabmenu keeps, not on the one in front.
    TabNew = 200,
    TabDuplicate,
    TabReopen,
    TabGroupToggle,
    TabRename,
    TabSleep,
    /// Move to Group › New Group, and › Remove from Group.
    TabNewGroup,
    TabOutOfGroup,
    GroupRename,
    GroupUngroup,
    GroupClose,
    /// Copy N Addresses: the picked tabs', one to a line.
    TabCopySelected,
    /// Copy Tab Details: what lets an agent find the tab with `tap` (inspect).
    TabCopyDetails,
    /// Move to Group › a group: 300 and its place among the groups.
    TabIntoGroup = 300,
    /// A heading's Color › a colour: 400 and its place in `groups::COLOURS`.
    GroupColour = 400,
}

/// Whether a command can act right now: AppKit asks through `validateMenuItem:`.
/// Back and Forward need history in the front tab, Stop needs a page loading,
/// Reload needs a page that can be asked for again (a blank tab and Tap's own
/// pages have nothing to reload, mirroring `Browser::reload_tab`); everything
/// else can always run.
fn can_run(tag: NSInteger) -> bool {
    match tag {
        t if t == Command::Reload as NSInteger
            || t == Command::ReloadOrStop as NSInteger
            || t == Command::ReloadFromOrigin as NSInteger =>
        {
            rt::read(|browser| {
                browser
                    .active_tab()
                    .is_some_and(|tab| !tab.is_blank() && !tab.is_internal())
            })
            .unwrap_or(false)
        }
        t if t == Command::Stop as NSInteger => {
            rt::read(|browser| browser.active_tab().is_some_and(Tab::loading)).unwrap_or(false)
        }
        t if t == Command::Back as NSInteger => {
            rt::read(|browser| browser.active_tab().is_some_and(Tab::can_back)).unwrap_or(false)
        }
        t if t == Command::Forward as NSInteger => {
            rt::read(|browser| browser.active_tab().is_some_and(Tab::can_forward)).unwrap_or(false)
        }
        _ => true,
    }
}

/// Runs a command as its menu item or button would.
pub fn run(tag: NSInteger) {
    // The tab menu's commands are tabmenu's own business.
    if crate::tabmenu::run(tag) {
        return;
    }
    // The options menu is the bar's own business: nothing in the model changes.
    if tag == Command::FindOptions as NSInteger {
        findbar::show_options();
        return;
    }
    // Panels and windows of their own, not the model's business.
    if tag == Command::ImportProfile as NSInteger {
        profile::choose_and_import();
        return;
    }
    if tag == Command::ExportProfile as NSInteger {
        profile::export();
        return;
    }
    if tag == Command::CheckForUpdates as NSInteger {
        update::chosen();
        return;
    }
    if tag == Command::RestartToUpdate as NSInteger {
        update::restart();
        return;
    }
    if tag == Command::CloseUpdate as NSInteger {
        update_window::close();
        return;
    }
    // The Web Inspector is the shell's business too: it acts on the front
    // tab's web view, and the model doesn't change.
    if tag == Command::Inspector as NSInteger {
        inspector::toggle();
        return;
    }
    if tag == Command::InspectorConsole as NSInteger {
        inspector::show_console();
        return;
    }
    if tag == Command::InspectElement as NSInteger {
        inspector::inspect_element();
        return;
    }
    rt::dispatch(move |browser: &mut Browser| match tag {
        1 => browser.begin_editing(true),
        2 => browser.reopen(),
        3 => browser.begin_editing(false),
        // ⌘W: the picked tabs if any are, or else the one in front.
        4 => browser.close_front(),
        5 => browser.reload(),
        6 => browser.stop(),
        7 => {
            if browser
                .active_tab()
                .is_some_and(tap_core::browser::Tab::loading)
            {
                browser.stop();
            } else {
                browser.reload();
            }
        }
        8 => browser.back(),
        9 => browser.forward(),
        10 => browser.select_beside(true),
        11 => browser.select_beside(false),
        12 => browser.open_find(),
        13 => browser.find_next(true),
        14 => browser.find_next(false),
        15 => browser.close_find(),
        17 => {
            let on = !browser.find().match_case();
            browser.set_find_match_case(on);
        }
        18 => {
            let on = !browser.find().whole_words();
            browser.set_find_whole_words(on);
        }
        19 => browser.open_page(Page::Settings),
        101..=109 => browser.select_number(usize::try_from(tag - 100).unwrap_or(1)),
        110 => browser.reload_from_origin(),
        _ => {}
    });
}

define_class!(
    /// Every menu item's and door's target, and the app's delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapCommands"]
    pub struct Commands;

    unsafe impl NSObjectProtocol for Commands {}

    unsafe impl NSApplicationDelegate for Commands {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, app: &NSApplication) -> NSApplicationTerminateReply {
            if !LAST_WINDOW_CLOSED.with(|last| {
                confirm_termination(last, QUITTING_NOW.get(), confirms_quit())
            }) {
                return NSApplicationTerminateReply::TerminateNow;
            }
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(&NSString::from_str("Quit Tap?"));
            alert.setInformativeText(&NSString::from_str(
                "Your tabs come back as they are when you open Tap again.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Quit"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let _ = app;
            if alert.runModal() == NSAlertFirstButtonReturn {
                NSApplicationTerminateReply::TerminateNow
            } else {
                NSApplicationTerminateReply::TerminateCancel
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _note: &NSNotification) {
            // Quitting doesn't wait for the writer thread: the session goes now.
            if let Some(shape) = rt::read(Browser::shape) {
                store::save_session_now(&shape);
            }
            // Only its own socket file goes: never another run's.
            bench::stop();
            // A staged update replaces the app now, and Restart to Update opens it.
            update::install_on_quit();
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            links::opened(urls);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn last_window_closed(&self, _app: &NSApplication) -> bool {
            LAST_WINDOW_CLOSED.set(true);
            true
        }
    }

    impl Commands {
        #[unsafe(method(run:))]
        fn run_action(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: every sender here is an NSMenuItem or an NSControl, and both
            // answer `tag` with an NSInteger.
            let tag: NSInteger = unsafe { msg_send![sender, tag] };
            run(tag);
        }

        // AppKit asks each menu item whether it can act (Search's
        // `validateMenuItem:`): Back and Forward need history, Stop needs a page
        // loading, Reload needs a page that can be asked for again. The rest stay
        // enabled.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            can_run(item.tag())
        }
    }
);

thread_local! {
    /// Restart to Update, or the bench's quit: no question first.
    static QUITTING_NOW: Cell<bool> = const { Cell::new(false) };
    static LAST_WINDOW_CLOSED: Cell<bool> = const { Cell::new(false) };
}

fn confirm_termination(
    last_window_closed: &Cell<bool>,
    quitting_now: bool,
    confirms: bool,
) -> bool {
    let automatic = last_window_closed.replace(false);
    confirms && !quitting_now && !automatic
}

/// Settings › General › "Confirm before quitting", never on a test run.
fn confirms_quit() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_none()
        && settings_index::flag(&prefs::Kept::new(), "quit.confirm")
}

/// Quits without asking. A sheet left open on a window (a save panel, a question) would
/// keep AppKit from quitting at all, so each is put away first, as Cancel.
pub fn quit_now(mtm: MainThreadMarker) {
    QUITTING_NOW.set(true);
    let app = NSApplication::sharedApplication(mtm);
    for window in &app.windows() {
        if let Some(sheet) = window.attachedSheet() {
            window.endSheet(&sheet);
        }
    }
    if app.modalWindow().is_some() {
        app.abortModal();
    }
    app.terminate(None);
}

impl Commands {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// One menu item. A `command` goes to Tap's commands; otherwise `action` goes up the
/// responder chain (Copy, Paste, Minimize...).
struct Item {
    title: &'static str,
    key: &'static str,
    modifiers: NSEventModifierFlags,
    action: Sel,
    command: Option<isize>,
}

fn item(title: &'static str, key: &'static str, command: Command) -> Item {
    Item {
        title,
        key,
        modifiers: NSEventModifierFlags::Command,
        action: sel!(run:),
        command: Some(command as isize),
    }
}

fn system(title: &'static str, key: &'static str, action: Sel) -> Item {
    Item {
        title,
        key,
        modifiers: NSEventModifierFlags::Command,
        action,
        command: None,
    }
}

fn with(mut item: Item, modifiers: NSEventModifierFlags) -> Item {
    item.modifiers = modifiers;
    item
}

const SEPARATOR: &str = "-";

fn separator() -> Item {
    system(SEPARATOR, "", sel!(run:))
}

fn submenu(
    mtm: MainThreadMarker,
    title: &str,
    items: Vec<Item>,
    commands: &Commands,
) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for spec in items {
        if spec.title == SEPARATOR {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        // SAFETY: a fresh allocation; every action takes the sender.
        let entry = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(spec.title),
                Some(spec.action),
                &NSString::from_str(spec.key),
            )
        };
        entry.setKeyEquivalentModifierMask(spec.modifiers);
        if let Some(tag) = spec.command {
            entry.setTag(tag);
            // SAFETY: the commands object lives for the app's life, longer than the menu.
            unsafe { entry.setTarget(Some(commands)) };
        }
        menu.addItem(&entry);
    }
    let holder = NSMenuItem::new(mtm);
    holder.setSubmenu(Some(&menu));
    holder
}

/// The menu bar, with Search's keys, and the Tap menu's update item, which changes its
/// title once an update is ready.
#[allow(clippy::too_many_lines)] // one declarative table, read top to bottom as the menus are
pub fn main_menu(
    mtm: MainThreadMarker,
    commands: &Commands,
) -> (Retained<NSMenu>, Option<Retained<NSMenuItem>>) {
    let option = NSEventModifierFlags::Command | NSEventModifierFlags::Option;
    let control = NSEventModifierFlags::Control;
    let control_shift = NSEventModifierFlags::Control | NSEventModifierFlags::Shift;
    let bar = NSMenu::new(mtm);
    let mut tabs = vec![
        with(item("Show Next Tab", "\t", Command::NextTab), control),
        // ⌃⇧Tab arrives as the back-tab character, not as a tab with Shift.
        with(
            item("Show Previous Tab", "\u{19}", Command::PreviousTab),
            control_shift,
        ),
        separator(),
    ];
    for number in 1..=9 {
        let title: &'static str = [
            "Tab 1", "Tab 2", "Tab 3", "Tab 4", "Tab 5", "Tab 6", "Tab 7", "Tab 8", "Last Tab",
        ][number - 1];
        let key: &'static str = ["1", "2", "3", "4", "5", "6", "7", "8", "9"][number - 1];
        let mut entry = item(title, key, Command::Tab);
        #[allow(clippy::cast_possible_wrap)] // 1 to 9
        let tag = 100 + number as isize;
        entry.command = Some(tag);
        tabs.push(entry);
    }
    let menus = [
        submenu(
            mtm,
            "Tap",
            vec![
                system("About Tap", "", sel!(orderFrontStandardAboutPanel:)),
                with(
                    item("Check for Updates…", "", Command::CheckForUpdates),
                    NSEventModifierFlags::empty(),
                ),
                separator(),
                item("Settings…", ",", Command::Settings),
                separator(),
                system("Hide Tap", "h", sel!(hide:)),
                with(
                    system("Hide Others", "h", sel!(hideOtherApplications:)),
                    option,
                ),
                system("Show All", "", sel!(unhideAllApplications:)),
                separator(),
                system("Quit Tap", "q", sel!(terminate:)),
            ],
            commands,
        ),
        submenu(
            mtm,
            "File",
            vec![
                item("New Tab", "t", Command::NewTab),
                // An upper-case key equivalent is how AppKit spells Shift: a real
                // ⇧⌘T arrives as "T", which "t" with a Shift mask never matches.
                item("Reopen Closed Tab", "T", Command::ReopenTab),
                item("Open Location…", "l", Command::OpenLocation),
                separator(),
                // A profile as one offline file for your other Mac, and one from it;
                // passwords and cookies never go in it (Search #845).
                with(
                    item("Import Profile…", "", Command::ImportProfile),
                    NSEventModifierFlags::empty(),
                ),
                with(
                    item("Export Tap Profile…", "", Command::ExportProfile),
                    NSEventModifierFlags::empty(),
                ),
                separator(),
                item("Close Tab", "w", Command::CloseTab),
            ],
            commands,
        ),
        submenu(
            mtm,
            "Edit",
            vec![
                system("Undo", "z", sel!(undo:)),
                system("Redo", "Z", sel!(redo:)),
                separator(),
                system("Cut", "x", sel!(cut:)),
                system("Copy", "c", sel!(copy:)),
                system("Paste", "v", sel!(paste:)),
                system("Select All", "a", sel!(selectAll:)),
                separator(),
                item("Find…", "f", Command::Find),
                item("Find Next", "g", Command::FindNext),
                // Upper case is how AppKit spells Shift (see Reopen Closed Tab).
                item("Find Previous", "G", Command::FindPrevious),
            ],
            commands,
        ),
        submenu(
            mtm,
            "View",
            vec![
                item("Reload Page", "r", Command::Reload),
                item("Reload Page From Origin", "R", Command::ReloadFromOrigin),
                item("Stop", ".", Command::Stop),
                separator(),
                with(
                    system("Enter Full Screen", "f", sel!(toggleFullScreen:)),
                    NSEventModifierFlags::Command | NSEventModifierFlags::Control,
                ),
                separator(),
                // The Web Inspector, on the keys Chrome and Arc use (inspector).
                with(item("Web Inspector", "i", Command::Inspector), option),
                with(
                    item("JavaScript Console", "j", Command::InspectorConsole),
                    option,
                ),
                with(
                    item("Inspect Element", "c", Command::InspectElement),
                    option,
                ),
            ],
            commands,
        ),
        submenu(
            mtm,
            "History",
            vec![
                item("Back", "[", Command::Back),
                item("Forward", "]", Command::Forward),
            ],
            commands,
        ),
        submenu(
            mtm,
            "Window",
            {
                let mut window = vec![
                    system("Minimize", "m", sel!(performMiniaturize:)),
                    system("Zoom", "", sel!(performZoom:)),
                    separator(),
                ];
                window.extend(tabs);
                window
            },
            commands,
        ),
    ];
    for menu in &menus {
        bar.addItem(menu);
    }
    if let Some(window_menu) = menus[5].submenu() {
        NSApplication::sharedApplication(mtm).setWindowsMenu(Some(&window_menu));
    }
    let update = menus[0]
        .submenu()
        .and_then(|menu| menu.itemWithTag(Command::CheckForUpdates as isize));
    (bar, update)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use objc2_foundation::NSInteger;
    use tap_core::{browser::Browser, fnd::Url, internal::Page};

    use super::{Command, can_run, confirm_termination};
    use crate::rt;

    #[test]
    fn closing_the_last_window_does_not_confirm_and_does_not_mask_later_quits() {
        let last_window_closed = Cell::new(true);
        assert!(!confirm_termination(&last_window_closed, false, true));
        assert!(confirm_termination(&last_window_closed, false, true));
        assert!(confirm_termination(&last_window_closed, false, true));
    }

    #[test]
    fn quitting_without_confirmation_respects_the_setting_and_forced_quit() {
        let last_window_closed = Cell::new(false);
        assert!(!confirm_termination(&last_window_closed, false, false));
        assert!(!confirm_termination(&last_window_closed, true, true));
    }

    #[test]
    fn reload_needs_a_page_and_stop_needs_a_load() {
        rt::install(Browser::new());
        let reload = Command::Reload as NSInteger;
        let stop = Command::Stop as NSInteger;

        // A blank tab has nothing to reload and nothing to stop.
        assert!(!can_run(reload));
        assert!(!can_run(stop));

        let id = rt::update(|browser| {
            let url = Url::parse("https://example.com").expect("parses");
            browser.open(Some(&url), true, None)
        })
        .expect("installed");
        assert!(can_run(reload), "an idle page reloads (⌘R)");
        assert!(!can_run(stop));

        rt::update(|browser| browser.page_loading(id, true, 0.1));
        assert!(can_run(reload));
        assert!(can_run(stop));

        rt::update(|browser| browser.open_page(Page::Settings));
        assert!(!can_run(reload), "Tap's own pages never reload");
        assert!(!can_run(stop));
    }
}
