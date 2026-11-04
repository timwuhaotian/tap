//! The only chrome there is: the row of tabs at the top of the window (Search's
//! `TabBar.swift`). Titles, the live one on a grey pill, a cross on the one under the
//! pointer, the plus after the last, and the field in place of the live tab while you
//! type. A tab group's name heads its tabs; a click on it folds the group to its name
//! (and the tab in front, if it is one of its own) or opens it again. Drawn as one view:
//! a row of three hundred tabs is one draw, not three hundred views.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use block2::RcBlock;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::NSObject,
};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSEventModifierFlags,
    NSGraphicsContext, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSPoint, NSProcessInfo, NSRect, NSSize, NSTimer};
use tap_core::{
    browser::{Browser, Tab, TabId},
    fnd::{Parts, Url},
    groups::Item,
    settings_index,
};

use crate::{icons, palette, prefs, rt, web};

/// The strip's look, read from Kept on every draw (and on every tick), so a Settings
/// change shows at once without anyone having to tell the strip (Search reads its
/// observable `Preferences` the same way). Keys and defaults are Search's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // five independent Settings switches, read together
struct Look {
    icons: bool,
    reading: bool,
    give: bool,
    shortcuts: bool,
    address: bool,
    /// Settings › Motion, while the Mac saves power: the give below lands at
    /// once instead of easing (Search's `Motion.light`: its springs become a
    /// short ease-out; the strip's one motion is this sink).
    light: bool,
}

/// Low Power Mode, or hot enough that macOS holds its processors back
/// (Search's `Motion.light` reads the same two, at the moment of moving).
fn power_saving() -> bool {
    let info = NSProcessInfo::processInfo();
    info.isLowPowerModeEnabled() || info.thermalState().0 >= 2
}

fn look() -> Look {
    let kept = prefs::Kept::new();
    Look {
        icons: settings_index::text(&kept, "glyph") == "icons",
        reading: settings_index::flag(&kept, "tabs.reading"),
        give: settings_index::flag(&kept, "tabs.give"),
        shortcuts: settings_index::flag(&kept, "tabs.shortcuts"),
        address: settings_index::flag(&kept, "tabs.address"),
        light: settings_index::flag(&kept, "motion.light") && power_saving(),
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
/// change land without a model change to redraw on.
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

/// Where everything in the row goes for a width, one rectangle per slot.
struct Layout {
    tabs: Vec<NSRect>,
    plus: NSRect,
}

/// A heading's width: its name, with air on both sides.
fn heading_width(name: &str) -> f64 {
    (palette::text_width(name) + 20.0)
        .ceil()
        .min(palette::TAB_WIDTH)
}

/// The row's geometry: tabs from the lights, each its fixed width until the row is full,
/// the live tab as wide as the field while it is being edited; headings keep their own
/// width. `editing` is the live tab's slot.
fn layout(width: f64, slots: &[Slot], editing: Option<usize>) -> Layout {
    let count = slots.len();
    let top = (palette::STRIP - palette::ROW) / 2.0;
    let room = (width - palette::LIGHTS - palette::HELM - palette::PLUS - 12.0).max(0.0);
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
    let plus = NSRect::new(
        NSPoint::new(x + 2.0, top),
        NSSize::new(palette::PLUS, palette::ROW),
    );
    Layout { tabs, plus }
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

/// The live tab's slot, while the field is open on it.
fn editing_slot(browser: &Browser, row: &[Item]) -> Option<usize> {
    let editing = browser.editing()?;
    if editing.launching {
        return None;
    }
    let active = browser.active()?;
    let index = browser.tabs().iter().position(|t| t.id == active)?;
    row.iter().position(|item| *item == Item::Tab(index))
}

pub struct Ivars {
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

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            // The press is over wherever it lands; the tick eases the tab back up.
            if self.ivars().pressed.take().is_some() {
                self.setNeedsDisplay(true);
            }
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
    }
);

impl Strip {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            spot: Cell::new(Spot::Nothing),
            inside: Cell::new(false),
            tracking: RefCell::new(None),
            pressed: Cell::new(None),
            press_scale: Cell::new(1.0),
            cmd_held: Cell::new(false),
            last_look: Cell::new(None),
            poll_ticks: Cell::new(0),
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
            // While the Mac saves power the sink is a step, not an ease.
            let step = if look.light {
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

    /// Where the live tab is drawn, for the field to sit in while it is edited.
    pub fn field_rect(&self) -> Option<NSRect> {
        let width = self.bounds().size.width;
        rt::read(|browser| {
            let (row, slots) = slots(browser);
            let index = editing_slot(browser, &row)?;
            let rect = layout(width, &slots, Some(index))
                .tabs
                .get(index)
                .copied()?;
            Some(NSRect::new(
                NSPoint::new(rect.origin.x + 10.0, rect.origin.y + 5.0),
                NSSize::new(rect.size.width - 20.0, rect.size.height - 10.0),
            ))
        })
        .flatten()
    }

    fn spot_at(&self, event: &NSEvent) -> Spot {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let width = self.bounds().size.width;
        rt::read(|browser| {
            let (row, slots) = slots(browser);
            let editing = editing_slot(browser, &row);
            let layout = layout(width, &slots, editing);
            for (slot, (item, rect)) in row.iter().zip(&layout.tabs).enumerate() {
                if contains(*rect, point) {
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
            if contains(layout.plus, point) {
                Spot::Plus
            } else {
                Spot::Nothing
            }
        })
        .unwrap_or_default()
    }

    fn hover(&self, event: Option<&NSEvent>) {
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
        // A tab (or its cross) goes down under the press; the tick sinks it if
        // Settings says tabs give, and `mouseUp:` lets it back up.
        if let Spot::Tab(index) | Spot::Close(index) = spot {
            self.ivars().pressed.set(Some(index));
            self.setNeedsDisplay(true);
        }
        match spot {
            Spot::Close(index) => {
                if let Some(id) = tab_at(index) {
                    rt::dispatch(move |browser| browser.close(id));
                }
            }
            Spot::Tab(index) => {
                let Some(id) = tab_at(index) else { return };
                // The live tab, clicked, becomes the field; any other is picked.
                rt::dispatch(move |browser| {
                    if browser.active() == Some(id) {
                        if browser.editing().is_none() {
                            browser.begin_editing(false);
                        }
                    } else {
                        browser.select(id);
                    }
                });
            }
            Spot::Heading(place) => rt::dispatch(move |browser| browser.toggle_group(place)),
            Spot::Plus => rt::dispatch(|browser| browser.begin_editing(true)),
            Spot::Nothing => {
                // The empty row is title bar: it moves the window, and a double click
                // zooms it.
                let Some(window) = self.window() else { return };
                if event.clickCount() == 2 {
                    window.performZoom(None);
                } else {
                    window.performWindowDragWithEvent(event);
                }
            }
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

    #[allow(clippy::too_many_lines)] // the row, left to right, in one pass
    fn draw(&self) {
        let dark = palette::is_dark(self);
        let bounds = self.bounds();
        palette::GROUND.resolve(dark).setFill();
        NSBezierPath::fillRect(bounds);
        // The line between the row and the page.
        palette::HAIRLINE.resolve(dark).setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0.0, bounds.size.height - 1.0),
            NSSize::new(bounds.size.width, 1.0),
        ));

        let spot = self.ivars().spot.get();
        let inside = self.ivars().inside.get();
        // Read on every draw, so a Settings change is in the next frame (see `Look`).
        let look = look();
        let pressed = self.ivars().pressed.get();
        let press_scale = self.ivars().press_scale.get();
        let cmd_held = self.ivars().cmd_held.get();
        // A row too long for the window stops at the doors rather than running under them.
        NSBezierPath::clipRect(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(bounds.size.width - palette::HELM, bounds.size.height),
        ));
        let _ = rt::read(|browser| {
            let (row, slots) = slots(browser);
            let editing = editing_slot(browser, &row);
            let layout = layout(bounds.size.width, &slots, editing);
            let active = browser.active();
            for (slot, (item, rect)) in row.iter().zip(&layout.tabs).enumerate() {
                let index = match item {
                    Item::Heading(place) => {
                        if spot == Spot::Heading(*place) {
                            palette::HOVER.resolve(dark).setFill();
                            pill(*rect).fill();
                        }
                        let name = browser.groups().get(*place).map_or("", |g| g.name.as_str());
                        let text = NSRect::new(
                            NSPoint::new(rect.origin.x + 10.0, rect.origin.y + 7.0),
                            NSSize::new(rect.size.width - 20.0, 17.0),
                        );
                        palette::draw_text(name, text, &palette::MUTED.resolve(dark));
                        continue;
                    }
                    Item::Tab(index) => *index,
                };
                let Some(tab) = browser.tabs().get(index) else {
                    continue;
                };
                let live = Some(tab.id) == active;
                let under = matches!(spot, Spot::Tab(i) | Spot::Close(i) if i == index);
                // A pressed tab sinks where it stands; its hit rect stays the laid-out
                // one, as SwiftUI's scale leaves the row's geometry alone.
                let rect = &if pressed == Some(index) && editing != Some(slot) {
                    sunk(*rect, press_scale)
                } else {
                    *rect
                };
                if live || under {
                    let grey = if live { palette::WASH } else { palette::HOVER };
                    grey.resolve(dark).setFill();
                    pill(*rect).fill();
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
                if editing == Some(slot) {
                    continue;
                }
                let crossed = (under || live) && titled;
                let number = if cmd_held {
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
                    palette::HAIRLINE.resolve(dark).setFill();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(square, 4.0, 4.0)
                        .fill();
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
                        NSSize::new(rect.size.width - left - right, 17.0),
                    );
                    let ink = if live { palette::INK } else { palette::MUTED };
                    // The tab you're on says where its page is instead of its title,
                    // and the title comes back under the pointer (Search's
                    // `TitleAtRest`): the two swap in the same rect.
                    let said = if live {
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
                    if spot == Spot::Close(index) {
                        palette::HAIRLINE.resolve(dark).setFill();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(square, 5.0, 5.0)
                            .fill();
                    }
                    cross(square, &palette::MUTED.resolve(dark));
                }
                if live && tab.loading() {
                    progress(*rect, tab.progress(), &palette::MUTED.resolve(dark));
                }
            }
            if inside {
                if spot == Spot::Plus {
                    palette::HOVER.resolve(dark).setFill();
                    pill(layout.plus).fill();
                }
                plus(layout.plus, &palette::MUTED.resolve(dark));
            }
        });
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
    pill(rect).addClip();
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

fn is_address(host: &str) -> bool {
    if host.contains(':') {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() == 4 && labels.iter().all(|label| label.parse::<u8>().is_ok())
}

fn is_loopback(host: &str) -> bool {
    // 127.x as an address only: 127.0.0.1.example.com is a website like any other.
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || (is_address(host) && host.starts_with("127."))
}

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

fn pill(rect: NSRect) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, palette::RADIUS, palette::RADIUS)
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
        let wide = layout(1600.0, &tabs(3), None);
        assert!(
            wide.tabs
                .iter()
                .all(|r| (r.size.width - palette::TAB_WIDTH).abs() < f64::EPSILON)
        );
        assert!((wide.tabs[0].origin.x - palette::LIGHTS).abs() < f64::EPSILON);

        let crowded = layout(900.0, &tabs(30), None);
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
        let row = layout(1600.0, &tabs(3), Some(1));
        assert!((row.tabs[1].size.width - palette::FIELD).abs() < f64::EPSILON);
        assert!(row.tabs[2].origin.x > row.tabs[1].origin.x + palette::FIELD);
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
        );
        assert!((row.tabs[0].size.width - 70.0).abs() < f64::EPSILON);
        assert!((row.tabs[3].size.width - 50.0).abs() < f64::EPSILON);
        assert!(row.tabs[1].size.width > palette::TAB_MIN);
        assert!(row.tabs[4].origin.x > row.tabs[3].origin.x + 50.0);
    }
}
