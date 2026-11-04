//! Tab groups in the row (Search's `TabGroups.swift` and the top bar's drawing in
//! `TabBar.swift`): where a group's tabs stand, and what the row shows.
//!
//! The row keeps one order, the one it is drawn in: pins first, then each group's tabs
//! together in the groups' order, then the tabs in no group. Search draws its top bar
//! that way whatever order its file holds them in; Tap keeps the tabs themselves in it, so
//! ⌃Tab, ⌘1–9 and a run of links all walk what you see.

use crate::session::{Entry, TabGroup};

/// The colours a group can wear, in the order new groups are handed them: Chrome's
/// nine, grey last rather than first.
pub const COLOURS: [&str; 9] = [
    "blue", "red", "yellow", "green", "pink", "purple", "cyan", "orange", "grey",
];

/// A colour's name as its menu says it (Chrome's English).
#[must_use]
pub fn colour_title(colour: &str) -> &'static str {
    match colour {
        "blue" => "Blue",
        "red" => "Red",
        "yellow" => "Yellow",
        "green" => "Green",
        "pink" => "Pink",
        "purple" => "Purple",
        "cyan" => "Cyan",
        "orange" => "Orange",
        _ => "Grey",
    }
}

/// A colour by Chrome's own number for it (`TabGroupColorId`), for its sessions.
#[must_use]
pub fn chrome_colour(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "grey",
        1 => "blue",
        2 => "red",
        3 => "yellow",
        4 => "green",
        5 => "pink",
        6 => "purple",
        7 => "cyan",
        8 => "orange",
        _ => return None,
    })
}

/// The colour a new group gets: one no group has yet, else the one fewest have,
/// earliest first (Chrome's `GetNextColor`).
#[must_use]
pub fn next_colour(groups: &[TabGroup]) -> &'static str {
    COLOURS
        .iter()
        .copied()
        .min_by_key(|c| {
            groups
                .iter()
                .filter(|g| g.colour.as_deref() == Some(*c))
                .count()
        })
        .unwrap_or(COLOURS[0])
}

/// Every group gets a colour it can keep: the ones that came without (Search's, an
/// older Tap's) are handed theirs in order, so no two near each other look alike.
pub fn colour_all(groups: &mut [TabGroup]) {
    for at in 0..groups.len() {
        let known = groups[at]
            .colour
            .as_deref()
            .is_some_and(|c| COLOURS.contains(&c));
        if !known {
            groups[at].colour = Some(next_colour(&groups[..at]).to_owned());
        }
    }
}

/// One thing in the row, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A group's heading, by its place among the groups.
    Heading(usize),
    /// A tab, by its place among the tabs.
    Tab(usize),
}

/// Where a tab stands: pins, then each group in turn, then the rest.
#[must_use]
pub fn rank(entry: &Entry, groups: &[TabGroup]) -> usize {
    if entry.pin.is_some() {
        return 0;
    }
    entry
        .group_id
        .as_deref()
        .and_then(|id| groups.iter().position(|g| g.id == id))
        .map_or(groups.len() + 1, |place| place + 1)
}

/// The row, given each tab's entry and whether it is the one in front: a folded group
/// shows its heading and, of its own tabs, only the one in front (Search's
/// `visibleTabs(in:)`). `entries` is already in [`rank`] order.
#[must_use]
pub fn row(entries: &[(&Entry, bool)], groups: &[TabGroup]) -> Vec<Item> {
    let mut items = Vec::with_capacity(entries.len() + groups.len());
    let mut headed: Option<usize> = None;
    for (index, (entry, live)) in entries.iter().enumerate() {
        let group = entry
            .group_id
            .as_deref()
            .filter(|_| entry.pin.is_none())
            .and_then(|id| groups.iter().position(|g| g.id == id));
        if let Some(place) = group {
            if headed != Some(place) {
                items.push(Item::Heading(place));
                headed = Some(place);
            }
            if groups[place].collapsed && !live {
                continue;
            }
        } else {
            headed = None;
        }
        items.push(Item::Tab(index));
    }
    items
}

/// The groups that still hold a tab, in their order: a group lasts as long as its tabs.
#[must_use]
pub fn kept(groups: &[TabGroup], entries: &[&Entry]) -> Vec<TabGroup> {
    groups
        .iter()
        .filter(|g| {
            entries
                .iter()
                .any(|e| e.group_id.as_deref() == Some(g.id.as_str()))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, collapsed: bool) -> TabGroup {
        TabGroup {
            id: id.to_owned(),
            name: id.to_lowercase(),
            collapsed,
            colour: None,
        }
    }

    fn entry(group: Option<&str>) -> Entry {
        Entry {
            url: "https://example.com".to_owned(),
            group_id: group.map(str::to_owned),
            ..Entry::default()
        }
    }

    #[test]
    fn pins_then_groups_in_their_order_then_the_rest() {
        let groups = [group("A", false), group("B", false)];
        let pin = Entry {
            pin: Some("P".to_owned()),
            ..entry(Some("B"))
        };
        assert_eq!(rank(&pin, &groups), 0);
        assert_eq!(rank(&entry(Some("A")), &groups), 1);
        assert_eq!(rank(&entry(Some("B")), &groups), 2);
        assert_eq!(rank(&entry(None), &groups), 3);
        assert_eq!(
            rank(&entry(Some("gone")), &groups),
            3,
            "no such group: the rest"
        );
    }

    #[test]
    fn a_folded_group_shows_its_heading_and_only_the_tab_in_front() {
        let groups = [group("A", true), group("B", false)];
        let (a1, a2, b1, loose) = (
            entry(Some("A")),
            entry(Some("A")),
            entry(Some("B")),
            entry(None),
        );
        let entries = [(&a1, false), (&a2, false), (&b1, false), (&loose, false)];
        assert_eq!(
            row(&entries, &groups),
            [
                Item::Heading(0),
                Item::Heading(1),
                Item::Tab(2),
                Item::Tab(3)
            ]
        );
        let entries = [(&a1, false), (&a2, true), (&b1, false), (&loose, false)];
        assert_eq!(
            row(&entries, &groups),
            [
                Item::Heading(0),
                Item::Tab(1),
                Item::Heading(1),
                Item::Tab(2),
                Item::Tab(3)
            ]
        );
    }

    #[test]
    fn groups_are_handed_colours_nobody_has_then_the_least_used() {
        let mut groups: Vec<TabGroup> = (0..11).map(|i| group(&i.to_string(), false)).collect();
        groups[1].colour = Some("green".to_owned());
        colour_all(&mut groups);
        let colours: Vec<&str> = groups.iter().filter_map(|g| g.colour.as_deref()).collect();
        assert_eq!(
            colours[..3],
            ["blue", "green", "red"],
            "a kept colour stays"
        );
        let mut sorted = colours[..9].to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "nine groups, nine colours");
        assert_eq!(colours[9], "blue", "then round again, fewest first");
        assert_eq!(chrome_colour(1), Some("blue"));
        assert_eq!(chrome_colour(9), None);
    }

    #[test]
    fn a_group_goes_with_its_last_tab() {
        let groups = [group("A", false), group("B", false)];
        let b = entry(Some("B"));
        assert_eq!(kept(&groups, &[&b]), [group("B", false)]);
    }
}
