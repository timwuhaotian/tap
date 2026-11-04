//! The only chrome there is: the row of tabs at the top of the window (Search's
//! `TabBar.swift`). Titles, the live one on a grey pill, a cross on the one under the
//! pointer, the plus after the last, and the field in place of the live tab while you
//! type. Drawn as one view: a row of three hundred tabs is one draw, not three hundred
//! views.

use std::cell::{Cell, RefCell};

use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::NSObject,
};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSEvent, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tap_core::browser::{Browser, TabId};

use crate::{palette, rt};

/// What the pointer is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Spot {
    #[default]
    Nothing,
    Tab(usize),
    Close(usize),
    Plus,
}

/// Where everything in the row goes for a width.
struct Layout {
    tabs: Vec<NSRect>,
    plus: NSRect,
}

/// The row's geometry: tabs from the lights, each its fixed width until the row is full,
/// the live tab as wide as the field while it is being edited.
fn layout(width: f64, count: usize, editing: Option<usize>) -> Layout {
    let top = (palette::STRIP - palette::ROW) / 2.0;
    let room = (width - palette::LIGHTS - palette::HELM - palette::PLUS - 12.0).max(0.0);
    #[allow(clippy::cast_precision_loss)] // a row never holds 2^52 tabs
    let gaps = palette::TAB_GAP * count.saturating_sub(1) as f64;
    let field = editing.map_or(0.0, |_| palette::FIELD.min(room * 0.6));
    let others = if editing.is_some() {
        count.saturating_sub(1)
    } else {
        count
    };
    #[allow(clippy::cast_precision_loss)]
    let each = if others == 0 {
        palette::TAB_WIDTH
    } else {
        ((room - field - gaps) / others as f64).clamp(palette::TAB_MIN, palette::TAB_WIDTH)
    };
    let mut x = palette::LIGHTS;
    let mut tabs = Vec::with_capacity(count);
    for index in 0..count {
        let w = if editing == Some(index) {
            field.max(each)
        } else {
            each
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

fn editing_index(browser: &Browser) -> Option<usize> {
    let editing = browser.editing()?;
    if editing.launching {
        return None;
    }
    let active = browser.active()?;
    browser.tabs().iter().position(|t| t.id == active)
}

pub struct Ivars {
    spot: Cell<Spot>,
    inside: Cell<bool>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
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
        });
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Where the live tab is drawn, for the field to sit in while it is edited.
    pub fn field_rect(&self) -> Option<NSRect> {
        let width = self.bounds().size.width;
        rt::read(|browser| {
            let index = editing_index(browser)?;
            let rect = layout(width, browser.tabs().len(), Some(index))
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
            let editing = editing_index(browser);
            let layout = layout(width, browser.tabs().len(), editing);
            for (index, rect) in layout.tabs.iter().enumerate() {
                if contains(*rect, point) {
                    let can_close =
                        editing != Some(index) && rect.size.width >= palette::TAB_TITLED;
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
        match self.spot_at(event) {
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
        let _ = rt::read(|browser| {
            let editing = editing_index(browser);
            let layout = layout(bounds.size.width, browser.tabs().len(), editing);
            let active = browser.active();
            for (index, (tab, rect)) in browser.tabs().iter().zip(&layout.tabs).enumerate() {
                let live = Some(tab.id) == active;
                let under = matches!(spot, Spot::Tab(i) | Spot::Close(i) if i == index);
                if live || under {
                    let grey = if live { palette::WASH } else { palette::HOVER };
                    grey.resolve(dark).setFill();
                    pill(*rect).fill();
                }
                if editing == Some(index) {
                    continue;
                }
                let crossed = (under || live) && rect.size.width >= palette::TAB_TITLED;
                if rect.size.width >= palette::TAB_TITLED {
                    let right = if crossed { 30.0 } else { 12.0 };
                    let text = NSRect::new(
                        NSPoint::new(rect.origin.x + 12.0, rect.origin.y + 7.0),
                        NSSize::new(rect.size.width - 12.0 - right, 17.0),
                    );
                    let ink = if live { palette::INK } else { palette::MUTED };
                    palette::draw_text(&tab.label(), text, &ink.resolve(dark));
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

    #[test]
    fn tabs_keep_their_width_until_the_row_is_full_then_give_way() {
        let wide = layout(1600.0, 3, None);
        assert!(
            wide.tabs
                .iter()
                .all(|r| (r.size.width - palette::TAB_WIDTH).abs() < f64::EPSILON)
        );
        assert!((wide.tabs[0].origin.x - palette::LIGHTS).abs() < f64::EPSILON);

        let crowded = layout(900.0, 30, None);
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
        let row = layout(1600.0, 3, Some(1));
        assert!((row.tabs[1].size.width - palette::FIELD).abs() < f64::EPSILON);
        assert!(row.tabs[2].origin.x > row.tabs[1].origin.x + palette::FIELD);
    }
}
