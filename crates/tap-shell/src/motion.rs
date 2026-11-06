//! How drawn things move (Search's `Motion`, `Design.swift:236-289`): one spring for
//! anything that moves between two places, the same everywhere, so the row reads as
//! one piece. `SwiftUI` animated Search's views for it; Tap's strip is one drawn view,
//! so the springs are run here, a slot at a time, keyed by what the slot is rather
//! than where it stands (docs/PLAN.md §5.1.6, "0.1.6").

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    hash::Hash,
    time::Instant,
};

use objc2::{
    rc::Retained,
    runtime::{NSObject, Sel},
};
use objc2_app_kit::{NSView, NSWorkspace};
use objc2_foundation::{NSProcessInfo, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use tap_core::{browser::TabId, settings_index};

use crate::prefs;

/// How a value travels to where it is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// `SwiftUI`'s `spring(response:dampingFraction:)`.
    Spring { response: f64, damping: f64 },
    /// There at once.
    Instant,
}

impl Curve {
    /// Search's `settle`, for the row: tabs and groups that move, come and go.
    pub const SETTLE: Curve = Curve::Spring {
        response: 0.30,
        damping: 0.86,
    };
    /// Search's `glide`, for what moves between two places: the live tab's pill.
    pub const GLIDE: Curve = Curve::Spring {
        response: 0.34,
        damping: 0.82,
    };
    /// While the Mac saves power: short and damped all the way. Search's is a 0.16 s
    /// ease-out; one family of curve keeps one integrator.
    pub const LIGHT: Curve = Curve::Spring {
        response: 0.12,
        damping: 1.0,
    };
    /// For what answers the pointer: a hover's wash coming and going, a press's give.
    /// Quick and damped all the way, so it never wobbles under the hand.
    pub const QUICK: Curve = Curve::Spring {
        response: 0.16,
        damping: 1.0,
    };
    /// For what fades in front of the page: the ground giving way to a page that has
    /// come, the load's line once it is done.
    pub const FADE: Curve = Curve::Spring {
        response: 0.34,
        damping: 1.0,
    };
    /// The load's line chasing the load: unhurried and never past where it is going,
    /// so WebKit's coarse steps read as one movement.
    pub const CREEP: Curve = Curve::Spring {
        response: 0.6,
        damping: 1.0,
    };

    /// `settle` as the Mac wants it now: nothing with Reduce Motion, the light one
    /// while it saves power and Settings says so.
    #[must_use]
    pub fn settle_now() -> Curve {
        Curve::SETTLE.now()
    }

    /// `glide` as the Mac wants it now, as `settle_now`.
    #[must_use]
    pub fn glide_now() -> Curve {
        Curve::GLIDE.now()
    }

    /// This curve as the Mac wants it now: nothing with Reduce Motion, the light one
    /// while it saves power and Settings says so.
    #[must_use]
    pub fn now(self) -> Curve {
        if reduced() {
            Curve::Instant
        } else if light() {
            Curve::LIGHT
        } else {
            self
        }
    }
}

/// System Settings › Accessibility › Reduce motion.
#[must_use]
pub fn reduced() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Settings › Reduce animations in Low Power Mode, while the Mac saves power: Low
/// Power Mode, or hot enough that macOS holds its processors back (Search's
/// `Motion.light`, read at the moment of moving).
#[must_use]
pub fn light() -> bool {
    if !settings_index::flag(&prefs::Kept::new(), "motion.light") {
        return false;
    }
    let info = NSProcessInfo::processInfo();
    info.isLowPowerModeEnabled() || info.thermalState().0 >= 2
}

/// One value on its way to a target.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spring {
    value: f64,
    velocity: f64,
    target: f64,
}

impl Spring {
    fn at(value: f64) -> Spring {
        Spring {
            value,
            velocity: 0.0,
            target: value,
        }
    }

    fn from(value: f64, target: f64) -> Spring {
        Spring {
            value,
            velocity: 0.0,
            target,
        }
    }

    fn rest(&mut self) {
        self.value = self.target;
        self.velocity = 0.0;
    }

    /// `dt` seconds on: a damped spring of unit mass, in small enough steps that a
    /// stiff one stays stable.
    fn step(&mut self, curve: Curve, dt: f64) {
        let Curve::Spring { response, damping } = curve else {
            self.rest();
            return;
        };
        let stiffness = (std::f64::consts::TAU / response).powi(2);
        let friction = 4.0 * std::f64::consts::PI * damping / response;
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(1.0 / 480.0);
            let pull = -stiffness * (self.value - self.target) - friction * self.velocity;
            self.velocity += pull * h;
            self.value += self.velocity * h;
            left -= h;
        }
    }

    /// Close enough, and slow enough, that the next frame wouldn't show a difference.
    fn settled(&self, near: f64) -> bool {
        (self.value - self.target).abs() < near && self.velocity.abs() < near * 10.0
    }
}

/// A way from one place to another, 0 to 1 on a spring: where in between to draw
/// something that travels between two slots that may themselves be moving (Search's
/// `matchedGeometryEffect`, for the live tab's pill).
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    spring: Spring,
    curve: Curve,
}

impl Default for Glide {
    fn default() -> Self {
        Glide {
            spring: Spring::at(1.0),
            curve: Curve::GLIDE,
        }
    }
}

impl Glide {
    /// Off again from the start, on `curve`.
    pub fn restart(&mut self, curve: Curve) {
        self.curve = curve;
        self.spring = Spring::from(0.0, 1.0);
        if curve == Curve::Instant {
            self.spring.rest();
        }
    }

    /// `dt` seconds on. Answers whether it is still on its way.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.spring.settled(0.002) {
            self.spring.rest();
            return false;
        }
        self.spring.step(self.curve, dt);
        true
    }

    /// How far along: 0 at the start, 1 there (a little past, as a spring does).
    #[must_use]
    pub fn progress(&self) -> f64 {
        self.spring.value
    }
}

/// One value, 0 to 1, on its way to a target on a curve: a wash fading, a press
/// sinking, the ground giving way. What `Glide` is for a fixed trip, for one whose
/// end may change on the way.
#[derive(Clone, Copy, Debug)]
pub struct Follow {
    spring: Spring,
    curve: Curve,
}

impl Follow {
    #[must_use]
    pub fn at(value: f64) -> Follow {
        Follow {
            spring: Spring::at(value),
            curve: Curve::Instant,
        }
    }

    /// Off towards `target` on `curve`, from wherever it is now and at whatever
    /// speed: a change of mind on the way carries on rather than restarting.
    pub fn aim(&mut self, target: f64, curve: Curve) {
        self.curve = curve;
        self.spring.target = target;
        if curve == Curve::Instant {
            self.spring.rest();
        }
    }

    /// There at once, still.
    pub fn set(&mut self, value: f64) {
        self.spring = Spring::at(value);
    }

    /// `dt` seconds on. Answers whether it is still on its way.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.spring.settled(0.001) {
            self.spring.rest();
            return false;
        }
        self.spring.step(self.curve, dt);
        true
    }

    #[must_use]
    pub fn value(&self) -> f64 {
        self.spring.value
    }

    #[must_use]
    pub fn target(&self) -> f64 {
        self.spring.target
    }
}

/// Where the load's line stops while WebKit says nothing new: close to the end but
/// short of it, so a stalled load still reads as on its way and never as done.
const CREEP_CAP: f64 = 0.92;
/// How quickly the line edges on by itself, as a time constant in seconds: an
/// eighth of the way left in about half a second, slower the further it gets.
const CREEP_PACE: f64 = 4.0;

/// The front tab's load as a line that moves, not one that jumps. WebKit's
/// `estimatedProgress` comes in a handful of coarse steps (a tenth, a third, nine
/// tenths), and drawn as it comes the line lurches from one to the next and then
/// vanishes. Here it chases the larger of what was said and a slow creep of its own,
/// on `CREEP`, never back; once the load is done it runs to the end and fades.
#[derive(Clone, Copy, Debug)]
pub struct Loading {
    tab: Option<TabId>,
    running: bool,
    /// Where the line would be by itself, edging on between WebKit's reports.
    creep: f64,
    width: Follow,
    alpha: Follow,
    /// The curve the load runs on, read once as it starts rather than on each frame.
    curve: Curve,
    /// How a curve is taken as the Mac wants it: `Curve::now`, but for tests.
    pace: fn(Curve) -> Curve,
}

impl Default for Loading {
    fn default() -> Self {
        Loading {
            tab: None,
            running: false,
            creep: 0.0,
            width: Follow::at(0.0),
            alpha: Follow::at(0.0),
            curve: Curve::CREEP,
            pace: Curve::now,
        }
    }
}

impl Loading {
    /// What the model says of the front tab now. Another tab in front takes its line
    /// as it stands, nothing carried over from the last. Answers whether the line
    /// wants frames.
    pub fn update(&mut self, tab: Option<TabId>, loading: bool, reported: f64) -> bool {
        let reported = reported.clamp(0.0, 1.0);
        if tab != self.tab {
            self.tab = tab;
            self.running = loading;
            self.curve = (self.pace)(Curve::CREEP);
            self.creep = reported;
            self.width.set(if loading { reported } else { 0.0 });
            self.alpha.set(if loading { 1.0 } else { 0.0 });
            return self.busy();
        }
        if loading && !self.running {
            // A load starts: from nothing, so the line grows out of the capsule's end.
            self.curve = (self.pace)(Curve::CREEP);
            self.creep = 0.0;
            self.width.set(0.0);
            self.alpha.set(1.0);
        }
        if loading {
            let goal = reported.max(self.creep).max(self.width.target());
            self.creep = self.creep.max(reported);
            self.width.aim(goal, self.curve);
        } else if self.running {
            // Done (or stopped): to the end, quickly, then away.
            self.width.aim(1.0, (self.pace)(Curve::QUICK));
        }
        self.running = loading;
        self.busy()
    }

    /// `dt` seconds on. Answers whether the line still wants frames.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.running && self.curve != Curve::Instant {
            if self.creep < CREEP_CAP {
                self.creep += (CREEP_CAP - self.creep) * (1.0 - (-dt / CREEP_PACE).exp());
            }
            if self.creep > self.width.target() {
                self.width.aim(self.creep, self.curve);
            }
        }
        if !self.running && self.alpha.target() > 0.0 && self.width.value() >= 0.995 {
            self.alpha.aim(0.0, (self.pace)(Curve::FADE));
        }
        let width = self.width.step(dt);
        let alpha = self.alpha.step(dt);
        // While a load runs the line creeps, so it wants frames; under Reduce Motion it
        // only ever moves when WebKit says so.
        (self.running && self.curve != Curve::Instant) || width || alpha
    }

    /// Whether there is anything to move: a load running, or a line on its way out.
    #[must_use]
    pub fn busy(&self) -> bool {
        (self.running && self.curve != Curve::Instant)
            || (self.width.value() - self.width.target()).abs() > 0.001
            || self.alpha.value() > 0.001
    }

    /// The line to draw this frame: how far along, 0 to 1, and how strongly. `None`
    /// once it has gone.
    #[must_use]
    pub fn drawn(&self) -> Option<(f64, f64)> {
        let alpha = self.alpha.value().clamp(0.0, 1.0);
        let width = self.width.value().clamp(0.0, 1.0);
        (alpha > 0.001 && width > 0.001).then_some((width, alpha))
    }
}

/// Frames from the display a view is on, only while something moves: the strip's,
/// the bar's and the ground's motion each run on one, paused the rest of the time.
#[derive(Default)]
pub struct Frames {
    link: RefCell<Option<Retained<CADisplayLink>>>,
    last: Cell<Option<Instant>>,
}

impl Frames {
    /// Frames from now on to `view`'s `selector`, which takes the link, until
    /// `pause`. Already running, it carries on.
    pub fn run(&self, view: &NSView, selector: Sel) {
        let mut slot = self.link.borrow_mut();
        if slot.is_none() {
            let target: &NSObject = view.as_ref();
            // SAFETY: the selector is the view's own method taking the link; the link
            // keeps the view, and each of these views lives for the app's life.
            let link = unsafe { view.displayLinkWithTarget_selector(target, selector) };
            // SAFETY: the main run loop and its common modes, so frames keep coming
            // while a menu is up or the hand is down.
            unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
            link.setPaused(true);
            *slot = Some(link);
        }
        if let Some(link) = slot.as_ref()
            && link.isPaused()
        {
            self.last.set(None);
            link.setPaused(false);
        }
    }

    /// No more frames until `run` again.
    pub fn pause(&self) {
        if let Some(link) = self.link.borrow().as_ref() {
            link.setPaused(true);
        }
        self.last.set(None);
    }

    /// Seconds since the last frame, the first a 60th, and never more than a 15th,
    /// so a stall (a page hogging the main thread) slows the motion, not skips it.
    pub fn dt(&self) -> f64 {
        let now = Instant::now();
        self.last
            .replace(Some(now))
            .map_or(1.0 / 60.0, |then| now.duration_since(then).as_secs_f64())
            .min(1.0 / 15.0)
    }
}

/// Where a slot is drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub x: f64,
    pub width: f64,
    pub alpha: f64,
}

/// A slot as the layout wants it now.
#[derive(Clone, Debug)]
pub struct Aim<K> {
    pub key: K,
    pub x: f64,
    pub width: f64,
    /// Where it comes from if it is new; `None` uses the row's short entrance.
    pub from: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Mover {
    x: Spring,
    width: Spring,
    alpha: Spring,
    leaving: bool,
}

impl Mover {
    fn rest(&mut self) {
        self.x.rest();
        self.width.rest();
        self.alpha.rest();
    }

    fn place(&self) -> Place {
        Place {
            x: self.x.value,
            width: self.width.value.max(0.0),
            alpha: self.alpha.value.clamp(0.0, 1.0),
        }
    }
}

/// A row of slots, each on its own springs (`SwiftUI`'s `ForEach` with `.animation`):
/// told where the row now wants everything, it moves each slot there, fades in the
/// new ones and fades out the ones that went, until all are still.
#[derive(Debug)]
pub struct Flock<K> {
    movers: HashMap<K, Mover>,
    curve: Curve,
}

impl<K: Eq + Hash + Clone> Default for Flock<K> {
    fn default() -> Self {
        Flock {
            movers: HashMap::new(),
            curve: Curve::SETTLE,
        }
    }
}

impl<K: Eq + Hash + Clone> Flock<K> {
    /// The row as it should stand. `reordered`: its slots came, went or changed order, and
    /// it goes there on `curve`. Otherwise (a resize, the field widening a tab) each
    /// slot is put where it belongs at once, whatever is in flight kept in flight. A
    /// slot that left goes towards `to(key)`, or makes the row's short exit.
    pub fn aim(
        &mut self,
        aims: &[Aim<K>],
        reordered: bool,
        curve: Curve,
        to: impl Fn(&K) -> Option<f64>,
    ) {
        let animate = reordered && curve != Curve::Instant;
        if reordered {
            self.curve = curve;
        }
        let mut seen = HashSet::with_capacity(aims.len());
        for aim in aims {
            seen.insert(aim.key.clone());
            if let Some(mover) = self.movers.get_mut(&aim.key) {
                if animate || mover.leaving {
                    mover.x.target = aim.x;
                    mover.width.target = aim.width;
                } else {
                    mover.x.value += aim.x - mover.x.target;
                    mover.x.target = aim.x;
                    mover.width.value += aim.width - mover.width.target;
                    mover.width.target = aim.width;
                }
                mover.leaving = false;
                mover.alpha.target = 1.0;
                continue;
            }
            let fresh = if animate {
                Mover {
                    x: Spring::from(
                        aim.from.unwrap_or(if curve == Curve::LIGHT {
                            aim.x
                        } else {
                            aim.x - 8.0
                        }),
                        aim.x,
                    ),
                    width: Spring::at(aim.width),
                    alpha: Spring::from(0.0, 1.0),
                    leaving: false,
                }
            } else {
                Mover {
                    x: Spring::at(aim.x),
                    width: Spring::at(aim.width),
                    alpha: Spring::at(1.0),
                    leaving: false,
                }
            };
            self.movers.insert(aim.key.clone(), fresh);
        }
        if animate {
            for (key, mover) in &mut self.movers {
                if !seen.contains(key) && !mover.leaving {
                    mover.leaving = true;
                    mover.alpha.target = 0.0;
                    mover.x.target = if let Some(x) = to(key) {
                        x
                    } else if curve == Curve::LIGHT {
                        mover.x.velocity = 0.0;
                        mover.x.value
                    } else {
                        mover.x.value - 8.0
                    };
                }
            }
        } else {
            self.movers
                .retain(|key, mover| seen.contains(key) || mover.leaving);
        }
        if curve == Curve::Instant && reordered {
            self.movers.retain(|_, mover| !mover.leaving);
            self.movers.values_mut().for_each(Mover::rest);
        }
    }

    /// Choose the curve for a released slot, including an immediate return when needed.
    pub fn release(&mut self, curve: Curve) {
        self.curve = curve;
        if curve == Curve::Instant {
            self.movers.retain(|_, mover| !mover.leaving);
            self.movers.values_mut().for_each(Mover::rest);
        }
    }

    /// `dt` seconds on. Answers whether anything is still moving.
    pub fn step(&mut self, dt: f64) -> bool {
        let curve = self.curve;
        let mut moving = false;
        self.movers.retain(|_, mover| {
            mover.x.step(curve, dt);
            mover.width.step(curve, dt);
            mover.alpha.step(curve, dt);
            let still =
                mover.x.settled(0.1) && mover.width.settled(0.1) && mover.alpha.settled(0.004);
            if still {
                if mover.leaving {
                    return false;
                }
                mover.rest();
            } else {
                moving = true;
            }
            true
        });
        moving
    }

    /// Where the slot is drawn now.
    #[must_use]
    pub fn place(&self, key: &K) -> Option<Place> {
        self.movers.get(key).map(Mover::place)
    }

    /// The slots on their way out, to be drawn under the rest.
    pub fn leaving(&self) -> impl Iterator<Item = (&K, Place)> {
        self.movers
            .iter()
            .filter(|(_, mover)| mover.leaving)
            .map(|(key, mover)| (key, mover.place()))
    }

    /// The hand has the slot: it is drawn where the hand is, and springs home from
    /// there once let go.
    pub fn hold(&mut self, key: &K, x: f64) {
        if let Some(mover) = self.movers.get_mut(key) {
            mover.x.value = x;
            mover.x.velocity = 0.0;
            mover.alpha.rest();
        }
    }

    /// Whether any slot is still on its way.
    #[cfg(test)]
    #[must_use]
    pub fn moving(&self) -> bool {
        self.movers.values().any(|mover| {
            !(mover.x.settled(0.1) && mover.width.settled(0.1) && mover.alpha.settled(0.004))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aim(key: &'static str, x: f64) -> Aim<&'static str> {
        Aim {
            key,
            x,
            width: 100.0,
            from: None,
        }
    }

    fn run(flock: &mut Flock<&'static str>, seconds: f64) {
        let mut left = seconds;
        while left > 0.0 {
            flock.step(1.0 / 60.0);
            left -= 1.0 / 60.0;
        }
    }

    fn line() -> Loading {
        Loading {
            pace: |curve| curve,
            ..Loading::default()
        }
    }

    fn load(line: &mut Loading, seconds: f64) -> Vec<f64> {
        let mut seen = Vec::new();
        let mut left = seconds;
        while left > 0.0 {
            line.step(1.0 / 60.0);
            seen.push(line.drawn().map_or(0.0, |(w, _)| w));
            left -= 1.0 / 60.0;
        }
        seen
    }

    #[test]
    fn the_load_line_moves_on_between_coarse_reports_and_never_back() {
        let mut line = line();
        line.update(Some(TabId(1)), false, 0.0);
        assert!(line.drawn().is_none(), "nothing loading, nothing drawn");
        assert!(
            line.update(Some(TabId(1)), true, 0.1),
            "a load wants frames"
        );
        let mut seen = load(&mut line, 0.3);
        // WebKit jumps from a tenth to two thirds: the line goes there smoothly.
        line.update(Some(TabId(1)), true, 0.66);
        let before = *seen.last().expect("frames");
        seen.extend(load(&mut line, 0.1));
        let after = *seen.last().expect("frames");
        assert!(
            after > before && after < 0.66,
            "on its way, not there: {after}"
        );
        seen.extend(load(&mut line, 2.0));
        // Then nothing more for a long while: it still edges on, short of the end.
        let stalled = *seen.last().expect("frames");
        seen.extend(load(&mut line, 3.0));
        let later = *seen.last().expect("frames");
        assert!(
            later > stalled && later < CREEP_CAP + 0.01,
            "creeps: {stalled} → {later}"
        );
        // WebKit says less for a redirect: the line doesn't go back.
        line.update(Some(TabId(1)), true, 0.2);
        seen.extend(load(&mut line, 0.5));
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0] - 1e-9),
            "never back while loading"
        );
        // No single frame jumps by more than a few percent.
        assert!(seen.windows(2).all(|w| (w[1] - w[0]).abs() < 0.05));
    }

    #[test]
    fn a_finished_load_runs_to_the_end_then_fades_and_stops() {
        let mut line = line();
        line.update(Some(TabId(1)), true, 0.5);
        load(&mut line, 1.0);
        line.update(Some(TabId(1)), false, 1.0);
        load(&mut line, 0.25);
        let (width, alpha) = line.drawn().expect("still there as it finishes");
        assert!(width > 0.99, "at the end first: {width}");
        assert!(alpha <= 1.0);
        load(&mut line, 1.5);
        assert!(line.drawn().is_none(), "gone");
        assert!(!line.step(1.0 / 60.0), "and asks for no more frames");
    }

    #[test]
    fn another_tab_in_front_takes_its_own_line_at_once() {
        let mut line = line();
        line.update(Some(TabId(1)), true, 0.3);
        load(&mut line, 1.0);
        line.update(Some(TabId(2)), false, 1.0);
        assert!(
            line.drawn().is_none(),
            "a loaded tab shows no line, no fade"
        );
        line.update(Some(TabId(3)), true, 0.7);
        let (width, _) = line.drawn().expect("a loading tab shows where it is");
        assert!((width - 0.7).abs() < 1e-9);
    }

    #[test]
    fn a_follow_changes_its_mind_without_a_jump() {
        let mut wash = Follow::at(0.0);
        wash.aim(1.0, Curve::QUICK);
        for _ in 0..4 {
            wash.step(1.0 / 60.0);
        }
        let halfway = wash.value();
        assert!(halfway > 0.0 && halfway < 1.0);
        wash.aim(0.0, Curve::QUICK);
        wash.step(1.0 / 60.0);
        assert!((wash.value() - halfway).abs() < 0.1, "from where it was");
        for _ in 0..60 {
            wash.step(1.0 / 60.0);
        }
        assert!(wash.value().abs() < 0.002 && !wash.step(1.0 / 60.0));
        wash.aim(1.0, Curve::Instant);
        assert!(
            (wash.value() - 1.0).abs() < f64::EPSILON,
            "no motion: there at once"
        );
    }

    #[test]
    fn a_spring_settles_on_its_target_without_running_away() {
        let mut spring = Spring::from(0.0, 100.0);
        let mut peak: f64 = 0.0;
        for _ in 0..60 {
            spring.step(Curve::SETTLE, 1.0 / 60.0);
            peak = peak.max(spring.value);
        }
        assert!(spring.settled(0.1), "settled within a second: {spring:?}");
        assert!(peak < 102.0, "settle barely overshoots: {peak}");
        let mut light = Spring::from(0.0, 100.0);
        for _ in 0..15 {
            light.step(Curve::LIGHT, 1.0 / 60.0);
        }
        assert!(
            (light.value - 100.0).abs() < 1.0,
            "the light curve is there in a quarter second"
        );
    }

    #[test]
    fn a_reordered_row_glides_and_a_resized_one_goes_at_once() {
        let mut flock = Flock::default();
        flock.aim(
            &[aim("a", 0.0), aim("b", 100.0)],
            false,
            Curve::SETTLE,
            |_| None,
        );
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(0.0));
        assert!(!flock.moving());
        // a and b swap: both are on their way, neither there yet.
        flock.aim(
            &[aim("b", 0.0), aim("a", 100.0)],
            true,
            Curve::SETTLE,
            |_| None,
        );
        flock.step(1.0 / 60.0);
        let a = flock.place(&"a").expect("a").x;
        assert!(a > 0.0 && a < 100.0, "in between: {a}");
        run(&mut flock, 1.0);
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(100.0));
        assert!(!flock.moving());
        // The window widens and the row moves right: there at once.
        flock.aim(
            &[aim("b", 50.0), aim("a", 150.0)],
            false,
            Curve::SETTLE,
            |_| None,
        );
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(150.0));
    }

    #[test]
    fn a_slot_that_goes_fades_out_towards_its_heading_and_one_that_comes_fades_in() {
        let mut flock = Flock::default();
        flock.aim(
            &[aim("h", 0.0), aim("t1", 100.0), aim("t2", 200.0)],
            false,
            Curve::SETTLE,
            |_| None,
        );
        // Folded: the group's tabs go back under the heading.
        flock.aim(&[aim("h", 0.0)], true, Curve::SETTLE, |_| Some(0.0));
        flock.step(1.0 / 60.0);
        assert_eq!(flock.leaving().count(), 2);
        run(&mut flock, 1.5);
        assert_eq!(flock.leaving().count(), 0, "gone once faded");
        assert!(flock.place(&"t1").is_none());
        // Opened again: they come out from under the heading, faded in.
        let mut back = aim("t1", 100.0);
        back.from = Some(0.0);
        flock.aim(&[aim("h", 0.0), back], true, Curve::SETTLE, |_| None);
        let start = flock.place(&"t1").expect("t1");
        assert!(start.x.abs() < f64::EPSILON && start.alpha < 0.01);
        run(&mut flock, 1.0);
        let end = flock.place(&"t1").expect("t1");
        assert!((end.x - 100.0).abs() < 0.2 && (end.alpha - 1.0).abs() < 0.01);
    }

    #[test]
    fn ordinary_slots_enter_and_leave_with_a_short_slide() {
        let mut flock = Flock::default();
        flock.aim(&[aim("a", 0.0)], false, Curve::SETTLE, |_| None);
        flock.aim(
            &[aim("a", 0.0), aim("b", 100.0)],
            true,
            Curve::SETTLE,
            |_| None,
        );
        assert_eq!(flock.place(&"b").map(|p| (p.x, p.alpha)), Some((92.0, 0.0)));
        run(&mut flock, 1.0);
        flock.aim(&[aim("a", 0.0)], true, Curve::SETTLE, |_| None);
        flock.step(1.0 / 60.0);
        let leaving = flock.place(&"b").expect("leaving slot");
        assert!(leaving.x < 100.0 && leaving.alpha < 1.0);
        run(&mut flock, 1.0);
        assert!(flock.place(&"b").is_none());
    }

    #[test]
    fn light_motion_fades_ordinary_slots_without_sliding() {
        let mut flock = Flock::default();
        flock.aim(&[aim("a", 0.0)], false, Curve::LIGHT, |_| None);
        flock.aim(
            &[aim("a", 0.0), aim("b", 100.0)],
            true,
            Curve::LIGHT,
            |_| None,
        );
        assert_eq!(
            flock.place(&"b").map(|p| (p.x, p.alpha)),
            Some((100.0, 0.0))
        );
        flock.aim(
            &[aim("a", 0.0), aim("b", 200.0)],
            true,
            Curve::SETTLE,
            |_| None,
        );
        flock.step(1.0 / 60.0);
        let x = flock.place(&"b").expect("b").x;
        flock.aim(&[aim("a", 0.0)], true, Curve::LIGHT, |_| None);
        flock.step(1.0 / 60.0);
        assert_eq!(flock.place(&"b").map(|p| p.x), Some(x));
    }

    #[test]
    fn a_glide_goes_from_nought_to_one_and_stops() {
        let mut glide = Glide::default();
        assert!(
            (glide.progress() - 1.0).abs() < f64::EPSILON,
            "at rest, there"
        );
        glide.restart(Curve::GLIDE);
        assert!(glide.progress().abs() < f64::EPSILON);
        assert!(glide.step(1.0 / 60.0));
        let early = glide.progress();
        assert!(early > 0.0 && early < 1.0);
        for _ in 0..90 {
            glide.step(1.0 / 60.0);
        }
        assert!(!glide.step(1.0 / 60.0), "settled");
        assert!((glide.progress() - 1.0).abs() < f64::EPSILON);
        glide.restart(Curve::Instant);
        assert!(
            (glide.progress() - 1.0).abs() < f64::EPSILON,
            "no motion: there at once"
        );
    }

    #[test]
    fn reduce_motion_puts_everything_in_place_at_once() {
        let mut flock = Flock::default();
        flock.aim(
            &[aim("a", 0.0), aim("b", 100.0)],
            false,
            Curve::SETTLE,
            |_| None,
        );
        flock.aim(&[aim("b", 0.0)], true, Curve::Instant, |_| None);
        assert_eq!(flock.place(&"b").map(|p| p.x), Some(0.0));
        assert!(flock.place(&"a").is_none() && !flock.moving());
    }

    #[test]
    fn releasing_a_held_slot_respects_the_current_curve() {
        let mut flock = Flock::default();
        flock.aim(&[aim("a", 0.0)], false, Curve::SETTLE, |_| None);
        flock.hold(&"a", 40.0);
        flock.release(Curve::Instant);
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(0.0));
        assert!(!flock.moving());
        flock.hold(&"a", 40.0);
        flock.release(Curve::LIGHT);
        run(&mut flock, 0.25);
        assert!(flock.place(&"a").expect("a").x < 1.0);
    }

    #[test]
    fn a_held_slot_springs_home_from_the_hand() {
        let mut flock = Flock::default();
        flock.aim(&[aim("a", 0.0)], false, Curve::SETTLE, |_| None);
        flock.hold(&"a", 40.0);
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(40.0));
        assert!(flock.moving());
        run(&mut flock, 1.0);
        assert_eq!(flock.place(&"a").map(|p| p.x), Some(0.0));
    }
}
