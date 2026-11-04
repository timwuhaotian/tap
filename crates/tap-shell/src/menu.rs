//! The menus and their keys (Search's `App.swift` commands and `Shortcuts.swift`), and
//! the one object every menu item and door sends its command to.

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSEventModifierFlags, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSInteger, NSNotification, NSString};
use tap_core::browser::Browser;

use crate::{rt, store};

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
    /// ⌘1 to ⌘9 are 101 to 109.
    Tab = 100,
}

fn run(tag: NSInteger) {
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
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _note: &NSNotification) {
            // Quitting doesn't wait for the writer thread: the session goes now.
            if let Some(shape) = rt::read(Browser::shape) {
                store::save_session_now(&shape);
            }
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

/// The menu bar, with Search's keys.
#[allow(clippy::too_many_lines)] // one declarative table, read top to bottom as the menus are
pub fn main_menu(mtm: MainThreadMarker, commands: &Commands) -> Retained<NSMenu> {
    let shift = NSEventModifierFlags::Command | NSEventModifierFlags::Shift;
    let option = NSEventModifierFlags::Command | NSEventModifierFlags::Option;
    let control = NSEventModifierFlags::Control;
    let control_shift = NSEventModifierFlags::Control | NSEventModifierFlags::Shift;
    let bar = NSMenu::new(mtm);
    let mut tabs = vec![
        with(item("Show Next Tab", "\t", Command::NextTab), control),
        with(
            item("Show Previous Tab", "\t", Command::PreviousTab),
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
                with(item("Reopen Closed Tab", "t", Command::ReopenTab), shift),
                item("Open Location…", "l", Command::OpenLocation),
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
                with(system("Redo", "z", sel!(redo:)), shift),
                separator(),
                system("Cut", "x", sel!(cut:)),
                system("Copy", "c", sel!(copy:)),
                system("Paste", "v", sel!(paste:)),
                system("Select All", "a", sel!(selectAll:)),
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
    bar
}
