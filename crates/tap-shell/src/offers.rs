//! The rows under the field (Search's `Omnibox.swift` list): the places it offers, best
//! first, and a search last when what is typed can't be a place. The row the arrow keys
//! picked is on the live tab's grey. A click goes there.

use std::{
    cell::{Cell, RefCell},
    time::Instant,
};

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObject, sel,
};
use objc2_app_kit::{NSEvent, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize};
use objc2_quartz_core::CADisplayLink;
use tap_core::{
    browser::Browser,
    history::{Kind, Suggestion},
};

use crate::{
    motion::{self, Curve, Glide},
    palette, rt,
};

/// A row's height, and the card's padding around the rows.
pub const ROW: f64 = 34.0;
const PAD: f64 = 6.0;
/// Room left around the card for its shadow.
pub const SHADOW: f64 = 16.0;

/// The card's height for `rows` rows, its shadow's room included.
#[must_use]
pub fn height(rows: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)] // a handful of rows
    let rows = rows as f64;
    rows * ROW + 2.0 * PAD + 2.0 * SHADOW
}

#[derive(Default)]
struct Selection {
    picked: Option<usize>,
    from: f64,
    to: f64,
    glide: Glide,
}

impl Selection {
    fn position(&self) -> f64 {
        self.from + (self.to - self.from) * self.glide.progress().clamp(0.0, 1.0)
    }

    fn pick(&mut self, picked: Option<usize>, curve: Curve) -> bool {
        if picked == self.picked {
            return false;
        }
        let from = self.position();
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let to = picked.unwrap_or_default() as f64;
        let slide = self.picked.is_some() && picked.is_some() && curve != Curve::LIGHT;
        self.from = if slide { from } else { to };
        self.to = to;
        self.picked = picked;
        self.glide
            .restart(if slide { curve } else { Curve::Instant });
        slide && curve != Curve::Instant
    }
}

pub struct Ivars {
    selection: RefCell<Selection>,
    entrance: RefCell<Glide>,
    link: RefCell<Option<Retained<CADisplayLink>>>,
    last_frame: Cell<Option<Instant>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapOffers"]
    #[ivars = Ivars]
    pub struct Offers;

    impl Offers {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(offersFrame:))]
        fn offers_frame(&self, _link: &CADisplayLink) {
            self.frame_tick();
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // The field keeps the keyboard: this view never takes it, so a click on a
            // row is a choice, not a click away.
            if let Some(row) = self.row_at(event) {
                rt::dispatch(move |browser| {
                    browser.pick_row(row);
                    browser.submit();
                });
            }
        }

        /// Only the card takes clicks; its shadow's room lets them through to whatever
        /// is under it.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: a plain read of a live view's superview.
            let holder = unsafe { self.superview() };
            let local = self.convertPoint_fromView(point, holder.as_deref());
            let card = self.card();
            let inside = local.x >= card.origin.x
                && local.x < card.origin.x + card.size.width
                && local.y >= card.origin.y
                && local.y < card.origin.y + card.size.height;
            // SAFETY: the superclass's own hit test, for a point inside the card.
            inside.then(|| unsafe { msg_send![super(self), hitTest: point] }).flatten()
        }
    }
);

impl Offers {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            selection: RefCell::new(Selection::default()),
            entrance: RefCell::new(Glide::default()),
            link: RefCell::new(None),
            last_frame: Cell::new(None),
        });
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    pub fn show(&self, picked: Option<usize>) {
        let curve = Curve::settle_now();
        let opening = self.isHidden();
        if opening {
            *self.ivars().selection.borrow_mut() = Selection::default();
            self.ivars().entrance.borrow_mut().restart(curve);
            self.setAlphaValue(self.ivars().entrance.borrow().progress().clamp(0.0, 1.0));
            self.setHidden(false);
        }
        let moving = self.ivars().selection.borrow_mut().pick(picked, curve);
        if (opening && curve != Curve::Instant) || moving {
            self.animate();
        }
        self.setNeedsDisplay(true);
    }

    pub fn hide(&self) {
        self.setHidden(true);
        if let Some(link) = self.ivars().link.borrow().as_ref() {
            link.setPaused(true);
        }
        self.ivars().last_frame.set(None);
        self.setAlphaValue(1.0);
    }

    fn animate(&self) {
        let mut slot = self.ivars().link.borrow_mut();
        if slot.is_none() {
            let target: &NSObject = self.as_ref();
            // SAFETY: offersFrame: is this view's own display-link callback.
            let link = unsafe { self.displayLinkWithTarget_selector(target, sel!(offersFrame:)) };
            // SAFETY: the main run loop and common modes keep frames running during input.
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
            .map_or(1.0 / 60.0, |then| {
                now.duration_since(then).as_secs_f64().min(1.0 / 15.0)
            });
        if motion::reduced() {
            self.ivars().entrance.borrow_mut().restart(Curve::Instant);
            self.ivars()
                .selection
                .borrow_mut()
                .glide
                .restart(Curve::Instant);
        } else if motion::light() {
            self.ivars()
                .selection
                .borrow_mut()
                .glide
                .restart(Curve::Instant);
        }
        let entering = self.ivars().entrance.borrow_mut().step(dt);
        let moving = self.ivars().selection.borrow_mut().glide.step(dt);
        self.setAlphaValue(self.ivars().entrance.borrow().progress().clamp(0.0, 1.0));
        self.setNeedsDisplay(true);
        if !entering && !moving {
            if let Some(link) = self.ivars().link.borrow().as_ref() {
                link.setPaused(true);
            }
            self.ivars().last_frame.set(None);
        }
    }

    fn card(&self) -> NSRect {
        let bounds = self.bounds();
        NSRect::new(
            NSPoint::new(SHADOW, SHADOW),
            NSSize::new(
                bounds.size.width - 2.0 * SHADOW,
                bounds.size.height - 2.0 * SHADOW,
            ),
        )
    }

    fn row_rect(&self, row: usize) -> NSRect {
        let card = self.card();
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let top = card.origin.y + PAD + row as f64 * ROW;
        NSRect::new(
            NSPoint::new(card.origin.x + PAD, top),
            NSSize::new(card.size.width - 2.0 * PAD, ROW),
        )
    }

    fn row_at(&self, event: &NSEvent) -> Option<usize> {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let count = rt::read(|b| b.editing().map_or(0, |e| e.offers.len())).unwrap_or(0);
        (0..count).find(|&row| {
            let r = self.row_rect(row);
            point.y >= r.origin.y
                && point.y < r.origin.y + r.size.height
                && point.x >= r.origin.x
                && point.x < r.origin.x + r.size.width
        })
    }

    fn draw(&self) {
        let dark = palette::is_dark(self);
        let Some((offers, picked)) =
            rt::read(|browser: &Browser| browser.editing().map(|e| (e.offers.clone(), e.picked)))
                .flatten()
        else {
            return;
        };
        if offers.is_empty() {
            return;
        }
        palette::shadowed(&palette::float_shadow(dark), || {
            palette::GROUND.resolve(dark).setFill();
            palette::card_path(self.card()).fill();
        });

        if let Some(row) = picked.filter(|&row| row < offers.len()) {
            let mut rect = self.row_rect(row);
            let selection = self.ivars().selection.borrow();
            if selection.picked == Some(row) {
                rect.origin.y = self.card().origin.y + PAD + selection.position() * ROW;
            }
            palette::WASH.resolve(dark).setFill();
            let plate = palette::pill_path(rect);
            plate.fill();
            palette::stroke_hairline(&plate, dark);
        }
        for (row, offer) in offers.iter().enumerate() {
            draw_row(offer, self.row_rect(row), dark);
        }
    }
}

/// One row: what it would go to, then what it is, quieter.
fn draw_row(offer: &Suggestion, rect: NSRect, dark: bool) {
    let line = palette::text_line(rect, 12.0, 12.0);
    let (main, detail) = match offer.kind {
        Kind::Search => (offer.key.clone(), format!("Search {}", offer.title)),
        Kind::Visited | Kind::Known => (offer.key.clone(), offer.title.clone()),
    };
    let main_width = palette::text_width(&main).min(line.size.width * 0.62);
    palette::draw_text(
        &main,
        NSRect::new(line.origin, NSSize::new(main_width, line.size.height)),
        &palette::INK.resolve(dark),
    );
    if !detail.is_empty() {
        palette::draw_text(
            &format!("— {detail}"),
            palette::text_line(line, main_width + 10.0, 0.0),
            &palette::MUTED.resolve(dark),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_slides_and_retargets_from_its_current_position() {
        let mut selection = Selection::default();
        assert!(!selection.pick(Some(0), Curve::SETTLE));
        assert!(selection.pick(Some(2), Curve::SETTLE));
        selection.glide.step(1.0 / 60.0);
        let in_between = selection.position();
        assert!(in_between > 0.0 && in_between < 2.0);
        assert!(selection.pick(Some(1), Curve::SETTLE));
        assert!((selection.position() - in_between).abs() < f64::EPSILON);
    }

    #[test]
    fn reduced_and_light_motion_put_selection_in_place() {
        for curve in [Curve::Instant, Curve::LIGHT] {
            let mut selection = Selection::default();
            selection.pick(Some(0), curve);
            assert!(!selection.pick(Some(2), curve));
            assert!((selection.position() - 2.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn clearing_and_reselecting_does_not_slide_from_an_old_row() {
        let mut selection = Selection::default();
        selection.pick(Some(0), Curve::SETTLE);
        selection.pick(Some(2), Curve::SETTLE);
        selection.pick(None, Curve::SETTLE);
        assert!(!selection.pick(Some(1), Curve::SETTLE));
        assert!((selection.position() - 1.0).abs() < f64::EPSILON);
    }
}
