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
use tap_core::{browser::Browser, identity, internal::Page, settings_index};

use crate::{findbar, links, prefs, profile, rt, store, update, update_window};

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
    /// ⌘1 to ⌘9 are 101 to 109.
    Tab = 100,
    /// The tab menu's (tabmenu): they act on the tab under the pointer, whose
    /// identity tabmenu keeps, not on the one in front.
    TabReload = 200,
    TabDuplicate,
    TabCopyAddress,
    TabCopyMarkdown,
    TabCloseOthers,
    TabCloseRight,
    TabClose,
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
    /// Move to Group › a group: 300 and its place among the groups.
    TabIntoGroup = 300,
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
    rt::dispatch(move |browser: &mut Browser| match tag {
        1 => browser.begin_editing(true),
        2 => browser.reopen(),
        3 => browser.begin_editing(false),
        4 => {
            if let Some(id) = browser.active() {
                browser.close(id);
            }
        }
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
            if QUITTING_NOW.get() || !confirms_quit() {
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
            // A staged update replaces the app now, and Restart to Update opens it.
            update::install_on_quit();
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            links::opened(urls);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn last_window_closed(&self, _app: &NSApplication) -> bool {
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
    }
);

thread_local! {
    /// Restart to Update, or the bench's quit: no question first.
    static QUITTING_NOW: Cell<bool> = const { Cell::new(false) };
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
                item("Stop", ".", Command::Stop),
                separator(),
                with(
                    system("Enter Full Screen", "f", sel!(toggleFullScreen:)),
                    NSEventModifierFlags::Command | NSEventModifierFlags::Control,
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
