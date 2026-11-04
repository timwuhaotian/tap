//! Every line of Settings, written down once (Search's `SettingsIndex.swift`).
//!
//! Settings draws each page's groups from this list, its find field filters the same
//! list, and a profile export carries exactly the keys these rows read and write (plus
//! the per-site shield pauses), so a setting can't travel that Settings doesn't show
//! (docs/PLAN.md §7). Keys and their values are Search's own, so a Search profile's
//! settings read as they are.
//!
//! Every row of Search's is here but translation's (dropped, docs/PLAN.md §2). Each says
//! whether Tap acts on it yet ([`live`]). Settings shows only the rows Tap acts on; the
//! others' values are still kept under their keys and still travel in a profile, ready
//! for the feature behind them.

use std::sync::OnceLock;

use serde_json::Value;

/// A page of Settings, down the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    General,
    Appearance,
    Tabs,
    Browsing,
    Spaces,
    Passwords,
    Privacy,
    Extensions,
    Keyboard,
    About,
}

impl Page {
    /// The rail's three blocks: the browser, your data and tools, About. Search's AI page
    /// isn't shipped (`AI.shipped = false`), so it isn't here either.
    pub const BLOCKS: [&'static [Page]; 3] = [
        &[
            Page::General,
            Page::Appearance,
            Page::Tabs,
            Page::Browsing,
            Page::Spaces,
        ],
        &[
            Page::Passwords,
            Page::Privacy,
            Page::Extensions,
            Page::Keyboard,
        ],
        &[Page::About],
    ];

    pub fn all() -> impl Iterator<Item = Page> {
        Page::BLOCKS.into_iter().flatten().copied()
    }

    #[must_use]
    pub fn raw(self) -> &'static str {
        match self {
            Page::General => "general",
            Page::Appearance => "appearance",
            Page::Tabs => "tabs",
            Page::Browsing => "browsing",
            Page::Spaces => "spaces",
            Page::Passwords => "passwords",
            Page::Privacy => "privacy",
            Page::Extensions => "extensions",
            Page::Keyboard => "keyboard",
            Page::About => "about",
        }
    }

    /// A page by its name, the old names included (Search's `Page.named`).
    #[must_use]
    pub fn named(name: &str) -> Option<Page> {
        match name.to_lowercase().as_str() {
            "downloads" => Some(Page::General),
            "shortcuts" | "mouse" => Some(Page::Keyboard),
            other => Page::all().find(|p| p.raw() == other),
        }
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Tabs => "Tabs",
            Page::Browsing => "Browsing",
            Page::Spaces => "Spaces & Pins",
            Page::Passwords => "Passwords",
            Page::Privacy => "Privacy",
            Page::Extensions => "Extensions",
            Page::Keyboard => "Keyboard & Mouse",
            Page::About => "About",
        }
    }

    /// Its SF Symbol.
    #[must_use]
    pub fn icon(self) -> &'static str {
        match self {
            Page::General => "gearshape",
            Page::Appearance => "paintbrush",
            Page::Tabs => "square.on.square",
            Page::Browsing => "globe",
            Page::Spaces => "square.stack",
            Page::Passwords => "key",
            Page::Privacy => "hand.raised",
            Page::Extensions => "puzzlepiece.extension",
            Page::Keyboard => "keyboard",
            Page::About => "info.circle",
        }
    }

    fn order(self) -> usize {
        Page::all().position(|p| p == self).unwrap_or(0)
    }
}

/// A titled group inside a page: a small heading with its icon, then its rows in a card.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub id: &'static str,
    pub page: Page,
    pub title: &'static str,
    pub icon: &'static str,
}

/// What sits in a row, on its right or under its words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Control {
    /// A switch over one key.
    Switch(&'static str),
    /// A switch that shows its key the other way round (`sites.keep`: on keeps sign-ins,
    /// the switch says "prevent tracking").
    SwitchOff(&'static str),
    /// Choices side by side over one key.
    Segments(&'static str, &'static [(&'static str, &'static str)]),
    /// A pop-up menu over one key.
    Menu(&'static str, &'static [(&'static str, &'static str)]),
    /// A pop-up menu over a whole number (`tabs.close.after`, in hours).
    Hours(&'static str, &'static [(i64, &'static str)]),
    /// Page zoom, 50% to 300% in steps of 5.
    Zoom,
    /// One choice made of several keys (Search's merged controls).
    Merged(Merged),
    /// Buttons that do something now, by what they do.
    Buttons(&'static [(&'static str, &'static str)]),
    /// The engine menu, and the address of a custom one under it.
    Engine,
    /// The site shortcuts, listed under the row.
    Keywords,
    /// The downloads folder, and Change….
    Folder,
    /// Always there: no control, said so.
    Note,
    Nothing,
}

/// The controls that read and write more than one key (Search's `TabsLayout`,
/// `SleepChoice`, `FloatOwn`, `pinsKeepSite` and `PeekChoice`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merged {
    /// `sidebar`, `sidebar.position`, `sidebar.icons`.
    TabsLayout,
    /// `tabs.sleep`, `tabs.sleep.pace`.
    Sleep,
    /// `float.leave`, `float.away`.
    FloatOwn,
    /// `pins.home`, `pins.peek`: a switch.
    PinsKeep,
    /// `links.peek`, `links.peekClick`.
    Peek,
}

impl Merged {
    /// Its choices, raw value and title; empty for a switch.
    #[must_use]
    pub fn options(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Merged::TabsLayout => &[
                ("top", "Top"),
                ("left", "Left"),
                ("right", "Right"),
                ("icons", "Icons only"),
            ],
            Merged::Sleep => &[
                ("never", "Never"),
                ("later", "Later"),
                ("normal", "Normal"),
                ("sooner", "Sooner"),
            ],
            Merged::FloatOwn => &[
                ("never", "Never"),
                ("tabs", "Switching tabs"),
                ("tabsOrApps", "Tabs or apps"),
            ],
            Merged::PinsKeep => &[],
            Merged::Peek => &[
                ("off", "Off"),
                ("shift", "Shift-click"),
                ("optionShift", "Option-Shift-click"),
                ("optionCommand", "Option-Command-click"),
            ],
        }
    }

    /// Whether it is drawn as a pop-up menu rather than side by side.
    #[must_use]
    pub fn is_menu(self) -> bool {
        self == Merged::Peek
    }
}

/// How a row is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The control on the right of the row.
    Control,
    /// A control wide enough to go under the words.
    Wide,
    /// No control: a feature that is always there. Only the find field shows it.
    Note,
}

/// One row of Settings.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: &'static str,
    pub group: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    /// One short line.
    pub detail: &'static str,
    /// The rest of the story.
    pub more: Option<&'static str>,
    /// The defaults keys the row reads and writes.
    pub keys: &'static [&'static str],
    /// Other words someone might type for it.
    pub words: &'static [&'static str],
    pub kind: Kind,
    /// The row that makes this one count (see [`applies`]).
    pub parent: Option<&'static str>,
    /// Only the find field shows it.
    pub find_only: bool,
    pub control: Control,
}

impl Item {
    fn new(
        id: &'static str,
        group: &'static str,
        icon: &'static str,
        title: &'static str,
        detail: &'static str,
    ) -> Item {
        Item {
            id,
            group,
            icon,
            title,
            detail,
            more: None,
            keys: &[],
            words: &[],
            kind: Kind::Control,
            parent: None,
            find_only: false,
            control: Control::Nothing,
        }
    }
    fn more(mut self, more: &'static str) -> Item {
        self.more = Some(more);
        self
    }
    fn keys(mut self, keys: &'static [&'static str]) -> Item {
        self.keys = keys;
        self
    }
    fn words(mut self, words: &'static [&'static str]) -> Item {
        self.words = words;
        self
    }
    fn wide(mut self) -> Item {
        self.kind = Kind::Wide;
        self
    }
    fn under(mut self, parent: &'static str) -> Item {
        self.parent = Some(parent);
        self
    }
    fn note(mut self) -> Item {
        self.kind = Kind::Note;
        self.find_only = true;
        self.control = Control::Note;
        self
    }
    fn find_only(mut self) -> Item {
        self.find_only = true;
        self
    }
    fn control(mut self, control: Control) -> Item {
        self.control = control;
        self
    }

    /// Its page, through its group.
    #[must_use]
    pub fn page(&self) -> Page {
        group(self.group).map_or(Page::General, |g| g.page)
    }
}

const fn g(id: &'static str, page: Page, title: &'static str, icon: &'static str) -> Group {
    Group {
        id,
        page,
        title,
        icon,
    }
}

/// Every group, in page order.
pub const GROUPS: [Group; 25] = [
    g("browser", Page::General, "Your browser", "globe.desk"),
    g("downloads", Page::General, "Downloads", "arrow.down.circle"),
    g(
        "theme",
        Page::Appearance,
        "Light and dark",
        "circle.lefthalf.filled",
    ),
    g("window", Page::Appearance, "Window", "macwindow"),
    g("tablook", Page::Appearance, "Tabs", "textformat.size"),
    g("motion", Page::Appearance, "Motion", "wind"),
    g("newtabs", Page::Tabs, "New tabs", "plus.square"),
    g("memory", Page::Tabs, "Memory", "moon.zzz"),
    g("tabbar", Page::Tabs, "In the tab bar", "number"),
    g("search", Page::Browsing, "Searching", "magnifyingglass"),
    g("links", Page::Browsing, "Links", "link"),
    g("reading", Page::Browsing, "Reading mode", "doc.plaintext"),
    g("video", Page::Browsing, "Video", "play.rectangle"),
    g("pages", Page::Browsing, "Pages", "doc.richtext"),
    g(
        "arc",
        Page::Spaces,
        "Coming from Arc?",
        "arrow.down.forward.square",
    ),
    g(
        "spaces",
        Page::Spaces,
        "Spaces and profiles",
        "rectangle.3.group",
    ),
    g("pins", Page::Spaces, "Pins", "pin"),
    g("opening", Page::Spaces, "Opening and starting", "command"),
    g("passwords", Page::Passwords, "Your passwords", "key"),
    g(
        "apple",
        Page::Passwords,
        "Apple Passwords",
        "lock.rectangle.stack",
    ),
    g("blocking", Page::Privacy, "Blocking", "shield"),
    g(
        "permissions",
        Page::Privacy,
        "Site permissions",
        "checkmark.shield",
    ),
    g("clear", Page::Privacy, "Clear", "trash"),
    g(
        "mouse",
        Page::Keyboard,
        "Mouse and trackpad",
        "computermouse",
    ),
    g(
        "updates",
        Page::About,
        "Updates",
        "arrow.triangle.2.circlepath",
    ),
];

/// The developer group sits after updates on About.
pub const DEVELOPER: Group = g("developer", Page::About, "Developer", "hammer");

/// A group by its id.
#[must_use]
pub fn group(id: &str) -> Option<Group> {
    GROUPS
        .iter()
        .chain(std::iter::once(&DEVELOPER))
        .find(|g| g.id == id)
        .copied()
}

/// A page's groups, in order.
#[must_use]
pub fn groups_of(page: Page) -> Vec<Group> {
    GROUPS
        .iter()
        .chain(std::iter::once(&DEVELOPER))
        .filter(|g| g.page == page)
        .copied()
        .collect()
}

fn group_order(id: &str) -> usize {
    GROUPS
        .iter()
        .chain(std::iter::once(&DEVELOPER))
        .position(|g| g.id == id)
        .unwrap_or(0)
}

/// A group's rows as its card shows them: the find field's own rows left out.
#[must_use]
pub fn items_in(group: &str) -> Vec<&'static Item> {
    items()
        .iter()
        .filter(|i| i.group == group && !i.find_only)
        .collect()
}

/// A row by its id.
#[must_use]
pub fn item(id: &str) -> Option<&'static Item> {
    items().iter().find(|i| i.id == id)
}

/// Every row, in page order.
#[allow(clippy::too_many_lines)] // one table, read top to bottom as Settings is
pub fn items() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        vec![
            // General
            Item::new("general.default", "browser", "link.badge.plus", "Open links from other apps",
                "Make Tap the browser Mail, Slack and the rest send links to")
                .words(&["default browser"])
                .control(Control::Buttons(&[("default", "Make default")])),
            Item::new("general.import", "browser", "square.and.arrow.down.on.square", "Bring things over",
                "Passwords, bookmarks, history and open tabs from another browser on this Mac, or from a file")
                .more("Search, Chrome, Arc, Brave, Edge, Dia and the other Chromium browsers are found on their own, and nothing in them changes; macOS asks once for a browser's key before its passwords come. Export writes an offline file you can AirDrop to your other Mac; open it there to bring it all in. Passwords and cookies are never carried in it.")
                .words(&["import", "export", "profile", "chrome", "safari", "arc", "brave", "edge", "search", "migrate", "airdrop", "passwords", "csv"])
                .control(Control::Buttons(&[("import", "Bring Over"), ("export", "Export Tap Profile…")])),
            Item::new("quit.confirm", "browser", "power", "Confirm before quitting",
                "Ask before ⌘Q or Quit Tap closes the browser")
                .keys(&["quit.confirm"]).words(&["quit", "cmd q"])
                .control(Control::Switch("quit.confirm")),
            Item::new("downloads.folder", "downloads", "folder", "Save to", "")
                .keys(&["downloads"]).words(&["download folder"])
                .control(Control::Folder),
            Item::new("downloads.ask", "downloads", "questionmark.folder", "Ask where to save each file",
                "A save sheet for every download")
                .keys(&["downloads.ask"]).control(Control::Switch("downloads.ask")),
            Item::new("downloads.button", "downloads", "arrow.down.to.line", "Always show the downloads button",
                "Off, it shows only while a file comes in")
                .keys(&["downloads.button"]).control(Control::Switch("downloads.button")),

            // Appearance
            Item::new("look", "theme", "circle.lefthalf.filled", "Appearance",
                "Light, dark, or whatever the Mac is doing. Pages follow it too")
                .keys(&["look"]).words(&["dark mode", "light mode", "theme", "system"]).wide()
                .control(Control::Segments("look", &[("light", "Light"), ("dark", "Dark"), ("system", "Auto")])),
            Item::new("pages.dusk", "theme", "moon", "Darken pages that stay light",
                "Pages with no dark look of their own are darkened too")
                .more("Sites with a dark look of their own already follow it. ⇧⌘D for the site you're on, ⌥⇧⌘D to pause everywhere.")
                .keys(&["pages.dusk"]).words(&["dark", "dusk", "night"]).under("look")
                .control(Control::Switch("pages.dusk")),
            Item::new("tabs.layout", "window", "sidebar.left", "Tabs",
                "Across the top of the window, down a side as a column, or as icons only")
                .more("Icons only is a narrow column of each tab's icon, the title in a tooltip. Nothing grows under the pointer.")
                .keys(&["sidebar", "sidebar.position", "sidebar.icons"])
                .words(&["sidebar", "vertical tabs", "column", "left", "right", "top", "tab bar", "position",
                    "icons only", "compact", "collapsed", "narrow", "favicons"]).wide()
                .control(Control::Merged(Merged::TabsLayout)),
            Item::new("sidebar.hides", "window", "sidebar.squares.left", "Hide the column until the pointer reaches the edge",
                "The page takes the whole window; ⌘S keeps the tabs out")
                .keys(&["sidebar.hides"]).words(&["auto hide", "sidebar"]).under("tabs.layout")
                .control(Control::Switch("sidebar.hides")),
            Item::new("layout.cards", "window", "rectangle.inset.filled", "The page as a card",
                "A little room around the page, with rounded corners")
                .more("Off, the page reaches the window's edges and the column is white with its hairline.")
                .keys(&["layout.cards"]).words(&["edge to edge", "rounded", "margin"]).wide().under("tabs.layout")
                .control(Control::Segments("layout.cards", &[("true", "As a card"), ("false", "Edge to edge")])),
            Item::new("toolbar.left", "window", "arrow.left.arrow.right", "Back, forward and reload on the left",
                "Beside the window's buttons, before the tabs")
                .keys(&["toolbar.left"]).words(&["toolbar", "buttons", "navigation"]).under("tabs.layout")
                .control(Control::Switch("toolbar.left")),
            // Tap's own row: Search has no address bar, its live tab says where it is.
            Item::new("toolbar.address", "window", "link", "Show the address bar",
                "Under the tabs: where the page is, and whether its connection is secure")
                .more("Click the address to type another, or the lock to see the site's certificate. ⌘L goes there too.")
                .keys(&["toolbar.address"]).words(&["url", "address bar", "location", "toolbar", "ssl", "https", "lock", "certificate"])
                .control(Control::Switch("toolbar.address")),
            Item::new("sidebar.roomy", "tablook", "textformat.size.larger", "Roomier tabs",
                "Taller rows and larger titles in the column")
                .keys(&["sidebar.roomy"]).words(&["bigger", "larger", "text size", "accessibility"]).under("tabs.layout")
                .control(Control::Switch("sidebar.roomy")),
            Item::new("glyph", "tablook", "app", "Tabs show", "Beside the title, and on a pinned square")
                .keys(&["glyph"]).words(&["favicon", "icons", "letters"])
                .control(Control::Segments("glyph", &[("letters", "Letters"), ("icons", "Site icons")])),
            Item::new("tabs.reading", "tablook", "text.line.first.and.arrowtriangle.forward", "Show how far you've read",
                "The tab you're on fills with grey as you scroll down")
                .keys(&["tabs.reading"]).words(&["progress", "scroll"]).control(Control::Switch("tabs.reading")),
            Item::new("tabs.give", "tablook", "hand.point.up.left", "Tabs give when pressed",
                "A tab sinks a little under the pointer, as in Arc")
                .keys(&["tabs.give"]).words(&["press", "animation", "bounce"]).control(Control::Switch("tabs.give")),
            Item::new("bookmarks.bar", "tablook", "bookmark", "Show the bookmarks bar",
                "Your bookmarks in a row above the page")
                .keys(&["bookmarks.bar"]).words(&["favorites bar"]).control(Control::Switch("bookmarks.bar")),
            Item::new("motion.light", "motion", "leaf", "Reduce animations in Low Power Mode",
                "Quicker, simpler motion while the Mac saves power")
                .more("In Low Power Mode, or while the Mac runs hot. Pages are left as they are.")
                .keys(&["motion.light"]).words(&["battery", "animations", "motion"])
                .control(Control::Switch("motion.light")),

            // Tabs
            Item::new("tabs.newPlace", "newtabs", "arrow.right.to.line", "New tab position",
                "Where a link or ⌘T's new tab goes in the row")
                .more("After current puts a link beside the tab you're on, and ⌘T's new tab at the end. Pins stay first; a link from a group stays in it.")
                .keys(&["tabs.newPlace"]).words(&["new tab", "order"])
                .control(Control::Menu("tabs.newPlace", &[("next", "After current"), ("top", "Leftmost"), ("end", "Rightmost")])),
            Item::new("tabs.newAfterLast", "newtabs", "plus.rectangle.on.rectangle", "Leave a new tab when the last tab closes",
                "Otherwise you land on a pinned tab")
                .keys(&["tabs.newAfterLast"]).words(&["last tab"]).control(Control::Switch("tabs.newAfterLast")),
            Item::new("tabs.closeGoesBack", "newtabs", "arrow.uturn.backward", "Closing a tab goes back to the one before",
                "Otherwise you land on the tab beside it")
                .more("The tab you viewed last in this space, as ⌃Tab would take you. The other half of a Split View stays first.")
                .keys(&["tabs.closeGoesBack"]).words(&["close", "previous tab", "recent"])
                .control(Control::Switch("tabs.closeGoesBack")),
            Item::new("tabs.lazy", "newtabs", "hourglass", "Load background tabs when you go to them",
                "A link opened behind the page waits until you go to its tab")
                .more("With ⌘-click, the middle button, or a batch of links from another app. ⇧⌘-click still takes you there at once.")
                .keys(&["tabs.lazy"]).words(&["lazy", "background"]).control(Control::Switch("tabs.lazy")),
            Item::new("tabs.popupWindows", "newtabs", "macwindow.on.rectangle", "Open sized pop-ups in small windows",
                "A site asking for a window at its own size gets one, not a tab")
                .more("A call, a sign-in or a chat asking for a window its size gets one, as in Safari. Pop-up blocking still applies.")
                .keys(&["tabs.popupWindows"]).words(&["popup", "pop-up", "window"])
                .control(Control::Switch("tabs.popupWindows")),
            Item::new("tabs.sleep", "memory", "moon.zzz", "Sleep tabs you aren't using",
                "A resting tab lets its page go and comes back where you left it")
                .more("Pinned tabs, sound, calls and anything typed stay awake. Right-click a site's card to keep it awake.")
                .keys(&["tabs.sleep", "tabs.sleep.pace"]).words(&["memory", "discard", "suspend", "ram"]).wide()
                .control(Control::Merged(Merged::Sleep)),
            Item::new("tabs.close.after", "memory", "timer", "Close tabs you leave alone",
                "Pinned, named and grouped tabs stay. ⇧⌘T brings one back")
                .more("Counted from when you last left the tab, while Tap is quit too. Private tabs, sound, calls and anything typed stay.")
                .keys(&["tabs.close.after"]).words(&["auto close", "archive", "clean up"])
                .control(Control::Hours("tabs.close.after", &[(0, "Never"), (12, "After 12 hours"), (24, "After a day"),
                    (168, "After a week"), (720, "After a month")])),
            Item::new("tabs.shortcuts", "tabbar", "command", "Show tab numbers while ⌘ is held",
                "Which tab each ⌘1–9 selects")
                .keys(&["tabs.shortcuts"]).words(&["numbers", "shortcut"]).control(Control::Switch("tabs.shortcuts")),
            Item::new("tabs.address", "tabbar", "lock", "Show the address on the tab you're on",
                "Its site in place of its title; the title comes back under the pointer")
                .more("Look-alike names in other alphabets show as they really are, and plain http says Not secure. Off, the tab you're on still says so for those two.")
                .keys(&["tabs.address"]).words(&["url", "domain", "security", "address bar"])
                .control(Control::Switch("tabs.address")),

            // Browsing
            Item::new("search.engine", "search", "magnifyingglass", "Search with", "Where words that aren't an address go")
                .keys(&["search.engine", "search.custom"]).words(&["google", "duckduckgo", "engine", "kagi", "bing"])
                .control(Control::Engine),
            Item::new("search.suggest", "search", "text.magnifyingglass", "Suggestions from your search engine",
                "What you type is sent to your engine as you type")
                .more("Its suggestions show under the address field. Never from a private tab, and never an address.")
                .keys(&["search.suggest"]).words(&["autocomplete", "suggest"]).control(Control::Switch("search.suggest")),
            Item::new("search.keywords", "search", "character.cursor.ibeam", "Site shortcuts",
                "A word before your search sends it to that site: \"yt cats\"")
                .keys(&["search.keywords"]).words(&["keyword", "bang"]).control(Control::Keywords),
            Item::new("links.little", "links", "rectangle.on.rectangle", "Open links from other apps in a small window",
                "To read and close, or keep with Open in Tap (⌘O)")
                .keys(&["links.little"]).words(&["little arc", "mini window"]).control(Control::Switch("links.little")),
            Item::new("reader.twins", "reading", "doc.text", "Read the site's own Markdown",
                "When a page has a Markdown copy, reading mode shows that")
                .more("⇧⌘R shows a page's article alone, in clean type. Most docs sites publish a Markdown copy; nothing in it can run, its text is escaped.")
                .keys(&["reader.twins"]).words(&["markdown", "reader"]).control(Control::Switch("reader.twins")),
            Item::new("reader.follows", "reading", "circle.righthalf.filled", "Reading mode follows light and dark",
                "In a dark window the article is light on dark")
                .more("A small switch in its corner makes an article light, dark or as the window is, kept for the next one.")
                .keys(&["reader.follows"]).words(&["reader", "dark"]).control(Control::Switch("reader.follows")),
            Item::new("float.style", "video", "pip", "The floating video", "Tap's own window, or macOS's picture in picture")
                .more("Tap's window floats a video without a click and keeps its page alive in it. Picture in picture is the one Safari uses, with the system's controls.")
                .keys(&["float.style"]).words(&["pip", "picture in picture", "mini player"])
                .control(Control::Segments("float.style", &[("search", "Tap's window"), ("system", "Picture in picture")])),
            Item::new("float.own", "video", "pip.exit", "Float the video on its own",
                "A playing video comes out when you leave its tab, or Tap")
                .more("It goes back into its tab when you return. ⇧⌘P still floats one by hand.")
                .keys(&["float.leave", "float.away"]).words(&["pip", "picture in picture", "switch apps"]).wide()
                .control(Control::Merged(Merged::FloatOwn)),
            Item::new("media.click", "video", "play.slash", "Videos wait for a click",
                "Videos don't start by themselves, even without sound")
                .keys(&["media.click"]).words(&["autoplay", "auto-play"]).control(Control::Switch("media.click")),
            Item::new("fullscreen.escapeTwice", "video", "escape", "Press Esc twice to leave full screen",
                "The first press shows a reminder, as in Arc")
                .keys(&["fullscreen.escapeTwice"]).words(&["escape", "full screen"])
                .control(Control::Switch("fullscreen.escapeTwice")),
            Item::new("pageZoom", "pages", "plus.magnifyingglass", "Page zoom",
                "Where every site starts. ⌘+ and ⌘− are kept for each site")
                .keys(&["pageZoom"]).words(&["zoom", "size", "scale"]).control(Control::Zoom),
            Item::new("pages.120", "pages", "speedometer", "Pages at 120 Hz",
                "Smoother scrolling on screens that can. On power spares the battery")
                .more("Animations and scrolling in pages at up to 120 frames a second, instead of 60 as in Safari.")
                .keys(&["pages.120.mode", "pages.120"]).words(&["promotion", "frame rate", "fps", "smooth", "120"])
                .control(Control::Segments("pages.120.mode", &[("never", "Never"), ("onPower", "On power"), ("always", "Always")])),

            // Spaces & Pins
            Item::new("arc.setup", "arc", "wand.and.stars", "Set Up Like Arc",
                "Tabs on the left, spaces, pinned rows and pins that keep their site")
                .more("Also turns on New tab in a floating field. Undo puts back what you had.")
                .words(&["arc", "the browser company"])
                .control(Control::Buttons(&[("arc", "Set Up")])),
            Item::new("arc.import", "arc", "square.and.arrow.down", "Bring Arc's spaces and pins over",
                "Your spaces, pinned tabs and folders, read from Arc on this Mac")
                .words(&["arc", "import"]).control(Control::Buttons(&[("arc.import", "Bring Over…")])),
            Item::new("spaces", "spaces", "square.stack.3d.up", "Spaces",
                "Separate sets of tabs, switched with ⌃1–⌃9 or a swipe")
                .more("Each space can be signed in where the others are or start afresh. Mission Control's own ⌃1–⌃9, if you turned them on, take those keys first.")
                .keys(&["spaces"]).words(&["workspaces", "profiles"]).control(Control::Switch("spaces")),
            Item::new("profiles", "spaces", "person.2", "Profiles", "Spaces with their own sign-ins, history and bookmarks")
                .more("In windows of their own. Extensions, passwords and settings are shared.")
                .words(&["profile", "accounts", "work", "personal"])
                .control(Control::Buttons(&[("profile.new", "New Profile…")])),
            Item::new("pins.keep", "pins", "pin.circle", "Pins keep their site",
                "Links elsewhere open over the pin, and ⌘W takes it back home")
                .more("As in Arc and Safari: a link from a pin to another site opens in a peek over it, and a pin closed with ⌘W, or left open when Tap quits, comes back at the page it was pinned at.")
                .keys(&["pins.home", "pins.peek"]).words(&["pinned tabs", "go home", "reset pin"])
                .control(Control::Merged(Merged::PinsKeep)),
            Item::new("pins.list", "pins", "list.bullet.rectangle", "Pinned rows",
                "Pins as rows under the squares, and Clear over the rest")
                .more("As in Arc: squares for the sites you live in, rows for pages you keep, and a line over the rest with Clear. Right-click a tab to pin it as a row.")
                .keys(&["pins.list"]).words(&["arc", "pinned"]).under("tabs.layout")
                .control(Control::Switch("pins.list")),
            Item::new("newtab.floating", "opening", "rectangle.and.text.magnifyingglass", "New tab in a floating field",
                "⌘T raises the address field over the page, as in Arc")
                .more("Return opens what you typed in a new tab; Escape, ⌘W or a click outside puts it away.")
                .keys(&["newtab.floating"]).words(&["command bar", "cmd t", "spotlight"])
                .control(Control::Switch("newtab.floating")),
            Item::new("commandbar", "opening", "chevron.right.square", "Address bar commands",
                "\"settings\" or \"new tab\" alone in the address field goes there")
                .keys(&["commandbar"]).words(&["commands", "command palette"]).control(Control::Switch("commandbar")),
            Item::new("start.fresh", "opening", "sunrise", "Start with a fresh window",
                "Pins, groups and named tabs come back; last time's others don't")
                .keys(&["start.fresh"]).words(&["restore", "session", "startup"]).control(Control::Switch("start.fresh")),

            // Passwords
            Item::new("passwords.open", "passwords", "key.viewfinder", "Your passwords",
                "In the macOS keychain, under Tap's name")
                .words(&["keychain"]).control(Control::Buttons(&[("passwords", "Open…")])),
            Item::new("passwords.save", "passwords", "square.and.arrow.down", "Offer to save passwords",
                "Asked once per site, never again for a site you refuse")
                .keys(&["passwords.save"]).words(&["save password"]).control(Control::Switch("passwords.save")),
            Item::new("passwords.fill", "passwords", "rectangle.and.pencil.and.ellipsis", "Fill in sign-ins",
                "Click a sign-in box and your accounts for the site hang from it")
                .keys(&["passwords.fill"]).words(&["autofill", "login"]).control(Control::Switch("passwords.fill")),
            Item::new("passkeys", "passwords", "person.badge.key", "Offer passkeys",
                "Touch ID or an iCloud passkey, on sites that offer one")
                .keys(&["passkeys"]).words(&["webauthn", "touch id"]).control(Control::Switch("passkeys")),
            Item::new("passwords.apple", "apple", "applelogo", "Passwords from Apple's Passwords app",
                "Your Passwords app's accounts under the sign-in box. Unofficial")
                .more("Tap talks to the helper Apple made for its Chrome extension, as that extension. Never in private tabs.")
                .keys(&["passwords.apple"]).words(&["icloud keychain", "apple"]).control(Control::Switch("passwords.apple")),

            // Privacy
            Item::new("shield", "blocking", "shield.lefthalf.filled", "Block ads and trackers",
                "Third parties whose only job is to watch, and YouTube's ads")
                .more("On YouTube, the ads announced in the player's own data are taken out before the video starts. Turn it off for one site from the site's row.")
                .keys(&["shield"]).words(&["adblock", "ad blocker", "ads", "youtube"]).control(Control::Switch("shield")),
            Item::new("sites.keep", "blocking", "eye.slash", "Prevent cross-site tracking", "As in Safari. Private tabs keep it on")
                .more("Off, sites you rarely open keep their sign-ins, and trackers inside other sites can follow you across them again, as in Chrome.")
                .keys(&["sites.keep"]).words(&["itp", "cookies", "tracking"]).control(Control::SwitchOff("sites.keep")),
            Item::new("privacy.clickIDs", "blocking", "scissors", "Remove click trackers from links",
                "gclid, fbclid and the like go before the page loads. Private tabs always")
                .more("The IDs an ad network adds to a link so the site can tell whose click it was. Campaign tags (utm_) stay, as in Safari.")
                .keys(&["privacy.clickIDs"])
                .words(&["gclid", "fbclid", "click id", "link tracking", "tracking parameters"])
                .control(Control::Switch("privacy.clickIDs")),
            Item::new("privacy.gpc", "blocking", "hand.raised", "Tell sites not to sell or share your data",
                "Global Privacy Control, with every request. Private tabs always")
                .more("Sends Sec-GPC: 1, which some laws make a site honour.")
                .keys(&["privacy.gpc"]).words(&["gpc", "do not sell", "do not track", "dnt", "global privacy control"])
                .control(Control::Switch("privacy.gpc")),
            Item::new("notifications.ask", "permissions", "bell.badge", "Let sites ask to send notifications",
                "Only a site you allow reaches your Mac's notifications")
                .keys(&["notifications.ask"]).words(&["notifications", "alerts", "push"])
                .control(Control::Switch("notifications.ask")),
            Item::new("privacy.forget", "permissions", "arrow.uturn.backward.circle", "Camera, microphone, location and notifications",
                "What each site was allowed or refused")
                .words(&["permissions", "camera", "microphone", "location", "reset"])
                .control(Control::Buttons(&[("forget", "Forget choices")])),
            Item::new("clear.history", "clear", "clock.arrow.circlepath", "History", "Every address you have been to")
                .words(&["clear history"]).control(Control::Buttons(&[("clear.history", "Clear")])),
            Item::new("clear.cookies", "clear", "person.crop.circle.badge.xmark", "Cookies and sign-ins", "Signs you out of every site")
                .words(&["cookies", "sign out"]).control(Control::Buttons(&[("clear.cookies", "Sign out of everything")])),
            Item::new("clear.cache", "clear", "internaldrive", "Cache", "Only what was fetched to draw pages")
                .words(&["clear cache"]).control(Control::Buttons(&[("clear.cache", "Clear")])),

            // Keyboard & Mouse
            Item::new("links.peek", "mouse", "eye", "Peek at a link", "Its page opens in a panel over the one you're reading")
                .more("Escape puts it away; the other button keeps it as a tab.")
                .keys(&["links.peek", "links.peekClick"]).words(&["preview", "shift click", "peek"])
                .control(Control::Merged(Merged::Peek)),
            Item::new("autoscroll", "mouse", "arrow.up.and.down.circle", "Scroll with the middle button",
                "Click the wheel, then move the mouse to scroll, as on Windows")
                .keys(&["autoscroll"]).words(&["middle click", "autoscroll", "wheel"]).control(Control::Switch("autoscroll")),
            Item::new("swipe.history", "mouse", "hand.draw", "Hold a swipe to pick from history",
                "Keep your fingers down after a swipe back to pick a page")
                .more("Swipe back or forward and keep your fingers down: the pages that way appear, and moving up or down picks one.")
                .keys(&["swipe.history"]).words(&["gesture", "trackpad", "back"]).control(Control::Switch("swipe.history")),

            // About
            Item::new("updates.version", "updates", "arrow.down.app", "Updates", "Checked every six hours on its own")
                .words(&["version", "update"]).control(Control::Buttons(&[("update.check", "Check for Updates…")])),
            Item::new("updates.auto", "updates", "arrow.triangle.2.circlepath", "Install updates on its own",
                "Off, Tap checks only when you choose Check for Updates…")
                .keys(&["update.install"]).words(&["automatic updates"]).control(Control::Switch("update.install")),
            Item::new("about.feedback", "updates", "envelope", "Found something wrong?",
                "Opens a draft with the version already in it")
                .words(&["bug", "feedback", "report"]).control(Control::Buttons(&[("feedback", "Send Feedback")])),
            Item::new("about.notes", "updates", "sparkles", "What's new", "Every version's notes, newest first")
                .words(&["release notes", "changelog"]).control(Control::Buttons(&[("notes", "What's New…")])),
            Item::new("bench", "developer", "terminal", "Let agents use Tap",
                "Coding agents on this Mac browse with the tap command; their tabs never take over")
                .more("Off takes the tap command and its agent skill away again.")
                .keys(&["bench"]).words(&["agents", "automation", "script", "developer", "cli"]).control(Control::Switch("bench")),

            // Moved to a menu, or always on: only the find field says so.
            Item::new("autocorrect", "pages", "textformat.abc.dottedunderline", "Correct spelling as you type",
                "Now in Edit › Spelling and Grammar, as in Safari")
                .keys(&["autocorrect"]).words(&["autocorrect", "spelling", "spell"]).find_only()
                .control(Control::Switch("autocorrect")),
            Item::new("note.split", "window", "rectangle.split.2x1", "Split View",
                "Always there: drag a tab onto a page, or press ⌥⌘N")
                .keys(&["splitView"]).words(&["split", "side by side"]).note(),
            Item::new("note.groups", "newtabs", "folder", "Tab groups", "Always there: right-click a tab › Add to New Group")
                .keys(&["tabs.groups"]).words(&["groups", "folders"]).note(),
            Item::new("note.panel", "links", "sidebar.right", "Websites in the side panel",
                "Always there: View › Side Panel, or right-click a tab")
                .keys(&["panel.sites"]).words(&["side panel", "claude", "chatgpt"]).note(),
            Item::new("note.awake", "memory", "sun.max", "Keep selected sites awake",
                "Always there: Keep This Site Awake, on the site's card")
                .keys(&["tabs.keepAwake"]).words(&["awake", "sleep"]).note(),
            Item::new("note.sitesearch", "search", "arrow.right.to.line.compact", "Search a site from the address field",
                "Always there: type red, then Tab, and search Reddit")
                .keys(&["search.sites"]).words(&["tab to search", "site search"]).note(),
            Item::new("note.sharedpins", "pins", "pin", "Pins in every space",
                "Always there with Spaces: right-click a pin › Keep in Every Space")
                .keys(&["pins.shared"]).words(&["favorites", "shared pins"]).note(),
            Item::new("note.pinback", "pins", "arrow.uturn.backward", "A pin's number again goes back",
                "Always there: press a pin's ⌘ number again to go back")
                .keys(&["pins.numberBack"]).words(&["cmd 1", "toggle"]).note(),
            Item::new("note.switcher", "mouse", "rectangle.stack", "Keys in the tab switcher",
                "Always there: ⌃W, ⌃R, ⌃M and ⌃O while ⌃Tab is up")
                .keys(&["switcher.keys"]).words(&["ctrl tab", "switcher"]).note(),
            Item::new("note.links", "links", "link", "Show where links go",
                "Always there: a link's address at the bottom of the page")
                .keys(&["links.show"]).words(&["status bar", "hover"]).note(),
            Item::new("note.flick", "video", "hand.raised.fingers.spread", "Flick the floating video to a corner",
                "Always there: two fingers send it to the corner they point at")
                .keys(&["float.flicks"]).words(&["flick", "corner"]).note(),
        ]
    })
}

/// The rows Tap acts on today, the only ones Settings shows. Every other row's value is
/// kept under its keys, and travels in a profile, until the feature behind it lands.
const LIVE: [&str; 56] = [
    "general.default",
    "general.import",
    "quit.confirm",
    "downloads.folder",
    "downloads.ask",
    "downloads.button",
    "look",
    "pages.dusk",
    "glyph",
    "tabs.reading",
    "tabs.give",
    "tabs.newPlace",
    "tabs.newAfterLast",
    "tabs.closeGoesBack",
    "tabs.lazy",
    "tabs.popupWindows",
    "tabs.sleep",
    "tabs.close.after",
    "tabs.shortcuts",
    "tabs.address",
    "toolbar.address",
    "search.engine",
    "search.suggest",
    "search.keywords",
    "reader.twins",
    "reader.follows",
    "float.style",
    "float.own",
    "media.click",
    "motion.light",
    "fullscreen.escapeTwice",
    "pageZoom",
    "pages.120",
    "newtab.floating",
    "commandbar",
    "start.fresh",
    "passwords.open",
    "passwords.save",
    "passwords.fill",
    "shield",
    "sites.keep",
    "privacy.clickIDs",
    "privacy.gpc",
    "notifications.ask",
    "autoscroll",
    "privacy.forget",
    "clear.history",
    "clear.cookies",
    "clear.cache",
    "updates.version",
    "updates.auto",
    "about.feedback",
    "about.notes",
    "bench",
    "autocorrect",
    "note.groups",
];

/// Whether Tap acts on the row yet.
#[must_use]
pub fn live(id: &str) -> bool {
    LIVE.contains(&id)
}

/// Whether a page has a row Tap acts on, for the rail.
#[must_use]
pub fn has_rows(page: Page) -> bool {
    groups_of(page)
        .iter()
        .any(|g| items_in(g.id).iter().any(|i| live(i.id)))
}

/// What every key holds until something is set (Search's `Preferences.init`).
#[must_use]
pub fn default(key: &str) -> Value {
    let on = Value::Bool(true);
    let off = Value::Bool(false);
    match key {
        "layout.cards" | "tabs.reading" | "tabs.sleep" | "tabs.address" | "float.leave"
        | "passwords.save" | "passwords.fill" | "shield" | "privacy.clickIDs" | "privacy.gpc"
        | "notifications.ask" | "links.peek" | "swipe.history" | "update.install"
        | "links.show" | "float.flicks" | "autocorrect" | "toolbar.address"
        // `bench`: Search's is off; Tap's agents count on it (docs/PLAN.md §8 "Agents use Tap").
        | "bench" => on,
        "look" => "system".into(),
        "sidebar.position" => "left".into(),
        "glyph" => "letters".into(),
        "tabs.newPlace" => "next".into(),
        "tabs.sleep.pace" => "normal".into(),
        "search.engine" => "google".into(),
        "search.custom" => "".into(),
        "float.style" => "search".into(),
        "pages.120.mode" => "onPower".into(),
        "links.peekClick" => "shift".into(),
        "tabs.close.after" => 0.into(),
        "pageZoom" => 1.0.into(),
        _ => off,
    }
}

/// Where Settings reads and writes: the shell's defaults, a map in tests.
pub trait Prefs {
    fn get(&self, key: &str) -> Option<Value>;
    fn set(&mut self, key: &str, value: Value);
}

/// A key's value, or its default.
#[must_use]
pub fn value(prefs: &dyn Prefs, key: &str) -> Value {
    prefs.get(key).unwrap_or_else(|| default(key))
}

#[must_use]
pub fn flag(prefs: &dyn Prefs, key: &str) -> bool {
    match value(prefs, key) {
        Value::Bool(on) => on,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => default(key).as_bool().unwrap_or(false),
    }
}

#[must_use]
pub fn text(prefs: &dyn Prefs, key: &str) -> String {
    match value(prefs, key) {
        Value::String(s) => s,
        Value::Bool(b) => b.to_string(),
        other => default(key)
            .as_str()
            .map_or_else(|| other.to_string(), str::to_owned),
    }
}

/// A merged control's choice as the keys under it say it.
#[must_use]
pub fn merged(prefs: &dyn Prefs, which: Merged) -> String {
    match which {
        Merged::TabsLayout => if !flag(prefs, "sidebar") {
            "top"
        } else if flag(prefs, "sidebar.icons") {
            "icons"
        } else if text(prefs, "sidebar.position") == "right" {
            "right"
        } else {
            "left"
        }
        .to_owned(),
        Merged::Sleep => {
            if !flag(prefs, "tabs.sleep") {
                return "never".to_owned();
            }
            let pace = text(prefs, "tabs.sleep.pace");
            if matches!(pace.as_str(), "later" | "normal" | "sooner") {
                pace
            } else {
                "normal".to_owned()
            }
        }
        Merged::FloatOwn => if flag(prefs, "float.away") {
            "tabsOrApps"
        } else if flag(prefs, "float.leave") {
            "tabs"
        } else {
            "never"
        }
        .to_owned(),
        // On when either was on, and on when neither was ever touched.
        Merged::PinsKeep => {
            let (home, peek) = (prefs.get("pins.home"), prefs.get("pins.peek"));
            let on = (home.is_none() && peek.is_none())
                || home.and_then(|v| v.as_bool()).unwrap_or(false)
                || peek.and_then(|v| v.as_bool()).unwrap_or(false);
            on.to_string()
        }
        Merged::Peek => {
            if !flag(prefs, "links.peek") {
                return "off".to_owned();
            }
            let click = text(prefs, "links.peekClick");
            if Merged::Peek.options().iter().any(|(raw, _)| *raw == click) {
                click
            } else {
                "shift".to_owned()
            }
        }
    }
}

/// A merged control moved, written as Search writes it. False for a value it doesn't
/// know.
pub fn set_merged(prefs: &mut dyn Prefs, which: Merged, raw: &str) -> bool {
    match (which, raw) {
        (Merged::TabsLayout, "top") => prefs.set("sidebar", false.into()),
        (Merged::TabsLayout, "icons") => {
            prefs.set("sidebar.icons", true.into());
            prefs.set("sidebar", true.into());
        }
        (Merged::TabsLayout, "left" | "right") => {
            prefs.set("sidebar.position", raw.into());
            prefs.set("sidebar.icons", false.into());
            prefs.set("sidebar", true.into());
        }
        (Merged::Sleep, "never") => prefs.set("tabs.sleep", false.into()),
        (Merged::Sleep, "later" | "normal" | "sooner") => {
            prefs.set("tabs.sleep.pace", raw.into());
            prefs.set("tabs.sleep", true.into());
        }
        (Merged::FloatOwn, "never" | "tabs" | "tabsOrApps") => {
            prefs.set("float.leave", (raw != "never").into());
            prefs.set("float.away", (raw == "tabsOrApps").into());
        }
        (Merged::PinsKeep, "true" | "false") => {
            prefs.set("pins.home", (raw == "true").into());
            prefs.set("pins.peek", (raw == "true").into());
        }
        (Merged::Peek, "off") => prefs.set("links.peek", false.into()),
        (Merged::Peek, click) if Merged::Peek.options().iter().any(|(r, _)| *r == click) => {
            prefs.set("links.peekClick", click.into());
            prefs.set("links.peek", true.into());
        }
        _ => return false,
    }
    true
}

/// Whether a dependent row means anything as things are set now; its page leaves it out
/// otherwise, and the find field shows it dimmed (Search's `applies`).
#[must_use]
pub fn applies(item: &Item, prefs: &dyn Prefs) -> bool {
    let sidebar = flag(prefs, "sidebar");
    match item.id {
        "pages.dusk" => text(prefs, "look") != "light",
        "sidebar.roomy" => sidebar && !flag(prefs, "sidebar.icons"),
        "sidebar.hides" | "layout.cards" | "pins.list" => sidebar,
        // The bar says where the page is; the tab then keeps its title.
        "tabs.address" => !flag(prefs, "toolbar.address"),
        "toolbar.left" => !sidebar,
        _ => true,
    }
}

/// What a dimmed dependent row says instead of its own line.
#[must_use]
pub fn needs(item: &Item) -> &'static str {
    match item.id {
        "pages.dusk" => "Only while Tap isn't set to Light",
        "toolbar.left" => "Only with the tabs across the top",
        "sidebar.roomy" => "Only with titles in the column",
        "tabs.address" => "Only without the address bar",
        _ => "Only with the tabs down a side",
    }
}

/// The rows whose words hold every word typed, in page order and group order (Search's
/// `find`).
#[must_use]
pub fn find(typed: &str) -> Vec<&'static Item> {
    let words: Vec<String> = typed
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<&'static Item> = items()
        .iter()
        .filter(|item| {
            let group = group(item.group);
            let mut hay = vec![
                item.title,
                item.detail,
                item.more.unwrap_or_default(),
                group.map_or("", |g| g.title),
                item.page().title(),
            ];
            hay.extend(item.words);
            hay.extend(item.keys);
            let hay = hay.join(" ").to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect();
    found.sort_by_key(|item| (item.page().order(), group_order(item.group)));
    found
}

/// The settings a profile file may carry, out and back in: the keys Settings shows, so a
/// setting can't travel that Settings doesn't, plus the per-site shield pauses, which
/// belong to the profile though no row lists them (Search's `allowedPrefs`).
#[must_use]
pub fn profile_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = items()
        .iter()
        .flat_map(|i| i.keys.iter().copied())
        .collect();
    keys.push("shield.paused");
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Map(HashMap<String, Value>);
    impl Prefs for Map {
        fn get(&self, key: &str) -> Option<Value> {
            self.0.get(key).cloned()
        }
        fn set(&mut self, key: &str, value: Value) {
            self.0.insert(key.to_owned(), value);
        }
    }

    #[test]
    fn every_row_is_in_a_group_and_every_page_but_extensions_has_rows() {
        for item in items() {
            assert!(group(item.group).is_some(), "{} has no group", item.id);
        }
        for page in Page::all().filter(|p| *p != Page::Extensions) {
            assert!(
                groups_of(page).iter().any(|g| !items_in(g.id).is_empty()),
                "{page:?} would be empty"
            );
        }
        let mut ids: Vec<&str> = items().iter().map(|i| i.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "ids are unique");
        assert!(LIVE.iter().all(|id| item(id).is_some()));
    }

    /// The allowlist is Search's, but for translation's key: the same index, read the same
    /// way (`SettingsIndex.items.flatMap { $0.keys }` plus `shield.paused`).
    #[test]
    fn a_profile_carries_what_settings_shows_and_nothing_secret() {
        let keys = profile_keys();
        for key in [
            "search.engine",
            "search.custom",
            "search.keywords",
            "tabs.sleep",
            "tabs.sleep.pace",
            "look",
            "shield",
            "shield.paused",
            "update.install",
            "sidebar",
            "sidebar.position",
            "sidebar.icons",
            "tabs.groups",
        ] {
            assert!(keys.contains(&key), "{key} should travel");
        }
        for key in [
            "passwords.never",
            "import.records",
            "translate.pages",
            "space.current",
            "welcomed",
        ] {
            assert!(!keys.contains(&key), "{key} must not travel");
        }
        assert_eq!(
            keys.len(),
            78,
            "Search's 78 less translate.pages, plus Tap's toolbar.address"
        );
    }

    #[test]
    fn merged_controls_read_and_write_search_s_keys() {
        let mut prefs = Map::default();
        assert_eq!(merged(&prefs, Merged::TabsLayout), "top");
        assert_eq!(merged(&prefs, Merged::Sleep), "normal");
        assert_eq!(
            merged(&prefs, Merged::PinsKeep),
            "true",
            "never touched: on"
        );
        assert_eq!(merged(&prefs, Merged::Peek), "shift");
        assert!(set_merged(&mut prefs, Merged::TabsLayout, "right"));
        assert_eq!(merged(&prefs, Merged::TabsLayout), "right");
        assert!(set_merged(&mut prefs, Merged::TabsLayout, "icons"));
        assert_eq!(
            (
                merged(&prefs, Merged::TabsLayout),
                text(&prefs, "sidebar.position")
            ),
            ("icons".to_owned(), "right".to_owned()),
            "icons only keeps the side"
        );
        assert!(set_merged(&mut prefs, Merged::Sleep, "never"));
        assert_eq!(merged(&prefs, Merged::Sleep), "never");
        assert!(set_merged(&mut prefs, Merged::Sleep, "sooner"));
        assert_eq!(
            (flag(&prefs, "tabs.sleep"), text(&prefs, "tabs.sleep.pace")),
            (true, "sooner".to_owned())
        );
        prefs.set("pins.home", false.into());
        assert_eq!(
            merged(&prefs, Merged::PinsKeep),
            "false",
            "one turned off, the other never set"
        );
        assert!(!set_merged(&mut prefs, Merged::Sleep, "soon"));
    }

    #[test]
    fn rows_that_need_another_count_only_while_it_does() {
        let mut prefs = Map::default();
        let address = item("tabs.address").expect("a row");
        assert!(!applies(address, &prefs), "the bar is on by default");
        prefs.set("toolbar.address", false.into());
        assert!(applies(address, &prefs));
        let roomy = item("sidebar.roomy").expect("a row");
        let left = item("toolbar.left").expect("a row");
        assert!(!applies(roomy, &prefs) && applies(left, &prefs));
        set_merged(&mut prefs, Merged::TabsLayout, "left");
        assert!(applies(roomy, &prefs) && !applies(left, &prefs));
    }

    #[test]
    fn agents_may_drive_tap_until_it_is_switched_off() {
        let mut prefs = Map::default();
        assert!(flag(&prefs, "bench"));
        prefs.set("bench", Value::Bool(false));
        assert!(!flag(&prefs, "bench"));
    }

    #[test]
    fn the_find_field_matches_every_word_in_page_order() {
        let found: Vec<&str> = find("dark").iter().map(|i| i.id).collect();
        assert_eq!(found[..3], ["look", "pages.dusk", "reader.follows"]);
        assert!(find("groups").iter().any(|i| i.id == "note.groups"));
        assert!(find("  ").is_empty());
        assert!(find("tabs sleep").iter().all(|i| i.page() == Page::Tabs));
        assert!(find("translate").is_empty(), "translation isn't in Tap");
    }

    /// Settings shows what Tap does: a page with no row Tap acts on isn't in the rail.
    #[test]
    fn only_pages_with_rows_tap_acts_on_are_shown() {
        let shown: Vec<Page> = Page::all().filter(|p| has_rows(*p)).collect();
        assert_eq!(
            shown,
            [
                Page::General,
                Page::Appearance,
                Page::Tabs,
                Page::Browsing,
                Page::Spaces,
                Page::Passwords,
                Page::Privacy,
                Page::Keyboard,
                Page::About
            ]
        );
    }
}
