//! Right-click on the strip (Search's `TabMenu`, `TabBar.swift:1213`, and the group
//! heading's menu, `TabGroups.swift`): a menu for the tab or heading under the pointer,
//! built fresh at each click from its state, the find bar's way. Only what Tap can do
//! today: no Pin, Mute, spaces, Split View or windows, whose subsystems aren't here
//! yet. The items act on the tab (or group) that was clicked, not the one in front, so
//! its identity waits here while the menu is up; there is only ever one menu at a time.

use std::cell::{Cell, RefCell};

use block2::RcBlock;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, runtime::Bool, sel};
use objc2_app_kit::{
    NSBezierPath, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSImage, NSMenu,
    NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSView,
};
use objc2_foundation::{NSInteger, NSRect, NSSize, NSString};
use tap_core::{
    browser::{Browser, TabId},
    groups::{self, COLOURS},
};

use crate::{
    dialogs,
    menu::{Command, Commands},
    palette, rt,
};

thread_local! {
    /// The tab the open menu is about, and the group, for a heading's menu.
    static TARGET: Cell<Option<TabId>> = const { Cell::new(None) };
    /// The group by its identity: its place may change while the menu is up.
    static GROUP: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// What the menu needs to know about the tab under the pointer, read from the
/// model in one borrow before the menu is built.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent menu facts, read together
pub struct TabState {
    /// A page you can reload, copy or duplicate: not a new tab, not Tap's own.
    pub has_page: bool,
    /// Another unpinned tab exists, for Close Other Tabs.
    pub others: bool,
    /// An unpinned tab stands to its right, for Close Tabs to the Right.
    pub right: bool,
    /// ⇧⌘T has something to bring back.
    pub can_reopen: bool,
    /// A pin stays out of groups.
    pub pinned: bool,
    /// Its group's place among the groups.
    pub group: Option<usize>,
    /// Every group's name, in order, for Move to Group.
    pub groups: Vec<String>,
    /// Nothing keeps it awake: Put to Sleep can let its page go.
    pub can_sleep: bool,
    /// How many tabs are picked, when it is one of them.
    pub picked: usize,
    /// Each picked tab's group's place, in the row's order, when it is one of
    /// several picked: what Move to Group acts on. Empty otherwise.
    pub picked_groups: Vec<Option<usize>>,
}

/// One line of the menu: a command, a submenu, or a separator when it has neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub tag: Option<NSInteger>,
    pub enabled: bool,
    pub sub: Vec<Entry>,
    /// Ticked: the choice in force.
    pub checked: bool,
    /// A group colour's dot beside the title.
    pub swatch: Option<&'static str>,
}

fn entry(title: &str, command: Command, enabled: bool) -> Entry {
    Entry {
        title: title.to_owned(),
        tag: Some(command as NSInteger),
        enabled,
        sub: Vec::new(),
        checked: false,
        swatch: None,
    }
}

fn separator() -> Entry {
    Entry {
        title: String::new(),
        tag: None,
        enabled: false,
        sub: Vec::new(),
        checked: false,
        swatch: None,
    }
}

/// Move to Group's submenu (Search's for one tab): New Group, each group (the
/// tab's own off), and Remove from Group for a tab in one. Right-clicked among
/// several picked, the same submenu moves them all: a group is off only when
/// every picked tab is already in it, and Remove from Group shows when any
/// picked tab is in one.
fn move_to_group(state: &TabState) -> Entry {
    let multi = state.picked > 1 && !state.picked_groups.is_empty();
    let mut sub = vec![entry("New Group", Command::TabNewGroup, true)];
    if !state.groups.is_empty() {
        sub.push(separator());
    }
    for (place, name) in state.groups.iter().enumerate() {
        let tag = NSInteger::try_from(place).unwrap_or(0);
        let here = usize::try_from(tag).ok();
        let enabled = if multi {
            !state.picked_groups.iter().all(|g| *g == here)
        } else {
            state.group != here
        };
        sub.push(Entry {
            title: name.clone(),
            tag: Some(Command::TabIntoGroup as NSInteger + tag),
            enabled,
            sub: Vec::new(),
            checked: false,
            swatch: None,
        });
    }
    let in_group = if multi {
        state.picked_groups.iter().any(Option::is_some)
    } else {
        state.group.is_some()
    };
    if in_group {
        sub.push(separator());
        sub.push(entry("Remove from Group", Command::TabOutOfGroup, true));
    }
    Entry {
        title: "Move to Group".to_owned(),
        tag: None,
        enabled: true,
        sub,
        checked: false,
        swatch: None,
    }
}

/// Close Tab, or Close N Tabs when it is one of several picked (Search's).
fn close_entry(state: &TabState) -> Entry {
    if state.picked > 1 {
        let mut close = entry("", Command::TabCloseSelected, true);
        close.title = format!("Close {} Tabs", state.picked);
        close
    } else {
        entry("Close Tab", Command::TabClose, true)
    }
}

/// The tab's menu, top to bottom, in Search's order where Tap has the operation at
/// all (Reload Tab is Tap's addition, first as in other browsers).
#[must_use]
pub fn tab_entries(state: &TabState, can_copy: bool) -> Vec<Entry> {
    let mut entries = Vec::new();
    if !state.pinned {
        entries.push(move_to_group(state));
    }
    entries.extend([
        entry("Reload Tab", Command::TabReload, state.has_page),
        entry("Rename…", Command::TabRename, true),
        entry("Duplicate Tab", Command::TabDuplicate, state.has_page),
        entry("Copy Address", Command::TabCopyAddress, state.has_page),
        entry(
            "Copy as Markdown Link",
            Command::TabCopyMarkdown,
            state.has_page,
        ),
        entry("Put to Sleep", Command::TabSleep, state.can_sleep),
        separator(),
        close_entry(state),
        entry("Close Other Tabs", Command::TabCloseOthers, state.others),
        entry(
            "Close Tabs to the Right",
            Command::TabCloseRight,
            state.right,
        ),
        entry("Reopen Closed Tab", Command::TabReopen, state.can_reopen),
    ]);
    // Right-clicked among picked tabs: all their addresses (Search's Copy Addresses).
    if state.picked > 1 {
        entries.push(separator());
        let mut copy = entry("", Command::TabCopySelected, can_copy);
        copy.title = format!("Copy {} Addresses", state.picked);
        entries.push(copy);
    }
    entries
}

/// A heading's Color submenu: Chrome's nine, each with its dot, the group's ticked
/// (drawn into its dot: AppKit hides an item's own tick behind its image).
fn colours(current: Option<&str>) -> Entry {
    let sub = COLOURS
        .iter()
        .enumerate()
        .map(|(place, colour)| Entry {
            title: groups::colour_title(colour).to_owned(),
            tag: Some(Command::GroupColour as NSInteger + NSInteger::try_from(place).unwrap_or(0)),
            enabled: true,
            sub: Vec::new(),
            checked: current == Some(*colour),
            swatch: Some(colour),
        })
        .collect();
    Entry {
        title: "Color".to_owned(),
        tag: None,
        enabled: true,
        sub,
        checked: false,
        swatch: None,
    }
}

/// A heading's menu, Search's (rename, fold, then ungroup or close the lot) with
/// Chrome's colour choice after the name.
#[must_use]
pub fn group_entries(collapsed: bool, colour: Option<&str>) -> Vec<Entry> {
    vec![
        entry("Rename Group…", Command::GroupRename, true),
        separator(),
        colours(colour),
        separator(),
        entry(
            if collapsed {
                "Expand Group"
            } else {
                "Collapse Group"
            },
            Command::TabGroupToggle,
            true,
        ),
        separator(),
        entry("Ungroup Tabs", Command::GroupUngroup, true),
        entry("Close Group", Command::GroupClose, true),
    ]
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
    state: &TabState,
) {
    TARGET.set(Some(id));
    GROUP.replace(None);
    let can_copy = rt::read(Browser::can_copy_selected).unwrap_or(false);
    pop_up(mtm, commands, event, view, &tab_entries(state, can_copy));
}

/// The heading's menu, up under the pointer.
pub fn show_group(
    mtm: MainThreadMarker,
    commands: &Commands,
    event: &NSEvent,
    view: &NSView,
    place: usize,
    collapsed: bool,
    colour: Option<&str>,
) {
    TARGET.set(None);
    GROUP.replace(rt::read(|browser| browser.groups().get(place).map(|g| g.id.clone())).flatten());
    pop_up(
        mtm,
        commands,
        event,
        view,
        &group_entries(collapsed, colour),
    );
}

/// A colour's dot for its menu item, drawn as the menu shows it (light or dark).
/// The chosen one carries its tick in the image: AppKit hides the item's own
/// tick behind an image, so only the chosen dot draws one.
fn swatch(colour: &'static str, ticked: bool) -> Retained<NSImage> {
    let draw = RcBlock::new(move |rect: NSRect| -> Bool {
        let dark = palette::drawing_dark();
        palette::group_colour(Some(colour)).resolve(dark).setFill();
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            objc2_foundation::NSPoint::new(rect.origin.x + 1.0, rect.origin.y + 1.0),
            NSSize::new(rect.size.width - 2.0, rect.size.height - 2.0),
        ))
        .fill();
        if ticked {
            palette::on_group(dark).setStroke();
            let tick = NSBezierPath::bezierPath();
            tick.moveToPoint(objc2_foundation::NSPoint::new(
                rect.origin.x + 3.4,
                rect.origin.y + 6.1,
            ));
            tick.lineToPoint(objc2_foundation::NSPoint::new(
                rect.origin.x + 5.4,
                rect.origin.y + 8.1,
            ));
            tick.lineToPoint(objc2_foundation::NSPoint::new(
                rect.origin.x + 8.7,
                rect.origin.y + 3.9,
            ));
            tick.setLineWidth(1.5);
            tick.stroke();
        }
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(12.0, 12.0), false, &draw)
}

fn build(
    mtm: MainThreadMarker,
    commands: &Commands,
    entries: &[Entry],
) -> objc2::rc::Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    // The entries' enabled flags are the model's verdict at this click; the
    // menu's own auto-enabling would undo them, every item's target answering.
    menu.setAutoenablesItems(false);
    for spec in entries {
        if spec.tag.is_none() && spec.sub.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        let action = spec.tag.map(|_| sel!(run:));
        // SAFETY: a fresh allocation; `run:` takes the sender.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&spec.title),
                action,
                &NSString::from_str(""),
            )
        };
        if let Some(tag) = spec.tag {
            item.setTag(tag);
            // SAFETY: the commands object lives for the app's life, longer than the menu.
            unsafe { item.setTarget(Some(commands)) };
        }
        if !spec.sub.is_empty() {
            item.setSubmenu(Some(&build(mtm, commands, &spec.sub)));
        }
        if let Some(colour) = spec.swatch {
            item.setImage(Some(&swatch(colour, spec.checked)));
        }
        item.setState(if spec.checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        item.setEnabled(spec.enabled);
        menu.addItem(&item);
    }
    menu
}

fn pop_up(
    mtm: MainThreadMarker,
    commands: &Commands,
    event: &NSEvent,
    view: &NSView,
    entries: &[Entry],
) {
    let menu = build(mtm, commands, entries);
    NSMenu::popUpContextMenu_withEvent_forView(&menu, event, view);
}

/// The group the open menu is about, by its place now.
fn group_place() -> Option<usize> {
    let id = GROUP.with_borrow(Clone::clone)?;
    rt::read(|browser| browser.groups().iter().position(|g| g.id == id)).flatten()
}

/// Runs one of the tab menu's commands. Answers whether the tag was one.
pub fn run(tag: NSInteger) -> bool {
    let target = TARGET.get();
    let colour = tag - Command::GroupColour as NSInteger;
    if let Some(colour) = usize::try_from(colour)
        .ok()
        .and_then(|place| COLOURS.get(place).copied())
    {
        if let Some(place) = group_place() {
            rt::dispatch(move |browser| browser.set_group_colour(place, colour));
        }
        return true;
    }
    if (Command::TabIntoGroup as NSInteger..Command::GroupColour as NSInteger).contains(&tag) {
        let place = usize::try_from(tag - Command::TabIntoGroup as NSInteger).ok();
        if let Some(id) = target {
            rt::dispatch(move |browser| {
                if browser.is_selected(id) && browser.selection_count() > 1 {
                    browser.move_selected_to_group(place);
                } else {
                    browser.move_to_group(id, place);
                }
            });
        }
        return true;
    }
    match tag {
        t if t == Command::TabReopen as NSInteger => {
            rt::dispatch(Browser::reopen);
        }
        t if t == Command::TabCopyAddress as NSInteger => copy(false),
        t if t == Command::TabCopySelected as NSInteger => copy_selected(),
        t if t == Command::TabCloseSelected as NSInteger => rt::dispatch(Browser::close_selected),
        t if t == Command::TabCopyMarkdown as NSInteger => copy(true),
        t if t == Command::TabNewGroup as NSInteger => {
            let Some(id) = target else { return true };
            let made = rt::update(|browser| {
                if browser.is_selected(id) && browser.selection_count() > 1 {
                    browser.add_group_selected()
                } else {
                    browser.add_group(id)
                }
            })
            .flatten();
            rt::settle();
            // Named as you make it, as Search opens its name for editing.
            if let Some(place) = made {
                rename_group(place);
            }
        }
        t if t == Command::TabOutOfGroup as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| {
                if browser.is_selected(id) && browser.selection_count() > 1 {
                    browser.move_selected_to_group(None);
                } else {
                    browser.move_to_group(id, None);
                }
            });
        }
        t if t == Command::TabRename as NSInteger => {
            let Some(id) = target else { return true };
            let Some(label) =
                rt::read(|browser| browser.tab(id).map(tap_core::browser::Tab::label)).flatten()
            else {
                return true;
            };
            dialogs::ask_name(
                "Rename Tab",
                "Leave it empty to show the page's own title again.",
                &label,
                move |name| rt::dispatch(move |browser| browser.rename_tab(id, &name)),
            );
        }
        t if t == Command::TabSleep as NSInteger => {
            let Some(id) = target else { return true };
            rt::dispatch(move |browser| browser.put_to_sleep(id));
        }
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
        _ => return run_group(tag),
    }
    true
}

/// The heading menu's commands, on the group it was opened for.
fn run_group(tag: NSInteger) -> bool {
    match tag {
        t if t == Command::TabGroupToggle as NSInteger => {
            if let Some(place) = group_place() {
                rt::dispatch(move |browser| browser.toggle_group(place));
            }
        }
        t if t == Command::GroupUngroup as NSInteger => {
            if let Some(place) = group_place() {
                rt::dispatch(move |browser| browser.ungroup(place));
            }
        }
        t if t == Command::GroupClose as NSInteger => {
            if let Some(place) = group_place() {
                rt::dispatch(move |browser| browser.close_group(place));
            }
        }
        t if t == Command::GroupRename as NSInteger => {
            if let Some(place) = group_place() {
                rename_group(place);
            }
        }
        _ => return false,
    }
    true
}

/// Rename Group: the sheet, then the name, if it says anything.
fn rename_group(place: usize) {
    let Some((id, name)) = rt::read(|browser| {
        browser
            .groups()
            .get(place)
            .map(|g| (g.id.clone(), g.name.clone()))
    })
    .flatten() else {
        return;
    };
    dialogs::ask_name(
        "Rename Group",
        "Leave it empty to keep the current name.",
        &name,
        move |name| {
            rt::dispatch(move |browser| {
                if let Some(place) = browser.groups().iter().position(|g| g.id == id) {
                    browser.rename_group(place, &name);
                }
            });
        },
    );
}

/// The picked tabs' addresses on the pasteboard, one to a line, in the row's order
/// (Search's `copySelectedTabLinks`). Nothing picked with a page, nothing copied.
pub fn copy_selected() {
    let links = rt::read(Browser::selected_links).unwrap_or_default();
    if links.is_empty() {
        return;
    }
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: the system's own pasteboard type constant, valid for the app's life.
    let kind = unsafe { NSPasteboardTypeString };
    board.setString_forType(&NSString::from_str(&links.join("\n")), kind);
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
            pinned: false,
            group: Some(1),
            groups: vec!["Work".to_owned(), "Reading".to_owned()],
            can_sleep: true,
            picked: 0,
            picked_groups: Vec::new(),
        }
    }

    fn titles(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.title.as_str()).collect()
    }

    #[test]
    fn the_tab_menu_is_in_search_order_with_its_separators() {
        let entries = tab_entries(&state(), true);
        assert_eq!(
            titles(&entries),
            [
                "Move to Group",
                "Reload Tab",
                "Rename…",
                "Duplicate Tab",
                "Copy Address",
                "Copy as Markdown Link",
                "Put to Sleep",
                "",
                "Close Tab",
                "Close Other Tabs",
                "Close Tabs to the Right",
                "Reopen Closed Tab",
            ]
        );
    }

    #[test]
    fn among_picked_tabs_the_menu_copies_all_their_addresses() {
        let picked = TabState {
            picked: 3,
            ..state()
        };
        let entries = tab_entries(&picked, true);
        assert_eq!(
            entries.last().map(|e| e.title.as_str()),
            Some("Copy 3 Addresses")
        );
        assert!(titles(&entries).contains(&"Close 3 Tabs"));
        assert!(!titles(&entries).contains(&"Close Tab"));
        assert!(!titles(&tab_entries(&state(), true)).contains(&"Copy 3 Addresses"));
        let off = tab_entries(&picked, false);
        let copy = off.last().expect("copy");
        assert_eq!(copy.title, "Copy 3 Addresses", "the title keeps its count");
        assert!(!copy.enabled, "no picked tab with a page");
    }

    #[test]
    fn move_to_group_offers_a_new_one_the_others_and_a_way_out() {
        let entries = tab_entries(&state(), true);
        let sub = &entries[0].sub;
        assert_eq!(
            titles(sub),
            ["New Group", "", "Work", "Reading", "", "Remove from Group"]
        );
        assert!(sub[2].enabled && !sub[3].enabled, "its own group is off");
        assert_eq!(sub[3].tag, Some(Command::TabIntoGroup as NSInteger + 1));
        let loose = TabState {
            group: None,
            groups: Vec::new(),
            ..state()
        };
        assert_eq!(titles(&tab_entries(&loose, true)[0].sub), ["New Group"]);
        let pin = TabState {
            pinned: true,
            ..state()
        };
        assert_eq!(
            tab_entries(&pin, true)[0].title,
            "Reload Tab",
            "a pin has no groups"
        );
    }

    #[test]
    fn among_picked_tabs_move_to_group_moves_them_all() {
        fn multi(group: Option<usize>, groups: Vec<Option<usize>>) -> TabState {
            TabState {
                group,
                picked: groups.len(),
                picked_groups: groups,
                ..state()
            }
        }
        let sub = &tab_entries(&multi(Some(1), vec![Some(1), Some(0), None]), true)[0].sub;
        assert_eq!(
            titles(sub),
            ["New Group", "", "Work", "Reading", "", "Remove from Group"],
            "any picked tab in a group keeps the way out"
        );
        assert!(
            sub[2].enabled && sub[3].enabled,
            "only all of them in one turns it off"
        );
        let sub = &tab_entries(&multi(Some(1), vec![Some(1), Some(1)]), true)[0].sub;
        assert!(!sub[3].enabled, "every picked tab in Reading: off");
        assert!(sub[2].enabled);
        assert_eq!(
            titles(&tab_entries(&multi(None, vec![None, None]), true)[0].sub),
            ["New Group", "", "Work", "Reading"],
            "none picked in a group: no way out"
        );
    }

    #[test]
    fn a_page_less_tab_and_a_lone_tab_disable_what_they_cannot_do() {
        let entries = tab_entries(
            &TabState {
                has_page: false,
                others: false,
                right: false,
                can_reopen: false,
                pinned: true,
                group: None,
                groups: Vec::new(),
                can_sleep: false,
                picked: 0,
                picked_groups: Vec::new(),
            },
            true,
        );
        let on: Vec<&str> = entries
            .iter()
            .filter(|e| e.enabled && e.tag.is_some())
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(on, ["Rename…", "Close Tab"]);
    }

    #[test]
    fn a_group_can_be_renamed_folded_ungrouped_or_closed() {
        assert_eq!(
            titles(&group_entries(false, Some("red"))),
            [
                "Rename Group…",
                "",
                "Color",
                "",
                "Collapse Group",
                "",
                "Ungroup Tabs",
                "Close Group"
            ]
        );
        assert_eq!(group_entries(true, None)[4].title, "Expand Group");
    }

    #[test]
    fn the_colour_menu_offers_chrome_s_nine_and_ticks_the_group_s() {
        let entries = group_entries(false, Some("red"));
        let colours = &entries[2].sub;
        assert_eq!(colours.len(), 9);
        assert_eq!(colours[0].title, "Blue");
        let ticked: Vec<&str> = colours
            .iter()
            .filter(|e| e.checked)
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(ticked, ["Red"]);
        assert_eq!(colours[1].tag, Some(Command::GroupColour as NSInteger + 1));
        assert!(colours.iter().all(|e| e.swatch.is_some()));
    }

    #[test]
    fn a_markdown_link_escapes_its_title() {
        assert_eq!(
            markdown_link("a [b] \\ c", "https://a.com"),
            "[a \\[b\\] \\\\ c](https://a.com)"
        );
    }
}
