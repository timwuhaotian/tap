//! Right-click on the strip (Search's `TabMenu`, `TabBar.swift:1213`): a menu
//! for the tab under the pointer, built fresh at each click from that tab's
//! state, the find bar's way. First cut (docs/PLAN.md, M3): only what Tap can
//! do today — no Pin, Mute, Rename or group moves, whose subsystems aren't
//! here yet. The menu's items act on the tab that was clicked, not the one in
//! front, so its identity waits here while the menu is up; there is only ever
//! one menu at a time.

use std::cell::Cell;

use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{NSEvent, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSView};
use objc2_foundation::{NSInteger, NSString};
use tap_core::browser::{Browser, TabId};

use crate::{
    menu::{Command, Commands},
    rt,
};

thread_local! {
    /// The tab the open menu is about, and the group, for a heading's menu.
    static TARGET: Cell<Option<TabId>> = const { Cell::new(None) };
    static GROUP: Cell<Option<usize>> = const { Cell::new(None) };
}

/// What the menu needs to know about the tab under the pointer, read from the
/// model in one borrow before the menu is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // four independent menu facts, read together
pub struct TabState {
    /// A page you can reload, copy or duplicate: not a new tab, not Tap's own.
    pub has_page: bool,
    /// Another unpinned tab exists, for Close Other Tabs.
    pub others: bool,
    /// An unpinned tab stands to its right, for Close Tabs to the Right.
    pub right: bool,
    /// ⇧⌘T has something to bring back.
    pub can_reopen: bool,
}

/// One line of the menu: a command, or a separator when it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub command: Option<Command>,
    pub enabled: bool,
}

fn entry(title: &str, command: Command, enabled: bool) -> Entry {
    Entry {
        title: title.to_owned(),
        command: Some(command),
        enabled,
    }
}

fn separator() -> Entry {
    Entry {
        title: String::new(),
        command: None,
        enabled: false,
    }
}

/// The tab's menu, top to bottom, as Search's order has it where Tap has the
/// operation at all.
#[must_use]
pub fn tab_entries(state: TabState) -> Vec<Entry> {
    vec![
        entry("Reload Tab", Command::TabReload, state.has_page),
        entry("Duplicate Tab", Command::TabDuplicate, state.has_page),
        separator(),
        entry("Copy Address", Command::TabCopyAddress, state.has_page),
        entry(
            "Copy as Markdown Link",
            Command::TabCopyMarkdown,
            state.has_page,
        ),
        separator(),
        entry(
            "Close Tabs to the Right",
            Command::TabCloseRight,
            state.right,
        ),
        entry("Close Other Tabs", Command::TabCloseOthers, state.others),
        separator(),
        entry("Close Tab", Command::TabClose, true),
        entry("Reopen Closed Tab", Command::TabReopen, state.can_reopen),
    ]
}

/// A heading's menu: the click it already answers, said as a menu item.
#[must_use]
pub fn group_entries(collapsed: bool) -> Vec<Entry> {
    vec![entry(
        if collapsed {
            "Expand Group"
        } else {
            "Collapse Group"
        },
        Command::TabGroupToggle,
        true,
    )]
}

/// A page as a Markdown link, its title the text (Search's `markdownLink`): a
/// backslash first, so the ones added next aren't doubled.
#[must_use]
pub fn markdown_link(title: &str, url: &str) -> String {
    let text = title
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!("[{text}]({url})")
}

/// The tab's menu, up under the pointer.
pub fn show_tab(
    mtm: MainThreadMarker,
    commands: &Commands,
    event: &NSEvent,
    view: &NSView,
    id: TabId,
    state: TabState,
) {
    TARGET.set(Some(id));
    GROUP.set(None);
    pop_up(mtm, commands, event, view, &tab_entries(state));
}

/// The heading's menu, up under the pointer.
pub fn show_group(
    mtm: MainThreadMarker,
    commands: &Commands,
    event: &NSEvent,
    view: &NSView,
    place: usize,
    collapsed: bool,
) {
    TARGET.set(None);
    GROUP.set(Some(place));
    pop_up(mtm, commands, event, view, &group_entries(collapsed));
}

fn pop_up(
    mtm: MainThreadMarker,
    commands: &Commands,
    event: &NSEvent,
    view: &NSView,
    entries: &[Entry],
) {
    let menu = NSMenu::new(mtm);
    // The entries' enabled flags are the model's verdict at this click; the
    // menu's own auto-enabling would undo them, every item's target answering.
    menu.setAutoenablesItems(false);
    for spec in entries {
        let Some(command) = spec.command else {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        };
        // SAFETY: a fresh allocation; `run:` takes the sender.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&spec.title),
                Some(sel!(run:)),
                &NSString::from_str(""),
            )
        };
        item.setTag(command as isize);
        item.setEnabled(spec.enabled);
        // SAFETY: the commands object lives for the app's life, longer than the menu.
        unsafe { item.setTarget(Some(commands)) };
        menu.addItem(&item);
    }
    NSMenu::popUpContextMenu_withEvent_forView(&menu, event, view);
}

/// Runs one of the tab menu's commands. Answers whether the tag was one.
pub fn run(tag: NSInteger) -> bool {
    let target = TARGET.get();
    match tag {
        t if t == Command::TabReopen as NSInteger => {
            rt::dispatch(Browser::reopen);
        }
        t if t == Command::TabGroupToggle as NSInteger => {
            let Some(place) = GROUP.get() else {
                return true;
            };
            rt::dispatch(move |browser| browser.toggle_group(place));
        }
        t if t == Command::TabCopyAddress as NSInteger => copy(false),
        t if t == Command::TabCopyMarkdown as NSInteger => copy(true),
        t if t == Command::TabReload as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| browser.reload_tab(id));
        }
        t if t == Command::TabDuplicate as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| {
                browser.duplicate(id);
            });
        }
        t if t == Command::TabClose as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| browser.close(id));
        }
        t if t == Command::TabCloseOthers as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| browser.close_others(id));
        }
        t if t == Command::TabCloseRight as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| browser.close_right(id));
        }
        _ => return false,
    }
    true
}

fn copy(markdown: bool) {
    let Some(id) = TARGET.get() else { return };
    let Some(text) = rt::read(|browser| {
        let tab = browser.tab(id)?;
        if tab.is_blank() || tab.is_internal() {
            return None;
        }
        Some(if markdown {
            markdown_link(&tab.label(), tab.url())
        } else {
            tab.url().to_owned()
        })
    })
    .flatten() else {
        return;
    };
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: the system's own pasteboard type constant, valid for the app's life.
    let kind = unsafe { NSPasteboardTypeString };
    board.setString_forType(&NSString::from_str(&text), kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> TabState {
        TabState {
            has_page: true,
            others: true,
            right: true,
            can_reopen: true,
        }
    }

    #[test]
    fn the_tab_menu_is_in_search_order_with_its_separators() {
        let entries = tab_entries(state());
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Reload Tab",
                "Duplicate Tab",
                "",
                "Copy Address",
                "Copy as Markdown Link",
                "",
                "Close Tabs to the Right",
                "Close Other Tabs",
                "",
                "Close Tab",
                "Reopen Closed Tab",
            ]
        );
    }

    #[test]
    fn a_page_less_tab_and_a_lone_tab_disable_what_they_cannot_do() {
        let entries = tab_entries(TabState {
            has_page: false,
            others: false,
            right: false,
            can_reopen: false,
        });
        let on: Vec<&str> = entries
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(on, ["Close Tab"], "only closing is always possible");
    }

    #[test]
    fn the_heading_menu_says_the_other_way_round() {
        assert_eq!(group_entries(false)[0].title, "Collapse Group");
        assert_eq!(group_entries(true)[0].title, "Expand Group");
    }

    #[test]
    fn a_markdown_link_escapes_its_title() {
        assert_eq!(
            markdown_link("a [b] \\ c", "https://a.com"),
            "[a \\[b\\] \\\\ c](https://a.com)"
        );
    }
}
