//! What a blank tab shows behind the field: a quiet instrument, drawn, not a page.
//!
//! Search leaves a new tab empty ground; Tap draws a small isometric
//! signal-bench there instead — base on feet, ticked rails, a carriage on
//! bushings, a dial whose needle breathes, one softly lit signal dot. Pure
//! ambience: nothing clickable, no model state, and no WebKit view, so a blank
//! tab stays as cheap as Search's. The motion is one 12 s loop on the display
//! link, paused while another tab is in front; Reduce Motion gets a single
//! still frame (plan §5.1.6, `motion::reduced`).

use std::{
    cell::{Cell, RefCell},
    time::Instant,
};

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObject, sel,
};
use objc2_app_kit::{NSBezierPath, NSColor, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize};
use objc2_quartz_core::CADisplayLink;

use crate::{motion, palette};

/// How long one calm loop lasts, in seconds.
pub const LOOP: f64 = 12.0;

/// Where the loop is at `t` seconds: 0 to 1, wrapping.
#[must_use]
pub fn cycle(t: f64) -> f64 {
    (t % LOOP) / LOOP
}

/// The dial needle's angle at `t` seconds, in radians: rest plus a slow breath.
#[must_use]
pub fn needle_angle(t: f64) -> f64 {
    const REST: f64 = -0.8;
    REST + 0.35 * (std::f64::consts::TAU * cycle(t)).sin()
}

/// How far the carriage and the lit dot drift at `t` seconds, in points.
#[must_use]
pub fn drift(t: f64) -> (f64, f64) {
    let a = std::f64::consts::TAU * cycle(t);
    (6.0 * a.sin(), 4.0 * (a + 1.3).sin())
}

/// How strongly the signal dot glows at `t` seconds: 0.55 to 1.0.
#[must_use]
pub fn signal_alpha(t: f64) -> f64 {
    0.775 + 0.225 * (std::f64::consts::TAU * cycle(t) + 0.6).sin()
}

/// Where the instrument sits in a stage of `size`: its centre and its unit, so
/// the bench fills about three fifths of the smaller side.
#[must_use]
pub fn fit(size: NSSize) -> (f64, f64, f64) {
    let smaller = size.width.min(size.height).max(1.0);
    (size.width / 2.0, size.height / 2.0, smaller / 120.0)
}

pub struct Ivars {
    time: Cell<f64>,
    /// Frames while a blank tab is in front, paused when it isn't.
    link: RefCell<Option<Retained<CADisplayLink>>>,
    last_frame: Cell<Option<Instant>>,
}

define_class!(
    /// The instrument behind a blank tab's field.
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

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            draw_in(self);
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
            time: Cell::new(0.0),
            link: RefCell::new(None),
            last_frame: Cell::new(None),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view
    }

    /// Starts the loop when a blank tab comes to the front, stops it when one
    /// goes. Under Reduce Motion (or while the Mac wants light motion) one
    /// still frame is drawn and no link runs.
    pub fn set_animating(&self, on: bool) {
        if !on || motion::reduced() || motion::light() {
            if let Some(link) = self.ivars().link.borrow().as_ref() {
                link.setPaused(true);
            }
            self.ivars().last_frame.set(None);
            if !on {
                self.ivars().time.set(0.0);
            }
            self.setNeedsDisplay(true);
            return;
        }
        let mut slot = self.ivars().link.borrow_mut();
        if slot.is_none() {
            let target: &NSObject = self.as_ref();
            // SAFETY: `blankFrame:` is this class's own method and takes the
            // link; the link keeps the view, which lives for the app's life.
            let link = unsafe { self.displayLinkWithTarget_selector(target, sel!(blankFrame:)) };
            // SAFETY: the main run loop and its common modes, so frames keep
            // coming while a menu is up.
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
        self.ivars().time.set(self.ivars().time.get() + dt);
        self.setNeedsDisplay(true);
    }
}

/// Projects world units onto the view: x and y on the floor, z up. The view
/// is flipped, so up is negative y.
fn project(center_x: f64, center_y: f64, scale: f64, wx: f64, wy: f64, wz: f64) -> NSPoint {
    NSPoint::new(
        center_x + (wx - wy) * 0.866 * scale,
        center_y + wx.midpoint(wy) * scale - wz * scale,
    )
}

fn path_of(points: &[NSPoint], close: bool) -> Retained<NSBezierPath> {
    let path = NSBezierPath::new();
    let mut first = true;
    for p in points {
        if first {
            path.moveToPoint(*p);
            first = false;
        } else {
            path.lineToPoint(*p);
        }
    }
    if close {
        path.closePath();
    }
    path
}

fn fill_poly(points: &[NSPoint], color: &NSColor) {
    color.setFill();
    path_of(points, true).fill();
}

fn stroke_poly(points: &[NSPoint], color: &NSColor, width: f64) {
    color.setStroke();
    let path = path_of(points, false);
    path.setLineWidth(width);
    path.stroke();
}

fn dot(at: NSPoint, r: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(at.x - r, at.y - r),
        NSSize::new(2.0 * r, 2.0 * r),
    ))
    .fill();
}

fn ring(at: NSPoint, r: f64, color: &NSColor, width: f64) {
    color.setStroke();
    let path = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(at.x - r, at.y - r),
        NSSize::new(2.0 * r, 2.0 * r),
    ));
    path.setLineWidth(width);
    path.stroke();
}

/// The whole bench, back to front: base, rails, carriage, dial, mast and dot.
#[allow(clippy::too_many_lines)] // one instrument, drawn in order
fn draw_in(view: &BlankView) {
    let dark = palette::is_dark(view);
    let bounds = view.bounds();
    palette::GROUND.resolve(dark).setFill();
    NSBezierPath::fillRect(bounds);

    let time = view.ivars().time.get();
    let (center_x, center_y, scale) = fit(bounds.size);
    let unit = scale.max(0.4);
    let (dx, dy) = drift(time);

    let hairline = palette::HAIRLINE.resolve(dark);
    let muted = palette::MUTED.resolve(dark);
    let ink = palette::INK.resolve(dark);
    let wash = palette::WASH.resolve(dark);

    // Halo under the bench: it stands on something.
    dot(
        project(center_x, center_y, scale, 0.0, 0.0, 0.0),
        46.0 * scale,
        &wash,
    );

    // Base slab: top, left and right faces.
    let half = 26.0;
    let top = 5.0;
    let c00 = project(center_x, center_y, scale, -half, -half, top);
    let c10 = project(center_x, center_y, scale, half, -half, top);
    let c11 = project(center_x, center_y, scale, half, half, top);
    let c01 = project(center_x, center_y, scale, -half, half, top);
    let e00 = project(center_x, center_y, scale, -half, -half, 0.0);
    let e10 = project(center_x, center_y, scale, half, -half, 0.0);
    let e11 = project(center_x, center_y, scale, half, half, 0.0);
    let e01 = project(center_x, center_y, scale, -half, half, 0.0);
    let _ = e00;
    fill_poly(&[e10, e11, c11, c10], &hairline);
    fill_poly(&[e01, e11, c11, c01], &wash);
    fill_poly(&[c00, c10, c11, c01], &wash);
    stroke_poly(&[c00, c10, c11, c01, c00], &muted, 1.0);

    // Feet: four short posts under the corners.
    for (fx, fy) in [
        (-half + 4.0, -half + 4.0),
        (half - 4.0, -half + 4.0),
        (half - 4.0, half - 4.0),
        (-half + 4.0, half - 4.0),
    ] {
        let top = project(center_x, center_y, scale, fx, fy, 0.0);
        let bottom = project(center_x, center_y, scale, fx, fy, -4.0);
        stroke_poly(&[top, bottom], &muted, 2.5 * unit);
        dot(bottom, 1.6 * unit, &muted);
    }

    // Corner screws on the top face.
    for (sx, sy) in [
        (-half + 3.0, -half + 3.0),
        (half - 3.0, -half + 3.0),
        (half - 3.0, half - 3.0),
        (-half + 3.0, half - 3.0),
    ] {
        let at = project(center_x, center_y, scale, sx, sy, top);
        dot(at, 1.1 * unit, &muted);
        ring(at, 1.1 * unit, &ink, 0.5);
    }

    // Label plate with three rule lines, near the front edge.
    let p00 = project(center_x, center_y, scale, -10.0, half - 9.0, top);
    let p10 = project(center_x, center_y, scale, 10.0, half - 9.0, top);
    let p11 = project(center_x, center_y, scale, 10.0, half - 4.0, top);
    let p01 = project(center_x, center_y, scale, -10.0, half - 4.0, top);
    stroke_poly(&[p00, p10, p11, p01, p00], &muted, 0.75);
    for idx in 0..3 {
        let y = half - 9.0 + f64::from(idx + 1) / 4.0 * 5.0;
        stroke_poly(
            &[
                project(center_x, center_y, scale, -7.0, y, top),
                project(center_x, center_y, scale, 7.0, y, top),
            ],
            &hairline,
            0.75,
        );
    }

    // Rails with tick rulers along both sides.
    for ry in [-14.0, 14.0] {
        let q0 = project(center_x, center_y, scale, -half + 3.0, ry - 1.6, top);
        let q1 = project(center_x, center_y, scale, half - 3.0, ry - 1.6, top);
        let q2 = project(center_x, center_y, scale, half - 3.0, ry + 1.6, top);
        let q3 = project(center_x, center_y, scale, -half + 3.0, ry + 1.6, top);
        fill_poly(&[q0, q1, q2, q3], &hairline);
        stroke_poly(&[q0, q1, q2, q3, q0], &muted, 0.75);
        // Ticks every 4 units, a longer one every 5th.
        let mut pos = -half + 5.0;
        let mut tick = 0;
        while pos < half - 4.0 {
            let len = if tick % 5 == 0 { 2.6 } else { 1.4 };
            stroke_poly(
                &[
                    project(center_x, center_y, scale, pos, ry + 1.6, top),
                    project(center_x, center_y, scale, pos, ry + 1.6 - len, top),
                ],
                &muted,
                0.6,
            );
            pos += 4.0;
            tick += 1;
        }
    }

    // Carriage on bushings, breathing with the loop.
    let car = 8.0;
    let k00u = project(center_x + dx, center_y + dy, scale, -car, -16.0, top + 5.0);
    let k10u = project(center_x + dx, center_y + dy, scale, car, -16.0, top + 5.0);
    let k11u = project(center_x + dx, center_y + dy, scale, car, 16.0, top + 5.0);
    let k01u = project(center_x + dx, center_y + dy, scale, -car, 16.0, top + 5.0);
    let k10 = project(center_x + dx, center_y + dy, scale, car, -16.0, top + 2.0);
    let k11 = project(center_x + dx, center_y + dy, scale, car, 16.0, top + 2.0);
    let k01 = project(center_x + dx, center_y + dy, scale, -car, 16.0, top + 2.0);
    fill_poly(&[k10, k11, k11u, k10u], &hairline);
    fill_poly(&[k01, k11, k11u, k01u], &wash);
    fill_poly(&[k00u, k10u, k11u, k01u], &wash);
    stroke_poly(&[k00u, k10u, k11u, k01u, k00u], &muted, 1.0);
    for by in [-14.0, 14.0] {
        let at = project(center_x + dx, center_y + dy, scale, 0.0, by, top + 2.0);
        ring(at, 2.2 * unit, &muted, 1.0);
        dot(at, 1.0 * unit, &ink);
    }

    // Coil springs around the rods behind the carriage.
    for sy in [-14.0, 14.0] {
        let mut pts = Vec::with_capacity(16);
        let mut coil_x = -half + 3.0;
        let mut up = true;
        while coil_x < -car {
            pts.push(project(
                center_x,
                center_y,
                scale,
                coil_x,
                if up { sy - 1.2 } else { sy + 1.2 },
                top + 2.0,
            ));
            coil_x += 2.0;
            up = !up;
        }
        stroke_poly(&pts, &muted, 0.8);
    }
    // The rods themselves, through the carriage.
    for ry in [-14.0, 14.0] {
        stroke_poly(
            &[
                project(center_x, center_y, scale, -half + 3.0, ry, top + 2.0),
                project(center_x, center_y, scale, half - 3.0, ry, top + 2.0),
            ],
            &ink,
            1.0,
        );
    }

    // Dial gauge on a post at the back-left.
    let post = project(center_x, center_y, scale, -18.0, -8.0, top);
    let post_top = project(center_x, center_y, scale, -18.0, -8.0, top + 12.0);
    stroke_poly(&[post, post_top], &muted, 2.0 * unit);
    let face = project(center_x, center_y, scale, -18.0, -8.0, top + 16.0);
    let face_r = 9.0 * unit;
    dot(face, face_r + 1.5, &hairline);
    dot(face, face_r, &wash);
    ring(face, face_r, &muted, 1.0);
    // Twelve ticks around the face; the zero mark runs long.
    for idx in 0..12 {
        let dial_a = f64::from(idx) / 12.0 * std::f64::consts::TAU;
        let long = idx == 0;
        let r0 = face_r - if long { 3.2 } else { 1.8 };
        let p0 = NSPoint::new(face.x + r0 * dial_a.cos(), face.y + r0 * dial_a.sin());
        let p1 = NSPoint::new(
            face.x + (face_r - 0.6) * dial_a.cos(),
            face.y + (face_r - 0.6) * dial_a.sin(),
        );
        stroke_poly(&[p0, p1], if long { &ink } else { &muted }, 0.8);
    }
    // The needle breathes; its tail may stop on the bare face (a pointer's tip).
    let needle_a = needle_angle(time);
    let tip = NSPoint::new(
        face.x + (face_r - 2.0) * needle_a.cos(),
        face.y + (face_r - 2.0) * needle_a.sin(),
    );
    stroke_poly(&[face, tip], &ink, 1.2);
    dot(face, 1.4, &ink);

    // Signal mast at the back-right, with the one lit dot.
    let mast = project(center_x, center_y, scale, 18.0, -10.0, top);
    let mast_top = project(
        center_x + dx * 0.5,
        center_y + dy * 0.5,
        scale,
        18.0,
        -10.0,
        top + 20.0,
    );
    stroke_poly(&[mast, mast_top], &muted, 2.0 * unit);
    dot(mast_top, 2.6 * unit, &hairline);
    let alpha = signal_alpha(time);
    let lit = if dark {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.54, 0.71, 0.97, alpha)
    } else {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.45, 0.91, alpha)
    };
    dot(mast_top, 6.5 * unit * alpha + 1.0, &wash);
    dot(mast_top, 2.2 * unit, &lit);

    // Knurled knob at the front-right corner.
    let knob = project(center_x, center_y, scale, 20.0, 16.0, top + 2.0);
    let knob_r = 4.5 * unit;
    dot(knob, knob_r, &wash);
    ring(knob, knob_r, &muted, 1.0);
    for idx in 0..16 {
        let knob_a = f64::from(idx) / 16.0 * std::f64::consts::TAU;
        let p0 = NSPoint::new(
            knob.x + (knob_r - 1.2) * knob_a.cos(),
            knob.y + (knob_r - 1.2) * knob_a.sin(),
        );
        let p1 = NSPoint::new(
            knob.x + knob_r * knob_a.cos(),
            knob.y + knob_r * knob_a.sin(),
        );
        stroke_poly(&[p0, p1], &muted, 0.6);
    }
    dot(knob, 1.2, &ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loop_wraps_and_the_needle_breathes() {
        assert!((cycle(0.0) - 0.0).abs() < f64::EPSILON);
        assert!((cycle(LOOP) - 0.0).abs() < f64::EPSILON);
        assert!((cycle(LOOP / 2.0) - 0.5).abs() < 1e-9);
        let rest = needle_angle(0.0);
        let quarter = needle_angle(LOOP / 4.0);
        assert!(
            (quarter - rest - 0.35).abs() < 1e-9,
            "full breath up: {quarter}"
        );
        assert!((needle_angle(LOOP) - rest).abs() < 1e-9, "wraps cleanly");
    }

    #[test]
    fn drift_stays_small_and_the_signal_pulses_softly() {
        for millis in [0, 1500, 3000, 6000, 9000, 11500] {
            let secs = f64::from(millis) / 1000.0;
            let (dx, dy) = drift(secs);
            assert!(dx.abs() <= 6.5 && dy.abs() <= 4.5, "calm: {dx},{dy}");
            let glow = signal_alpha(secs);
            assert!((0.5..=1.0).contains(&glow), "soft pulse: {glow}");
        }
    }

    #[test]
    fn the_bench_fits_any_stage() {
        for (wide, high) in [
            (300.0, 200.0),
            (1280.0, 768.0),
            (560.0, 360.0),
            (4000.0, 200.0),
        ] {
            let (mid_x, mid_y, mag) = fit(NSSize::new(wide, high));
            assert!((mid_x - wide / 2.0).abs() < f64::EPSILON);
            assert!((mid_y - high / 2.0).abs() < f64::EPSILON);
            assert!(mag > 0.0, "a positive unit: {mag}");
            // The bench (about ±46 world units wide on screen) stays inside.
            assert!(46.0 * 0.866 * mag < wide / 2.0, "fits width {wide}: {mag}");
        }
    }
}
