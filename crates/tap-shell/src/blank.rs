//! What the stage shows where there is no page to show yet: a small, quiet mark —
//! a dot with rings rippling slowly out of it, a tap on still water.
//!
//! Search leaves a new tab empty ground; Tap draws this there instead, and keeps it
//! up while a tab's first page is on its way, so a new tab or a link opened in one
//! never flashes WebKit's empty white. While it waits the rings quicken, the dot takes
//! the accent and the site's name sits under it; once the page arrives the ground
//! fades away over it. Pure ambience: nothing clickable, no model state of its own,
//! no WebKit view. Its frames run only while it is in front, and redraw only the
//! mark; Reduce Motion (and light motion) get a still frame and no fades (plan
//! §5.1.6, `motion::reduced`).

use std::cell::{Cell, RefCell};

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObject, sel,
};
use objc2_app_kit::{NSBezierPath, NSColor, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CADisplayLink;

use crate::{
    motion::{self, Curve, Follow, Frames},
    palette,
};

/// How many rings are out at once.
pub const RINGS: usize = 3;
/// Seconds for one ring to travel out, at rest and while a page is on its way.
pub const CALM: f64 = 7.2;
pub const BUSY: f64 = 2.1;
/// Where a ring is born and where it has faded by, in points from the dot.
pub const INNER: f64 = 5.0;
pub const OUTER: f64 = 38.0;
/// How far under the mark the site's name sits.
const LABEL_GAP: f64 = 16.0;

/// What the ground shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    /// Nothing: the tab in front has a page (or a page of Tap's own) to show.
    Away,
    /// A blank tab: the mark at rest.
    Calm,
    /// A tab's first page is on its way to `host`; `busy` while it is loading.
    Waiting { host: String, busy: bool },
}

/// Ring `k` at `phase` (0 to 1, one ring's trip): how far out and how strong. It
/// eases out as a ripple does, and comes in from nothing so none is born at full
/// strength.
#[must_use]
pub fn ring(phase: f64, k: usize) -> (f64, f64) {
    #[allow(clippy::cast_precision_loss)] // three rings
    let p = (phase + k as f64 / RINGS as f64).rem_euclid(1.0);
    let eased = 1.0 - (1.0 - p).powi(3);
    let radius = INNER + (OUTER - INNER) * eased;
    let strength = (p / 0.12).min(1.0) * (1.0 - p).powf(1.4);
    (radius, strength)
}

/// Where the mark's dot sits in a stage of `size`: across the middle, a little above
/// halfway down, where the eye puts the middle of a page.
#[must_use]
pub fn centre(size: NSSize) -> NSPoint {
    NSPoint::new(size.width / 2.0, (size.height * 0.44).round())
}

/// The band the mark and its label are drawn in, all the frames ever redraw.
fn band(size: NSSize) -> NSRect {
    let mid = centre(size);
    NSRect::new(
        NSPoint::new(0.0, mid.y - OUTER - 4.0),
        NSSize::new(
            size.width,
            2.0 * OUTER + LABEL_GAP + 2.0 * palette::line_height(),
        ),
    )
}

pub struct Ivars {
    /// How far the rings have come, 0 to 1, wrapping.
    phase: Cell<f64>,
    /// 0 at rest, 1 while a page loads: the rings' pace and the dot's colour.
    busy: RefCell<Follow>,
    /// How strongly the site's name shows.
    label: RefCell<Follow>,
    host: RefCell<String>,
    /// The whole ground's alpha: 1 in front, to 0 as a page arrives over it.
    veil: RefCell<Follow>,
    /// Drawn still: Reduce Motion or light motion when it was last shown.
    still: Cell<bool>,
    frames: Frames,
}

define_class!(
    /// The ground behind a blank tab's field, and in front of a first page loading.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBlank"]
    #[ivars = Ivars]
    pub struct BlankView;

    impl BlankView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Not opaque: it fades over the page as the page arrives.
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            draw_in(self, dirty);
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }

        /// One frame of motion, from the display link.
        #[unsafe(method(blankFrame:))]
        fn blank_frame(&self, _link: &CADisplayLink) {
            self.frame_tick();
        }
    }
);

impl BlankView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            phase: Cell::new(0.0),
            busy: RefCell::new(Follow::at(0.0)),
            label: RefCell::new(Follow::at(0.0)),
            host: RefCell::new(String::new()),
            veil: RefCell::new(Follow::at(1.0)),
            still: Cell::new(false),
            frames: Frames::default(),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Its own layer, so the fade is the layer's opacity, not a redraw.
        view.setWantsLayer(true);
        view
    }

    /// Shows `ground`. `fade`: the page it gives way to arrived in the same tab, so
    /// the ground fades over it; anything else (another tab in front) is at once.
    pub fn show(&self, ground: &Ground, fade: bool) {
        let ivars = self.ivars();
        let still = motion::reduced() || motion::light();
        ivars.still.set(still);
        let (busy, label) = match ground {
            Ground::Away => {
                if self.isHidden() {
                    return;
                }
                if fade && !still {
                    ivars.veil.borrow_mut().aim(0.0, Curve::FADE);
                    ivars.frames.run(self, sel!(blankFrame:));
                } else {
                    self.put_away();
                }
                return;
            }
            Ground::Calm => (0.0, 0.0),
            Ground::Waiting { host, busy } => {
                ivars.host.replace(host.clone());
                (if *busy { 1.0 } else { 0.0 }, 1.0)
            }
        };
        let fresh = self.isHidden() || ivars.veil.borrow().target() < 1.0;
        ivars.veil.borrow_mut().set(1.0);
        self.setAlphaValue(1.0);
        self.setHidden(false);
        if fresh || still {
            // Coming to the front: as it is, not easing in from the last tab's look.
            ivars.busy.borrow_mut().set(busy);
            ivars.label.borrow_mut().set(label);
        } else {
            ivars.busy.borrow_mut().aim(busy, Curve::FADE);
            ivars.label.borrow_mut().aim(label, Curve::FADE);
        }
        self.setNeedsDisplay(true);
        if still {
            ivars.frames.pause();
        } else {
            ivars.frames.run(self, sel!(blankFrame:));
        }
    }

    /// Hidden, with no frames, and whole again for the next time it is shown.
    fn put_away(&self) {
        let ivars = self.ivars();
        ivars.frames.pause();
        self.setHidden(true);
        ivars.veil.borrow_mut().set(1.0);
        self.setAlphaValue(1.0);
        ivars.phase.set(0.0);
    }

    fn frame_tick(&self) {
        let ivars = self.ivars();
        let dt = ivars.frames.dt();
        let busy = ivars.busy.borrow().value().clamp(0.0, 1.0);
        let pace = (1.0 - busy) / CALM + busy / BUSY;
        ivars
            .phase
            .set((ivars.phase.get() + dt * pace).rem_euclid(1.0));
        ivars.busy.borrow_mut().step(dt);
        ivars.label.borrow_mut().step(dt);
        let fading = ivars.veil.borrow_mut().step(dt);
        let veil = ivars.veil.borrow().value().clamp(0.0, 1.0);
        if ivars.veil.borrow().target() <= 0.0 && (!fading || veil <= 0.001) {
            self.put_away();
            return;
        }
        if fading {
            self.setAlphaValue(veil);
        }
        self.setNeedsDisplayInRect(band(self.bounds().size));
    }
}

fn dot(at: NSPoint, r: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(at.x - r, at.y - r),
        NSSize::new(2.0 * r, 2.0 * r),
    ))
    .fill();
}

fn ring_path(at: NSPoint, r: f64, color: &NSColor, width: f64) {
    color.setStroke();
    let path = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(at.x - r, at.y - r),
        NSSize::new(2.0 * r, 2.0 * r),
    ));
    path.setLineWidth(width);
    path.stroke();
}

/// The ground, then the rings, the dot, and the site's name while one is awaited.
fn draw_in(view: &BlankView, dirty: NSRect) {
    let dark = palette::is_dark(view);
    palette::GROUND.resolve(dark).setFill();
    NSBezierPath::fillRect(dirty);

    let ivars = view.ivars();
    let bounds = view.bounds();
    let mid = centre(bounds.size);
    let busy = ivars.busy.borrow().value().clamp(0.0, 1.0);
    // A still frame shows the rings part way out, as a photograph of the ripple.
    let phase = if ivars.still.get() {
        0.18
    } else {
        ivars.phase.get()
    };
    let muted = palette::MUTED.resolve(dark);
    for k in 0..RINGS {
        let (radius, strength) = ring(phase, k);
        let alpha = strength * (0.42 + 0.3 * busy);
        if alpha > 0.004 {
            ring_path(mid, radius, &muted.colorWithAlphaComponent(alpha), 1.0);
        }
    }
    // The dot: ink at rest, the accent while a page is on its way.
    dot(mid, 9.0, &palette::WASH.resolve(dark));
    if busy < 0.999 {
        let ink = palette::INK.resolve(dark);
        dot(mid, 3.2, &ink.colorWithAlphaComponent(0.7 * (1.0 - busy)));
    }
    if busy > 0.001 {
        let accent = NSColor::controlAccentColor();
        dot(mid, 3.2, &accent.colorWithAlphaComponent(busy));
    }
    let label = ivars.label.borrow().value().clamp(0.0, 1.0);
    let host = ivars.host.borrow();
    if label > 0.004 && !host.is_empty() {
        let width = palette::text_width(&host).min(bounds.size.width - 48.0);
        let rect = NSRect::new(
            NSPoint::new((bounds.size.width - width) / 2.0, mid.y + OUTER + LABEL_GAP),
            NSSize::new(width.max(0.0) + 1.0, palette::line_height()),
        );
        palette::draw_text(&host, rect, &muted.colorWithAlphaComponent(label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_ripples_out_and_fades_and_none_is_born_at_full_strength() {
        let (born, at_birth) = ring(0.0, 0);
        assert!((born - INNER).abs() < 1e-9 && at_birth.abs() < 1e-9);
        let mut last = born;
        for step in 1..100 {
            let (radius, strength) = ring(f64::from(step) / 100.0, 0);
            assert!(radius > last, "always outwards");
            assert!((0.0..=1.0).contains(&strength));
            last = radius;
        }
        assert!(last <= OUTER + 1e-9, "never past the mark's edge: {last}");
        let (_, late) = ring(0.97, 0);
        assert!(late < 0.01, "faded by the edge: {late}");
    }

    #[test]
    fn the_rings_are_spread_evenly_and_the_loop_wraps() {
        let radii: Vec<f64> = (0..RINGS).map(|k| ring(0.1, k).0).collect();
        assert!(radii[0] < radii[1] && radii[1] < radii[2], "{radii:?}");
        for k in 0..RINGS {
            let (a, b) = (ring(0.0, k), ring(1.0, k));
            assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        }
    }

    #[test]
    fn the_mark_is_small_and_sits_a_little_above_the_middle_of_any_stage() {
        for (wide, high) in [(560.0, 300.0), (1280.0, 768.0), (3000.0, 1800.0)] {
            let size = NSSize::new(wide, high);
            let mid = centre(size);
            assert!((mid.x - wide / 2.0).abs() < f64::EPSILON);
            assert!(mid.y < high / 2.0 && mid.y > high / 3.0);
            // The same few points across whatever the window is: a mark, not a picture.
            let band = band(size);
            assert!(band.size.height < 140.0, "{band:?}");
            assert!(band.origin.y >= 0.0 && band.origin.y + band.size.height <= high);
        }
    }
}
