//! The cards that hang under the chrome: the ground they are drawn on, how far their
//! list is scrolled, and how they come in and go.
//!
//! One card says what is kept, the other what came down. Both are drawn the same way:
//! a shadowed ground, a list in it, and a scroll for a list longer than the card.

use std::cell::{Cell, RefCell};

use objc2::runtime::Sel;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::{
    motion::{Curve, Frames, Glide},
    palette,
};

/// The room left around a card for its shadow.
pub const SHADOW: f64 = 16.0;
/// The card's own padding around its rows.
pub const PAD: f64 = 8.0;

/// A card's own state: how far its list is scrolled, and how far it has come in.
#[derive(Default)]
pub struct State {
    offset: Cell<f64>,
    entrance: RefCell<Glide>,
    frames: Frames,
}

impl State {
    /// The card comes up, on `frame` for its own frames.
    pub fn show(&self, view: &NSView, frame: Sel) {
        let curve = Curve::settle_now();
        if view.isHidden() {
            self.offset.set(0.0);
            self.entrance.borrow_mut().restart(curve);
            view.setHidden(false);
        }
        // Under Reduce Motion the card is simply there: the spring rests at its end.
        view.setAlphaValue(self.entrance.borrow().progress().clamp(0.0, 1.0));
        view.setNeedsDisplay(true);
        if curve != Curve::Instant {
            self.frames.run(view, frame);
        }
    }

    /// The card goes away.
    pub fn hide(&self, view: &NSView) {
        view.setHidden(true);
        self.frames.pause();
        view.setAlphaValue(1.0);
    }

    /// One frame of the card's coming in. Answers whether it is still on its way.
    pub fn tick(&self, view: &NSView, dt: f64) -> bool {
        let moving = self.entrance.borrow_mut().step(dt);
        view.setAlphaValue(self.entrance.borrow().progress().clamp(0.0, 1.0));
        view.setNeedsDisplay(true);
        moving
    }

    /// Seconds since the last frame.
    pub fn dt(&self) -> f64 {
        self.frames.dt()
    }

    /// No more frames until the card is shown again.
    pub fn pause(&self) {
        self.frames.pause();
    }

    /// How far the list is scrolled.
    pub fn offset(&self) -> f64 {
        self.offset.get()
    }

    /// The wheel moves the list, as far as it goes.
    pub fn scroll(&self, delta: f64, max: f64) {
        self.offset.set((self.offset.get() - delta).clamp(0.0, max));
    }

    /// The card's own ground, without the shadow's room around it.
    pub fn card(bounds: NSRect) -> NSRect {
        NSRect::new(
            NSPoint::new(SHADOW, SHADOW),
            NSSize::new(
                bounds.size.width - 2.0 * SHADOW,
                bounds.size.height - 2.0 * SHADOW,
            ),
        )
    }
}

/// The card's ground, with its shadow, and a hairline edge: a card hangs over a
/// page of any colour, and has to read against it.
pub fn ground(card: NSRect, dark: bool) {
    palette::shadowed(&palette::float_shadow(dark), || {
        palette::GROUND.resolve(dark).setFill();
        palette::card_path(card).fill();
    });
    palette::stroke_hairline(&palette::card_path(card), dark);
}
