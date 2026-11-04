//! One window's tabs and what is being typed: the model half of Search's `Browser.swift`
//! and `Tab.swift` (docs/PLAN.md §5.1).
//!
//! Setters mark which fields changed; the shell reads the marks once per turn and
//! redraws only what they name. Anything that has to happen to a web view is pushed as
//! an [`Effect`] and carried out by the shell after the model is let go of.

use std::{collections::HashMap, time::Instant};

use bitflags::bitflags;

use crate::{
    address,
    crash::{CrashCount, PageEnd},
    engine::{self, Engine},
    find::{self, Find},
    fnd::{self, Url},
    groups::{self, Item},
    history::{self, History, Suggestion},
    internal::{self, Page},
    keyword::{self, Keyword},
    session::{self, TabGroup},
};

/// A tab's identity for as long as the app runs. Not saved: a restored tab gets a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

bitflags! {
    /// What changed on a tab since the views last looked.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct TabFields: u32 {
        const URL = 1;
        const TITLE = 1 << 1;
        const LOADING = 1 << 2;
        const PROGRESS = 1 << 3;
        const HISTORY = 1 << 4;
        const FAILURE = 1 << 5;
        const AWAKE = 1 << 6;
        /// The site's icon arrived.
        const ICON = 1 << 7;
    }

    /// What changed on the window's model since the views last looked.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct BrowserFields: u32 {
        /// Tabs added, removed or moved.
        const TABS = 1;
        const ACTIVE = 1 << 1;
        /// The field opened, closed, or moved between the tab and the floating panel.
        const EDITING = 1 << 2;
        /// What is typed in the field.
        const TYPED = 1 << 3;
        /// Return gave nothing to go to.
        const REFUSED = 1 << 4;
        /// The rows under the field, the grey ending, or the picked row.
        const OFFERS = 1 << 5;
        /// The model rewrote the field's text itself (Tab took the ending), so the
        /// field has to be told; what the keyboard typed the field already shows.
        const RETYPED = 1 << 6;
        /// The find bar opened, closed, or its count changed.
        const FIND = 1 << 7;
        /// ⌘F again: the find field takes the keys, its word selected.
        const FIND_FOCUS = 1 << 8;
    }
}

/// Something the shell does to a web view or the disk, after the model is let go of.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Load an address in the tab, making its web view first if it has none.
    Load {
        tab: TabId,
        url: String,
    },
    Reload(TabId),
    Back(TabId),
    Forward(TabId),
    Stop(TabId),
    /// The tab is gone: let its web view go.
    Discard(TabId),
    /// Give the page the keyboard.
    FocusPage(TabId),
    /// Write the session (debounced by the shell's disk writer).
    Save(session::Shape),
    /// Write `history.json`, already encoded.
    SaveHistory(Vec<u8>),
    /// Ask the tab's page about the word looked for.
    Find(find::Ask),
    /// Take what the find painted off the tab's page.
    ClearFind {
        tab: TabId,
        generation: u64,
    },
    /// The tab has been left alone long enough: if nothing the page is doing keeps it,
    /// it gives its page back (`slept`).
    Sleep(TabId),
    /// The tab has been left alone past Settings › Close tabs you leave alone: if nothing
    /// the page is doing keeps it, it closes as ⌘W would (`retire`).
    Retire {
        tab: TabId,
        since: f64,
    },
}

/// How tabs open and close: Settings › Tabs (Search's `Preferences`), set by the shell
/// from the kept settings whenever they change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // one switch per row of Settings, as Search's `Preferences`
pub struct Behaviour {
    /// `tabs.newPlace`: where a link from another app and ⌘T's tab go.
    pub new_place: NewPlace,
    /// `tabs.newAfterLast`: the last tab that isn't a pin, closed, leaves a new tab.
    pub new_after_last: bool,
    /// `tabs.closeGoesBack`: closing the tab in front goes to the one viewed before it.
    pub close_goes_back: bool,
    /// `tabs.lazy`: a link opened behind its page waits until its tab is shown.
    pub lazy: bool,
    /// `commandbar`: "settings" or "new tab", alone in the field, go there.
    pub commands: bool,
    /// `tabs.close.after`, in seconds: how long a tab may be left alone before it
    /// closes. `None`: never.
    pub close_after: Option<f64>,
}

/// Words alone in the field that mean a place in Tap rather than on the web (Search's
/// `AddressCommand`), with Settings › Address bar commands on. Only the whole of what
/// was typed: "settings for gmail" still searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Settings,
    NewTab,
}

impl Command {
    fn matching(typed: &str) -> Option<Command> {
        match typed.trim().to_lowercase().as_str() {
            "settings" | "preferences" => Some(Command::Settings),
            "new tab" => Some(Command::NewTab),
            _ => None,
        }
    }
}

/// Settings › Tabs › New tab position (Search's `NewTabPlace`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewPlace {
    /// After current: beside the tab in front, ⌘T's at the end.
    #[default]
    Next,
    /// Leftmost: first after the pins.
    Top,
    /// Rightmost.
    End,
}

impl NewPlace {
    #[must_use]
    pub fn from_raw(raw: &str) -> NewPlace {
        match raw {
            "top" => NewPlace::Top,
            "end" => NewPlace::End,
            _ => NewPlace::Next,
        }
    }
}

/// What was done about a page whose process ended, as the log says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    LoadedAgain,
    KeptDying,
    WhenShown,
    /// A new tab: nothing to make again.
    Nothing,
}

impl Recovery {
    #[must_use]
    pub fn said(self) -> &'static str {
        match self {
            Recovery::LoadedAgain => "loaded again",
            Recovery::KeptDying => "kept dying, left for Try again",
            Recovery::WhenShown => "loaded again when shown",
            Recovery::Nothing => "nothing to load",
        }
    }
}

/// One page.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // the page's own states, as Search's Tab keeps them
pub struct Tab {
    pub id: TabId,
    url: String,
    title: String,
    loading: bool,
    progress: f64,
    can_back: bool,
    can_forward: bool,
    /// Said in place of the page: a connection that failed, a process that kept dying.
    failure: Option<String>,
    /// Set while the failure is the one a crashing page gave up with: there is no
    /// process behind it, so Try again asks for the address afresh (Search #849).
    dead: bool,
    crashes: CrashCount,
    /// A restored tab costs nothing until it is first shown: it has no web view yet.
    awake: bool,
    /// Its process went away while it was behind another tab: made again when shown.
    stale: bool,
    /// A page has been committed in it: it is more than a tab opened for a link that
    /// turned out to be a download.
    committed: bool,
    /// The tab whose page opened this one.
    opener: Option<TabId>,
    /// When you last left the tab, in seconds since 2001 (`history::now`): how long it
    /// has gone without being looked at. Kept in the session (`touched`), so the night
    /// Tap was quit counts.
    touched: f64,
    entry: session::Entry,
    dirty: TabFields,
}

impl Tab {
    fn new(id: TabId) -> Tab {
        Tab {
            id,
            url: String::new(),
            title: String::new(),
            loading: false,
            progress: 0.0,
            can_back: false,
            can_forward: false,
            failure: None,
            dead: false,
            crashes: CrashCount::default(),
            awake: false,
            stale: false,
            committed: false,
            opener: None,
            touched: history::now(),
            entry: session::Entry::default(),
            dirty: TabFields::all(),
        }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
    #[must_use]
    pub fn loading(&self) -> bool {
        self.loading
    }
    #[must_use]
    pub fn progress(&self) -> f64 {
        self.progress
    }
    #[must_use]
    pub fn can_back(&self) -> bool {
        self.can_back
    }
    #[must_use]
    pub fn can_forward(&self) -> bool {
        self.can_forward
    }
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    #[must_use]
    pub fn awake(&self) -> bool {
        self.awake
    }
    /// Nothing loaded and nothing asked for: a new tab.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.url.is_empty()
    }
    /// One of Tap's own pages, drawn by Tap where a web page would be.
    #[must_use]
    pub fn page(&self) -> Option<Page> {
        internal::page(&self.url)
    }
    /// Under Tap's own scheme: never handed to WebKit, whether Tap has the page or not.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        internal::is_internal(&self.url)
    }
    /// The group the tab is in, by its identity.
    #[must_use]
    pub fn group(&self) -> Option<&str> {
        self.entry.group_id.as_deref()
    }

    /// Kept rather than opened: bulk closes leave it standing.
    #[must_use]
    pub fn is_pinned(&self) -> bool {
        self.entry.pin.is_some()
    }

    /// What the tab says: the name you gave it, the page's title, the address until
    /// there is a title, or "New Tab" (Search's `Tab.label`).
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(name) = self.entry.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_owned();
        }
        if let Some(page) = self.page() {
            return page.title().to_owned();
        }
        if !self.title.is_empty() {
            return self.title.clone();
        }
        match Url::parse(&self.url) {
            Some(url) if !self.url.is_empty() => address::pretty(&url),
            _ => "New Tab".to_owned(),
        }
    }

    fn set<T: PartialEq>(field: &mut T, value: T, dirty: &mut TabFields, flag: TabFields) {
        if *field != value {
            *field = value;
            dirty.insert(flag);
        }
    }

    fn entry(&self) -> session::Entry {
        session::Entry {
            url: self.url.clone(),
            title: self.title.clone(),
            touched: Some(self.touched),
            ..self.entry.clone()
        }
    }
}

/// The field, open on the live tab or floating over the page for a new one (⌘T).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editing {
    pub typed: String,
    /// ⌘T: what is typed gets a tab of its own on Return, and the page the field was
    /// raised over stays as it was. Nothing is opened if it is put away.
    pub launching: bool,
    /// What the field offers, best first: places from history, then a search when what
    /// is typed can't be a place (Search's `placeOffers`).
    pub offers: Vec<Suggestion>,
    /// The rest of the best match, drawn grey after the caret. Tab takes it.
    pub ending: Option<String>,
    /// The row the arrow keys have walked to.
    pub picked: Option<usize>,
}

/// A run of links opened from one tab land one after another beside it.
#[derive(Clone, Copy, Debug)]
struct LinkRun {
    opener: TabId,
    last: TabId,
}

/// One window's model.
#[derive(Debug)]
pub struct Browser {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    editing: Option<Editing>,
    pub engine: Engine,
    pub custom_engine: String,
    pub keywords: Vec<Keyword>,
    pub behaviour: Behaviour,
    /// The tabs left, the latest last: where closing the one in front goes back to.
    viewed: Vec<TabId>,
    /// Closed tabs, newest last, with where they were: ⇧⌘T brings them back.
    closed: Vec<(usize, session::Entry)>,
    link_run: Option<LinkRun>,
    refusals: u32,
    effects: Vec<Effect>,
    dirty: BrowserFields,
    session_changed: bool,
    /// Where you have been (Search's per-window `history`, the profile's).
    history: History,
    history_changed: bool,
    /// The find bar (⌘F), for whichever tab is in front.
    find: Find,
    /// The row's tab groups, in order; each tab's entry says which it is in.
    groups: Vec<TabGroup>,
    next: u64,
}

impl Default for Browser {
    fn default() -> Self {
        Browser::new()
    }
}

impl Browser {
    /// A window with one new tab.
    #[must_use]
    pub fn new() -> Browser {
        let mut browser = Browser {
            tabs: Vec::new(),
            active: None,
            editing: None,
            engine: Engine::default(),
            custom_engine: String::new(),
            keywords: Vec::new(),
            behaviour: Behaviour::default(),
            viewed: Vec::new(),
            closed: Vec::new(),
            link_run: None,
            refusals: 0,
            effects: Vec::new(),
            dirty: BrowserFields::all(),
            session_changed: false,
            history: History::default(),
            history_changed: false,
            find: Find::default(),
            groups: Vec::new(),
            next: 1,
        };
        let id = browser.make_tab();
        browser.active = Some(id);
        browser
    }

    /// The window as last time left it. Every tab comes back asleep, costing nothing
    /// until it is shown; the one in front wakes now.
    #[must_use]
    pub fn restored(shape: session::Shape) -> Browser {
        let mut browser = Browser::new();
        if shape.tabs.is_empty() {
            return browser;
        }
        browser.tabs.clear();
        browser.groups = unique(shape.groups.unwrap_or_default());
        for entry in shape.tabs {
            let tab = browser.tab_from(entry);
            browser.tabs.push(tab);
        }
        let front = usize::try_from(shape.active)
            .ok()
            .and_then(|i| browser.tabs.get(i))
            .map(|t| t.id);
        browser.order();
        if let Some(id) = front {
            browser.select(id);
        } else {
            // No tab was in front (a fresh start): a new one beside the rest.
            let id = browser.make_tab();
            browser.select(id);
        }
        browser.session_changed = false;
        browser
    }

    /// A tab as a session keeps it, asleep until it is shown.
    fn tab_from(&mut self, entry: session::Entry) -> Tab {
        let id = TabId(self.next);
        self.next += 1;
        let mut tab = Tab::new(id);
        tab.url.clone_from(&entry.url);
        tab.title.clone_from(&entry.title);
        // Sessions written before tabs kept it start counting at launch.
        tab.touched = entry.touched.unwrap_or(tab.touched);
        tab.entry = entry;
        tab
    }

    /// The row put back in its one order (see [`groups`]): a tab whose group is gone is
    /// in none, the tabs stand pins first, then group by group, then the rest, each
    /// keeping its place among its own, and a group left with no tab goes.
    fn order(&mut self) {
        for tab in &mut self.tabs {
            if tab
                .entry
                .group_id
                .as_deref()
                .is_some_and(|id| !self.groups.iter().any(|g| g.id == id))
            {
                tab.entry.group_id = None;
            }
        }
        let groups = &self.groups;
        self.tabs
            .sort_by_cached_key(|tab| groups::rank(&tab.entry, groups));
        self.prune();
    }

    /// A group lasts as long as its tabs.
    fn prune(&mut self) {
        let entries: Vec<&session::Entry> = self.tabs.iter().map(|t| &t.entry).collect();
        let kept = groups::kept(&self.groups, &entries);
        if kept.len() != self.groups.len() {
            self.groups = kept;
            self.dirty.insert(BrowserFields::TABS);
        }
    }

    fn make_tab(&mut self) -> TabId {
        let id = TabId(self.next);
        self.next += 1;
        self.tabs.push(Tab::new(id));
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        id
    }

    // MARK: reading

    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }
    #[must_use]
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }
    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }
    fn index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }
    #[must_use]
    pub fn active(&self) -> Option<TabId> {
        self.active
    }
    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tab(id))
    }
    #[must_use]
    pub fn editing(&self) -> Option<&Editing> {
        self.editing.as_ref()
    }
    #[must_use]
    pub fn refusals(&self) -> u32 {
        self.refusals
    }

    /// What changed since the last call, on the window and on each tab; the marks are
    /// cleared.
    pub fn take_dirty(&mut self) -> (BrowserFields, Vec<(TabId, TabFields)>) {
        let window = std::mem::take(&mut self.dirty);
        let tabs = self
            .tabs
            .iter_mut()
            .filter(|t| !t.dirty.is_empty())
            .map(|t| (t.id, std::mem::take(&mut t.dirty)))
            .collect();
        (window, tabs)
    }

    /// What the shell has to do, in order; the session last, once, if it changed.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        let mut effects = std::mem::take(&mut self.effects);
        if std::mem::take(&mut self.session_changed) {
            effects.push(Effect::Save(self.shape()));
        }
        if std::mem::take(&mut self.history_changed) {
            effects.push(Effect::SaveHistory(self.history.encode(history::now())));
        }
        effects
    }

    /// Hands over the history read at launch. `rewrite`: it was cleaned on the way in (a
    /// password an older version kept), and goes back to the disk now.
    pub fn set_history(&mut self, history: History, rewrite: bool) {
        self.history = history;
        self.history_changed |= rewrite;
    }

    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// A `history.json` from elsewhere (a profile file) added to this one, as Search's
    /// import takes it: a place already here keeps the larger count. Answers how many
    /// places came in.
    pub fn take_history(&mut self, data: &[u8]) -> usize {
        let taken = self.history.take_file(data, history::now());
        self.history_changed |= taken > 0;
        taken
    }

    /// Settings › Privacy › Clear › History.
    pub fn clear_history(&mut self) {
        self.history = History::default();
        self.history_changed = true;
    }

    /// The session as Search writes it.
    #[must_use]
    pub fn shape(&self) -> session::Shape {
        let kept: Vec<&Tab> = self.tabs.iter().filter(|t| !t.is_blank()).collect();
        // Counted among the tabs written, not the row: a new tab before it isn't kept.
        let active = self
            .active
            .and_then(|id| kept.iter().position(|t| t.id == id))
            .map_or(-1, |i| i64::try_from(i).unwrap_or(-1));
        let entries: Vec<&session::Entry> = kept.iter().map(|t| &t.entry).collect();
        let groups = groups::kept(&self.groups, &entries);
        session::Shape {
            tabs: kept.iter().map(|t| t.entry()).collect(),
            active,
            groups: (!groups.is_empty()).then_some(groups),
            ..session::Shape::default()
        }
    }

    // MARK: groups

    /// The row's tab groups, in order.
    #[must_use]
    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    /// What the row shows, left to right: group headings among the tabs, a folded
    /// group's tabs left out but for the one in front.
    #[must_use]
    pub fn row(&self) -> Vec<Item> {
        let entries: Vec<(&session::Entry, bool)> = self
            .tabs
            .iter()
            .map(|t| (&t.entry, Some(t.id) == self.active))
            .collect();
        groups::row(&entries, &self.groups)
    }

    /// A click on a group's heading folds it, or opens it again (Search's
    /// `toggleTabGroup`).
    pub fn toggle_group(&mut self, place: usize) {
        let Some(group) = self.groups.get_mut(place) else {
            return;
        };
        group.collapsed = !group.collapsed;
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
    }

    /// Sessions from elsewhere (a profile file), their tabs added after this row's with
    /// their groups as they were, folded or not. Each group gets a fresh identity, so
    /// the same file brought in twice makes two groups rather than one that two rows
    /// fight over. A tab of a page Tap can't show (Search's own History, say) stays out.
    /// The tabs come in asleep; the one that was in front in the first session comes
    /// in front. A lone new tab gives way. Answers how many tabs and groups came in.
    pub fn take_tabs(&mut self, shapes: Vec<session::Shape>) -> (usize, usize) {
        let mut made: Vec<String> = Vec::new();
        let mut came: Vec<TabId> = Vec::new();
        let mut front = None;
        for shape in shapes {
            let mut fresh: HashMap<String, String> = HashMap::new();
            for group in unique(shape.groups.unwrap_or_default()) {
                let id = fnd::uuid();
                fresh.insert(group.id.clone(), id.clone());
                made.push(id.clone());
                self.groups.push(TabGroup { id, ..group });
            }
            for (index, mut entry) in shape.tabs.into_iter().enumerate() {
                let Some(url) = internal::from_search(&entry.url).filter(|u| !u.is_empty()) else {
                    continue;
                };
                entry.url = url;
                entry.group_id = entry.group_id.and_then(|g| fresh.get(&g).cloned());
                let tab = self.tab_from(entry);
                if front.is_none() && i64::try_from(index).ok() == Some(shape.active) {
                    front = Some(tab.id);
                }
                came.push(tab.id);
                self.tabs.push(tab);
            }
        }
        let Some(&first) = came.first() else {
            self.prune();
            return (0, 0);
        };
        if self.tabs.len() > came.len() {
            let blank = self.tabs[0].id;
            if self.tabs.len() == came.len() + 1 && self.tabs[0].is_blank() {
                self.tabs.remove(0);
                self.effects.push(Effect::Discard(blank));
                if self.active == Some(blank) {
                    self.active = None;
                }
            }
        }
        self.order();
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        self.select(front.unwrap_or(first));
        let groups = self.groups.iter().filter(|g| made.contains(&g.id)).count();
        (came.len(), groups)
    }

    // MARK: Tap's own pages

    /// Settings (⌘,), or another of Tap's own pages: the tab it is already in, else the
    /// new tab in front, else a tab of its own at the end.
    pub fn open_page(&mut self, page: Page) {
        if let Some(id) = self
            .tabs
            .iter()
            .find(|t| t.page() == Some(page))
            .map(|t| t.id)
        {
            self.select(id);
            return;
        }
        let Some(url) = Url::parse(&page.address()) else {
            return;
        };
        match self.active {
            Some(id) if self.active_tab().is_some_and(Tab::is_blank) => {
                if self.editing.take().is_some() {
                    self.dirty.insert(BrowserFields::EDITING);
                }
                self.go(id, &url);
            }
            _ => {
                self.open(Some(&url), true, None);
            }
        }
    }

    // MARK: tabs

    /// Puts a tab in front, waking it if it was asleep. The field closes.
    pub fn select(&mut self, id: TabId) {
        if self.tab(id).is_none() {
            return;
        }
        if self.active != Some(id) {
            // The tab you leave starts its count now.
            let left = self.active;
            if let Some(left) = left.and_then(|l| self.tab_mut(l)) {
                left.touched = history::now();
            }
            if let Some(left) = left {
                self.viewed.retain(|v| *v != left);
                self.viewed.push(left);
            }
            self.active = Some(id);
            self.dirty.insert(BrowserFields::ACTIVE);
            self.session_changed = true;
            // The find goes with you: asked again of the page now in front, once it has
            // loaded (`page_finished`) if it is still loading.
            if self.find.open() {
                let retry = self
                    .tab(id)
                    .is_some_and(|t| !t.loading && t.awake && !t.stale);
                let outs = self.find.moved(Some(id), retry);
                self.finding(outs);
            }
        }
        if self.editing.take().is_some() {
            self.dirty.insert(BrowserFields::EDITING);
        }
        self.wake(id);
        self.effects.push(Effect::FocusPage(id));
    }

    fn wake(&mut self, id: TabId) {
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.stale {
            tab.stale = false;
            self.effects.push(Effect::Reload(id));
            return;
        }
        if tab.awake || tab.url.is_empty() {
            return;
        }
        tab.awake = true;
        tab.dirty.insert(TabFields::AWAKE);
        if tab.is_internal() {
            return;
        }
        let url = tab.url.clone();
        self.effects.push(Effect::Load { tab: id, url });
    }

    /// Opens an address in a new tab: after a run of links from `opener`, or at the
    /// end of the row.
    pub fn open(&mut self, url: Option<&Url>, foreground: bool, opener: Option<TabId>) -> TabId {
        self.open_at(url, foreground, opener, None)
    }

    /// Where a new tab from another app goes (Search's `placeForNew`): beside the tab in
    /// front, but never among the pins; or first after them; or at the end.
    fn place_for_new(&self) -> usize {
        let pins = self.pins();
        match self.behaviour.new_place {
            NewPlace::Top => pins,
            NewPlace::End => self.tabs.len(),
            NewPlace::Next => self
                .active
                .and_then(|id| self.index(id))
                .map_or(self.tabs.len(), |here| (here + 1).max(pins)),
        }
    }

    /// Where ⌘T's tab goes (Search's `placeForFront`): the end of the row, as always,
    /// unless the top was chosen. After current is for links.
    fn place_for_front(&self) -> usize {
        match self.behaviour.new_place {
            NewPlace::Top => self.pins(),
            _ => self.tabs.len(),
        }
    }

    fn pins(&self) -> usize {
        self.tabs
            .iter()
            .take_while(|t| t.entry.pin.is_some())
            .count()
    }

    fn open_at(
        &mut self,
        url: Option<&Url>,
        foreground: bool,
        opener: Option<TabId>,
        place: Option<usize>,
    ) -> TabId {
        let id = TabId(self.next);
        self.next += 1;
        let mut tab = Tab::new(id);
        tab.opener = opener;
        // A link from a group stays in it (Search's tabs.newPlace note).
        tab.entry.group_id = opener
            .and_then(|from| self.tab(from))
            .filter(|from| from.entry.pin.is_none())
            .and_then(|from| from.entry.group_id.clone());
        if let Some(url) = url {
            url.as_str().clone_into(&mut tab.url);
            // A link opened behind its page waits for its tab to be shown, asleep
            // (Search's `tabs.lazy`); ⇧⌘-click still loads it at once.
            let waits = self.behaviour.lazy && opener.is_some() && !foreground;
            tab.awake = !waits;
            if !waits && !tab.is_internal() {
                self.effects.push(Effect::Load {
                    tab: id,
                    url: tab.url.clone(),
                });
            }
        }
        let place = place.unwrap_or_else(|| {
            opener
                .and_then(|from| {
                    let after = match self.link_run {
                        Some(run) if run.opener == from && self.index(run.last).is_some() => {
                            run.last
                        }
                        _ => from,
                    };
                    self.index(after).map(|i| i + 1)
                })
                .unwrap_or(self.tabs.len())
        });
        self.tabs.insert(place, tab);
        self.order();
        self.link_run = opener.map(|from| LinkRun {
            opener: from,
            last: id,
        });
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        if foreground {
            self.select(id);
        }
        id
    }

    /// A tab WebKit made for a page that asked for a new window: it already has its web
    /// view, so nothing is loaded here.
    pub fn adopt(&mut self, opener: Option<TabId>, foreground: bool) -> TabId {
        let id = self.open(None, false, opener);
        if let Some(tab) = self.tab_mut(id) {
            tab.awake = true;
        }
        if foreground {
            self.select(id);
        }
        id
    }

    /// ⌘W, as Search closes: the neighbour on the right takes its place, or the last
    /// tab; with Settings › Closing a tab goes back, the tab viewed before it. The only
    /// tab is replaced by a new one, unless it is already new, and so is the last loose
    /// tab beside groups, and the last but pins when Settings says so: landing on a pin
    /// or a grouped page you hadn't asked for wakes it and opens its group.
    pub fn close(&mut self, id: TabId) {
        let Some(index) = self.index(id) else { return };
        let closing = &self.tabs[index];
        let kept = |t: &Tab| t.entry.pin.is_some() || t.entry.group_id.is_some();
        let mut others = self.tabs.iter().filter(|t| t.id != id);
        let beside_groups = others
            .clone()
            .any(|t| t.entry.pin.is_none() && t.entry.group_id.is_some());
        let only_pins =
            self.behaviour.new_after_last && !others.clone().any(|t| t.entry.pin.is_none());
        let only_kept = beside_groups && !kept(closing) && !others.any(|t| !kept(t));
        let internal = closing.is_internal();
        if self.tabs.len() == 1 || (self.active == Some(id) && (only_pins || only_kept)) {
            if closing.is_blank() {
                return;
            }
            self.remember(index);
            self.effects.push(Effect::Discard(id));
            let fresh = TabId(self.next);
            self.next += 1;
            self.tabs[index] = Tab::new(fresh);
            self.order();
            self.active = Some(fresh);
            self.dirty
                .insert(BrowserFields::TABS | BrowserFields::ACTIVE);
            self.session_changed = true;
            self.begin_editing(false);
            return;
        }
        self.remember(index);
        self.effects.push(Effect::Discard(id));
        self.tabs.remove(index);
        self.prune();
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        self.viewed.retain(|v| *v != id);
        if self.active == Some(id) {
            // Tap's own pages go back whatever the switch says: Settings is called up
            // over what you were doing.
            let before = (self.behaviour.close_goes_back || internal)
                .then(|| {
                    let viewed = self.viewed.iter().rev();
                    viewed.copied().find(|v| self.index(*v).is_some())
                })
                .flatten();
            let next = before.unwrap_or(self.tabs[index.min(self.tabs.len() - 1)].id);
            self.active = None;
            self.select(next);
        }
    }

    /// The same page beside itself, in front (Search's `duplicate`, from the tab
    /// menu as much as ⌘D). A new tab or one of Tap's own pages has nothing to copy.
    pub fn duplicate(&mut self, id: TabId) -> Option<TabId> {
        let tab = self.tab(id)?;
        if tab.is_blank() || tab.is_internal() {
            return None;
        }
        let url = Url::parse(tab.url())?;
        Some(self.open(Some(&url), true, Some(id)))
    }

    /// Everything but this one goes, as ⌘W would close it. Pinned tabs stay:
    /// Search puts a pin down rather than removing it, and Tap has no put-down
    /// yet, so leaving it standing is the same promise.
    pub fn close_others(&mut self, keep: TabId) {
        if self.tab(keep).is_none() {
            return;
        }
        let going: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.id != keep && !t.is_pinned())
            .map(|t| t.id)
            .collect();
        if going.is_empty() {
            return;
        }
        // Off a page that is going before walking the row, so its neighbours do
        // not wake only to be closed in turn.
        if self.active.is_some_and(|active| going.contains(&active)) {
            self.select(keep);
        }
        for id in going {
            self.close(id);
        }
        if self.active != Some(keep) && self.tab(keep).is_some() {
            self.select(keep);
        }
    }

    /// Everything to this tab's right goes, pins again excepted (Search's
    /// `closeTabsBelow`, the top row's wording). Closed from the bottom, so
    /// ⇧⌘T brings them back in their order.
    pub fn close_right(&mut self, id: TabId) {
        let Some(index) = self.index(id) else { return };
        let going: Vec<TabId> = self.tabs[index + 1..]
            .iter()
            .filter(|t| !t.is_pinned())
            .map(|t| t.id)
            .collect();
        if going.is_empty() {
            return;
        }
        if self.active.is_some_and(|active| going.contains(&active)) {
            self.select(id);
        }
        for going_id in going.iter().rev() {
            self.close(*going_id);
        }
    }

    /// Whether ⇧⌘T has anything to bring back.
    #[must_use]
    pub fn can_reopen(&self) -> bool {
        !self.closed.is_empty()
    }

    fn remember(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if !tab.is_blank() {
            self.closed.push((index, tab.entry()));
        }
    }

    /// ⇧⌘T: the last closed tab, back where it was.
    pub fn reopen(&mut self) {
        let Some((index, entry)) = self.closed.pop() else {
            return;
        };
        let id = TabId(self.next);
        self.next += 1;
        let mut tab = Tab::new(id);
        tab.url.clone_from(&entry.url);
        tab.title.clone_from(&entry.title);
        tab.entry = entry;
        // An empty new tab in front is what closing the only tab left: it gives way.
        if self.tabs.len() == 1 && self.tabs[0].is_blank() {
            self.tabs.clear();
        }
        let place = index.min(self.tabs.len());
        self.tabs.insert(place, tab);
        // Its group may have gone with it: then it comes back in none.
        self.order();
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        self.select(id);
    }

    /// ⌃Tab and ⌃⇧Tab: along the row, round from one end to the other.
    pub fn select_beside(&mut self, forward: bool) {
        let Some(index) = self.active.and_then(|id| self.index(id)) else {
            return;
        };
        let count = self.tabs.len();
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        self.select(self.tabs[next].id);
    }

    /// ⌘1 to ⌘8 by place; ⌘9 is always the last.
    pub fn select_number(&mut self, number: usize) {
        let index = if number >= 9 {
            self.tabs.len().saturating_sub(1)
        } else {
            number.saturating_sub(1)
        };
        if let Some(tab) = self.tabs.get(index) {
            self.select(tab.id);
        }
    }

    // MARK: the field

    /// ⌘L opens the field on the live tab with its address; ⌘T floats it over the page.
    pub fn begin_editing(&mut self, launching: bool) {
        let typed = if launching {
            String::new()
        } else {
            self.active_tab()
                .and_then(|t| Url::parse(&t.url).filter(|_| !t.url.is_empty()))
                .map(|u| address::editable(&u))
                .unwrap_or_default()
        };
        // The field opens on the page's own address, selected: nothing to offer for it
        // until a key changes it.
        self.editing = Some(Editing {
            typed,
            launching,
            offers: Vec::new(),
            ending: None,
            picked: None,
        });
        self.dirty
            .insert(BrowserFields::EDITING | BrowserFields::TYPED | BrowserFields::OFFERS);
    }

    /// What the keyboard put in the field. `composing`: an input method is still
    /// composing (pinyin, kana), so nothing is completed: an ending drawn after marked
    /// text, or text written back into the field under it, breaks the composition
    /// (Search's own gap, docs/PLAN.md §3.5).
    pub fn set_typed(&mut self, text: &str, composing: bool) {
        if self.editing.is_none() {
            return;
        }
        let (offers, ending) = self.place_offers(text, composing);
        let Some(editing) = self.editing.as_mut() else {
            return;
        };
        text.clone_into(&mut editing.typed);
        editing.offers = offers;
        editing.ending = ending;
        editing.picked = None;
        self.dirty
            .insert(BrowserFields::TYPED | BrowserFields::OFFERS);
    }

    /// What the field offers for `typed`, and the rest of the address it would complete
    /// to (Search's `Browser.placeOffers`): three places from history, and a search last
    /// when what is typed cannot be a place.
    fn place_offers(&self, typed: &str, composing: bool) -> (Vec<Suggestion>, Option<String>) {
        let mut offers = self.history.suggestions(typed, 3, history::now());
        let ending = if composing {
            None
        } else {
            History::completion(typed, &offers)
        };
        if !typed.trim().is_empty() && address::url(typed).is_none() {
            let search = match keyword::find(typed, &self.keywords) {
                Some((keyword, rest)) => engine::url_for(&rest, &keyword.template).map(|url| {
                    (
                        url,
                        engine::bare_host(&keyword.template).unwrap_or_default(),
                    )
                }),
                None => engine::url_for(typed, &self.engine.template(&self.custom_engine))
                    .map(|url| (url, self.engine.name(&self.custom_engine))),
            };
            if let Some((url, name)) = search {
                offers.push(Suggestion {
                    key: typed.to_owned(),
                    title: name,
                    url: url.as_str().to_owned(),
                    kind: history::Kind::Search,
                });
            }
        }
        (offers, ending)
    }

    /// ↓ and ↑ walk the rows: down from the field into the first, up from the first back
    /// into the field.
    pub fn pick(&mut self, down: bool) {
        let Some(editing) = self.editing.as_mut() else {
            return;
        };
        let count = editing.offers.len();
        if count == 0 {
            return;
        }
        editing.picked = match (editing.picked, down) {
            (None, true) => Some(0),
            (Some(row), true) => Some((row + 1).min(count - 1)),
            (Some(0) | None, false) => None,
            (Some(row), false) => Some(row - 1),
        };
        self.dirty.insert(BrowserFields::OFFERS);
    }

    /// A row chosen by the pointer.
    pub fn pick_row(&mut self, row: usize) {
        if let Some(editing) = self.editing.as_mut().filter(|e| row < e.offers.len()) {
            editing.picked = Some(row);
            self.dirty.insert(BrowserFields::OFFERS);
        }
    }

    /// Tab takes the grey ending into the field. Answers whether there was one.
    pub fn accept_ending(&mut self) -> bool {
        let Some(editing) = self.editing.as_ref() else {
            return false;
        };
        let Some(ending) = editing.ending.clone() else {
            return false;
        };
        let typed = format!("{}{ending}", editing.typed);
        self.set_typed(&typed, false);
        self.dirty.insert(BrowserFields::RETYPED);
        true
    }

    /// Esc, or a click away: the field closes and nothing is opened.
    pub fn cancel_editing(&mut self) {
        if self.editing.take().is_some() {
            self.dirty.insert(BrowserFields::EDITING);
            if let Some(id) = self.active {
                self.effects.push(Effect::FocusPage(id));
            }
        }
    }

    /// Where Return takes what was typed: a local file, an address, a keyword's search,
    /// or the engine's (Search's `Browser.destination(for:)`).
    #[must_use]
    pub fn destination(&self, typed: &str) -> Option<Url> {
        if let Some(url) = address::file(typed).or_else(|| address::url(typed)) {
            return Some(address::reachable(&url).unwrap_or(url));
        }
        if let Some((keyword, rest)) = keyword::find(typed, &self.keywords)
            && let Some(url) = engine::url_for(&rest, &keyword.template)
        {
            return Some(url);
        }
        engine::url_for(typed, &self.engine.template(&self.custom_engine))
    }

    /// Return in the field.
    pub fn submit(&mut self) {
        let Some(editing) = self.editing.clone() else {
            return;
        };
        if self.behaviour.commands
            && editing.picked.is_none()
            && let Some(command) = Command::matching(&editing.typed)
        {
            self.editing = None;
            self.dirty.insert(BrowserFields::EDITING);
            match command {
                Command::Settings => self.open_page(Page::Settings),
                Command::NewTab => {
                    if !self.active_tab().is_some_and(Tab::is_blank) {
                        let place = self.place_for_front();
                        self.open_at(None, true, None, Some(place));
                    }
                    self.begin_editing(false);
                }
            }
            return;
        }
        // The row the arrow keys chose, or else the address the field was finishing,
        // or else where the words go.
        let target = if let Some(row) = editing.picked.and_then(|i| editing.offers.get(i)) {
            Url::parse(&row.url)
        } else if let Some(ending) = &editing.ending {
            address::url(&format!("{}{ending}", editing.typed))
        } else {
            self.destination(&editing.typed)
        };
        let Some(url) = target else {
            self.refusals += 1;
            self.dirty.insert(BrowserFields::REFUSED);
            return;
        };
        self.editing = None;
        self.dirty.insert(BrowserFields::EDITING);
        let blank_in_front = self.active_tab().is_some_and(Tab::is_blank);
        if editing.launching && !blank_in_front {
            let place = self.place_for_front();
            self.open_at(Some(&url), true, None, Some(place));
        } else if let Some(id) = self.active {
            self.go(id, &url);
        }
    }

    /// Sends a tab to an address.
    pub fn go(&mut self, id: TabId, url: &Url) {
        let Some(tab) = self.tab_mut(id) else { return };
        url.as_str().clone_into(&mut tab.url);
        tab.title.clear();
        tab.awake = true;
        tab.failure = None;
        tab.dead = false;
        tab.dirty
            .insert(TabFields::URL | TabFields::TITLE | TabFields::FAILURE | TabFields::AWAKE);
        let internal = tab.is_internal();
        self.session_changed = true;
        if internal {
            // Its web view, if it had one, has nothing to show now.
            self.effects.push(Effect::Discard(id));
        } else {
            self.effects.push(Effect::Load {
                tab: id,
                url: url.as_str().to_owned(),
            });
        }
        self.effects.push(Effect::FocusPage(id));
    }

    /// An address another app handed over (Search's `Browser.arrive`): into the new tab
    /// in front if nothing has been typed in it, else into a tab of its own, in front.
    pub fn arrive(&mut self, url: &Url) {
        let typed = self.editing.as_ref().is_some_and(|e| !e.typed.is_empty());
        match self.active {
            Some(id) if !typed && self.active_tab().is_some_and(Tab::is_blank) => {
                if self.editing.take().is_some() {
                    self.dirty.insert(BrowserFields::EDITING);
                }
                self.go(id, url);
            }
            _ => {
                let place = self.place_for_new();
                self.open_at(Some(url), true, None, Some(place));
            }
        }
    }

    // MARK: find on page

    #[must_use]
    pub fn find(&self) -> &Find {
        &self.find
    }

    fn finding(&mut self, outs: Vec<find::Out>) {
        for out in outs {
            self.effects.push(match out {
                find::Out::Ask(ask) => Effect::Find(ask),
                find::Out::Clear { tab, generation } => Effect::ClearFind { tab, generation },
            });
        }
        self.dirty.insert(BrowserFields::FIND);
    }

    /// ⌘F: the bar, with the keys in its field and its word selected. Only over a page.
    pub fn open_find(&mut self) {
        if self
            .active_tab()
            .is_none_or(|t| t.is_blank() || t.is_internal())
        {
            return;
        }
        self.find.show();
        self.dirty
            .insert(BrowserFields::FIND | BrowserFields::FIND_FOCUS);
    }

    /// Esc or Done: the bar goes, and the page has the keyboard again.
    pub fn close_find(&mut self) {
        if !self.find.open() {
            return;
        }
        let outs = self.find.close();
        self.finding(outs);
        if let Some(id) = self.active {
            self.effects.push(Effect::FocusPage(id));
        }
    }

    pub fn set_find_needle(&mut self, needle: &str) {
        let outs = self.find.set_needle(needle, self.active);
        self.finding(outs);
    }

    pub fn set_find_match_case(&mut self, on: bool) {
        let outs = self.find.set_match_case(on, self.active);
        self.finding(outs);
    }

    pub fn set_find_whole_words(&mut self, on: bool) {
        let outs = self.find.set_whole_words(on, self.active);
        self.finding(outs);
    }

    /// Return, ⌘G, the arrows: the next match, or the one before.
    pub fn find_next(&mut self, forward: bool) {
        let outs = self.find.look(self.active, forward);
        self.finding(outs);
    }

    /// The page answered.
    pub fn find_answered(&mut self, ask: &find::Ask, answer: find::Answer) {
        let outs = self
            .find
            .answered(ask.asking, ask.generation, ask.tab, answer, self.active);
        self.finding(outs);
    }

    // MARK: the page

    /// Gives the live page the keyboard: once the window exists, for a restored tab.
    pub fn focus_page(&mut self) {
        if self.editing.is_none()
            && let Some(id) = self.active
        {
            self.effects.push(Effect::FocusPage(id));
        }
    }

    pub fn back(&mut self) {
        if let Some(tab) = self.active_tab().filter(|t| t.can_back) {
            self.effects.push(Effect::Back(tab.id));
        }
    }
    pub fn forward(&mut self) {
        if let Some(tab) = self.active_tab().filter(|t| t.can_forward) {
            self.effects.push(Effect::Forward(tab.id));
        }
    }
    /// ⌘R, or Try again over a page that said why it isn't there.
    pub fn reload(&mut self) {
        if let Some(id) = self.active {
            self.reload_tab(id);
        }
    }

    /// Reloads one tab, whichever is in front: the tab menu's Reload Tab.
    pub fn reload_tab(&mut self, id: TabId) {
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.url.is_empty() || tab.is_internal() {
            return;
        }
        // Try again on a page that kept crashing: its process is gone, so the address
        // is asked for afresh rather than reloading what isn't there (Search #849).
        if tab.dead {
            tab.dead = false;
            tab.failure = None;
            tab.dirty.insert(TabFields::FAILURE);
            let url = tab.url.clone();
            self.effects.push(Effect::Load { tab: id, url });
            return;
        }
        if tab.failure.take().is_some() {
            tab.dirty.insert(TabFields::FAILURE);
        }
        self.effects.push(Effect::Reload(id));
    }
    pub fn stop(&mut self) {
        if let Some(id) = self.active {
            self.effects.push(Effect::Stop(id));
        }
    }

    // MARK: what the web view reports

    pub fn page_url(&mut self, id: TabId, url: &str) {
        let Some(tab) = self.tab_mut(id) else { return };
        if url.is_empty() || url == "about:blank" && tab.url.is_empty() {
            return;
        }
        Tab::set(&mut tab.url, url.to_owned(), &mut tab.dirty, TabFields::URL);
        self.session_changed = true;
    }

    pub fn page_title(&mut self, id: TabId, title: &str) {
        let Some(tab) = self.tab_mut(id) else { return };
        Tab::set(
            &mut tab.title,
            title.to_owned(),
            &mut tab.dirty,
            TabFields::TITLE,
        );
        let url = tab.url.clone();
        self.session_changed = true;
        if !url.is_empty() && self.history.retitle(&url, title) {
            self.history_changed = true;
        }
    }

    /// A page arrived in the tab (WebKit committed its navigation).
    pub fn page_committed(&mut self, id: TabId) {
        if let Some(tab) = self.tab_mut(id) {
            tab.committed = true;
        }
        // A new page under an open find bar: the old page's answers are retired; the new
        // one is asked once it has finished loading.
        if self.find.open() && self.active == Some(id) {
            let outs = self.find.moved(Some(id), false);
            self.finding(outs);
        }
    }

    /// The tab's navigation became a download. A tab opened for it alone, with nothing
    /// ever shown in it, goes: left, it is an empty tab nobody asked for, and it is not
    /// offered back by ⇧⌘T either (Search's `dropEmpty`). Its opener comes back in front.
    pub fn drop_empty(&mut self, id: TabId) {
        let Some(index) = self.index(id) else { return };
        let tab = &self.tabs[index];
        if tab.committed || self.tabs.len() < 2 {
            return;
        }
        let opener = tab.opener.filter(|o| self.index(*o).is_some());
        self.effects.push(Effect::Discard(id));
        self.tabs.remove(index);
        self.prune();
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        if self.active == Some(id) {
            self.active = None;
            let next = opener.unwrap_or(self.tabs[index.min(self.tabs.len() - 1)].id);
            self.select(next);
        }
    }

    // MARK: sleep

    /// Why a tab has to stay awake, as far as the window can tell, or `None` when nothing
    /// here keeps it (Search's `Browser.awake(because:)`). What only the page knows
    /// (sound, a call, a download, a question, something typed) the shell asks.
    #[must_use]
    pub fn awake_because(&self, id: TabId) -> Option<&'static str> {
        let Some(tab) = self.tab(id) else {
            return Some("no such tab");
        };
        if self.active == Some(id) {
            return Some("on screen");
        }
        if tab.is_blank() {
            return Some("blank");
        }
        if tab.is_internal() {
            return Some("one of Tap's own pages");
        }
        if !tab.awake || tab.stale {
            return Some("already asleep");
        }
        if tab.loading {
            return Some("still loading");
        }
        // A sign-in window hands its answer back to the page that opened it.
        if self.active_tab().and_then(|t| t.opener) == Some(id) {
            return Some("the page on screen came from it");
        }
        None
    }

    /// Every tab left alone `wait` seconds or more that nothing here keeps awake, the one
    /// left longest first: each is asked to sleep (Search's `sleepIdle`).
    pub fn sleep_idle(&mut self, now: f64, wait: f64) {
        let mut idle: Vec<(f64, TabId)> = self
            .tabs
            .iter()
            .filter(|t| now - t.touched >= wait && self.awake_because(t.id).is_none())
            .map(|t| (t.touched, t.id))
            .collect();
        idle.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.effects
            .extend(idle.into_iter().map(|(_, id)| Effect::Sleep(id)));
    }

    /// Why a tab stays open however long it is left (Search's `stays(because:)`), or
    /// `None` when nothing here keeps it: what you said to keep, a pinned, named or
    /// grouped tab, stays. What only the page knows the shell asks.
    #[must_use]
    pub fn stays_because(&self, id: TabId) -> Option<&'static str> {
        let Some(tab) = self.tab(id) else {
            return Some("no such tab");
        };
        if self.active == Some(id) {
            return Some("on screen");
        }
        if tab.entry.pin.is_some() {
            return Some("pinned");
        }
        if tab.entry.name.is_some() {
            return Some("named");
        }
        if tab.entry.group_id.is_some() {
            return Some("in a group");
        }
        if tab.is_blank() {
            return Some("blank");
        }
        None
    }

    /// Every tab left alone past Settings › Close tabs you leave alone that nothing here
    /// keeps (Search's `closeLeftAlone`): each is asked to close.
    pub fn close_left_alone(&mut self, now: f64) {
        let Some(wait) = self.behaviour.close_after else {
            return;
        };
        let since = now - wait;
        let due: Vec<Effect> = self
            .tabs
            .iter()
            .filter(|t| t.touched <= since && self.stays_because(t.id).is_none())
            .map(|t| Effect::Retire { tab: t.id, since })
            .collect();
        self.effects.extend(due);
    }

    /// The page had nothing to keep it: the tab closes as ⌘W would, ⇧⌘T bringing it
    /// back, if it is still left alone since `since` and nothing keeps it.
    pub fn retire(&mut self, id: TabId, since: f64) {
        let left = self.tab(id).is_some_and(|t| t.touched <= since);
        if left && self.behaviour.close_after.is_some() && self.stays_because(id).is_none() {
            self.close(id);
        }
    }

    /// The tab gave its page back: it wakes, where it was, the next time it is shown.
    pub fn slept(&mut self, id: TabId) {
        if self.awake_because(id).is_some() {
            return;
        }
        if let Some(tab) = self.tab_mut(id) {
            tab.awake = false;
            tab.loading = false;
            tab.dirty.insert(TabFields::AWAKE | TabFields::LOADING);
        }
    }

    /// The tab's site icon arrived: the row draws it.
    pub fn icon_arrived(&mut self, id: TabId) {
        if let Some(tab) = self.tab_mut(id) {
            tab.dirty.insert(TabFields::ICON);
        }
    }

    /// A page finished loading in the tab: a visit, kept in history.
    pub fn page_finished(&mut self, id: TabId) {
        if self.find.open()
            && self.active == Some(id)
            && !self.find.asked_for()
            && !self.find.needle().trim().is_empty()
        {
            let outs = self.find.look(Some(id), true);
            self.finding(outs);
        }
        let Some(tab) = self.tab(id) else { return };
        let (url, title) = (tab.url.clone(), tab.title.clone());
        if self.history.record(&url, &title, history::now()) {
            self.history_changed = true;
        }
    }

    pub fn page_loading(&mut self, id: TabId, loading: bool, progress: f64) {
        let Some(tab) = self.tab_mut(id) else { return };
        Tab::set(
            &mut tab.loading,
            loading,
            &mut tab.dirty,
            TabFields::LOADING,
        );
        if (tab.progress - progress).abs() > f64::EPSILON {
            tab.progress = progress;
            tab.dirty.insert(TabFields::PROGRESS);
        }
    }

    pub fn page_history(&mut self, id: TabId, can_back: bool, can_forward: bool) {
        let Some(tab) = self.tab_mut(id) else { return };
        Tab::set(
            &mut tab.can_back,
            can_back,
            &mut tab.dirty,
            TabFields::HISTORY,
        );
        Tab::set(
            &mut tab.can_forward,
            can_forward,
            &mut tab.dirty,
            TabFields::HISTORY,
        );
    }

    /// A load that never arrived: said in place of the page.
    pub fn page_failed(&mut self, id: TabId, message: &str) {
        let Some(tab) = self.tab_mut(id) else { return };
        Tab::set(
            &mut tab.failure,
            Some(message.to_owned()),
            &mut tab.dirty,
            TabFields::FAILURE,
        );
    }

    /// The tab's WebContent process ended. In front, it is made again straight away,
    /// twice a minute at most; past that the page says so and waits for Try again
    /// (Search #849). Behind another tab, it is made again when you come back to it.
    pub fn process_ended(&mut self, id: TabId, end: PageEnd, now: Instant) -> Recovery {
        let in_front = self.active == Some(id);
        let Some(tab) = self.tab_mut(id) else {
            return Recovery::Nothing;
        };
        if tab.url.is_empty() {
            return Recovery::Nothing;
        }
        if !in_front {
            tab.stale = true;
            return Recovery::WhenShown;
        }
        if !tab.crashes.another(now) {
            tab.failure = Some(end.message().to_owned());
            tab.dead = true;
            tab.loading = false;
            tab.dirty.insert(TabFields::FAILURE | TabFields::LOADING);
            return Recovery::KeptDying;
        }
        self.effects.push(Effect::Reload(id));
        Recovery::LoadedAgain
    }
}

/// Groups by identity, the first of each kept: two with one identity would be one group
/// drawn twice.
fn unique(groups: Vec<TabGroup>) -> Vec<TabGroup> {
    let mut seen: Vec<TabGroup> = Vec::with_capacity(groups.len());
    for group in groups {
        if !seen.iter().any(|g| g.id == group.id) {
            seen.push(group);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("parses")
    }

    fn with_pages(pages: &[&str]) -> Browser {
        let mut browser = Browser::new();
        let first = browser.active().expect("a tab");
        browser.go(first, &url(pages[0]));
        for page in &pages[1..] {
            browser.open(Some(&url(page)), false, None);
        }
        browser.take_effects();
        browser
    }

    #[test]
    fn closing_the_live_tab_lands_on_its_right_hand_neighbour_or_the_last() {
        let mut browser = with_pages(&["https://a.com", "https://b.com", "https://c.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[1]);
        browser.close(ids[1]);
        assert_eq!(browser.active(), Some(ids[2]));
        browser.close(ids[2]);
        assert_eq!(browser.active(), Some(ids[0]));
    }

    #[test]
    fn closing_the_only_tab_leaves_a_new_one_and_reopen_brings_it_back() {
        let mut browser = with_pages(&["https://a.com"]);
        let only = browser.active().expect("a tab");
        browser.close(only);
        assert_eq!(browser.tabs().len(), 1);
        assert!(browser.active_tab().is_some_and(Tab::is_blank));
        assert!(browser.editing().is_some(), "the new tab's field is open");
        let blank = browser.active().expect("a tab");
        browser.close(blank);
        assert_eq!(browser.tabs().len(), 1, "an empty last tab stays");

        browser.reopen();
        assert_eq!(browser.tabs().len(), 1, "the empty tab gives way");
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://a.com"));
    }

    #[test]
    fn a_link_from_another_app_uses_the_new_tab_unless_something_is_typed_in_it() {
        let mut browser = Browser::new();
        browser.begin_editing(true);
        let blank = browser.active().expect("a tab");
        browser.arrive(&url("https://a.com/"));
        assert_eq!(browser.tabs().len(), 1);
        assert_eq!(browser.active(), Some(blank));
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://a.com/"));
        assert!(browser.editing().is_none());

        browser.arrive(&url("https://b.com/"));
        assert_eq!(browser.tabs().len(), 2, "a page in front keeps its tab");
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://b.com/"));

        browser.open(None, true, None);
        browser.begin_editing(true);
        browser.set_typed("half a thou", false);
        browser.arrive(&url("https://c.com/"));
        assert_eq!(
            browser.tabs().len(),
            4,
            "what you were typing isn't thrown away"
        );
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://c.com/"));
    }

    #[test]
    fn duplicating_a_tab_puts_the_same_page_beside_it_in_front() {
        let mut browser = with_pages(&["https://a.com", "https://b.com", "https://c.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        let copy = browser.duplicate(ids[0]).expect("a copy");
        assert_eq!(browser.tabs().len(), 4);
        assert_eq!(browser.tabs()[1].id, copy, "beside the tab it copies");
        assert_eq!(browser.tabs()[1].url(), "https://a.com");
        assert_eq!(browser.active(), Some(copy), "the copy comes to the front");
    }

    #[test]
    fn a_new_tab_cannot_be_duplicated_and_reloading_is_by_tab() {
        let mut browser = Browser::new();
        let blank = browser.active().expect("a tab");
        assert_eq!(browser.duplicate(blank), None);
        assert_eq!(browser.tabs().len(), 1);
        browser.take_effects();
        browser.reload_tab(blank);
        assert!(browser.take_effects().is_empty(), "nothing to reload");

        let mut browser = with_pages(&["https://a.com", "https://b.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[0]);
        browser.take_effects();
        browser.reload_tab(ids[1]);
        assert_eq!(browser.take_effects(), vec![Effect::Reload(ids[1])]);
        assert_eq!(browser.active(), Some(ids[0]), "the front tab is untouched");
    }

    #[test]
    fn closing_the_others_keeps_the_tab_and_the_pins() {
        let mut browser =
            with_pages(&["https://a.com", "https://b.com", "https://c.com", "https://d.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.tab_mut(ids[0]).expect("a tab").entry.pin = Some("A".to_owned());
        assert!(!browser.can_reopen());
        browser.select(ids[3]);
        browser.close_others(ids[2]);
        let left: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        assert_eq!(left, vec![ids[0], ids[2]], "the pin and the kept tab stay");
        assert_eq!(browser.active(), Some(ids[2]));
        assert!(browser.can_reopen(), "what was closed can come back");
    }

    #[test]
    fn closing_to_the_right_keeps_pins_and_lands_on_the_tab() {
        let mut browser =
            with_pages(&["https://a.com", "https://b.com", "https://c.com", "https://d.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.tab_mut(ids[3]).expect("a tab").entry.pin = Some("D".to_owned());
        browser.select(ids[2]);
        browser.close_right(ids[1]);
        let left: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        assert_eq!(left, vec![ids[0], ids[1], ids[3]], "the pin to the right stays");
        assert_eq!(browser.active(), Some(ids[1]), "off a page that went");
    }

    #[test]
    fn tabs_walk_round_and_nine_is_the_last() {
        let mut browser = with_pages(&["https://a.com", "https://b.com", "https://c.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[2]);
        browser.select_beside(true);
        assert_eq!(browser.active(), Some(ids[0]));
        browser.select_beside(false);
        assert_eq!(browser.active(), Some(ids[2]));
        browser.select_number(1);
        assert_eq!(browser.active(), Some(ids[0]));
        browser.select_number(9);
        assert_eq!(browser.active(), Some(ids[2]));
    }

    #[test]
    fn return_goes_to_an_address_or_searches() {
        let mut browser = Browser::new();
        browser.begin_editing(false);
        browser.set_typed("example.com", false);
        browser.submit();
        assert_eq!(
            browser.active_tab().map(Tab::url),
            Some("https://example.com")
        );
        assert!(browser.editing().is_none());

        browser.begin_editing(false);
        assert_eq!(
            browser.editing().map(|e| e.typed.as_str()),
            Some("example.com")
        );
        browser.set_typed("rust ownership", false);
        browser.submit();
        assert_eq!(
            browser.active_tab().map(Tab::url),
            Some("https://www.google.com/search?q=rust%20ownership")
        );
    }

    #[test]
    fn launching_opens_a_tab_only_on_return() {
        let mut browser = with_pages(&["https://a.com"]);
        browser.begin_editing(true);
        browser.cancel_editing();
        assert_eq!(browser.tabs().len(), 1, "put away, nothing is opened");

        browser.begin_editing(true);
        browser.set_typed("b.com", false);
        browser.submit();
        assert_eq!(browser.tabs().len(), 2);
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://b.com"));
        assert_eq!(
            browser.tabs()[0].url(),
            "https://a.com",
            "the page under the field stays"
        );
    }

    #[test]
    fn links_from_one_tab_land_one_after_another_beside_it() {
        let mut browser = with_pages(&["https://a.com", "https://z.com"]);
        let opener = browser.tabs()[0].id;
        let one = browser.open(Some(&url("https://one.com")), false, Some(opener));
        let two = browser.open(Some(&url("https://two.com")), false, Some(opener));
        let order: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        assert_eq!(&order[..3], &[opener, one, two]);
    }

    #[test]
    fn a_page_that_keeps_crashing_stops_reloading_and_try_again_loads_it_afresh() {
        let mut browser = with_pages(&["https://heavy.example"]);
        let id = browser.active().expect("a tab");
        let start = Instant::now();
        assert_eq!(
            browser.process_ended(id, PageEnd::Crash, start),
            Recovery::LoadedAgain
        );
        assert_eq!(
            browser.process_ended(id, PageEnd::Crash, start + Duration::from_secs(5)),
            Recovery::LoadedAgain
        );
        assert_eq!(
            browser
                .take_effects()
                .iter()
                .filter(|e| matches!(e, Effect::Reload(_)))
                .count(),
            2
        );

        assert_eq!(
            browser.process_ended(id, PageEnd::Memory, start + Duration::from_secs(9)),
            Recovery::KeptDying
        );
        assert_eq!(
            browser.active_tab().and_then(Tab::failure),
            Some("This page kept running out of memory.")
        );
        assert!(
            !browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Reload(_))),
            "nothing reloads behind the message"
        );

        browser.reload();
        assert!(browser.active_tab().and_then(Tab::failure).is_none());
        assert!(browser.take_effects().contains(&Effect::Load {
            tab: id,
            url: "https://heavy.example".into()
        }));
    }

    #[test]
    fn a_session_comes_back_asleep_but_for_the_tab_in_front() {
        let mut browser = with_pages(&["https://a.com", "https://b.com"]);
        let second = browser.tabs()[1].id;
        browser.select(second);
        let shape = browser.shape();
        assert_eq!(shape.active, 1);

        let mut again = Browser::restored(shape);
        assert_eq!(again.tabs().len(), 2);
        assert!(!again.tabs()[0].awake());
        assert!(again.tabs()[1].awake());
        let effects = again.take_effects();
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::Load { .. }))
                .count(),
            1
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Save(_))),
            "restoring changes nothing to save"
        );
    }

    /// A session as Search writes one with groups: tabs of a group need not stand
    /// together in the file, and a folded group's tabs stay out of the row.
    fn grouped() -> session::Shape {
        let shape = r#"{"tabs":[
            {"url":"https://loose.com","title":"Loose"},
            {"url":"https://a1.com","title":"A1","groupID":"AAAAAAAA-0000-4000-8000-000000000001"},
            {"url":"https://b1.com","title":"B1","groupID":"BBBBBBBB-0000-4000-8000-000000000002"},
            {"url":"https://a2.com","title":"A2","groupID":"AAAAAAAA-0000-4000-8000-000000000001"},
            {"url":"https://gone.com","title":"Gone","groupID":"CCCCCCCC-0000-4000-8000-000000000003"}],
          "active":3,
          "groups":[{"id":"AAAAAAAA-0000-4000-8000-000000000001","name":"软件开发","collapsed":true},
                    {"id":"BBBBBBBB-0000-4000-8000-000000000002","name":"AI Providers","collapsed":false}]}"#;
        session::Shape::decode(shape.as_bytes()).expect("decodes")
    }

    fn urls(browser: &Browser) -> Vec<&str> {
        browser.tabs().iter().map(Tab::url).collect()
    }

    #[test]
    fn a_session_with_groups_comes_back_group_by_group() {
        let browser = Browser::restored(grouped());
        assert_eq!(
            urls(&browser),
            [
                "https://a1.com",
                "https://a2.com",
                "https://b1.com",
                "https://loose.com",
                "https://gone.com"
            ],
            "each group's tabs together, in the groups' order; a tab of no known group is loose"
        );
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://a2.com"));
        // The first group is folded: its heading, and of its tabs only the one in front.
        assert_eq!(
            browser.row(),
            [
                Item::Heading(0),
                Item::Tab(1),
                Item::Heading(1),
                Item::Tab(2),
                Item::Tab(3),
                Item::Tab(4)
            ]
        );
        let shape = browser.shape();
        let groups = shape.groups.expect("groups are written back");
        assert_eq!(
            groups
                .iter()
                .map(|g| (g.name.as_str(), g.collapsed))
                .collect::<Vec<_>>(),
            [("软件开发", true), ("AI Providers", false)]
        );
        assert_eq!(shape.active, 1);
        assert_eq!(shape.tabs[4].group_id, None);
    }

    #[test]
    fn a_heading_folds_its_group_and_a_link_from_a_group_stays_in_it() {
        let mut browser = Browser::restored(grouped());
        browser.toggle_group(0);
        assert!(!browser.groups()[0].collapsed);
        assert_eq!(
            browser.row().len(),
            7,
            "every tab shows once its group is open"
        );
        let a1 = browser.tabs()[0].id;
        let link = browser.open(Some(&url("https://a3.com")), false, Some(a1));
        assert_eq!(
            browser.tab(link).and_then(Tab::group),
            browser.tabs()[0].group()
        );
        assert_eq!(
            urls(&browser)[..3],
            ["https://a1.com", "https://a3.com", "https://a2.com"]
        );
        // The group goes with its last tab.
        let b1 = browser.tabs()[3].id;
        browser.close(b1);
        assert_eq!(browser.groups().len(), 1);
        assert!(!browser.row().contains(&Item::Heading(1)));
    }

    #[test]
    fn a_profile_s_tabs_come_in_with_their_groups_and_a_lone_new_tab_gives_way() {
        let mut browser = Browser::new();
        let (tabs, groups) = browser.take_tabs(vec![grouped()]);
        assert_eq!((tabs, groups), (5, 2));
        assert_eq!(browser.tabs().len(), 5, "the new tab gave way");
        assert_eq!(browser.active_tab().map(Tab::url), Some("https://a2.com"));
        assert!(
            browser.tabs().iter().filter(|t| t.awake()).count() == 1,
            "only the tab in front loads"
        );
        assert!(
            !browser
                .groups()
                .iter()
                .any(|g| g.id == "AAAAAAAA-0000-4000-8000-000000000001"),
            "each group has a fresh identity"
        );

        // Brought in twice: two of each group, never one shared by both.
        let (tabs, groups) = browser.take_tabs(vec![grouped()]);
        assert_eq!((tabs, groups, browser.groups().len()), (5, 2, 4));
        assert_eq!(browser.tabs().len(), 10);
        let names: Vec<&str> = browser.groups().iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["软件开发", "AI Providers", "软件开发", "AI Providers"]
        );
        assert!(
            browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Save(_)))
        );
    }

    #[test]
    fn search_s_own_pages_come_over_as_tap_s_or_not_at_all() {
        let shape = session::Shape::decode(
            br#"{"tabs":[{"url":"search://settings","title":"Settings"},
                         {"url":"search://history","title":"History"}],"active":0}"#,
        )
        .expect("decodes");
        let mut browser = with_pages(&["https://a.com"]);
        assert_eq!(browser.take_tabs(vec![shape]).0, 1);
        assert_eq!(urls(&browser), ["https://a.com", "tap://settings"]);
    }

    #[test]
    fn settings_is_a_page_of_tap_s_own_that_never_reaches_webkit() {
        let mut browser = with_pages(&["https://a.com"]);
        browser.open_page(Page::Settings);
        let effects = browser.take_effects();
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Load { .. })),
            "nothing is loaded: {effects:?}"
        );
        let settings = browser.active_tab().expect("in front");
        assert_eq!(
            (settings.url(), settings.label().as_str()),
            ("tap://settings", "Settings")
        );
        assert!(browser.awake_because(settings.id).is_some());
        // Asked for again, the same tab comes in front.
        let first = browser.tabs()[0].id;
        browser.select(first);
        browser.open_page(Page::Settings);
        assert_eq!(browser.tabs().len(), 2);
        assert_eq!(
            browser.active_tab().and_then(Tab::page),
            Some(Page::Settings)
        );
        browser.open_find();
        assert!(!browser.find().open(), "no finding in Tap's own page");
        // Restored, it comes back the same way, still without a load.
        let mut again = Browser::restored(browser.shape());
        assert!(
            !again
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Load { .. }))
        );
    }

    #[test]
    fn the_tab_in_front_is_counted_among_the_tabs_written() {
        let mut browser = Browser::new();
        let blank = browser.active().expect("a tab");
        let second = browser.open(Some(&url("https://b.com")), false, None);
        browser.select(second);
        assert_eq!(
            browser.shape().active,
            0,
            "the new tab before it isn't written"
        );
        let _ = blank;
    }

    fn visited(pages: &[(&str, &str)]) -> Browser {
        let mut browser = Browser::new();
        let id = browser.active().expect("a tab");
        for (address, title) in pages {
            browser.go(id, &url(address));
            browser.page_title(id, title);
            browser.page_finished(id);
        }
        browser.take_effects();
        browser
    }

    #[test]
    fn a_finished_page_is_a_visit_and_history_is_written() {
        let mut browser = Browser::new();
        let id = browser.active().expect("a tab");
        browser.go(id, &url("https://example.com/"));
        browser.page_title(id, "Example Domain");
        browser.page_finished(id);
        assert_eq!(browser.history().len(), 1);
        let effects = browser.take_effects();
        assert!(effects.iter().any(|e| matches!(e, Effect::SaveHistory(data) if String::from_utf8_lossy(data).contains("Example Domain"))));
    }

    #[test]
    fn typing_offers_places_and_finishes_the_address_return_goes_there() {
        let mut browser = visited(&[
            ("https://github.com/timwuhaotian/tap", "tap"),
            ("https://example.com/", "Example"),
        ]);
        browser.begin_editing(true);
        browser.set_typed("git", false);
        let editing = browser.editing().expect("editing");
        assert_eq!(
            editing.ending.as_deref(),
            Some("hub.com"),
            "the front door the deep page credited"
        );
        assert_eq!(
            editing.offers.first().map(|o| o.key.as_str()),
            Some("github.com")
        );
        browser.submit();
        assert_eq!(
            browser.active_tab().map(Tab::url),
            Some("https://github.com")
        );
    }

    #[test]
    fn nothing_is_finished_while_an_input_method_composes() {
        let mut browser = visited(&[("https://example.com/", "Example")]);
        browser.begin_editing(false);
        browser.set_typed("exa", true);
        let editing = browser.editing().expect("editing");
        assert_eq!(editing.ending, None);
        assert!(!editing.offers.is_empty(), "rows are still offered");
        browser.set_typed("exa", false);
        assert_eq!(
            browser.editing().and_then(|e| e.ending.clone()).as_deref(),
            Some("mple.com")
        );
    }

    #[test]
    fn arrows_walk_the_rows_and_return_takes_the_picked_one() {
        let mut browser = visited(&[("https://docs.example.test/a", "A")]);
        browser.begin_editing(false);
        browser.set_typed("rust ownership", false);
        let offers = browser
            .editing()
            .map(|e| e.offers.clone())
            .unwrap_or_default();
        assert_eq!(
            offers.last().map(|o| o.kind),
            Some(history::Kind::Search),
            "words end with a search"
        );
        browser.pick(false);
        assert_eq!(
            browser.editing().and_then(|e| e.picked),
            None,
            "up from the field stays in it"
        );
        browser.pick(true);
        assert_eq!(browser.editing().and_then(|e| e.picked), Some(0));
        browser.submit();
        assert_eq!(
            browser.active_tab().map(Tab::url),
            Some(offers[0].url.as_str())
        );
    }

    #[test]
    fn tab_takes_the_ending_into_the_field() {
        let mut browser = visited(&[("https://example.com/", "Example")]);
        browser.begin_editing(false);
        browser.set_typed("ex", false);
        browser.take_dirty();
        assert!(browser.accept_ending());
        assert_eq!(
            browser.editing().map(|e| e.typed.as_str()),
            Some("example.com")
        );
        assert!(browser.take_dirty().0.contains(BrowserFields::RETYPED));
        assert!(!browser.accept_ending(), "nothing left to take");
    }

    #[test]
    fn a_tab_behind_whose_process_ended_is_made_again_when_shown() {
        let mut browser = with_pages(&["https://front.example", "https://behind.example"]);
        let (front, behind) = (browser.tabs()[0].id, browser.tabs()[1].id);
        browser.select(front);
        browser.take_effects();
        assert_eq!(
            browser.process_ended(behind, PageEnd::Memory, Instant::now()),
            Recovery::WhenShown
        );
        assert!(
            !browser.take_effects().contains(&Effect::Reload(behind)),
            "nothing reloads behind your back"
        );
        browser.select(behind);
        assert!(browser.take_effects().contains(&Effect::Reload(behind)));
        browser.select(front);
        browser.select(behind);
        assert!(
            !browser.take_effects().contains(&Effect::Reload(behind)),
            "once"
        );
    }

    #[test]
    fn a_tab_opened_only_for_a_download_goes_and_its_opener_comes_back() {
        let mut browser = with_pages(&["https://mail.example"]);
        let opener = browser.tabs()[0].id;
        browser.page_committed(opener);
        let fresh = browser.adopt(Some(opener), true);
        assert_eq!(browser.active(), Some(fresh));
        browser.drop_empty(fresh);
        assert_eq!(browser.tabs().len(), 1);
        assert_eq!(browser.active(), Some(opener));
        browser.reopen();
        assert_eq!(browser.tabs().len(), 1, "not offered back by ⇧⌘T");

        browser.drop_empty(opener);
        assert_eq!(browser.tabs().len(), 1, "a tab that has shown a page stays");
    }

    #[test]
    fn find_asks_the_page_in_front_and_follows_you_to_another_tab() {
        let mut browser = with_pages(&["https://a.example", "https://b.example"]);
        let (a, b) = (browser.tabs()[0].id, browser.tabs()[1].id);
        browser.select(a);
        browser.page_committed(a);
        browser.page_loading(a, false, 1.0);
        browser.take_effects();
        browser.open_find();
        browser.set_find_needle("tap");
        let effects = browser.take_effects();
        let Some(Effect::Find(ask)) = effects.iter().find(|e| matches!(e, Effect::Find(_))) else {
            panic!("asked the page: {effects:?}")
        };
        assert_eq!((ask.tab, ask.query.as_str()), (a, "tap"));
        let ask = ask.clone();
        browser.find_answered(
            &ask,
            find::Answer {
                count: Some(2),
                index: Some(1),
                found: true,
                available: true,
                ..find::Answer::default()
            },
        );
        assert_eq!(browser.find().status().as_deref(), Some("1 of 2"));

        browser.page_loading(b, false, 1.0);
        browser.select(b);
        let effects = browser.take_effects();
        assert!(
            effects.contains(&Effect::ClearFind {
                tab: a,
                generation: effects
                    .iter()
                    .find_map(|e| if let Effect::ClearFind { generation, .. } = e {
                        Some(*generation)
                    } else {
                        None
                    })
                    .unwrap_or(0)
            })
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Find(ask) if ask.tab == b && ask.query == "tap"))
        );

        browser.close_find();
        assert!(!browser.find().open());
        assert!(
            browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::ClearFind { tab, .. } if *tab == b))
        );
    }

    #[test]
    fn tabs_left_alone_long_enough_sleep_and_wake_where_they_were() {
        let mut browser = with_pages(&[
            "https://a.example",
            "https://b.example",
            "https://c.example",
        ]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        for id in &ids {
            browser.page_loading(*id, false, 1.0);
        }
        browser.select(ids[2]);
        browser.take_effects();
        let later = history::now() + 31.0 * 60.0;
        browser.sleep_idle(later, 30.0 * 60.0);
        let asked: Vec<Effect> = browser
            .take_effects()
            .into_iter()
            .filter(|e| matches!(e, Effect::Sleep(_)))
            .collect();
        // B was never looked at; A was left just now, when C came to the front.
        assert_eq!(
            asked,
            vec![Effect::Sleep(ids[1]), Effect::Sleep(ids[0])],
            "left longest first, not the one on screen"
        );
        assert_eq!(browser.awake_because(ids[2]), Some("on screen"));

        browser.slept(ids[0]);
        assert!(!browser.tab(ids[0]).is_some_and(Tab::awake));
        browser.select(ids[0]);
        assert!(
            browser.take_effects().contains(&Effect::Load {
                tab: ids[0],
                url: "https://a.example".into()
            }),
            "woken where it was"
        );

        browser.sleep_idle(history::now() + 60.0, 30.0 * 60.0);
        assert!(
            !browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Sleep(_))),
            "a tab just left counts from now"
        );
    }

    #[test]
    fn a_page_opened_from_the_one_on_screen_keeps_its_opener_awake() {
        let mut browser = with_pages(&["https://mail.example"]);
        let opener = browser.tabs()[0].id;
        browser.page_loading(opener, false, 1.0);
        let signin = browser.adopt(Some(opener), true);
        assert_eq!(browser.active(), Some(signin));
        assert_eq!(
            browser.awake_because(opener),
            Some("the page on screen came from it")
        );
    }

    // MARK: Settings › Tabs

    fn behaving(pages: &[&str], behaviour: Behaviour) -> Browser {
        let mut browser = with_pages(pages);
        browser.behaviour = behaviour;
        browser
    }

    #[test]
    fn a_link_from_another_app_goes_where_new_tab_position_says() {
        for (place, expected) in [
            (
                NewPlace::Next,
                [
                    "https://a.com",
                    "https://in.com",
                    "https://b.com",
                    "https://c.com",
                ],
            ),
            (
                NewPlace::Top,
                [
                    "https://in.com",
                    "https://a.com",
                    "https://b.com",
                    "https://c.com",
                ],
            ),
            (
                NewPlace::End,
                [
                    "https://a.com",
                    "https://b.com",
                    "https://c.com",
                    "https://in.com",
                ],
            ),
        ] {
            let mut browser = behaving(
                &["https://a.com", "https://b.com", "https://c.com"],
                Behaviour {
                    new_place: place,
                    ..Behaviour::default()
                },
            );
            browser.arrive(&url("https://in.com/"));
            let shown: Vec<&str> = urls(&browser)
                .into_iter()
                .map(|u| u.trim_end_matches('/'))
                .collect();
            assert_eq!(shown, expected, "{place:?}");
            assert_eq!(browser.active_tab().map(Tab::url), Some("https://in.com/"));
        }
    }

    #[test]
    fn new_tabs_tab_goes_at_the_end_unless_leftmost_is_chosen() {
        for (place, at) in [(NewPlace::Next, 2), (NewPlace::Top, 0)] {
            let mut browser = behaving(
                &["https://a.com", "https://b.com"],
                Behaviour {
                    new_place: place,
                    ..Behaviour::default()
                },
            );
            browser.begin_editing(true);
            browser.set_typed("https://new.com", false);
            browser.submit();
            assert_eq!(browser.tabs()[at].url(), "https://new.com", "{place:?}");
        }
    }

    #[test]
    fn a_background_link_waits_for_its_tab_when_asked() {
        let mut browser = behaving(
            &["https://a.com"],
            Behaviour {
                lazy: true,
                ..Behaviour::default()
            },
        );
        let opener = browser.tabs()[0].id;
        let link = browser.open(Some(&url("https://later.com")), false, Some(opener));
        assert!(!browser.tab(link).is_some_and(Tab::awake));
        assert!(
            !browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Load { .. })),
            "nothing loads behind the page"
        );
        browser.select(link);
        assert!(
            browser
                .take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Load { tab, .. } if *tab == link)),
            "it loads when shown"
        );

        let now = browser.open(Some(&url("https://now.com")), true, Some(opener));
        assert!(
            browser.tab(now).is_some_and(Tab::awake),
            "⇧⌘-click loads at once"
        );
    }

    #[test]
    fn closing_goes_back_to_the_tab_viewed_before_when_asked() {
        let pages = [
            "https://a.com",
            "https://b.com",
            "https://c.com",
            "https://d.com",
        ];
        let mut browser = behaving(&pages, Behaviour::default());
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[3]);
        browser.select(ids[1]);
        browser.close(ids[1]);
        assert_eq!(browser.active(), Some(ids[2]), "the neighbour, as before");

        let mut browser = behaving(
            &pages,
            Behaviour {
                close_goes_back: true,
                ..Behaviour::default()
            },
        );
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[3]);
        browser.select(ids[1]);
        browser.close(ids[1]);
        assert_eq!(browser.active(), Some(ids[3]), "the one viewed before");
        browser.close(ids[3]);
        assert_eq!(browser.active(), Some(ids[0]), "and the one before that");
    }

    #[test]
    fn settings_closed_goes_back_whatever_the_switch_says() {
        let mut browser = with_pages(&["https://a.com", "https://b.com", "https://c.com"]);
        let ids: Vec<TabId> = browser.tabs().iter().map(|t| t.id).collect();
        browser.select(ids[0]);
        browser.open_page(Page::Settings);
        let settings = browser.active().expect("settings");
        browser.close(settings);
        assert_eq!(browser.active(), Some(ids[0]));
    }

    #[test]
    fn the_last_loose_tab_beside_groups_leaves_a_new_tab() {
        let mut browser = Browser::restored(grouped());
        let loose: Vec<TabId> = browser
            .tabs()
            .iter()
            .filter(|t| t.group().is_none())
            .map(|t| t.id)
            .collect();
        browser.close(loose[1]);
        browser.select(loose[0]);
        browser.close(loose[0]);
        assert!(
            browser.active_tab().is_some_and(Tab::is_blank),
            "a new tab, not a group's page"
        );
        assert_eq!(browser.groups().len(), 2, "the groups stay as they were");
        assert!(browser.editing().is_some());
    }

    #[test]
    fn a_new_tab_is_left_when_only_pins_would_be_when_asked() {
        let shape = r#"{"tabs":[{"url":"https://pin.com","title":"P","pin":"P"},{"url":"https://a.com","title":"A"}],"active":1}"#;
        for (after_last, blank) in [(false, false), (true, true)] {
            let mut browser =
                Browser::restored(session::Shape::decode(shape.as_bytes()).expect("decodes"));
            browser.behaviour.new_after_last = after_last;
            let last = browser.active().expect("a tab");
            browser.close(last);
            assert_eq!(browser.active_tab().is_some_and(Tab::is_blank), blank);
        }
    }

    #[test]
    fn settings_or_new_tab_alone_in_the_field_go_there_when_asked() {
        let mut browser = with_pages(&["https://a.com"]);
        browser.begin_editing(false);
        browser.set_typed("settings", false);
        browser.submit();
        assert_ne!(
            browser.active_tab().and_then(Tab::page),
            Some(Page::Settings),
            "off: it searches"
        );

        let mut browser = behaving(
            &["https://a.com"],
            Behaviour {
                commands: true,
                ..Behaviour::default()
            },
        );
        browser.begin_editing(false);
        browser.set_typed("Settings ", false);
        browser.submit();
        assert_eq!(
            browser.active_tab().and_then(Tab::page),
            Some(Page::Settings)
        );

        browser.begin_editing(false);
        browser.set_typed("new tab", false);
        browser.submit();
        assert!(browser.active_tab().is_some_and(Tab::is_blank));
        assert_eq!(browser.tabs().len(), 3);

        browser.set_typed("settings for gmail", false);
        browser.submit();
        assert_ne!(
            browser.active_tab().and_then(Tab::page),
            Some(Page::Settings),
            "only the whole"
        );
    }

    #[test]
    fn tabs_left_alone_close_unless_kept_and_reopen_brings_them_back() {
        let mut browser = Browser::restored(grouped());
        browser.take_effects();
        browser.close_left_alone(history::now() + 1e9);
        assert!(browser.take_effects().is_empty(), "never, unless chosen");

        browser.behaviour.close_after = Some(12.0 * 3600.0);
        let later = history::now() + 13.0 * 3600.0;
        browser.close_left_alone(later);
        let due: Vec<TabId> = browser
            .take_effects()
            .into_iter()
            .filter_map(|e| match e {
                Effect::Retire { tab, .. } => Some(tab),
                _ => None,
            })
            .collect();
        let loose: Vec<TabId> = browser
            .tabs()
            .iter()
            .filter(|t| t.group().is_none())
            .map(|t| t.id)
            .collect();
        assert_eq!(due, loose, "grouped tabs and the one in front stay");
        for tab in due {
            browser.retire(tab, later - 12.0 * 3600.0);
        }
        assert_eq!(browser.tabs().len(), 3);
        browser.reopen();
        assert_eq!(browser.tabs().len(), 4);
    }
}
