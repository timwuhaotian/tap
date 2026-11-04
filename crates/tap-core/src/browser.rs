//! One window's tabs and what is being typed: the model half of Search's `Browser.swift`
//! and `Tab.swift` (docs/PLAN.md §5.1).
//!
//! Setters mark which fields changed; the shell reads the marks once per turn and
//! redraws only what they name. Anything that has to happen to a web view is pushed as
//! an [`Effect`] and carried out by the shell after the model is let go of.

use std::time::Instant;

use bitflags::bitflags;

use crate::{
    address,
    crash::{CrashCount, PageEnd},
    engine::{self, Engine},
    find::{self, Find},
    fnd::Url,
    history::{self, History, Suggestion},
    keyword::{self, Keyword},
    session,
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

    /// What the tab says: the name you gave it, the page's title, the address until
    /// there is a title, or "New Tab" (Search's `Tab.label`).
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(name) = self.entry.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_owned();
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
            closed: Vec::new(),
            link_run: None,
            refusals: 0,
            effects: Vec::new(),
            dirty: BrowserFields::all(),
            session_changed: false,
            history: History::default(),
            history_changed: false,
            find: Find::default(),
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
        for entry in shape.tabs {
            let id = TabId(browser.next);
            browser.next += 1;
            let mut tab = Tab::new(id);
            tab.url.clone_from(&entry.url);
            tab.title.clone_from(&entry.title);
            // Sessions written before tabs kept it start counting at launch.
            tab.touched = entry.touched.unwrap_or(tab.touched);
            tab.entry = entry;
            browser.tabs.push(tab);
        }
        let front = usize::try_from(shape.active)
            .ok()
            .and_then(|i| browser.tabs.get(i))
            .map(|t| t.id);
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

    /// The session as Search writes it.
    #[must_use]
    pub fn shape(&self) -> session::Shape {
        let active = self
            .active
            .and_then(|id| self.index(id))
            .map_or(-1, |i| i64::try_from(i).unwrap_or(-1));
        session::Shape {
            tabs: self
                .tabs
                .iter()
                .filter(|t| !t.is_blank())
                .map(Tab::entry)
                .collect(),
            active: if self.active_tab().is_some_and(Tab::is_blank) {
                -1
            } else {
                active
            },
            ..session::Shape::default()
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
        let url = tab.url.clone();
        self.effects.push(Effect::Load { tab: id, url });
    }

    /// Opens an address in a new tab: after a run of links from `opener`, or at the
    /// end of the row.
    pub fn open(&mut self, url: Option<&Url>, foreground: bool, opener: Option<TabId>) -> TabId {
        let id = TabId(self.next);
        self.next += 1;
        let mut tab = Tab::new(id);
        tab.opener = opener;
        if let Some(url) = url {
            url.as_str().clone_into(&mut tab.url);
            tab.awake = true;
            self.effects.push(Effect::Load {
                tab: id,
                url: tab.url.clone(),
            });
        }
        let place = opener
            .and_then(|from| {
                let after = match self.link_run {
                    Some(run) if run.opener == from && self.index(run.last).is_some() => run.last,
                    _ => from,
                };
                self.index(after).map(|i| i + 1)
            })
            .unwrap_or(self.tabs.len());
        self.tabs.insert(place, tab);
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
    /// tab. The only tab is replaced by a new one, unless it is already new.
    pub fn close(&mut self, id: TabId) {
        let Some(index) = self.index(id) else { return };
        if self.tabs.len() == 1 {
            if self.tabs[0].is_blank() {
                return;
            }
            self.remember(index);
            self.effects.push(Effect::Discard(id));
            let fresh = TabId(self.next);
            self.next += 1;
            self.tabs[0] = Tab::new(fresh);
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
        self.dirty.insert(BrowserFields::TABS);
        self.session_changed = true;
        if self.active == Some(id) {
            let next = self.tabs[index.min(self.tabs.len() - 1)].id;
            self.active = None;
            self.select(next);
        }
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
            self.open(Some(&url), true, None);
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
        self.session_changed = true;
        self.effects.push(Effect::Load {
            tab: id,
            url: url.as_str().to_owned(),
        });
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
                self.open(Some(url), true, None);
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
        if self.active_tab().is_none_or(Tab::is_blank) {
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
        let Some(id) = self.active else { return };
        let Some(tab) = self.tab_mut(id) else { return };
        if tab.url.is_empty() {
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
}
