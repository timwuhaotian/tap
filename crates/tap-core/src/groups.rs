//! Tab groups in the row (Search's `TabGroups.swift` and the top bar's drawing in
//! `TabBar.swift`): where a group's tabs stand, and what the row shows.
//!
//! The row keeps one order, the one it is drawn in: pins first, then each group's tabs
//! together in the groups' order, then the tabs in no group. Search draws its top bar
//! that way whatever order its file holds them in; Tap keeps the tabs themselves in it, so
//! ⌃Tab, ⌘1–9 and a run of links all walk what you see.

use crate::{
    browser::TabId,
    session::{Entry, TabGroup},
};

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

/// Where tabs let go over a group land (Search's `GroupDropTarget`): in `group`, before
/// `member`, or after it when `after`; with no member, after the group's last tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupDrop {
    pub group: String,
    pub member: Option<TabId>,
    pub after: bool,
}

/// A group as the row lays it out, for a drop: its heading's left and right edge, and
/// each of its tabs' shown, in the row's order.
pub struct GroupSpans<'a> {
    pub group: &'a str,
    pub collapsed: bool,
    pub heading: (f64, f64),
    pub members: Vec<(TabId, (f64, f64))>,
}

/// Search's `GroupDropFrames.resolve`, along the row: the group whose stretch (its
/// heading through its last tab, the carried tabs left out) holds `x`. Over the
/// heading, over a folded group or one with nothing else shown: its end. Otherwise
/// before the first of its tabs whose middle is right of `x`, or after the last.
#[must_use]
pub fn drop_target(x: f64, groups: &[GroupSpans<'_>], carried: &[TabId]) -> Option<GroupDrop> {
    groups.iter().find_map(|spans| {
        let members: Vec<(TabId, (f64, f64))> = spans
            .members
            .iter()
            .filter(|(id, _)| !carried.contains(id))
            .copied()
            .collect();
        let end = members
            .last()
            .map_or(spans.heading.1, |(_, (_, right))| *right);
        if x < spans.heading.0 || x >= end {
            return None;
        }
        let whole = GroupDrop {
            group: spans.group.to_owned(),
            member: None,
            after: false,
        };
        if x < spans.heading.1 || spans.collapsed || members.is_empty() {
            return Some(whole);
        }
        let next = members
            .iter()
            .find(|(_, (left, right))| x < f64::midpoint(*left, *right));
        Some(match next {
            Some((id, _)) => GroupDrop {
                member: Some(*id),
                ..whole
            },
            None => GroupDrop {
                member: members.last().map(|(id, _)| *id),
                after: true,
                ..whole
            },
        })
    })
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

    fn spans<'a>(group: &'a str, heading: (f64, f64), members: &[(u64, f64)]) -> GroupSpans<'a> {
        GroupSpans {
            group,
            collapsed: false,
            heading,
            members: members
                .iter()
                .map(|(id, left)| (TabId(*id), (*left, left + 100.0)))
                .collect(),
        }
    }

    #[test]
    fn tabs_let_go_over_a_group_land_beside_the_tab_under_the_hand() {
        // A's heading 100–150 and tabs 152–252, 254–354; B's heading 356–406, one tab.
        let row = [
            spans("A", (100.0, 150.0), &[(1, 152.0), (2, 254.0)]),
            spans("B", (356.0, 406.0), &[(3, 408.0)]),
        ];
        let drop = |x, carried: &[TabId]| drop_target(x, &row, carried);
        let a = |member: Option<u64>, after| {
            Some(GroupDrop {
                group: "A".to_owned(),
                member: member.map(TabId),
                after,
            })
        };
        assert_eq!(
            drop(120.0, &[]),
            a(None, false),
            "the heading: the group's end"
        );
        assert_eq!(
            drop(180.0, &[]),
            a(Some(1), false),
            "left of a tab's middle"
        );
        assert_eq!(
            drop(220.0, &[]),
            a(Some(2), false),
            "past it: before the next"
        );
        assert_eq!(
            drop(330.0, &[]),
            a(Some(2), true),
            "past the last: after it"
        );
        assert_eq!(drop(99.0, &[]), None, "before the group");
        assert_eq!(drop(355.0, &[]), None, "between groups");
        assert_eq!(drop(600.0, &[]), None, "past every group");
        assert_eq!(
            drop(330.0, &[TabId(2)]),
            None,
            "a carried tab's own place is no group's"
        );
        assert_eq!(drop(420.0, &[]).map(|d| d.group), Some("B".to_owned()));
        let folded = [GroupSpans {
            collapsed: true,
            ..spans("A", (100.0, 150.0), &[(1, 152.0)])
        }];
        assert_eq!(
            drop_target(200.0, &folded, &[]),
            a(None, false),
            "a folded group takes it at its end"
        );
    }
}
