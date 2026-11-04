//! The rows under the field (Search's `Omnibox.swift` list): the places it offers, best
//! first, and a search last when what is typed can't be a place. The row the arrow keys
//! picked is on the live tab's grey. A click goes there.

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::NSObject,
};
use objc2_app_kit::{NSEvent, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tap_core::{
    browser::Browser,
    history::{Kind, Suggestion},
};

use crate::{palette, rt};

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

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapOffers"]
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
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
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

        for (row, offer) in offers.iter().enumerate() {
            let rect = self.row_rect(row);
            if picked == Some(row) {
                palette::WASH.resolve(dark).setFill();
                let plate = palette::pill_path(rect);
                plate.fill();
                palette::stroke_hairline(&plate, dark);
            }
            draw_row(offer, rect, dark);
        }
    }
}

/// One row: what it would go to, then what it is, quieter.
fn draw_row(offer: &Suggestion, rect: NSRect, dark: bool) {
    let text_top = rect.origin.y + (ROW - 17.0) / 2.0;
    let left = rect.origin.x + 12.0;
    let width = rect.size.width - 24.0;
    let (main, detail) = match offer.kind {
        Kind::Search => (offer.key.clone(), format!("Search {}", offer.title)),
        Kind::Visited | Kind::Known => (offer.key.clone(), offer.title.clone()),
    };
    let main_width = palette::text_width(&main).min(width * 0.62);
    palette::draw_text(
        &main,
        NSRect::new(NSPoint::new(left, text_top), NSSize::new(main_width, 17.0)),
        &palette::INK.resolve(dark),
    );
    if !detail.is_empty() {
        let x = left + main_width + 10.0;
        palette::draw_text(
            &format!("— {detail}"),
            NSRect::new(
                NSPoint::new(x, text_top),
                NSSize::new((left + width - x).max(0.0), 17.0),
            ),
            &palette::MUTED.resolve(dark),
        );
    }
}
