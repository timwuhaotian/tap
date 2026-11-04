//! The row of tabs at the top of the window (Search's `TabBar.swift`). Titles, the live
//! one on a grey pill, a cross on the one under the pointer, the plus after the last,
//! and the field in place of the live tab while you type (unless the address bar holds
//! it). A tab group's name heads its tabs on a chip of the group's colour, a line of it
//! under them; a click on it folds the group to its name (and the tab in front, if it
//! is one of its own) or opens it again. Drawn as one view: a row of three hundred tabs
//! is one draw, not three hundred views.
//!
//! Every slot (tab, heading, plus) is followed by what it is, not where it stands, and
//! springs to where the layout puts it (`motion.rs`): opened, closed, moved, folded,
//! the row glides there. A tab, or a group by its heading, carried by the hand reorders
//! the row (Search's `Carried`).

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    time::Instant,
};

use block2::RcBlock;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
    sel,
};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSEventModifierFlags,
    NSGraphicsContext, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer};
use objc2_quartz_core::CADisplayLink;
use tap_core::{
    browser::{Browser, Tab, TabId},
    fnd::{Parts, Url},
    groups::{self, GroupDrop, GroupSpans, Item},
    location::{is_address, is_loopback},
    settings_index,
};

use crate::{
    downloads, icons,
    menu::Commands,
    motion::{self, Aim, Curve, Flock, Glide},
    palette, prefs, rt,
    tabmenu::{self, TabState},
    web,
};

/// The strip's look, read from Kept on every draw (and on every tick), so a Settings
/// change shows at once without anyone having to tell the strip (Search reads its
/// observable `Preferences` the same way). Keys and defaults are Search's, but for the
/// address bar's, which is Tap's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent Settings switches, read together
struct Look {
    icons: bool,
    reading: bool,
    give: bool,
    shortcuts: bool,
    address: bool,
    /// The address bar is up: it says where the page is and holds the field, so the
    /// live tab keeps its title and the helm leaves this row.
    bar: bool,
    /// Settings › Motion, while the Mac saves power: the give below lands at
    /// once instead of easing (Search's `Motion.light`).
    light: bool,
}

fn look() -> Look {
    let kept = prefs::Kept::new();
    Look {
        icons: settings_index::text(&kept, "glyph") == "icons",
        reading: settings_index::flag(&kept, "tabs.reading"),
        give: settings_index::flag(&kept, "tabs.give"),
        shortcuts: settings_index::flag(&kept, "tabs.shortcuts"),
        address: settings_index::flag(&kept, "tabs.address"),
        bar: settings_index::flag(&kept, "toolbar.address"),
        light: motion::light(),
    }
}

thread_local! {
    /// How far each tab's page has been read, reported by the tick's script below.
    /// The model has no such field and `web.rs` is not the strip's to change, so the
    /// numbers live here, keyed by tab, and only the live tab's is ever drawn.
    static READING: RefCell<HashMap<TabId, f64>> = RefCell::new(HashMap::new());
    /// The one strip, so the tick and a reading report can ask it to draw again.
    static LIVE_STRIP: RefCell<Option<Retained<Strip>>> = const { RefCell::new(None) };
    static TICKER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
}

/// What the tick asks the live tab's page, in Tap's own content world: Search's
/// `ScrollRelay` fraction, read on demand rather than pushed, since the strip may
/// not add a script-message handler to WebKit's shared configuration.
const READING_SCRIPT: &str = r"(() => {
  const root = document.documentElement;
  const y = window.scrollY || root.scrollTop || 0;
  const ceiling = Math.max(1, (root.scrollHeight || 0) - window.innerHeight);
  return Math.min(1, Math.max(0, y / ceiling));
})()";

/// Starts the strip's one tick, once: at 20 frames a second it is cheap, and it is
/// what makes held-⌘ numbers, a pressed tab's give, reading reports and a Settings
/// change land without a model change to redraw on. Motion has its own frames.
fn start_ticker() {
    TICKER.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        let block = RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| tick());
        // SAFETY: a repeating timer on the main run loop; its block is kept by the
        // timer, and the timer is kept here for the app's life.
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &block) };
        timer.setTolerance(0.01);
        *slot = Some(timer);
    });
}

fn tick() {
    LIVE_STRIP.with_borrow(|slot| {
        if let Some(strip) = slot {
            strip.tick();
        }
    });
}

/// Where the row lays out each of its slots, as the bench aims a hand at them: left,
/// top, width and height in the strip's points, which are the window's from its top
/// left (the strip sits at the window's top edge).
pub fn slot_rects() -> Vec<[f64; 4]> {
    LIVE_STRIP.with_borrow(|slot| {
        slot.as_ref()
            .and_then(|strip| {
                strip.with_layout(|_, _, layout| {
                    layout
                        .tabs
                        .iter()
                        .map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height])
                        .collect()
                })
            })
            .unwrap_or_default()
    })
}

fn redraw_strip() {
    LIVE_STRIP.with_borrow(|slot| {
        if let Some(strip) = slot {
            strip.setNeedsDisplay(true);
        }
    });
}

/// Asks the live tab's page how far it has been read, and keeps the answer in
/// hundredths, as Search's `Tab.scrolled` does, so a report that changes nothing
/// redraws nothing.
fn poll_reading() {
    let Some(active) = rt::read(Browser::active).flatten() else {
        return;
    };
    // Tabs that are gone leave no number behind.
    if let Some(ids) = rt::read(|browser| browser.tabs().iter().map(|t| t.id).collect::<Vec<_>>()) {
        READING.with_borrow_mut(|map| map.retain(|id, _| ids.contains(id)));
    }
    let Some(page) = web::existing(active) else {
        return;
    };
    web::evaluate_in_world(&page, READING_SCRIPT, move |value| {
        let Some(fraction) = value.as_ref().and_then(serde_json::Value::as_f64) else {
            return;
        };
        let fraction = (fraction.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        let changed = READING.with_borrow_mut(|map| map.insert(active, fraction) != Some(fraction));
        if changed {
            redraw_strip();
        }
    });
}

/// A site icon's side, in points.
const ICON: f64 = 16.0;

/// How far the hand goes before a press becomes a carry (Search's drag, which starts
/// at 5).
const CARRY: f64 = 5.0;

/// What the pointer is over. Tabs by their place among the tabs, headings by their
/// group's place among the groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Spot {
    #[default]
    Nothing,
    Tab(usize),
    Close(usize),
    Heading(usize),
    Plus,
}

/// A thing in the row, as the layout sees it: a tab, or a heading as wide as its name.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Tab,
    Heading(f64),
}

/// What a slot is, wherever it stands: the row's springs follow it by this.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Tab(TabId),
    /// A group's heading, by the group's identity.
    Heading(String),
    Plus,
}

/// What the hand carries: tabs side by side (one, or the picked ones gathered round
/// the one pressed, `lead`), or a group by its heading.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Carried {
    Tabs { lead: TabId, run: Vec<TabId> },
    Group(String),
}

/// A press that may become a carry.
#[derive(Clone, Debug)]
struct Grab {
    what: Carried,
    /// Where the pointer went down, and where the slot stood then.
    start: f64,
    origin: f64,
    travel: f64,
    /// Past `CARRY`: the slot is in the hand.
    carrying: bool,
}

/// A reorder the hand asked for: a tab to a place among its block's, or a group from
/// one place among the groups to another.
enum Move {
    Tabs(Vec<TabId>, usize),
    Group(usize, usize),
}

/// A click whose meaning waits for the release, so the press can still become a carry.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Click {
    /// Any tab plainly clicked: the field opens on the tab in front.
    Edit,
    /// A heading: its group folds or opens.
    Fold(String),
    /// One of several picked tabs, pressed and let go without a carry: the others
    /// are let go and it is the tab in front, as a plain click would have it. Not
    /// on the press: picking a tab gives the page the keys, and the row losing them
    /// would let the picked tabs go before a carry could take them along.
    Unpick(TabId),
}

/// Where everything in the row goes for a width, one rectangle per slot.
struct Layout {
    tabs: Vec<NSRect>,
    plus: NSRect,
    /// How far right the tabs may be drawn: the doors' edge, or just before
    /// the plus when the row has overflowed and the plus is pinned.
    end: f64,
}

/// What the row's right end must keep clear: the helm while it is up here (with the
/// address bar up it stands on the bar), and the downloads door while that door is
/// shown (the same call the window makes to show it), so the plus can never run under
/// either.
fn trailing(bar: bool) -> f64 {
    let kept = prefs::Kept::new();
    let fetch = downloads::shows_button(
        settings_index::flag(&kept, "downloads.button"),
        downloads::going(),
    );
    let door = if fetch { palette::DOOR + 4.0 } else { 0.0 };
    if bar {
        door + 8.0
    } else {
        palette::HELM + door
    }
}

/// A heading's width: its name, with air on both sides.
fn heading_width(name: &str) -> f64 {
    (palette::text_width(name) + 20.0)
        .ceil()
        .min(palette::TAB_WIDTH)
}

/// The row's geometry: tabs from the lights, each its fixed width until the row is full,
/// the live tab as wide as the field while it is being edited; headings keep their own
/// width. `editing` is the live tab's slot, `trailing` what the right end keeps clear
/// (see `trailing`). However far the tabs overflow at their narrowest, the plus is
/// pinned before `trailing` rather than running under the doors there.
fn layout(width: f64, slots: &[Slot], editing: Option<usize>, trailing: f64) -> Layout {
    let count = slots.len();
    let top = (palette::STRIP - palette::ROW) / 2.0;
    let room = (width - palette::LIGHTS - trailing - palette::PLUS - 12.0).max(0.0);
    #[allow(clippy::cast_precision_loss)] // a row never holds 2^52 tabs
    let gaps = palette::TAB_GAP * count.saturating_sub(1) as f64;
    let headings: f64 = slots
        .iter()
        .map(|s| match s {
            Slot::Heading(w) => *w,
            Slot::Tab => 0.0,
        })
        .sum();
    let field = editing.map_or(0.0, |_| palette::FIELD.min(room * 0.6));
    let tabs_count = slots.iter().filter(|s| **s == Slot::Tab).count();
    let others = if editing.is_some() {
        tabs_count.saturating_sub(1)
    } else {
        tabs_count
    };
    #[allow(clippy::cast_precision_loss)]
    let each = if others == 0 {
        palette::TAB_WIDTH
    } else {
        ((room - field - gaps - headings) / others as f64)
            .clamp(palette::TAB_MIN, palette::TAB_WIDTH)
    };
    let mut x = palette::LIGHTS;
    let mut tabs = Vec::with_capacity(count);
    for (index, slot) in slots.iter().enumerate() {
        let w = match slot {
            Slot::Heading(w) => *w,
            Slot::Tab if editing == Some(index) => field.max(each),
            Slot::Tab => each,
        };
        tabs.push(NSRect::new(
            NSPoint::new(x, top),
            NSSize::new(w, palette::ROW),
        ));
        x += w + palette::TAB_GAP;
    }
    let pinned = (width - trailing - palette::PLUS - 4.0).max(palette::LIGHTS);
    let plus_x = (x + 2.0).min(pinned);
    let plus = NSRect::new(
        NSPoint::new(plus_x, top),
        NSSize::new(palette::PLUS, palette::ROW),
    );
    let end = if plus_x < x + 2.0 {
        plus_x - 2.0
    } else {
        width - trailing
    };
    Layout { tabs, plus, end }
}

/// Where a carried slot lands among its peers (`spans`, each a peer's left and right
/// edge, the carried one at `at`), drawn from `left` to `right`: past a neighbour once
/// its leading edge crosses the neighbour's middle, and back the same way. Search's
/// `Carried` rounds the travel by one tab's step; this is the same for tabs of one
/// width and holds for groups, which are not.
fn place_among(spans: &[(f64, f64)], at: usize, left: f64, right: f64) -> usize {
    spans
        .iter()
        .enumerate()
        .filter(|(i, (start, end))| {
            let middle = (start + end) / 2.0;
            match (*i).cmp(&at) {
                std::cmp::Ordering::Less => left >= middle,
                std::cmp::Ordering::Greater => right > middle,
                std::cmp::Ordering::Equal => false,
            }
        })
        .count()
}

/// The cross's square, at the tab's right end.
fn close_rect(tab: NSRect) -> NSRect {
    let size = 18.0;
    NSRect::new(
        NSPoint::new(
            tab.origin.x + tab.size.width - size - 6.0,
            tab.origin.y + (tab.size.height - size) / 2.0,
        ),
        NSSize::new(size, size),
    )
}

/// What a press takes a slot by: its whole column, the strip's height and the gap to
/// its right. Above, below or between the pills is still the tab, not title bar, so
/// a press meant for a tab never moves the window instead.
fn column(rect: NSRect, height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x, 0.0),
        NSSize::new(rect.size.width + palette::TAB_GAP, height),
    )
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

fn tab_at(index: usize) -> Option<TabId> {
    rt::read(|browser| browser.tabs().get(index).map(|t| t.id)).flatten()
}

/// The row as drawn: what each slot is, and the slots for layout.
fn slots(browser: &Browser) -> (Vec<Item>, Vec<Slot>) {
    let row = browser.row();
    let slots = row
        .iter()
        .map(|item| match item {
            Item::Heading(place) => Slot::Heading(heading_width(
                browser.groups().get(*place).map_or("", |g| g.name.as_str()),
            )),
            Item::Tab(_) => Slot::Tab,
        })
        .collect();
    (row, slots)
}

/// What a slot of the row is.
fn key_of(browser: &Browser, item: &Item) -> Key {
    match item {
        Item::Heading(place) => Key::Heading(
            browser
                .groups()
                .get(*place)
                .map(|g| g.id.clone())
                .unwrap_or_default(),
        ),
        Item::Tab(index) => Key::Tab(browser.tabs()[*index].id),
    }
}

/// The group a tab stands in in the row: none for a pin.
fn group_of(tab: &Tab) -> Option<&str> {
    tab.group().filter(|_| !tab.is_pinned())
}

/// Whether a slot of the row belongs to the group: its heading, or one of its tabs.
fn in_group(browser: &Browser, item: &Item, group: &str) -> bool {
    match item {
        Item::Heading(place) => browser.groups().get(*place).is_some_and(|g| g.id == group),
        Item::Tab(index) => group_of(&browser.tabs()[*index]) == Some(group),
    }
}

/// The live tab's slot, while the field is open on it in the row (not in the address
/// bar, nor floating for ⌘T).
fn editing_slot(browser: &Browser, row: &[Item], bar: bool) -> Option<usize> {
    if bar {
        return None;
    }
    let editing = browser.editing()?;
    if editing.launching {
        return None;
    }
    let active = browser.active()?;
    let index = browser.tabs().iter().position(|t| t.id == active)?;
    row.iter().position(|item| *item == Item::Tab(index))
}

/// Draws with everything at `alpha`: a slot fading in or out.
fn faded(alpha: f64, draw: impl FnOnce()) {
    if alpha >= 0.999 {
        draw();
        return;
    }
    if alpha <= 0.001 {
        return;
    }
    NSGraphicsContext::saveGraphicsState_class();
    if let Some(context) = NSGraphicsContext::currentContext() {
        CGContext::set_alpha(Some(&context.CGContext()), alpha);
    }
    draw();
    NSGraphicsContext::restoreGraphicsState_class();
}

/// Search's lift for a carried tab: a soft shadow under it while the hand has it.
fn lifted(draw: impl FnOnce()) {
    NSGraphicsContext::saveGraphicsState_class();
    palette::lift_shadow().set();
    draw();
    NSGraphicsContext::restoreGraphicsState_class();
}

/// What a draw of the row reads once.
#[derive(Clone, Copy)]
struct Paint {
    dark: bool,
    look: Look,
    spot: Spot,
    pressed: Option<usize>,
    press_scale: f64,
    cmd_held: bool,
    active: Option<TabId>,
    editing: Option<usize>,
    /// The group carried tabs would join if let go now, by its place.
    drop: Option<usize>,
}

pub struct Ivars {
    /// Every menu item's target, the app delegate: the tab menu's items send
    /// their commands to it, as the find bar's do.
    commands: Retained<Commands>,
    spot: Cell<Spot>,
    inside: Cell<bool>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    /// The tab under a press still down, by its place among the tabs, for its give.
    pressed: Cell<Option<usize>>,
    /// How far the pressed tab has sunk, 1 down to Search's 0.97, eased by the tick.
    press_scale: Cell<f64>,
    cmd_held: Cell<bool>,
    last_look: Cell<Option<Look>>,
    poll_ticks: Cell<u8>,
    /// Every slot's springs.
    flock: RefCell<Flock<Key>>,
    /// The row's slots as last aimed, in order: a change in them is what moves.
    shown: RefCell<Vec<Key>>,
    /// Each group's fold as last aimed, to tell a group unfolding from a tab opening.
    folded: RefCell<HashMap<String, bool>>,
    /// What each slot last said, to draw it while it fades once gone.
    faces: RefCell<HashMap<Key, String>>,
    grab: RefCell<Option<Grab>>,
    /// Where the carried tabs land if let go now: another group, under the hand.
    drop: RefCell<Option<GroupDrop>>,
    click: RefCell<Option<Click>>,
    /// Frames while something moves, paused when nothing does.
    link: RefCell<Option<Retained<CADisplayLink>>>,
    last_frame: Cell<Option<Instant>>,
    /// The live tab's pill on its way from the tab that was live (Search's
    /// `matchedGeometryEffect("live")` on `glide`): from, to, and how far along.
    live: RefCell<(Option<TabId>, Option<TabId>, Glide)>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapStrip"]
    #[ivars = Ivars]
    pub struct Strip;

    impl Strip {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            self.track();
            // SAFETY: the superclass's own implementation, as AppKit requires.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.hover(Some(event));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.hover(Some(event));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.hover(None);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.release();
        }

        /// The row takes the keys only while tabs are picked, so ⌘C and Esc are
        /// theirs; any other time a click on a tab leaves the keys where they were.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            rt::read(|browser| browser.selection_count() > 0).unwrap_or(false)
        }

        /// The keys went elsewhere (the page, the field): the picked tabs are let
        /// go, so ⌘C copies whatever the keys are on now.
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            rt::dispatch(Browser::clear_selection);
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            // Esc lets the picked tabs go and gives the page the keys back.
            if event.keyCode() == 53 {
                rt::dispatch(|browser| {
                    browser.clear_selection();
                    browser.focus_page();
                });
                return;
            }
            // SAFETY: the superclass's own, passing the key on up the chain.
            let _: () = unsafe { msg_send![super(self), keyDown: event] };
        }

        /// ⌘C while tabs are picked: their addresses, one to a line.
        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            tabmenu::copy_selected();
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.context_menu(event);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            // The wheel closes a tab, as everywhere.
            if let Spot::Tab(index) | Spot::Close(index) = self.spot_at(event)
                && let Some(id) = tab_at(index)
            {
                rt::dispatch(move |browser| browser.close(id));
            }
        }

        /// One frame of motion, from the display link.
        #[unsafe(method(stripFrame:))]
        fn strip_frame(&self, _link: &CADisplayLink) {
            self.frame_tick();
        }
    }
);

impl Strip {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, commands: &Commands) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            commands: commands.retain(),
            spot: Cell::new(Spot::Nothing),
            inside: Cell::new(false),
            tracking: RefCell::new(None),
            pressed: Cell::new(None),
            press_scale: Cell::new(1.0),
            cmd_held: Cell::new(false),
            last_look: Cell::new(None),
            poll_ticks: Cell::new(0),
            flock: RefCell::new(Flock::default()),
            shown: RefCell::new(Vec::new()),
            folded: RefCell::new(HashMap::new()),
            faces: RefCell::new(HashMap::new()),
            grab: RefCell::new(None),
            drop: RefCell::new(None),
            click: RefCell::new(None),
            link: RefCell::new(None),
            last_frame: Cell::new(None),
            live: RefCell::new((None, None, Glide::default())),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        LIVE_STRIP.with_borrow_mut(|slot| *slot = Some(view.clone()));
        start_ticker();
        view
    }

    /// The tick's work, twenty times a second: watch ⌘ and Kept for changes no model
    /// change announces, ease a pressed tab, and ask for a reading report four times
    /// a second while that setting is on.
    fn tick(&self) {
        let look = look();
        if self.ivars().last_look.get() != Some(look) {
            self.ivars().last_look.set(Some(look));
            self.setNeedsDisplay(true);
        }
        let held = look.shortcuts
            && NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Command);
        if held != self.ivars().cmd_held.get() {
            self.ivars().cmd_held.set(held);
            self.setNeedsDisplay(true);
        }
        let target = if look.give && self.ivars().pressed.get().is_some() {
            0.97
        } else {
            1.0
        };
        let scale = self.ivars().press_scale.get();
        if (scale - target).abs() > 0.001 {
            // Down answers at once, back up settles, as Search's `Give` has it.
            // With no motion to spare the sink lands at once rather than easing.
            let step = if motion::reduced() || look.light {
                1.0
            } else if target < scale {
                0.45
            } else {
                0.3
            };
            let next = scale + (target - scale) * step;
            self.ivars()
                .press_scale
                .set(if (next - target).abs() <= 0.001 {
                    target
                } else {
                    next
                });
            self.setNeedsDisplay(true);
        }
        if look.reading {
            let ticks = self.ivars().poll_ticks.get().saturating_add(1);
            if ticks >= 4 {
                self.ivars().poll_ticks.set(0);
                poll_reading();
            } else {
                self.ivars().poll_ticks.set(ticks);
            }
        } else {
            self.ivars().poll_ticks.set(0);
        }
    }

    /// The look as last read, for the many questions a pointer asks.
    fn look_now(&self) -> Look {
        self.ivars().last_look.get().unwrap_or_else(look)
    }

    /// The row's target geometry now, with what each slot is.
    fn with_layout<R>(&self, f: impl FnOnce(&Browser, &[Item], &Layout) -> R) -> Option<R> {
        let bar = self.look_now().bar;
        let width = self.bounds().size.width;
        rt::read(|browser| {
            let (row, slots) = slots(browser);
            let editing = editing_slot(browser, &row, bar);
            let layout = layout(width, &slots, editing, trailing(bar));
            f(browser, &row, &layout)
        })
    }

    /// Where the live tab is drawn, for the field to sit in while it is edited there.
    pub fn field_rect(&self) -> Option<NSRect> {
        self.with_layout(|browser, row, layout| {
            let index = editing_slot(browser, row, self.look_now().bar)?;
            let rect = layout.tabs.get(index).copied()?;
            Some(NSRect::new(
                NSPoint::new(rect.origin.x + 10.0, rect.origin.y + 5.0),
                NSSize::new(rect.size.width - 20.0, rect.size.height - 10.0),
            ))
        })
        .flatten()
    }

    /// Right-click: a tab's menu, or its heading's (see `tabmenu`). Clicking
    /// nowhere in particular offers nothing, as it does nothing.
    fn context_menu(&self, event: &NSEvent) {
        match self.spot_at(event) {
            Spot::Tab(index) | Spot::Close(index) => {
                let Some(id) = tab_at(index) else { return };
                let Some(state) = rt::read(|browser| {
                    let tab = browser.tab(id)?;
                    let tabs = browser.tabs();
                    let among = browser.is_selected(id);
                    Some(TabState {
                        has_page: !tab.is_blank() && !tab.is_internal(),
                        others: tabs.iter().any(|t| t.id != id && !t.is_pinned()),
                        right: tabs[index + 1..].iter().any(|t| !t.is_pinned()),
                        can_reopen: browser.can_reopen(),
                        pinned: tab.is_pinned(),
                        group: tab
                            .group()
                            .and_then(|g| browser.groups().iter().position(|x| x.id == g)),
                        groups: browser.groups().iter().map(|g| g.name.clone()).collect(),
                        can_sleep: browser.awake_because(id).is_none(),
                        picked: if among { browser.selection_count() } else { 0 },
                        picked_groups: if among && browser.selection_count() > 1 {
                            browser
                                .tabs()
                                .iter()
                                .filter(|t| browser.is_selected(t.id))
                                .map(|t| {
                                    t.group().and_then(|g| {
                                        browser.groups().iter().position(|x| x.id == g)
                                    })
                                })
                                .collect()
                        } else {
                            Vec::new()
                        },
                    })
                })
                .flatten() else {
                    return;
                };
                tabmenu::show_tab(
                    self.mtm(),
                    &self.ivars().commands,
                    event,
                    self.as_ref(),
                    id,
                    &state,
                );
            }
            Spot::Heading(place) => {
                let Some((collapsed, colour)) = rt::read(|browser| {
                    browser
                        .groups()
                        .get(place)
                        .map(|g| (g.collapsed, g.colour.clone()))
                })
                .flatten() else {
                    return;
                };
                tabmenu::show_group(
                    self.mtm(),
                    &self.ivars().commands,
                    event,
                    self.as_ref(),
                    place,
                    collapsed,
                    colour.as_deref(),
                );
            }
            Spot::Plus | Spot::Nothing => {}
        }
    }

    fn spot_at(&self, event: &NSEvent) -> Spot {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let bar = self.look_now().bar;
        let height = self.bounds().size.height;
        self.with_layout(|browser, row, layout| {
            let editing = editing_slot(browser, row, bar);
            // The plus first: where it is pinned it stands over the run of
            // tabs, and it, not the tab beneath, takes the click.
            if contains(column(layout.plus, height), point) {
                return Spot::Plus;
            }
            for (slot, (item, rect)) in row.iter().zip(&layout.tabs).enumerate() {
                if contains(column(*rect, height), point) {
                    let index = match item {
                        Item::Heading(place) => return Spot::Heading(*place),
                        Item::Tab(index) => *index,
                    };
                    let can_close = editing != Some(slot) && rect.size.width >= palette::TAB_TITLED;
                    return if can_close && contains(close_rect(*rect), point) {
                        Spot::Close(index)
                    } else {
                        Spot::Tab(index)
                    };
                }
            }
            Spot::Nothing
        })
        .unwrap_or_default()
    }

    fn carrying(&self) -> bool {
        self.ivars()
            .grab
            .borrow()
            .as_ref()
            .is_some_and(|g| g.carrying)
    }

    fn hover(&self, event: Option<&NSEvent>) {
        if self.carrying() {
            return;
        }
        let spot = event.map_or(Spot::Nothing, |e| self.spot_at(e));
        let inside = event.is_some();
        if spot != self.ivars().spot.get() || inside != self.ivars().inside.get() {
            self.ivars().spot.set(spot);
            self.ivars().inside.set(inside);
            self.setNeedsDisplay(true);
        }
    }

    fn press(&self, event: &NSEvent) {
        let spot = self.spot_at(event);
        let x = self.convertPoint_fromView(event.locationInWindow(), None).x;
        // ⌘-click and ⇧-click pick tabs rather than going to one (Search's
        // `extendTabSelection`); the row takes the keys while any are picked.
        let flags = event.modifierFlags();
        let (shift, command) = (
            flags.contains(NSEventModifierFlags::Shift),
            flags.contains(NSEventModifierFlags::Command),
        );
        if (shift || command)
            && let Spot::Tab(index) = spot
        {
            let Some(id) = tab_at(index) else { return };
            rt::dispatch(move |browser| {
                browser.extend_selection(id, shift, command);
                if browser.selection_count() == 0 {
                    browser.focus_page();
                }
            });
            if rt::read(|browser| browser.selection_count() > 0).unwrap_or(false)
                && let Some(window) = self.window()
            {
                window.makeFirstResponder(Some(self));
            }
            return;
        }
        // A tab (or its cross) goes down under the press; the tick sinks it if
        // Settings says tabs give, and `mouseUp:` lets it back up.
        if let Spot::Tab(index) | Spot::Close(index) = spot {
            self.ivars().pressed.set(Some(index));
            self.setNeedsDisplay(true);
        }
        self.ivars().grab.replace(None);
        self.ivars().drop.replace(None);
        self.ivars().click.replace(None);
        match spot {
            Spot::Close(index) => {
                if let Some(id) = tab_at(index) {
                    rt::dispatch(move |browser| browser.close(id));
                }
            }
            Spot::Tab(index) => {
                let Some(id) = tab_at(index) else { return };
                let live = rt::read(|browser| browser.active() == Some(id)).unwrap_or(false);
                // Any other tab is picked at once, as it goes into the hand; any tab
                // becomes the field on release, unless the press became a carry. Tap
                // departs from Search's `DragStrip` here, which opens the field on the
                // live tab alone and only while the address bar is away (docs/PLAN.md).
                // A plain click lets any picked tabs go, and the page has the keys
                // until the field opens on release; on one of several picked, only
                // once it is let go without a carry, since a carry takes them all
                // along.
                let (picked, among) = rt::read(|browser| {
                    let count = browser.selection_count();
                    (count > 0, count > 1 && browser.is_selected(id))
                })
                .unwrap_or_default();
                if picked && !among {
                    rt::dispatch(|browser| {
                        browser.clear_selection();
                        browser.focus_page();
                    });
                }
                if among {
                    self.ivars().click.replace(Some(Click::Unpick(id)));
                } else {
                    self.ivars().click.replace(Some(Click::Edit));
                }
                if !live && !among {
                    rt::dispatch(move |browser| browser.select(id));
                }
                self.grab(
                    Carried::Tabs {
                        lead: id,
                        run: vec![id],
                    },
                    &Key::Tab(id),
                    x,
                );
            }
            Spot::Heading(place) => {
                let Some(group) =
                    rt::read(|browser| browser.groups().get(place).map(|g| g.id.clone())).flatten()
                else {
                    return;
                };
                self.ivars().click.replace(Some(Click::Fold(group.clone())));
                self.grab(Carried::Group(group.clone()), &Key::Heading(group), x);
            }
            Spot::Plus => rt::dispatch(|browser| browser.begin_editing(true)),
            Spot::Nothing => {
                // The empty row is title bar: a press moves the window, and a
                // double click opens a new tab instead of zooming, as
                // Search's `DragStrip` has it — the plus, without the aim.
                if event.clickCount() == 2 {
                    rt::dispatch(|browser| browser.begin_editing(true));
                } else if let Some(window) = self.window() {
                    window.performWindowDragWithEvent(event);
                }
            }
        }
    }

    /// A press on a slot that may become a carry, from where it stands now.
    fn grab(&self, what: Carried, key: &Key, x: f64) {
        let origin = self
            .with_layout(|browser, row, layout| {
                row.iter()
                    .zip(&layout.tabs)
                    .find(|(item, _)| key_of(browser, item) == *key)
                    .map(|(_, rect)| rect.origin.x)
            })
            .flatten();
        let Some(origin) = origin else { return };
        self.ivars().grab.replace(Some(Grab {
            what,
            start: x,
            origin,
            travel: 0.0,
            carrying: false,
        }));
    }

    fn drag(&self, event: &NSEvent) {
        let x = self.convertPoint_fromView(event.locationInWindow(), None).x;
        {
            let mut grab = self.ivars().grab.borrow_mut();
            let Some(grab) = grab.as_mut() else { return };
            grab.travel = x - grab.start;
            if !grab.carrying {
                if grab.travel.abs() < CARRY {
                    return;
                }
                // In the hand now: no click on release, and no give under it.
                grab.carrying = true;
                self.ivars().click.replace(None);
                self.ivars().pressed.set(None);
                // One of several picked: the others of its block gather beside it
                // and go along.
                if let Carried::Tabs { lead, run } = &mut grab.what {
                    let lead = *lead;
                    let gathered = rt::update(|browser| browser.gather(lead)).unwrap_or_default();
                    if !gathered.is_empty() {
                        *run = gathered;
                    }
                }
            }
        }
        rt::settle();
        self.carry();
        self.animate();
        self.setNeedsDisplay(true);
    }

    fn release(&self) {
        // The press is over wherever it lands; the tick eases the tab back up.
        if self.ivars().pressed.take().is_some() {
            self.setNeedsDisplay(true);
        }
        let grab = self.ivars().grab.take();
        let click = self.ivars().click.take();
        let drop = self.ivars().drop.take();
        if let Some(grab) = grab.filter(|g| g.carrying) {
            // Let go over another group: the carried tabs join it there.
            if let (Some(to), Carried::Tabs { run, .. }) = (drop, grab.what) {
                rt::dispatch(move |browser| browser.drop_into_group(&run, &to));
            }
            // Let go: the slot springs from the hand into its place.
            self.animate();
            self.setNeedsDisplay(true);
            return;
        }
        match click {
            Some(Click::Edit) => rt::dispatch(|browser| {
                if browser.editing().is_none() {
                    browser.begin_editing(false);
                }
            }),
            Some(Click::Fold(group)) => rt::dispatch(move |browser| {
                if let Some(place) = browser.groups().iter().position(|g| g.id == group) {
                    browser.toggle_group(place);
                }
            }),
            Some(Click::Unpick(id)) => rt::dispatch(move |browser| {
                browser.clear_selection();
                browser.select(id);
                browser.focus_page();
            }),
            None => {}
        }
    }

    /// The carried slot past a neighbour: the model reorders, and the row glides.
    fn carry(&self) {
        let Some(grab) = self.ivars().grab.borrow().clone().filter(|g| g.carrying) else {
            return;
        };
        let left = grab.origin + grab.travel;
        // Over another group, the tabs wait to join it on release (Search's
        // `groupDestination`) rather than moving along their own block.
        let drop = self
            .with_layout(|browser, row, layout| {
                let Carried::Tabs { lead, run } = &grab.what else {
                    return None;
                };
                let me = browser.tab(*lead).filter(|t| !t.is_pinned())?;
                let spans = drop_spans(browser, row, &layout.tabs);
                groups::drop_target(grab.start + grab.travel, &spans, run)
                    .filter(|to| Some(to.group.as_str()) != group_of(me))
            })
            .flatten();
        let over = drop.is_some();
        self.ivars().drop.replace(drop);
        if over {
            return;
        }
        let wanted = self
            .with_layout(|browser, row, layout| match &grab.what {
                Carried::Tabs { lead, run } => {
                    let me = browser.tab(*lead)?;
                    let block = (me.is_pinned(), group_of(me));
                    // Each tab of the block on its own, the carried run as one.
                    let mut spans: Vec<(f64, f64)> = Vec::new();
                    let mut at: Option<usize> = None;
                    let mut lead_x = None;
                    for (item, rect) in row.iter().zip(&layout.tabs) {
                        let Item::Tab(index) = item else { continue };
                        let tab = &browser.tabs()[*index];
                        if (tab.is_pinned(), group_of(tab)) != block {
                            continue;
                        }
                        let span = (rect.origin.x, rect.origin.x + rect.size.width);
                        if tab.id == *lead {
                            lead_x = Some(rect.origin.x);
                        }
                        if !run.contains(&tab.id) {
                            spans.push(span);
                        } else if let Some(open) = at {
                            spans[open].1 = span.1;
                        } else {
                            at = Some(spans.len());
                            spans.push(span);
                        }
                    }
                    let (at, lead_x) = (at?, lead_x?);
                    let (start, end) = spans[at];
                    // The run's left edge where the hand has it: the pressed tab is
                    // under the hand, the rest of the run beside it.
                    let left = left - (lead_x - start);
                    let to = place_among(&spans, at, left, left + (end - start));
                    (to != at).then_some(Move::Tabs(run.clone(), to))
                }
                Carried::Group(group) => {
                    let spans = group_spans(browser, row, &layout.tabs);
                    let at = spans.iter().position(|(id, _)| id == group)?;
                    let (start, end) = spans[at].1;
                    let edges: Vec<(f64, f64)> = spans.iter().map(|(_, span)| *span).collect();
                    let to = place_among(&edges, at, left, left + (end - start));
                    (to != at).then_some(Move::Group(at, to))
                }
            })
            .flatten();
        match wanted {
            Some(Move::Tabs(run, to)) => rt::dispatch(move |browser| browser.move_tabs(&run, to)),
            Some(Move::Group(from, to)) => {
                rt::dispatch(move |browser| browser.move_group(from, to));
            }
            None => {}
        }
    }

    /// Frames from now on, until nothing moves.
    fn animate(&self) {
        let mut slot = self.ivars().link.borrow_mut();
        if slot.is_none() {
            let target: &NSObject = self.as_ref();
            // SAFETY: `stripFrame:` is this class's own method and takes the link; the
            // link keeps the strip, which lives for the app's life anyway.
            let link = unsafe { self.displayLinkWithTarget_selector(target, sel!(stripFrame:)) };
            // SAFETY: the main run loop and its common modes, so frames keep coming
            // while a menu is up or the hand is down.
            unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
            link.setPaused(true);
            *slot = Some(link);
        }
        if let Some(link) = slot.as_ref()
            && link.isPaused()
        {
            self.ivars().last_frame.set(None);
            link.setPaused(false);
        }
    }

    fn frame_tick(&self) {
        let now = Instant::now();
        let dt = self
            .ivars()
            .last_frame
            .replace(Some(now))
            .map_or(1.0 / 60.0, |then| now.duration_since(then).as_secs_f64())
            .min(1.0 / 15.0);
        let moving = self.ivars().flock.borrow_mut().step(dt);
        let gliding = self.ivars().live.borrow_mut().2.step(dt);
        let moving = moving || gliding;
        self.setNeedsDisplay(true);
        if !moving && !self.carrying() {
            if let Some(link) = self.ivars().link.borrow().as_ref() {
                link.setPaused(true);
            }
            self.ivars().last_frame.set(None);
            // What the gone slots said is no longer wanted.
            let flock = self.ivars().flock.borrow();
            self.ivars()
                .faces
                .borrow_mut()
                .retain(|key, _| flock.place(key).is_some());
        }
    }

    fn track(&self) {
        if let Some(old) = self.ivars().tracking.borrow_mut().take() {
            self.removeTrackingArea(&old);
        }
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: a fresh allocation; the owner is this view, which outlives the area
        // (the area is removed above before a new one replaces it).
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                self.bounds(),
                options,
                Some(self.as_ref()),
                None,
            )
        };
        self.addTrackingArea(&area);
        *self.ivars().tracking.borrow_mut() = Some(area);
    }

    /// Tells the springs where the row now wants every slot. A change in the slots
    /// themselves (opened, closed, moved, folded) glides there; anything else (a
    /// resize, the field widening the live tab) is there at once. A group that
    /// unfolds sends its tabs out from under its heading; one that folds takes them
    /// back under it.
    fn aim(&self, browser: &Browser, row: &[Item], layout: &Layout) {
        let keys: Vec<Key> = row
            .iter()
            .map(|item| key_of(browser, item))
            .chain([Key::Plus])
            .collect();
        let moved = {
            let shown = self.ivars().shown.borrow();
            !shown.is_empty() && *shown != keys
        };
        let now: HashMap<String, bool> = browser
            .groups()
            .iter()
            .map(|g| (g.id.clone(), g.collapsed))
            .collect();
        let before = self.ivars().folded.replace(now.clone());
        let turned = |group: &str, folded: bool| {
            before.get(group) == Some(&!folded) && now.get(group) == Some(&folded)
        };
        let headings: HashMap<&str, f64> = row
            .iter()
            .zip(&layout.tabs)
            .filter_map(|(item, rect)| match item {
                Item::Heading(place) => browser
                    .groups()
                    .get(*place)
                    .map(|g| (g.id.as_str(), rect.origin.x)),
                Item::Tab(_) => None,
            })
            .collect();
        let mut aims: Vec<Aim<Key>> = row
            .iter()
            .zip(&layout.tabs)
            .zip(&keys)
            .map(|((item, rect), key)| Aim {
                key: key.clone(),
                x: rect.origin.x,
                width: rect.size.width,
                from: match item {
                    Item::Tab(index) => group_of(&browser.tabs()[*index])
                        .filter(|g| turned(g, false))
                        .and_then(|g| headings.get(g).copied()),
                    Item::Heading(_) => None,
                },
            })
            .collect();
        aims.push(Aim {
            key: Key::Plus,
            x: layout.plus.origin.x,
            width: layout.plus.size.width,
            from: None,
        });
        let curve = if moved {
            Curve::settle_now()
        } else {
            Curve::SETTLE
        };
        self.ivars()
            .flock
            .borrow_mut()
            .aim(&aims, moved, curve, |key| match key {
                Key::Tab(id) => browser
                    .tab(*id)
                    .and_then(group_of)
                    .filter(|g| turned(g, true))
                    .and_then(|g| headings.get(g).copied()),
                Key::Heading(_) | Key::Plus => None,
            });
        if *self.ivars().shown.borrow() != keys {
            self.ivars().shown.replace(keys);
        }
        // Another tab is live: the pill sets off from the one that was.
        let active = browser.active();
        let switched = {
            let mut live = self.ivars().live.borrow_mut();
            if live.1 == active {
                false
            } else {
                let from = live.1.take();
                *live = (from, active, live.2);
                if from.is_some() {
                    live.2.restart(Curve::glide_now());
                }
                from.is_some()
            }
        };
        if moved || switched {
            self.animate();
        }
    }

    /// The live tab's pill where it is this frame: between the tab it left and the
    /// one it is going to, each where it is drawn now, so it stays on them while they
    /// move too.
    fn live_pill(
        &self,
        browser: &Browser,
        row: &[Item],
        layout: &Layout,
        at: &dyn Fn(&Key, NSRect) -> (NSRect, f64),
    ) -> Option<NSRect> {
        let (from, to, glide) = *self.ivars().live.borrow();
        let drawn = |id: TabId| {
            row.iter()
                .zip(&layout.tabs)
                .find(
                    |(item, _)| matches!(item, Item::Tab(index) if browser.tabs()[*index].id == id),
                )
                .map(|(_, rect)| at(&Key::Tab(id), *rect).0)
        };
        let target = drawn(to?)?;
        let Some(start) = from.and_then(drawn) else {
            return Some(target);
        };
        let t = glide.progress();
        let lerp = |a: f64, b: f64| a + (b - a) * t;
        Some(NSRect::new(
            NSPoint::new(lerp(start.origin.x, target.origin.x), target.origin.y),
            NSSize::new(
                lerp(start.size.width, target.size.width),
                target.size.height,
            ),
        ))
    }

    /// The carried slot (or group) where the hand has it.
    fn hold_carried(&self, browser: &Browser, row: &[Item], layout: &Layout) -> Vec<Key> {
        let Some(grab) = self.ivars().grab.borrow().clone().filter(|g| g.carrying) else {
            return Vec::new();
        };
        let x = grab.origin + grab.travel;
        let mut flock = self.ivars().flock.borrow_mut();
        match &grab.what {
            Carried::Tabs { lead, run } => {
                let Some(lead_x) = row
                    .iter()
                    .zip(&layout.tabs)
                    .find(|(item, _)| matches!(item, Item::Tab(index) if browser.tabs()[*index].id == *lead))
                    .map(|(_, rect)| rect.origin.x)
                else {
                    return Vec::new();
                };
                let mut held = Vec::new();
                for (item, rect) in row.iter().zip(&layout.tabs) {
                    if let Item::Tab(index) = item
                        && run.contains(&browser.tabs()[*index].id)
                    {
                        let key = Key::Tab(browser.tabs()[*index].id);
                        flock.hold(&key, rect.origin.x + (x - lead_x));
                        held.push(key);
                    }
                }
                held
            }
            Carried::Group(group) => {
                let Some(heading_x) = row
                    .iter()
                    .zip(&layout.tabs)
                    .find(|(item, _)| {
                        matches!(item, Item::Heading(_)) && in_group(browser, item, group)
                    })
                    .map(|(_, rect)| rect.origin.x)
                else {
                    return Vec::new();
                };
                let mut held = Vec::new();
                for (item, rect) in row.iter().zip(&layout.tabs) {
                    if in_group(browser, item, group) {
                        let key = key_of(browser, item);
                        flock.hold(&key, rect.origin.x + (x - heading_x));
                        held.push(key);
                    }
                }
                held
            }
        }
    }

    #[allow(clippy::too_many_lines)] // the row, gone slots, carried ones, then the plus
    fn draw(&self) {
        let dark = palette::is_dark(self);
        let bounds = self.bounds();
        // Read on every draw, so a Settings change is in the next frame (see `Look`).
        let look = look();
        palette::GROUND.resolve(dark).setFill();
        NSBezierPath::fillRect(bounds);
        if !look.bar {
            // The line between the row and the page; the address bar draws its own.
            palette::HAIRLINE.resolve(dark).setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(0.0, bounds.size.height - 1.0),
                NSSize::new(bounds.size.width, 1.0),
            ));
        }
        let inside = self.ivars().inside.get();
        let trailing = trailing(look.bar);
        // A row too long for the window stops at the doors rather than running under them.
        NSBezierPath::clipRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(bounds.size.width - trailing, bounds.size.height),
        ));
        let _ = rt::read(|browser| {
            let (row, slots) = slots(browser);
            let editing = editing_slot(browser, &row, look.bar);
            let layout = layout(bounds.size.width, &slots, editing, trailing);
            self.aim(browser, &row, &layout);
            let carried = self.hold_carried(browser, &row, &layout);
            let paint = Paint {
                dark,
                look,
                spot: self.ivars().spot.get(),
                pressed: self.ivars().pressed.get(),
                press_scale: self.ivars().press_scale.get(),
                cmd_held: self.ivars().cmd_held.get(),
                active: browser.active(),
                editing,
                drop: self
                    .ivars()
                    .drop
                    .borrow()
                    .as_ref()
                    .and_then(|to| browser.groups().iter().position(|g| g.id == to.group)),
            };
            let flock = self.ivars().flock.borrow();
            // Where a slot is drawn this frame: its springs' place, at the row's height.
            let at = |key: &Key, rect: NSRect| -> (NSRect, f64) {
                flock.place(key).map_or((rect, 1.0), |p| {
                    (
                        NSRect::new(
                            NSPoint::new(p.x, rect.origin.y),
                            NSSize::new(p.width, rect.size.height),
                        ),
                        p.alpha,
                    )
                })
            };
            // The tabs stop where the layout says (before a pinned plus);
            // the state is restored below, before the plus itself is drawn.
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(layout.end, bounds.size.height),
            ));
            let top = (palette::STRIP - palette::ROW) / 2.0;
            // What has gone, fading under the rest.
            for (key, place) in flock.leaving() {
                let rect = NSRect::new(
                    NSPoint::new(place.x, top),
                    NSSize::new(place.width, palette::ROW),
                );
                faded(place.alpha, || self.draw_gone(browser, key, rect, &paint));
            }
            draw_group_lines(browser, &row, &layout, &at, dark);
            if let Some(place) = paint.drop {
                drop_plate(browser, &row, &layout, place, dark);
            }
            // The live tab's pill, under every tab, gliding when another is picked.
            if let Some(rect) = self.live_pill(browser, &row, &layout, &at) {
                live_pill(rect, dark);
            }
            let mut faces = self.ivars().faces.borrow_mut();
            for (slot, (item, rect)) in row.iter().zip(&layout.tabs).enumerate() {
                let key = key_of(browser, item);
                let label = match item {
                    Item::Heading(place) => browser.groups()[*place].name.clone(),
                    Item::Tab(index) => browser.tabs()[*index].label(),
                };
                if faces.get(&key) != Some(&label) {
                    faces.insert(key.clone(), label);
                }
                if carried.contains(&key) {
                    continue;
                }
                let (rect, alpha) = at(&key, *rect);
                faded(alpha, || draw_item(browser, &paint, slot, item, rect));
            }
            drop(faces);
            // In the hand: over everything, lifted.
            for (slot, (item, rect)) in row.iter().zip(&layout.tabs).enumerate() {
                let key = key_of(browser, item);
                if !carried.contains(&key) {
                    continue;
                }
                let (rect, _) = at(&key, *rect);
                lifted(|| match item {
                    Item::Heading(place) => {
                        let colour = browser.groups()[*place].colour.as_deref();
                        palette::group_colour(colour).resolve(dark).setFill();
                        chip(rect).fill();
                    }
                    Item::Tab(_) => {
                        palette::WASH.resolve(dark).setFill();
                        palette::pill_path(rect).fill();
                    }
                });
                draw_item(browser, &paint, slot, item, rect);
            }
            NSGraphicsContext::restoreGraphicsState_class();
            let (plus_rect, _) = at(&Key::Plus, layout.plus);
            // The plus is always there to be found; only its wash waits for the pointer.
            if inside && paint.spot == Spot::Plus {
                palette::HOVER.resolve(dark).setFill();
                palette::pill_path(plus_rect).fill();
            }
            plus(plus_rect, &palette::MUTED.resolve(dark));
        });
    }

    /// A slot that has gone, as it fades: a tab still in the row's model (folded away)
    /// as it was drawn, one that closed, or a group that went, by what it last said.
    fn draw_gone(&self, browser: &Browser, key: &Key, rect: NSRect, paint: &Paint) {
        let dark = paint.dark;
        match key {
            Key::Tab(id) => {
                if let Some((index, tab)) =
                    browser.tabs().iter().enumerate().find(|(_, t)| t.id == *id)
                {
                    draw_tab(browser, paint, None, index, tab, rect, true);
                    return;
                }
            }
            Key::Heading(group) => {
                if let Some(group) = browser.groups().iter().find(|g| g.id == *group) {
                    draw_heading(&group.name, group.colour.as_deref(), rect, false, dark);
                    return;
                }
            }
            Key::Plus => return,
        }
        if let Some(label) = self.ivars().faces.borrow().get(key) {
            let text = NSRect::new(
                NSPoint::new(rect.origin.x + 12.0, rect.origin.y + 7.0),
                NSSize::new((rect.size.width - 24.0).max(0.0), 17.0),
            );
            palette::draw_text(label, text, &palette::MUTED.resolve(dark));
        }
    }
}

/// The group carried tabs would join if let go now, heading through last tab, on a
/// plate of its colour under them: the hand covers the heading it is over, so the
/// whole group says it.
fn drop_plate(browser: &Browser, row: &[Item], layout: &Layout, place: usize, dark: bool) {
    let Some(group) = browser.groups().get(place) else {
        return;
    };
    let Some((_, (start, end))) = group_spans(browser, row, &layout.tabs)
        .into_iter()
        .find(|(id, _)| *id == group.id)
    else {
        return;
    };
    let top = (palette::STRIP - palette::ROW) / 2.0;
    let path = palette::pill_path(NSRect::new(
        NSPoint::new(start - 3.0, top - 3.0),
        NSSize::new(end - start + 6.0, palette::ROW + 6.0),
    ));
    let colour = palette::group_colour(group.colour.as_deref()).resolve(dark);
    colour
        .colorWithAlphaComponent(if dark { 0.22 } else { 0.14 })
        .setFill();
    path.fill();
    colour.colorWithAlphaComponent(0.7).setStroke();
    path.setLineWidth(1.5);
    path.stroke();
}

/// Each group in the row for a drop: its heading's span and its shown tabs'.
fn drop_spans<'a>(browser: &'a Browser, row: &[Item], rects: &[NSRect]) -> Vec<GroupSpans<'a>> {
    let mut spans: Vec<GroupSpans<'a>> = Vec::new();
    for (item, rect) in row.iter().zip(rects) {
        let span = (rect.origin.x, rect.origin.x + rect.size.width);
        match item {
            Item::Heading(place) => {
                if let Some(group) = browser.groups().get(*place) {
                    spans.push(GroupSpans {
                        group: &group.id,
                        collapsed: group.collapsed,
                        heading: span,
                        members: Vec::new(),
                    });
                }
            }
            Item::Tab(index) => {
                let tab = &browser.tabs()[*index];
                if let (Some(group), Some(last)) = (group_of(tab), spans.last_mut())
                    && last.group == group
                {
                    last.members.push((tab.id, span));
                }
            }
        }
    }
    spans
}

/// Each group in the row, by its identity, from its heading's left edge to its last
/// tab's right one.
fn group_spans(browser: &Browser, row: &[Item], rects: &[NSRect]) -> Vec<(String, (f64, f64))> {
    let mut spans: Vec<(String, (f64, f64))> = Vec::new();
    for (item, rect) in row.iter().zip(rects) {
        let right = rect.origin.x + rect.size.width;
        match item {
            Item::Heading(place) => {
                if let Some(group) = browser.groups().get(*place) {
                    spans.push((group.id.clone(), (rect.origin.x, right)));
                }
            }
            Item::Tab(index) => {
                if let Some(group) = group_of(&browser.tabs()[*index])
                    && let Some((id, span)) = spans.last_mut()
                    && id == group
                {
                    span.1 = right;
                }
            }
        }
    }
    spans
}

/// Under each group's tabs, a line of its colour from its chip to its last tab, where
/// they are drawn this frame (Chrome's group underline).
fn draw_group_lines(
    browser: &Browser,
    row: &[Item],
    layout: &Layout,
    at: &dyn Fn(&Key, NSRect) -> (NSRect, f64),
    dark: bool,
) {
    let mut line: Option<(Option<&str>, f64, f64, bool)> = None;
    let draw = |line: Option<(Option<&str>, f64, f64, bool)>| {
        if let Some((colour, start, end, with_tabs)) = line
            && with_tabs
        {
            palette::group_colour(colour).resolve(dark).setFill();
            let y = f64::midpoint(palette::STRIP, palette::ROW) + 1.5;
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(
                    NSPoint::new(start + 4.0, y),
                    NSSize::new((end - start - 8.0).max(0.0), 2.0),
                ),
                1.0,
                1.0,
            )
            .fill();
        }
    };
    for (item, rect) in row.iter().zip(&layout.tabs) {
        let key = key_of(browser, item);
        let (rect, _) = at(&key, *rect);
        let right = rect.origin.x + rect.size.width;
        match item {
            Item::Heading(place) => {
                draw(line.take());
                let colour = browser
                    .groups()
                    .get(*place)
                    .and_then(|g| g.colour.as_deref());
                line = Some((colour, rect.origin.x, right, false));
            }
            Item::Tab(index) => {
                let grouped = group_of(&browser.tabs()[*index]).is_some();
                match line.as_mut() {
                    Some(open) if grouped => {
                        open.2 = open.2.max(right);
                        open.3 = true;
                    }
                    _ => draw(line.take()),
                }
            }
        }
    }
    draw(line);
}

/// One slot of the row where it is drawn this frame.
fn draw_item(browser: &Browser, paint: &Paint, slot: usize, item: &Item, rect: NSRect) {
    match item {
        Item::Heading(place) => {
            let group = &browser.groups()[*place];
            let dropping = paint.drop == Some(*place);
            draw_heading(
                &group.name,
                group.colour.as_deref(),
                rect,
                dropping || paint.spot == Spot::Heading(*place),
                paint.dark,
            );
        }
        Item::Tab(index) => {
            if let Some(tab) = browser.tabs().get(*index) {
                draw_tab(browser, paint, Some(slot), *index, tab, rect, false);
            }
        }
    }
}

/// A group's heading: its name on a chip of its colour, a little deeper under the
/// pointer.
fn draw_heading(name: &str, colour: Option<&str>, rect: NSRect, under: bool, dark: bool) {
    let fill = palette::group_colour(colour).resolve(dark);
    let fill = if under {
        fill
    } else {
        fill.colorWithAlphaComponent(0.88)
    };
    fill.setFill();
    chip(rect).fill();
    let text = NSRect::new(
        NSPoint::new(rect.origin.x + 10.0, rect.origin.y + 7.0),
        NSSize::new((rect.size.width - 20.0).max(0.0), 17.0),
    );
    palette::draw_text(name, text, &palette::on_group(dark));
}

/// The live tab's pill: Search's wash, lifted a little off the row so the tab you
/// are on reads as the one in front — a soft shadow in a light window, a faint
/// edge in a dark one, where a shadow wouldn't show — and a hairline round it in
/// both, so the pill reads as a plate.
fn live_pill(rect: NSRect, dark: bool) {
    let path = palette::pill_path(rect);
    NSGraphicsContext::saveGraphicsState_class();
    if !dark {
        palette::live_shadow().set();
    }
    palette::WASH.resolve(dark).setFill();
    path.fill();
    NSGraphicsContext::restoreGraphicsState_class();
    if dark {
        NSColor::colorWithWhite_alpha(1.0, 0.07).setStroke();
        path.setLineWidth(1.0);
        path.stroke();
    } else {
        palette::stroke_hairline(&path, dark);
    }
}

/// A heading's chip: a little shorter than a tab's pill.
fn chip(rect: NSRect) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new(
            NSPoint::new(rect.origin.x, rect.origin.y + 4.0),
            NSSize::new(rect.size.width, (rect.size.height - 8.0).max(0.0)),
        ),
        7.0,
        7.0,
    )
}

/// One tab where it is drawn this frame. `slot` is its place in the row (`None` for
/// one that has gone); `gone` draws it as it fades, at rest: no pill, no cross.
#[allow(clippy::too_many_lines)] // one tab, left to right, in one pass
fn draw_tab(
    browser: &Browser,
    paint: &Paint,
    slot: Option<usize>,
    index: usize,
    tab: &Tab,
    rect: NSRect,
    gone: bool,
) {
    let dark = paint.dark;
    let look = paint.look;
    let editing_here = slot.is_some() && paint.editing == slot;
    let live = !gone && Some(tab.id) == paint.active;
    let under = !gone && matches!(paint.spot, Spot::Tab(i) | Spot::Close(i) if i == index);
    // A pressed tab sinks where it stands; its hit rect stays the laid-out
    // one, as SwiftUI's scale leaves the row's geometry alone.
    let rect = &if !gone && paint.pressed == Some(index) && !editing_here {
        sunk(rect, paint.press_scale)
    } else {
        rect
    };
    // The live tab's own pill is drawn once for the row (`live_pill`), so it can
    // glide; a tab under the pointer gets its own.
    // A picked tab wears the accent, lightly; one under the pointer its own grey.
    let picked = !gone && browser.is_selected(tab.id);
    if picked {
        let accent = NSColor::controlAccentColor();
        accent
            .colorWithAlphaComponent(if dark { 0.32 } else { 0.18 })
            .setFill();
        let plate = palette::pill_path(*rect);
        plate.fill();
        accent
            .colorWithAlphaComponent(if dark { 0.50 } else { 0.35 })
            .setStroke();
        plate.setLineWidth(1.0);
        plate.stroke();
    } else if under && !live {
        palette::HOVER.resolve(dark).setFill();
        let plate = palette::pill_path(*rect);
        plate.fill();
        palette::stroke_hairline(&plate, dark);
    } else if under && live {
        // Over the live pill, so only a breath of hover lest it cover the live one.
        palette::HOVER
            .resolve(dark)
            .colorWithAlphaComponent(0.5)
            .setFill();
        palette::pill_path(*rect).fill();
    }
    let titled = rect.size.width >= palette::TAB_TITLED;
    // Search's `ReadingFill`: ink at 5.5% over the live pill, from its
    // left, as far as the page has been read. Not on a tab down to its
    // mark, where thirty points of grey would say nothing.
    if live
        && look.reading
        && titled
        && let Some(read) = READING.with_borrow(|map| map.get(&tab.id).copied())
        && read > 0.0
    {
        reading_fill(*rect, read, dark);
    }
    if editing_here {
        return;
    }
    let crossed = (under || live) && titled;
    let number = if paint.cmd_held && !gone {
        shortcut_number(index, browser.tabs().len())
    } else {
        None
    };
    // "Tabs show": a site icon only when that is the choice; the letter
    // tile stands in wherever Search's `Mark` would show one without an
    // icon (a narrow tab always, a titled one only in icons mode).
    let icon = if look.icons {
        icons::image_for(tab.url())
    } else {
        None
    };
    if let Some(number) = number.filter(|_| !titled) {
        let digit = number.to_string();
        let width = palette::text_width(&digit);
        palette::draw_text(
            &digit,
            NSRect::new(
                NSPoint::new(
                    rect.origin.x + (rect.size.width - width) / 2.0,
                    rect.origin.y + 7.0,
                ),
                NSSize::new(width + 1.0, 17.0),
            ),
            &palette::MUTED.resolve(dark),
        );
    } else if let Some(icon) = &icon {
        // Beside the title, or alone in the middle of a narrow tab.
        let x = if titled {
            rect.origin.x + 10.0
        } else {
            rect.origin.x + (rect.size.width - ICON) / 2.0
        };
        let square = NSRect::new(
            NSPoint::new(x, rect.origin.y + (rect.size.height - ICON) / 2.0),
            NSSize::new(ICON, ICON),
        );
        // SAFETY: drawing a decoded image into the view's own context, no hints.
        unsafe {
            icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                square,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            );
        }
    } else if !titled || look.icons {
        // No icon to show (none kept yet, or Letters is the choice): a
        // narrow tab shows its letter on a tile rather than nothing at
        // all, and in icons mode a titled tab does the same beside its
        // title, as Search's `Mark` falls back to the monogram.
        let x = if titled {
            rect.origin.x + 10.0
        } else {
            rect.origin.x + (rect.size.width - ICON) / 2.0
        };
        let square = NSRect::new(
            NSPoint::new(x, rect.origin.y + (rect.size.height - ICON) / 2.0),
            NSSize::new(ICON, ICON),
        );
        palette::HOVER.resolve(dark).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(square, 4.0, 4.0).fill();
        let letter = monogram(tab);
        let width = palette::text_width(&letter);
        palette::draw_text(
            &letter,
            NSRect::new(
                NSPoint::new(
                    square.origin.x + (ICON - width) / 2.0,
                    square.origin.y - 0.5,
                ),
                NSSize::new(width + 1.0, 17.0),
            ),
            &palette::MUTED.resolve(dark),
        );
    }
    if titled {
        let mut right = if crossed { 30.0 } else { 12.0 };
        if number.is_some() {
            right += 18.0;
        }
        // In icons mode a glyph (icon or letter tile) always stands
        // beside the title; in letters mode the title starts at the edge.
        let left = if look.icons { 10.0 + ICON + 6.0 } else { 12.0 };
        let text = NSRect::new(
            NSPoint::new(rect.origin.x + left, rect.origin.y + 7.0),
            NSSize::new((rect.size.width - left - right).max(0.0), 17.0),
        );
        let ink = if live || picked {
            palette::INK
        } else {
            palette::MUTED
        };
        // The tab you're on says where its page is instead of its title,
        // and the title comes back under the pointer (Search's
        // `TitleAtRest`): the two swap in the same rect. Not with the address
        // bar up: the bar says where, and the tab keeps its title.
        let said = if live && !look.bar {
            at_rest(tab.url(), look.address)
        } else {
            None
        };
        match said.filter(|_| !under) {
            Some(said) => draw_rest(&said, text, dark),
            None => palette::draw_text(&tab.label(), text, &ink.resolve(dark)),
        }
        if let Some(number) = number {
            let digit = number.to_string();
            let at = if crossed { 30.0 } else { 12.0 };
            palette::draw_text(
                &digit,
                NSRect::new(
                    NSPoint::new(
                        rect.origin.x + rect.size.width - at - 15.0,
                        rect.origin.y + 7.0,
                    ),
                    NSSize::new(16.0, 17.0),
                ),
                &palette::MUTED.resolve(dark),
            );
        }
    }
    if crossed {
        let square = close_rect(*rect);
        if paint.spot == Spot::Close(index) {
            palette::HAIRLINE.resolve(dark).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(square, 5.0, 5.0).fill();
        }
        cross(square, &palette::MUTED.resolve(dark));
    }
    // The load's progress: along the pill's foot, unless the address bar says it.
    if live && tab.loading() && !look.bar {
        progress(*rect, tab.progress(), &palette::INK.resolve(dark));
    }
}

/// A tab's rect sunk under a press: scaled about its centre (Search's `Give`,
/// 0.97) and a point down, so it reads as sinking rather than shrinking in place.
fn sunk(rect: NSRect, scale: f64) -> NSRect {
    if scale >= 0.999 {
        return rect;
    }
    let (w, h) = (rect.size.width * scale, rect.size.height * scale);
    NSRect::new(
        NSPoint::new(
            rect.origin.x + (rect.size.width - w) / 2.0,
            rect.origin.y + (rect.size.height - h) / 2.0 + (1.0 - scale) * 30.0,
        ),
        NSSize::new(w, h),
    )
}

/// The grey that fills the live tab from the left as its page is read: ink at
/// Search's 5.5% over the pill, clipped to the pill so its ends stay round.
fn reading_fill(rect: NSRect, read: f64, dark: bool) {
    let tone = if dark { palette::INK.1 } else { palette::INK.0 };
    // The context is saved and restored around the clip, balanced on this call,
    // so the pill's clip never leaks into what is drawn after the fill.
    NSGraphicsContext::saveGraphicsState_class();
    palette::pill_path(rect).addClip();
    NSColor::colorWithWhite_alpha(tone, 0.055).setFill();
    NSBezierPath::fillRect(NSRect::new(
        rect.origin,
        NSSize::new(rect.size.width * read, rect.size.height),
    ));
    NSGraphicsContext::restoreGraphicsState_class();
}

/// Which ⌘ number picks a tab, as Tap's own `Browser::select_number` counts them:
/// 1–8 by place among all the tabs, 9 always the last. A tab past the eighth that
/// isn't the last has none.
fn shortcut_number(index: usize, count: usize) -> Option<usize> {
    if count == 0 || index >= count {
        return None;
    }
    if index < 8 {
        Some(index + 1)
    } else if index == count - 1 {
        Some(9)
    } else {
        None
    }
}

/// The letter a tab stands for (Search's `Tab.monogram`): its site's first letter
/// once "www." is off, a file's name's, or what it says until it has a site.
fn monogram(tab: &Tab) -> String {
    if let Some(url) = Url::parse(tab.url())
        && let Some(host) = url.host()
    {
        let bare = host.strip_prefix("www.").unwrap_or(&host);
        let found = bare.chars().find(|c| c.is_alphanumeric());
        if let Some(first) = found {
            return first.to_uppercase().collect();
        }
    }
    if let Some(url) = Url::parse(tab.url()).filter(Url::is_file) {
        let name = url
            .path()
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("")
            .to_owned();
        return letter(&name);
    }
    letter(&tab.label())
}

/// Where the page on the tab you're on comes from, said at rest (Search's
/// `AtRest`): its registrable domain, "Not secure" before a plain-http one. Only
/// that much is drawn across the top, as Search's compact line does; the
/// subdomain gives way entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rest {
    not_secure: bool,
    /// The registrable domain (or a file's name), with a non-standard port after it.
    name: String,
    /// Worth saying even with the setting off: plain http, or a name kept in
    /// punycode because it could pass for a Latin one.
    warns: bool,
}

/// The suffixes under which a registrable name takes three labels, the common
/// ones from the public suffix list Search asks the system for. Tap has no list
/// of its own in the core yet, so this stands in and errs toward saying less:
/// a name it doesn't know is cut to its last two labels.
const TWO_LEVEL: [&str; 20] = [
    "ac.uk", "co.jp", "co.kr", "co.nz", "co.uk", "co.in", "com.au", "com.br", "com.cn", "com.hk",
    "com.mx", "com.sg", "com.tw", "gov.uk", "ne.jp", "net.au", "net.cn", "org.au", "org.cn",
    "org.uk",
];

/// A name's registrable part: the last two labels, or three under a two-level
/// suffix; the whole of an address, or of a name too short to cut.
fn registrable(host: &str) -> String {
    if is_address(host) {
        return if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
    }
    let labels: Vec<&str> = host.split('.').collect();
    let keep = if labels.len() >= 3
        && TWO_LEVEL.contains(&labels[labels.len() - 2..].join(".").as_str())
    {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(at + 1).and_then(|b| char::from(*b).to_digit(16)),
                bytes.get(at + 2).and_then(|b| char::from(*b).to_digit(16)),
            )
        {
            // Two hex digits always make a byte.
            out.push(u8::try_from(high * 16 + low).unwrap_or(b'%'));
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the tab you're on says at rest: everything with the setting on, only a
/// warning with it off (Search's `AtRest.said`). A name in another alphabet is
/// never decoded here: `Url::host` hands it over in punycode, and without
/// Search's script-safety check beside the decoder, showing it as it really is
/// (xn--…) is the safe side to err on — a look-alike can never pass for Latin.
fn at_rest(address: &str, always: bool) -> Option<Rest> {
    if address.is_empty() {
        return None;
    }
    let url = Url::parse(address)?;
    let scheme = url.scheme()?.to_lowercase();
    let rest = match scheme.as_str() {
        "http" | "https" => {
            let raw = url.host().filter(|h| !h.is_empty())?;
            let ascii = raw.to_lowercase().trim_matches('.').to_owned();
            let bare =
                if !is_address(&ascii) && ascii.starts_with("www.") && ascii.split('.').count() > 2
                {
                    ascii[4..].to_owned()
                } else {
                    ascii.clone()
                };
            let name = registrable(&bare);
            let standard: i64 = if scheme == "https" { 443 } else { 80 };
            let tail = Parts::of(address)
                .and_then(|parts| parts.port())
                .filter(|port| *port != standard)
                .map_or_else(String::new, |port| format!(":{port}"));
            let not_secure = scheme == "http" && !is_loopback(&ascii);
            Rest {
                // As Search's: any label kept in punycode, the subdomain's too.
                warns: not_secure || bare.split('.').any(|label| label.starts_with("xn--")),
                not_secure,
                name: format!("{name}{tail}"),
            }
        }
        "file" => {
            let name = url
                .path()
                .rsplit('/')
                .find(|part| !part.is_empty())
                .map(percent_decoded)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "File".to_owned());
            Rest {
                not_secure: false,
                name,
                warns: false,
            }
        }
        // Tap's own pages and `about:` keep their titles, as a new tab does.
        "about" | "tap" | "" => return None,
        other if other.contains("extension") => Rest {
            not_secure: false,
            name: "Extension page".to_owned(),
            warns: false,
        },
        _ => Rest {
            not_secure: false,
            name: "Not a website".to_owned(),
            warns: false,
        },
    };
    if always || rest.warns {
        Some(rest)
    } else {
        None
    }
}

/// The rest, drawn in its place: "Not secure" dim before the name, the name in ink.
fn draw_rest(said: &Rest, text: NSRect, dark: bool) {
    if !said.not_secure {
        palette::draw_text(&said.name, text, &palette::INK.resolve(dark));
        return;
    }
    let lead = "Not secure — ";
    let lead_width = palette::text_width(lead);
    palette::draw_text(lead, text, &palette::MUTED.resolve(dark));
    palette::draw_text(
        &said.name,
        NSRect::new(
            NSPoint::new(text.origin.x + lead_width, text.origin.y),
            NSSize::new((text.size.width - lead_width).max(0.0), text.size.height),
        ),
        &palette::INK.resolve(dark),
    );
}

/// The first letter of what a tab says, upper case; a dot for nothing.
fn letter(label: &str) -> String {
    label
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "•".to_owned(), |c| c.to_uppercase().collect())
}

fn cross(square: NSRect, color: &NSColor) {
    let inset = 5.5;
    let path = NSBezierPath::bezierPath();
    let (x0, y0) = (square.origin.x + inset, square.origin.y + inset);
    let (x1, y1) = (
        square.origin.x + square.size.width - inset,
        square.origin.y + square.size.height - inset,
    );
    path.moveToPoint(NSPoint::new(x0, y0));
    path.lineToPoint(NSPoint::new(x1, y1));
    path.moveToPoint(NSPoint::new(x1, y0));
    path.lineToPoint(NSPoint::new(x0, y1));
    path.setLineWidth(1.4);
    color.setStroke();
    path.stroke();
}

fn plus(rect: NSRect, color: &NSColor) {
    let (cx, cy) = (
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    let arm = 5.5;
    let path = NSBezierPath::bezierPath();
    path.moveToPoint(NSPoint::new(cx - arm, cy));
    path.lineToPoint(NSPoint::new(cx + arm, cy));
    path.moveToPoint(NSPoint::new(cx, cy - arm));
    path.lineToPoint(NSPoint::new(cx, cy + arm));
    path.setLineWidth(1.5);
    color.setStroke();
    path.stroke();
}

/// How far the live tab's page has come, as a thin line along the pill's foot.
fn progress(rect: NSRect, done: f64, color: &NSColor) {
    let width = (rect.size.width - 2.0 * palette::RADIUS) * done.clamp(0.08, 1.0);
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new(
            NSPoint::new(
                rect.origin.x + palette::RADIUS,
                rect.origin.y + rect.size.height - 2.5,
            ),
            NSSize::new(width, 2.0),
        ),
        1.0,
        1.0,
    )
    .fill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(count: usize) -> Vec<Slot> {
        vec![Slot::Tab; count]
    }

    #[test]
    fn tabs_keep_their_width_until_the_row_is_full_then_give_way() {
        let wide = layout(1600.0, &tabs(3), None, palette::HELM);
        assert!(
            wide.tabs
                .iter()
                .all(|r| (r.size.width - palette::TAB_WIDTH).abs() < f64::EPSILON)
        );
        assert!((wide.tabs[0].origin.x - palette::LIGHTS).abs() < f64::EPSILON);

        let crowded = layout(900.0, &tabs(30), None, palette::HELM);
        assert!(
            crowded
                .tabs
                .iter()
                .all(|r| r.size.width >= palette::TAB_MIN)
        );
        assert!(crowded.tabs[0].size.width < palette::TAB_WIDTH);
    }

    #[test]
    fn the_live_tab_widens_into_the_field() {
        let row = layout(1600.0, &tabs(3), Some(1), palette::HELM);
        assert!((row.tabs[1].size.width - palette::FIELD).abs() < f64::EPSILON);
        assert!(row.tabs[2].origin.x > row.tabs[1].origin.x + palette::FIELD);
    }

    #[test]
    fn the_plus_stays_before_the_doors_however_many_tabs_there_are() {
        // Few tabs: the plus sits after the last one, as before.
        let few = layout(1600.0, &tabs(3), None, palette::HELM);
        let after_last = few.tabs[2].origin.x + few.tabs[2].size.width + palette::TAB_GAP + 2.0;
        assert!((few.plus.origin.x - after_last).abs() < f64::EPSILON);
        assert!((few.end - (1600.0 - palette::HELM)).abs() < f64::EPSILON);

        // Tabs at their narrowest still overflow: the plus is pinned before
        // the helm instead of running under it, and the tabs stop before it.
        let crowded = layout(900.0, &tabs(30), None, palette::HELM);
        let doors = 900.0 - palette::HELM;
        assert!(crowded.plus.origin.x + palette::PLUS <= doors - 4.0 + f64::EPSILON);
        assert!(crowded.end <= crowded.plus.origin.x - 2.0 + f64::EPSILON);

        // With the downloads door shown, the reserve grows by its width and
        // the plus keeps the same distance from it.
        let trailing = palette::HELM + palette::DOOR + 4.0;
        let fetching = layout(900.0, &tabs(30), None, trailing);
        assert!(fetching.plus.origin.x + palette::PLUS <= 900.0 - trailing - 4.0 + f64::EPSILON);
    }

    #[test]
    fn a_carried_slot_passes_a_neighbour_at_its_middle_and_comes_back_the_same_way() {
        // Three tabs of 100 with gaps of 2; the first carried right.
        let spans = [(0.0, 100.0), (102.0, 202.0), (204.0, 304.0)];
        assert_eq!(
            place_among(&spans, 0, 40.0, 140.0),
            0,
            "not yet past the middle"
        );
        assert_eq!(
            place_among(&spans, 0, 60.0, 160.0),
            1,
            "past the second's middle"
        );
        assert_eq!(place_among(&spans, 0, 170.0, 270.0), 2, "and the third's");
        // Now second, it comes back once its left edge is back over the first's middle.
        assert_eq!(place_among(&spans, 1, 60.0, 160.0), 1);
        assert_eq!(place_among(&spans, 1, 40.0, 140.0), 0);
        // Groups are not all one width: a wide one passes a narrow one by its middle.
        let groups = [(0.0, 300.0), (310.0, 400.0)];
        assert_eq!(place_among(&groups, 0, 50.0, 350.0), 0);
        assert_eq!(place_among(&groups, 0, 60.0, 360.0), 1);
    }

    #[test]
    fn shortcut_numbers_follow_the_tabs_own_counting() {
        assert_eq!(shortcut_number(0, 3), Some(1));
        assert_eq!(shortcut_number(2, 3), Some(3));
        assert_eq!(shortcut_number(7, 20), Some(8));
        assert_eq!(shortcut_number(8, 20), None, "past the eighth and not last");
        assert_eq!(shortcut_number(19, 20), Some(9), "the last is always 9");
        assert_eq!(shortcut_number(8, 9), Some(9));
        assert_eq!(shortcut_number(0, 0), None);
    }

    #[test]
    fn a_pressed_tab_sinks_a_little_and_comes_back() {
        let rect = NSRect::new(NSPoint::new(100.0, 10.0), NSSize::new(186.0, 30.0));
        assert_eq!(sunk(rect, 1.0), rect);
        let down = sunk(rect, 0.97);
        assert!(down.size.width < rect.size.width && down.size.height < rect.size.height);
        assert!(
            down.origin.y > rect.origin.y,
            "and a point down, not just smaller"
        );
        assert!(
            (down.origin.x + down.size.width / 2.0 - 193.0).abs() < 0.01,
            "about its centre"
        );
    }

    #[test]
    fn the_rest_says_where_a_page_is_and_warns_when_it_must() {
        let rest = at_rest("https://gist.github.com/tim", true).expect("a page");
        assert_eq!(
            rest.name, "github.com",
            "the registrable part, subdomain gone"
        );
        assert!(!rest.not_secure && !rest.warns);
        assert_eq!(
            at_rest("https://www.bbc.co.uk/news", true).unwrap().name,
            "bbc.co.uk"
        );
        assert_eq!(
            at_rest("https://example.com:8443/", true).unwrap().name,
            "example.com:8443"
        );
        // With the setting off, an ordinary page says nothing and a plain-http one warns.
        assert_eq!(at_rest("https://example.com/", false), None);
        let plain = at_rest("http://example.com/", false).expect("warns even off");
        assert!(plain.not_secure && plain.warns);
        // This Mac over plain http is the rule rather than a risk.
        assert_eq!(at_rest("http://localhost:3000/", false), None);
        assert_eq!(
            at_rest("http://127.0.0.1:8080/", true).unwrap().name,
            "127.0.0.1:8080"
        );
        // A punycode name is shown as it really is, and counts as a warning.
        let puny = at_rest("https://xn--80ak6aa92e.com/", false).expect("warns even off");
        assert!(puny.warns && puny.name.contains("xn--"));
        assert_eq!(
            at_rest("file:///tmp/My%20Report.pdf", true).unwrap().name,
            "My Report.pdf"
        );
        assert_eq!(at_rest("", true), None);
    }

    #[test]
    fn registrable_names_take_three_labels_under_a_two_level_suffix() {
        assert_eq!(registrable("www.example.co.uk"), "example.co.uk");
        assert_eq!(registrable("a.b.example.com"), "example.com");
        assert_eq!(registrable("example.com"), "example.com");
        assert_eq!(registrable("127.0.0.1"), "127.0.0.1");
        assert_eq!(registrable("::1"), "[::1]");
    }

    #[test]
    fn a_heading_keeps_its_width_and_the_tabs_share_the_rest() {
        let row = layout(
            900.0,
            &[
                Slot::Heading(70.0),
                Slot::Tab,
                Slot::Tab,
                Slot::Heading(50.0),
                Slot::Tab,
            ],
            None,
            palette::HELM,
        );
        assert!((row.tabs[0].size.width - 70.0).abs() < f64::EPSILON);
        assert!((row.tabs[3].size.width - 50.0).abs() < f64::EPSILON);
        assert!(row.tabs[1].size.width > palette::TAB_MIN);
        assert!(row.tabs[4].origin.x > row.tabs[3].origin.x + 50.0);
    }
}
