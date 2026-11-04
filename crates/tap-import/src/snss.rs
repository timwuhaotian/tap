//! Open tabs, windows and tab groups read from a Chromium browser's session file (Search
//! #841, `ImportSession.swift`): the newest `Session_*` in a profile's `Sessions/` folder,
//! or the `Current Session` beside it in an older profile.
//!
//! A session file is SNSS frames: "SNSS", a version, then a size, a command and the
//! command's bytes, over and over. Only the commands that say what is open now are read;
//! anything else is passed over by its size, and a torn frame at the end (Chrome appends
//! without waiting for the disk) ends the read instead of failing it. Saved tab groups
//! aren't here (they live in the sync store, which needs `LevelDB`), and recently closed
//! tabs (`Tabs_*`) are left behind on purpose.

use std::collections::{HashMap, HashSet};

/// A group's identity in the file: random per browser run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    pub high: u64,
    pub low: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub group: Option<Token>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub collapsed: bool,
    /// Chrome's number for its colour (`TabGroupColorId`).
    pub colour: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Each open normal window's tabs, in the order they stand there.
    pub windows: Vec<Vec<Tab>>,
    /// The groups those tabs are in, by token. Only ones with tabs.
    pub groups: HashMap<Token, Group>,
}

impl Snapshot {
    #[must_use]
    pub fn tabs(&self) -> usize {
        self.windows.iter().map(Vec::len).sum()
    }
}

// The commands that say what is open (Chromium's components/sessions).
const WINDOW: u8 = 0;
const INDEX: u8 = 2;
const NAVIGATION: u8 = 6;
const SELECTED_NAVIGATION: u8 = 7;
const WINDOW_TYPE: u8 = 9;
const PINNED: u8 = 12;
const TAB_CLOSED: u8 = 16;
const WINDOW_CLOSED: u8 = 17;
const PRUNED: u8 = 24;
const GROUP: u8 = 25;
const GROUP_METADATA: u8 = 27;

/// Everything a run of frames says, by tab and window, before it is known which tabs
/// are still open and which entry of each is showing.
#[derive(Default)]
struct Seen {
    tab_window: HashMap<i32, i32>,
    window_order: Vec<i32>,
    window_type: HashMap<i32, i32>,
    tab_index: HashMap<i32, i32>,
    tab_order: HashMap<i32, usize>,
    noted: usize,
    entries: HashMap<i32, HashMap<i32, (String, String)>>,
    selected: HashMap<i32, i32>,
    pinned: HashSet<i32>,
    tab_group: HashMap<i32, Token>,
    meta: HashMap<Token, Group>,
}

impl Seen {
    fn note(&mut self, tab: i32) {
        if !self.tab_order.contains_key(&tab) {
            self.tab_order.insert(tab, self.noted);
            self.noted += 1;
        }
    }

    fn window(&mut self, window: i32) {
        if !self.window_order.contains(&window) {
            self.window_order.push(window);
        }
    }

    fn close(&mut self, tab: i32) {
        self.tab_window.remove(&tab);
        self.tab_index.remove(&tab);
        self.tab_order.remove(&tab);
        self.entries.remove(&tab);
        self.selected.remove(&tab);
        self.pinned.remove(&tab);
        self.tab_group.remove(&tab);
    }

    #[allow(clippy::too_many_lines)] // one arm per command, as Search's switch is
    fn frame(&mut self, id: u8, frame: &mut Cursor<'_>) {
        match id {
            WINDOW => {
                if let (Some(window), Some(tab)) = (frame.i32(), frame.i32()) {
                    self.tab_window.insert(tab, window);
                    self.window(window);
                    self.note(tab);
                }
            }
            INDEX => {
                if let (Some(tab), Some(index)) = (frame.i32(), frame.i32()) {
                    self.tab_index.insert(tab, index);
                    self.note(tab);
                }
            }
            NAVIGATION => {
                // A pickle: its size, the tab, the entry's index, its address, its
                // title, then the page's own state, which bringing the tab over doesn't
                // need.
                frame.skip(4);
                if let (Some(tab), Some(index), Some(url), Some(title)) =
                    (frame.i32(), frame.i32(), frame.utf8(), frame.utf16())
                {
                    self.entries
                        .entry(tab)
                        .or_default()
                        .insert(index, (url, title));
                    self.note(tab);
                }
            }
            SELECTED_NAVIGATION => {
                if let (Some(tab), Some(index)) = (frame.i32(), frame.i32()) {
                    self.selected.insert(tab, index);
                    self.note(tab);
                }
            }
            WINDOW_TYPE => {
                if let (Some(window), Some(kind)) = (frame.i32(), frame.i32()) {
                    self.window_type.insert(window, kind);
                    self.window(window);
                }
            }
            PINNED => {
                if let (Some(tab), Some(state)) = (frame.i32(), frame.u8()) {
                    if state == 0 {
                        self.pinned.remove(&tab);
                    } else {
                        self.pinned.insert(tab);
                    }
                    self.note(tab);
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = frame.i32() {
                    self.close(tab);
                }
            }
            WINDOW_CLOSED => {
                if let Some(window) = frame.i32() {
                    self.window_order.retain(|w| *w != window);
                    self.window_type.remove(&window);
                    let gone: Vec<i32> = self
                        .tab_window
                        .iter()
                        .filter(|(_, w)| **w == window)
                        .map(|(t, _)| *t)
                        .collect();
                    for tab in gone {
                        self.close(tab);
                    }
                }
            }
            PRUNED => {
                if let (Some(tab), Some(index), Some(count)) =
                    (frame.i32(), frame.i32(), frame.i32())
                    && count > 0
                    && let Some(entries) = self.entries.get_mut(&tab)
                {
                    // An index past the entry type's range ends the prune, never traps it.
                    for offset in 0..count {
                        let Some(gone) = index.checked_add(offset) else {
                            break;
                        };
                        entries.remove(&gone);
                    }
                }
            }
            GROUP => {
                // A raw struct: the tab, padding for the token's alignment, the token,
                // and whether it is still in a group. Files from before the flag carry
                // the tab and the token alone.
                if let Some(tab) = frame.i32() {
                    frame.skip(4);
                    if let (Some(high), Some(low)) = (frame.u64(), frame.u64()) {
                        if frame.u8() == Some(0) {
                            self.tab_group.remove(&tab);
                        } else {
                            self.tab_group.insert(tab, Token { high, low });
                        }
                    }
                    self.note(tab);
                }
            }
            GROUP_METADATA => {
                // A pickle: the token, the name, the colour, then (in newer files)
                // folded, saved and the saved group's id. The name, the colour and the
                // fold come over.
                frame.skip(4);
                if let (Some(high), Some(low), Some(title)) =
                    (frame.u64(), frame.u64(), frame.utf16())
                {
                    let colour = frame.i32().and_then(|c| u32::try_from(c).ok()).unwrap_or(0);
                    let collapsed = frame.i32().is_some_and(|c| c != 0);
                    self.meta.insert(
                        Token { high, low },
                        Group {
                            title,
                            collapsed,
                            colour,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn snapshot(self) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for window in &self.window_order {
            // Popups, apps and devtools are that window's own business, not tabs to keep.
            // A window never named a type is a normal one.
            if self.window_type.get(window).is_some_and(|kind| *kind != 0) {
                continue;
            }
            let mut tabs: Vec<i32> = self
                .tab_window
                .iter()
                .filter(|(_, w)| *w == window)
                .map(|(t, _)| *t)
                .collect();
            tabs.sort_by_key(|tab| {
                (
                    self.tab_index.get(tab).copied().unwrap_or(i32::MAX),
                    self.tab_order.get(tab).copied().unwrap_or(0),
                )
            });
            let tabs: Vec<Tab> = tabs.iter().filter_map(|tab| self.tab(*tab)).collect();
            if !tabs.is_empty() {
                snapshot.windows.push(tabs);
            }
        }
        // Only the groups that still have tabs. One never named is still a group (its
        // tabs came over together) under a plain name.
        let mut unnamed = 0;
        for token in snapshot.windows.iter().flatten().filter_map(|t| t.group) {
            if snapshot.groups.contains_key(&token) {
                continue;
            }
            let group = if let Some(found) = self.meta.get(&token) {
                Group {
                    title: titled(&found.title, None),
                    collapsed: found.collapsed,
                    colour: found.colour,
                }
            } else {
                unnamed += 1;
                Group {
                    title: format!("Group {unnamed}"),
                    collapsed: false,
                    colour: 0,
                }
            };
            snapshot.groups.insert(token, group);
        }
        snapshot
    }

    /// The tab as it shows now: its selected entry, or else its newest.
    fn tab(&self, tab: i32) -> Option<Tab> {
        let entries = self.entries.get(&tab).filter(|e| !e.is_empty())?;
        let at = self
            .selected
            .get(&tab)
            .filter(|at| entries.contains_key(at))
            .or_else(|| entries.keys().max())?;
        let (url, title) = entries.get(at)?;
        if !is_web(url) {
            return None;
        }
        let pinned = self.pinned.contains(&tab);
        Some(Tab {
            url: url.clone(),
            title: titled(title, Some(url)),
            pinned,
            // A pin is never in a group, as in Chrome.
            group: if pinned {
                None
            } else {
                self.tab_group.get(&tab).copied()
            },
        })
    }
}

/// Reads one session file. Anything that isn't one, an encrypted one included, has
/// nothing open to bring.
#[must_use]
pub fn read(data: &[u8]) -> Snapshot {
    let mut file = Cursor::new(data);
    // The magic, then a version this reads: 1, or 3 with its marker.
    if file.bytes(4) != Some(b"SNSS".as_slice()) || !matches!(file.i32(), Some(1 | 3)) {
        return Snapshot::default();
    }
    let mut seen = Seen::default();
    while file.left() > 0 {
        let Some(size) = file.u16().map(usize::from).filter(|s| *s >= 1) else {
            break;
        };
        if file.left() < size {
            break;
        }
        let Some(id) = file.u8() else { break };
        let Some(body) = file.bytes(size - 1) else {
            break;
        };
        seen.frame(id, &mut Cursor::new(body));
    }
    seen.snapshot()
}

/// An address to bring: a web page, never the browser's own (`chrome://newtab`).
pub(crate) fn is_web(url: &str) -> bool {
    tap_core::fnd::Url::parse(url)
        .and_then(|u| u.scheme())
        .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
}

/// A title fit to keep: trimmed, capped, and never empty.
fn titled(title: &str, url: Option<&str>) -> String {
    let trimmed = title.trim();
    if !trimmed.is_empty() {
        return trimmed.chars().take(300).collect();
    }
    url.map_or_else(
        || "Untitled".to_owned(),
        |url| {
            tap_core::fnd::Url::parse(url)
                .and_then(|u| u.host())
                .unwrap_or_else(|| url.to_owned())
        },
    )
}

/// Reading bytes a frame at a time. Everything is little-endian; every read past the end
/// fails instead of trapping.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn left(&self) -> usize {
        self.data.len() - self.at
    }

    fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(out)
    }

    fn skip(&mut self, count: usize) {
        self.at += count.min(self.left());
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.bytes(N).and_then(|b| b.try_into().ok())
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn i32(&mut self) -> Option<i32> {
        self.array().map(i32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }

    /// A pickle string: a byte count, the bytes, padding to four.
    fn utf8(&mut self) -> Option<String> {
        let count = usize::try_from(self.i32()?).ok()?;
        let bytes = self.bytes(count)?;
        self.skip((4 - count % 4) % 4);
        String::from_utf8(bytes.to_vec()).ok()
    }

    /// A pickle string16: a unit count, UTF-16LE, padding to four. Lone halves become
    /// the replacement character rather than losing the whole title.
    fn utf16(&mut self) -> Option<String> {
        let count = usize::try_from(self.i32()?).ok()?;
        let bytes = self.bytes(count.checked_mul(2)?)?;
        self.skip((4 - (count * 2) % 4) % 4);
        let units = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_le_bytes(*p));
        Some(
            char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! The vectors are written by hand, field by field the way Chromium writes them
    //! (Search's `SessionImportTests`), so a wrong offset fails here rather than in
    //! someone's browser.

    use super::*;

    pub fn raw(ints: &[i32]) -> Vec<u8> {
        ints.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn pad(out: &mut Vec<u8>, len: usize) {
        out.extend(std::iter::repeat_n(0, (4 - len % 4) % 4));
    }

    fn string(s: &str, out: &mut Vec<u8>) {
        out.extend(raw(&[i32::try_from(s.len()).unwrap_or(0)]));
        out.extend(s.as_bytes());
        pad(out, s.len());
    }

    fn string16(s: &str, out: &mut Vec<u8>) {
        let units: Vec<u16> = s.encode_utf16().collect();
        out.extend(raw(&[i32::try_from(units.len()).unwrap_or(0)]));
        out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        pad(out, units.len() * 2);
    }

    pub fn frame(id: u8, body: &[u8]) -> Vec<u8> {
        let mut out = u16::try_from(body.len() + 1)
            .unwrap_or(0)
            .to_le_bytes()
            .to_vec();
        out.push(id);
        out.extend(body);
        out
    }

    /// An `UpdateTabNavigation` for one entry: tab, entry index, address, title.
    pub fn navigation(tab: i32, index: i32, url: &str, title: &str) -> Vec<u8> {
        let mut body = raw(&[0, tab, index]);
        string(url, &mut body);
        string16(title, &mut body);
        frame(NAVIGATION, &body)
    }

    /// A `SetTabGroup`: tab, alignment padding, token, still grouped or not.
    pub fn grouping(tab: i32, high: u64, low: u64, grouped: bool) -> Vec<u8> {
        let mut body = raw(&[tab, 0]);
        body.extend(high.to_le_bytes());
        body.extend(low.to_le_bytes());
        body.push(u8::from(grouped));
        frame(GROUP, &body)
    }

    /// A `SetTabGroupMetadata2`: token, title, colour, folded, saved.
    pub fn metadata(high: u64, low: u64, title: &str, collapsed: bool) -> Vec<u8> {
        let mut body = raw(&[0]);
        body.extend(high.to_le_bytes());
        body.extend(low.to_le_bytes());
        string16(title, &mut body);
        body.extend(raw(&[1, i32::from(collapsed), 0]));
        frame(GROUP_METADATA, &body)
    }

    pub fn session(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"SNSS".to_vec();
        out.extend(raw(&[3]));
        for f in frames {
            out.extend(f);
        }
        out
    }

    fn with(mut body: Vec<u8>, more: &[u8]) -> Vec<u8> {
        body.extend(more);
        body
    }

    #[test]
    fn windows_tabs_and_groups() {
        let data = session(&[
            frame(WINDOW_TYPE, &raw(&[7, 0])),
            frame(WINDOW, &raw(&[7, 101])),
            frame(INDEX, &raw(&[101, 1])),
            navigation(101, 0, "https://b.example/", "B"),
            frame(SELECTED_NAVIGATION, &raw(&[101, 0])),
            frame(WINDOW, &raw(&[7, 100])),
            frame(INDEX, &raw(&[100, 0])),
            navigation(100, 0, "https://a.example/", "A"),
            grouping(100, 1, 2, true),
            grouping(101, 1, 2, true),
            metadata(1, 2, "Work", true),
            frame(WINDOW, &raw(&[7, 102])),
            frame(INDEX, &raw(&[102, 2])),
            navigation(102, 0, "https://c.example/", "C"),
            frame(PINNED, &with(raw(&[102]), &[1])),
            // A popup window's tabs are its own business.
            frame(WINDOW_TYPE, &raw(&[8, 1])),
            frame(WINDOW, &raw(&[8, 200])),
            navigation(200, 0, "https://popup.example/", "P"),
            // A closed tab leaves nothing behind.
            frame(WINDOW, &raw(&[7, 103])),
            navigation(103, 0, "https://gone.example/", "Gone"),
            frame(TAB_CLOSED, &raw(&[103])),
            // Not a page to keep.
            frame(WINDOW, &raw(&[7, 104])),
            frame(INDEX, &raw(&[104, 3])),
            navigation(104, 0, "chrome://newtab/", "New Tab"),
        ]);
        let read = read(&data);
        assert_eq!(read.windows.len(), 1);
        let tabs = &read.windows[0];
        let urls: Vec<&str> = tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://a.example/",
                "https://b.example/",
                "https://c.example/"
            ]
        );
        assert_eq!(tabs[0].title, "A");
        assert!(!tabs[0].pinned);
        assert!(tabs[2].pinned);
        let token = Token { high: 1, low: 2 };
        assert_eq!(read.groups.len(), 1);
        assert_eq!(
            read.groups[&token],
            Group {
                title: "Work".into(),
                collapsed: true,
                colour: 1
            }
        );
        assert_eq!(
            (tabs[0].group, tabs[1].group, tabs[2].group),
            (Some(token), Some(token), None)
        );
        assert_eq!(read.tabs(), 3);
    }

    #[test]
    fn the_selected_entry_ungrouping_and_short_metadata() {
        let mut short = raw(&[0]);
        short.extend(9u64.to_le_bytes());
        short.extend(9u64.to_le_bytes());
        string16("Short", &mut short);
        short.extend(raw(&[2]));
        let data = session(&[
            frame(WINDOW, &raw(&[7, 105])),
            frame(INDEX, &raw(&[105, 0])),
            navigation(105, 0, "https://old.example/", "Old"),
            navigation(105, 1, "https://new.example/", "New"),
            frame(SELECTED_NAVIGATION, &raw(&[105, 0])), // went back: the old entry shows
            frame(WINDOW, &raw(&[7, 106])),
            grouping(106, 5, 6, true),
            navigation(106, 0, "https://g.example/", "G"),
            grouping(106, 5, 6, false),
            // Metadata from before folding was stored: name and colour only.
            frame(GROUP_METADATA, &short),
            frame(WINDOW, &raw(&[7, 107])),
            grouping(107, 9, 9, true),
            navigation(107, 0, "https://h.example/", "H"),
        ]);
        let read = read(&data);
        let tabs: Vec<&Tab> = read.windows.iter().flatten().collect();
        let find = |url: &str| tabs.iter().find(|t| t.url == url);
        assert_eq!(
            find("https://old.example/").map(|t| t.title.as_str()),
            Some("Old")
        );
        assert!(find("https://new.example/").is_none());
        assert_eq!(find("https://g.example/").map(|t| t.group), Some(None));
        assert_eq!(
            read.groups.get(&Token { high: 9, low: 9 }),
            Some(&Group {
                title: "Short".into(),
                collapsed: false,
                colour: 2
            })
        );
        // Ungrouped away, the group has no tabs and is gone with it.
        assert!(!read.groups.contains_key(&Token { high: 5, low: 6 }));
    }

    #[test]
    fn a_torn_tail_unknown_commands_and_other_versions() {
        let mut torn = session(&[
            frame(WINDOW, &raw(&[7, 101])),
            navigation(101, 0, "https://a.example/", "A"),
            frame(99, &raw(&[1, 2, 3])), // a newer Chrome's command
        ]);
        torn.extend([0x20, 0x00, 6]); // a frame Chrome never finished
        assert_eq!(read(&torn).tabs(), 1);

        let mut magic = b"X NSS".to_vec();
        magic.extend(raw(&[3]));
        assert_eq!(read(&magic).windows.len(), 0);
        let mut encrypted = b"SNSS".to_vec();
        encrypted.extend(raw(&[5]));
        assert_eq!(read(&encrypted).windows.len(), 0);
        assert_eq!(read(&[]).windows.len(), 0);
    }

    #[test]
    fn an_untitled_tab_is_named_for_its_site() {
        let data = session(&[
            frame(WINDOW, &raw(&[7, 1])),
            navigation(1, 0, "https://docs.example.com/a", "  "),
        ]);
        assert_eq!(read(&data).windows[0][0].title, "docs.example.com");
    }
}
