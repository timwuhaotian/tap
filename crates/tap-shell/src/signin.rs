//! Sign-ins on a page, the browser's half (Search's `hang`, `pick`, `onCredentials` and
//! the `PasswordOffer` bar): with the caret in a sign-in box, the accounts kept for the
//! site hang from the box, and a click fills the form in — nothing is filled on its own,
//! the way Safari does it. After a sign-in that took, the password is offered a place in
//! the keychain, in a bar at the bottom of the page.
//!
//! A page that came over plain http can have been written by anyone on the way; it is
//! offered only what was kept from plain http too.

use std::{cell::RefCell, time::Instant};

use objc2::{
    AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSControlSize, NSEvent, NSResponder,
    NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::WKWebView;
use tap_core::browser::TabId;

use crate::{forms, palette, prefs, rt, vault, web, window};

/// A row's height, the card's padding, and the room left for its shadow.
const ROW: f64 = 38.0;
const PAD: f64 = 5.0;
const SHADOW: f64 = 16.0;
/// At most this many accounts hang from a box.
const MOST: usize = 5;
/// A page can put the caret in a sign-in box itself: a click already on its way is not a
/// choice, so the list takes none for its first half second, as Chrome's does.
const SETTLING: f64 = 0.5;

/// The list up under a box.
struct Up {
    tab: TabId,
    /// The page it was made for: a click fills only a page that still is that one.
    host: String,
    clear: bool,
    /// Hung from a first step's name box: a pick puts in the name alone.
    name_only: bool,
    logins: Vec<vault::Kept>,
    shown: Instant,
    hover: Option<usize>,
}

/// A name and password a page has just sent, waiting for an answer.
struct Offer {
    host: String,
    user: String,
    password: String,
    clear: bool,
    /// The same account is kept already, with another password.
    changed: bool,
}

#[derive(PartialEq)]
struct Looked {
    tab: TabId,
    host: String,
    clear: bool,
    name: bool,
    wide: bool,
}

#[derive(Default)]
struct State {
    up: Option<Up>,
    /// The page whose accounts were looked up for the box the caret is in: the box says
    /// where it is on every frame of a scroll, and the keychain is asked once per box.
    looked: Option<Looked>,
    /// Picked into: the list doesn't come straight back for the box still in use.
    picked_into: Option<TabId>,
    /// Counts lowerings asked for: one overtaken by the caret coming back does nothing.
    lowering: u64,
    offer: Option<Offer>,
    card: Option<Retained<Card>>,
    bar: Option<Bar>,
}

struct Bar {
    pill: Retained<Pill>,
    words: Retained<NSTextField>,
    save: Retained<NSButton>,
    later: Retained<NSButton>,
    never: Retained<NSButton>,
    _target: Retained<Target>,
    /// Counts what the bar has said: a word that has had its time goes only if nothing
    /// came after it.
    said: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn fills() -> bool {
    use tap_core::settings_index as index;
    index::flag(&prefs::Kept::new(), "passwords.fill")
}

fn saves() -> bool {
    use tap_core::settings_index as index;
    index::flag(&prefs::Kept::new(), "passwords.save")
}

fn active() -> Option<TabId> {
    rt::read(tap_core::browser::Browser::active).flatten()
}

// MARK: - the list under a box

/// The caret moved into a sign-in box (`rect`, in the page's CSS pixels), or out of one
/// (`None`).
pub fn field(
    tab: TabId,
    web: &WKWebView,
    rect: Option<NSRect>,
    passwords: bool,
    name: bool,
    wide: bool,
) {
    let Some(rect) = rect else {
        let mine = STATE.with_borrow_mut(|s| {
            if s.looked.as_ref().is_some_and(|l| l.tab == tab) {
                s.looked = None;
            }
            if s.picked_into == Some(tab) {
                s.picked_into = None;
            }
            s.up.as_ref().is_some_and(|u| u.tab == tab)
        });
        if mine {
            lower_soon(tab);
        }
        return;
    };
    STATE.with_borrow_mut(|s| s.lowering += 1);
    if active() != Some(tab) || STATE.with_borrow(|s| s.picked_into == Some(tab)) {
        return;
    }
    let Some((scheme, host)) = forms::page(web) else {
        return;
    };
    if scheme != "https" && scheme != "http" {
        return;
    }
    let clear = scheme == "http";
    let spot = spot(web, rect);
    let looked = Looked {
        tab,
        host: host.clone(),
        clear,
        name,
        wide,
    };
    // The same box, moved by a scroll: the list follows it and keeps the moment it came
    // up; a box with no accounts stays without until the caret leaves.
    if STATE.with_borrow(|s| s.looked.as_ref() == Some(&looked)) {
        if STATE.with_borrow(|s| s.up.as_ref().is_some_and(|u| u.tab == tab)) {
            place(spot);
        }
        return;
    }
    STATE.with_borrow_mut(|s| s.looked = Some(looked));
    // Passwords only for a box that goes with one, and Search's rule for a box that
    // only might be a sign-in's: this host's own accounts, never the rest of the site's.
    let mut kept: Vec<vault::Kept> = if fills() && passwords {
        let bare = host.strip_prefix("www.").unwrap_or(&host);
        vault::kept_matching(bare, wide)
            .into_iter()
            .filter(|k| !clear || k.clear)
            .collect()
    } else {
        Vec::new()
    };
    // The account whose name the step before was sent with comes first.
    if let Some(given) = forms::given_name(tab, &host)
        && let Some(at) = kept
            .iter()
            .position(|k| k.user.eq_ignore_ascii_case(&given))
    {
        let first = kept.remove(at);
        kept.insert(0, first);
    }
    kept.truncate(MOST);
    if kept.is_empty() {
        lower_now(tab);
        return;
    }
    let rows = kept.len();
    STATE.with_borrow_mut(|s| {
        s.up = Some(Up {
            tab,
            host,
            clear,
            name_only: name,
            logins: kept,
            shown: Instant::now(),
            hover: None,
        });
    });
    show(spot, rows);
}

/// The box's rect in the stage's points: CSS pixels times the page's zoom, in the web
/// view's (flipped) space, then the stage's.
fn spot(web: &WKWebView, rect: NSRect) -> NSRect {
    // SAFETY: a plain property read on a live web view.
    let zoom = unsafe { web.pageZoom() };
    let local = NSRect::new(
        NSPoint::new(rect.origin.x * zoom, rect.origin.y * zoom),
        NSSize::new(rect.size.width * zoom, rect.size.height * zoom),
    );
    match window::stage() {
        Some(stage) => web.convertRect_toView(local, Some(&stage)),
        None => local,
    }
}

fn card_size(rows: usize, spot: NSRect) -> NSSize {
    #[allow(clippy::cast_precision_loss)] // a handful of rows
    let height = rows as f64 * ROW + 2.0 * PAD;
    NSSize::new(spot.size.width.clamp(260.0, 380.0), height)
}

fn show(spot: NSRect, rows: usize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(stage) = window::stage() else { return };
    let card = STATE.with_borrow_mut(|s| s.card.get_or_insert_with(|| Card::new(mtm)).clone());
    // SAFETY: a plain read of a live view's superview.
    if unsafe { card.superview() }.as_deref() != Some(&*stage) {
        card.removeFromSuperview();
        stage.addSubview(&card);
    }
    let size = card_size(rows, spot);
    card.setFrameSize(NSSize::new(
        size.width + 2.0 * SHADOW,
        size.height + 2.0 * SHADOW,
    ));
    place(spot);
    card.setHidden(false);
    card.setNeedsDisplay(true);
}

/// Under the box, or over it when there is no room below; inside the stage either way.
fn place(spot: NSRect) {
    let Some(stage) = window::stage() else { return };
    let Some(card) = STATE.with_borrow(|s| s.card.clone()) else {
        return;
    };
    let bounds = stage.bounds();
    let size = card.frame().size;
    let inner = size.height - 2.0 * SHADOW;
    let below = spot.origin.y + spot.size.height + 4.0;
    let top = if below + inner > bounds.size.height && spot.origin.y - 4.0 - inner > 0.0 {
        spot.origin.y - 4.0 - inner
    } else {
        below
    };
    let x = spot
        .origin
        .x
        .min(bounds.size.width - (size.width - 2.0 * SHADOW) - 8.0)
        .max(8.0);
    card.setFrameOrigin(NSPoint::new(x - SHADOW, top - SHADOW));
}

/// Taken down a beat after the caret leaves, not the same instant: clicking a row can
/// take the caret out of the page first, and a list gone on the way down is never
/// clicked.
fn lower_soon(tab: TabId) {
    let ticket = STATE.with_borrow_mut(|s| {
        s.lowering += 1;
        s.lowering
    });
    let later = dispatch2::DispatchTime::NOW.time(200_000_000);
    let _ = dispatch2::DispatchQueue::main().after(later, move || {
        if STATE.with_borrow(|s| s.lowering == ticket) {
            lower_now(tab);
        }
    });
}

/// Takes the list down at once, if it is `tab`'s.
pub fn lower_now(tab: TabId) {
    let card = STATE.with_borrow_mut(|s| {
        if s.up.as_ref().is_some_and(|u| u.tab == tab) {
            s.up = None;
            s.card.clone()
        } else {
            None
        }
    });
    if let Some(card) = card {
        card.setHidden(true);
    }
}

/// Another tab came to the front, or none: the list goes.
pub fn tab_changed() {
    let up = STATE.with_borrow(|s| s.up.as_ref().map(|u| u.tab));
    if let Some(tab) = up
        && active() != Some(tab)
    {
        lower_now(tab);
    }
}

/// One of the accounts in the list, picked.
fn pick(row: usize) {
    let Some(up) = STATE.with_borrow_mut(|s| {
        let up = s.up.as_ref()?;
        if up.shown.elapsed().as_secs_f64() < SETTLING || row >= up.logins.len() {
            return None;
        }
        s.up.take()
    }) else {
        return;
    };
    lower_hidden();
    let Some(web) = web::existing(up.tab) else {
        return;
    };
    // The tab may have gone somewhere else while the list was up: what was offered for
    // one site is never put into another's page.
    let Some((scheme, host)) = forms::page(&web) else {
        return;
    };
    if host != up.host || (scheme == "http") != up.clear {
        return;
    }
    STATE.with_borrow_mut(|s| s.picked_into = Some(up.tab));
    let kept = up.logins[row].clone();
    if up.name_only {
        forms::fill(&web, &kept.user, None, &scheme, &host, |_| {});
        return;
    }
    // Read the selected secret off the main thread.
    let tab = up.tab;
    std::thread::spawn(move || {
        let login = vault::login(&kept);
        dispatch2::DispatchQueue::main().exec_async(move || {
            let Some(login) = login else {
                return say("The keychain didn't hand that password over");
            };
            let Some(web) = web::existing(tab) else {
                return;
            };
            forms::fill(
                &web,
                &login.user,
                Some(&login.password),
                &scheme,
                &host,
                |worked| {
                    if !worked {
                        say("Couldn't find the sign-in fields anymore");
                    }
                },
            );
            vault::touch(&login);
        });
    });
}

fn lower_hidden() {
    if let Some(card) = STATE.with_borrow(|s| s.card.clone()) {
        card.setHidden(true);
    }
}

define_class!(
    /// The accounts under a sign-in box: a card like the field's, one row each.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSignInCard"]
    struct Card;

    impl Card {
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
            if let Some(row) = self.row_at(event) {
                pick(row);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let row = self.row_at(event);
            let changed = STATE.with_borrow_mut(|s| {
                s.up.as_mut()
                    .is_some_and(|u| std::mem::replace(&mut u.hover, row) != row)
            });
            if changed {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            STATE.with_borrow_mut(|s| {
                if let Some(up) = s.up.as_mut() {
                    up.hover = None;
                }
            });
            self.setNeedsDisplay(true);
        }

        /// Only the card takes clicks; its shadow's room lets them through.
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

impl Card {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        let card: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // SAFETY: a tracking area owned by the view it tracks, for its whole visible rect.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveInKeyWindow
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&card),
                None,
            )
        };
        card.addTrackingArea(&area);
        card.setHidden(true);
        card
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
        let count = STATE.with_borrow(|s| s.up.as_ref().map_or(0, |u| u.logins.len()));
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
        let Some((logins, hover)) =
            STATE.with_borrow(|s| s.up.as_ref().map(|u| (u.logins.clone(), u.hover)))
        else {
            return;
        };
        palette::shadowed(&palette::float_shadow(dark), || {
            palette::GROUND.resolve(dark).setFill();
            palette::card_path(self.card()).fill();
        });
        for (row, kept) in logins.iter().enumerate() {
            let rect = self.row_rect(row);
            if hover == Some(row) {
                palette::WASH.resolve(dark).setFill();
                let plate = palette::pill_path(rect);
                plate.fill();
                palette::stroke_hairline(&plate, dark);
            }
            let left = rect.origin.x + 12.0;
            let width = rect.size.width - 24.0;
            let user = if kept.user.is_empty() {
                "No name"
            } else {
                &kept.user
            };
            palette::draw_text(
                user,
                NSRect::new(
                    NSPoint::new(left, rect.origin.y + 3.0),
                    NSSize::new(width, 17.0),
                ),
                &palette::INK.resolve(dark),
            );
            palette::draw_text(
                &kept.host,
                NSRect::new(
                    NSPoint::new(left, rect.origin.y + 19.0),
                    NSSize::new(width, 17.0),
                ),
                &palette::MUTED.resolve(dark),
            );
        }
    }
}

// MARK: - offering to keep one

/// A sign-in took (`forms::settle`): its password is offered a place in the keychain,
/// unless it is kept already, saving is off, or the site said never.
pub fn credentials(tab: TabId, host: String, user: String, password: String, clear: bool) {
    let _ = tab;
    if !saves() || password.is_empty() || vault::is_never(&host) {
        return;
    }
    // Off the main thread: reading a kept password is the keychain's to allow, and a
    // window never waits on it.
    std::thread::spawn(move || {
        let ours: Vec<vault::Kept> = vault::kept_for(&host)
            .into_iter()
            .filter(|k| k.user == user)
            .collect();
        if let Some(same) = ours.first()
            && let Some(mut login) = vault::login(same)
            && login.password == password
        {
            // Where it was last used is where it is offered from now on.
            login.clear = clear;
            vault::touch(&login);
            return;
        }
        let changed = !ours.is_empty();
        dispatch2::DispatchQueue::main().exec_async(move || {
            STATE.with_borrow_mut(|s| {
                s.offer = Some(Offer {
                    host,
                    user,
                    password,
                    clear,
                    changed,
                });
            });
            show_offer();
        });
    });
}

fn question(offer: &Offer) -> String {
    if offer.changed {
        format!("Update the password for {} on {}?", offer.user, offer.host)
    } else if offer.user.is_empty() {
        format!("Save this password for {}?", offer.host)
    } else {
        format!("Save the password for {} on {}?", offer.user, offer.host)
    }
}

fn show_offer() {
    let Some(words) = STATE.with_borrow(|s| s.offer.as_ref().map(question)) else {
        return;
    };
    let changed = STATE.with_borrow(|s| s.offer.as_ref().is_some_and(|o| o.changed));
    bar_show(&words, true, changed);
}

#[derive(Clone, Copy)]
enum Answer {
    Keep,
    Later,
    Never,
}

fn answered(answer: Answer) {
    let Some(offer) = STATE.with_borrow_mut(|s| s.offer.take()) else {
        return bar_hide();
    };
    match answer {
        Answer::Keep => {
            if vault::save(
                &offer.host,
                &offer.user,
                &offer.password,
                Some(tap_core::history::now()),
                offer.clear,
            ) {
                say(&if offer.changed {
                    format!("Password updated for {}", offer.host)
                } else {
                    format!("Password saved for {}", offer.host)
                });
            } else {
                say("The keychain refused it");
            }
        }
        Answer::Later => bar_hide(),
        Answer::Never => {
            vault::add_never([offer.host.clone()]);
            say(&format!("Never for {}", offer.host));
        }
    }
}

/// A word in the bar, gone after a moment.
pub fn say(words: &str) {
    if STATE.with_borrow(|s| s.offer.is_some()) {
        return;
    }
    bar_show(words, false, false);
    let ticket = STATE.with_borrow(|s| s.bar.as_ref().map_or(0, |b| b.said));
    let later = dispatch2::DispatchTime::NOW.time(2_500_000_000);
    let _ = dispatch2::DispatchQueue::main().after(later, move || {
        let same = STATE
            .with_borrow(|s| s.offer.is_none() && s.bar.as_ref().is_some_and(|b| b.said == ticket));
        if same {
            bar_hide();
        }
    });
}

fn bar_hide() {
    STATE.with_borrow(|s| {
        if let Some(bar) = &s.bar {
            bar.pill.setHidden(true);
        }
    });
}

const BAR_HEIGHT: f64 = 40.0;
const BAR_SHADOW: f64 = 18.0;

fn bar_show(words: &str, asking: bool, changed: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(stage) = window::stage() else { return };
    STATE.with_borrow_mut(|s| {
        let bar = s.bar.get_or_insert_with(|| make_bar(mtm));
        bar.said += 1;
        // SAFETY: a plain read of a live view's superview.
        if unsafe { bar.pill.superview() }.as_deref() != Some(&*stage) {
            bar.pill.removeFromSuperview();
            stage.addSubview(&bar.pill);
        }
        bar.words.setStringValue(&NSString::from_str(words));
        bar.words.sizeToFit();
        bar.save
            .setTitle(&NSString::from_str(if changed { "Update" } else { "Save" }));
        for button in [&bar.save, &bar.later, &bar.never] {
            button.sizeToFit();
        }
        bar.save.setHidden(!asking);
        bar.later.setHidden(!asking);
        bar.never.setHidden(!asking || changed);
        let mut x = BAR_SHADOW + 16.0;
        let text = bar.words.frame().size;
        bar.words.setFrameOrigin(NSPoint::new(
            x,
            BAR_SHADOW + (BAR_HEIGHT - text.height) / 2.0,
        ));
        x += text.width + 12.0;
        for button in [&bar.save, &bar.later, &bar.never] {
            if button.isHidden() {
                continue;
            }
            let size = button.frame().size;
            button.setFrameOrigin(NSPoint::new(
                x,
                BAR_SHADOW + (BAR_HEIGHT - size.height) / 2.0,
            ));
            x += size.width + 6.0;
        }
        let width = x + 10.0 - BAR_SHADOW;
        let bounds = stage.bounds();
        bar.pill.setFrame(NSRect::new(
            NSPoint::new(
                (bounds.size.width - width) / 2.0 - BAR_SHADOW,
                bounds.size.height - 30.0 - BAR_HEIGHT - BAR_SHADOW,
            ),
            NSSize::new(width + 2.0 * BAR_SHADOW, BAR_HEIGHT + 2.0 * BAR_SHADOW),
        ));
        bar.pill.setHidden(false);
        bar.pill.setNeedsDisplay(true);
    });
}

fn make_bar(mtm: MainThreadMarker) -> Bar {
    let pill = Pill::new(mtm);
    pill.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin
            | NSAutoresizingMaskOptions::ViewMaxXMargin
            | NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    let words = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    words.setFont(Some(&palette::font()));
    words.setTextColor(Some(&palette::INK.dynamic()));
    pill.addSubview(&words);
    let target = Target::new(mtm);
    let button = |title: &str, action: Sel| {
        // SAFETY: the target lives as long as the bar, and the action takes the sender.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&target),
                Some(action),
                mtm,
            )
        };
        button.setControlSize(NSControlSize::Small);
        pill.addSubview(&button);
        button
    };
    let save = button("Save", sel!(keep:));
    let later = button("Not now", sel!(later:));
    let never = button("Never here", sel!(never:));
    pill.setHidden(true);
    Bar {
        pill,
        words,
        save,
        later,
        never,
        _target: target,
        said: 0,
    }
}

define_class!(
    /// The bar's buttons' target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSignInTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(keep:))]
        fn keep(&self, _sender: Option<&AnyObject>) {
            answered(Answer::Keep);
        }

        #[unsafe(method(later:))]
        fn later(&self, _sender: Option<&AnyObject>) {
            answered(Answer::Later);
        }

        #[unsafe(method(never:))]
        fn never(&self, _sender: Option<&AnyObject>) {
            answered(Answer::Never);
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// The bar's ground: a rounded pill with a hairline and a shadow, as Search's.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSignInPill"]
    struct Pill;

    impl Pill {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            let bounds = self.bounds();
            let rect = NSRect::new(
                NSPoint::new(BAR_SHADOW, BAR_SHADOW),
                NSSize::new(
                    bounds.size.width - 2.0 * BAR_SHADOW,
                    bounds.size.height - 2.0 * BAR_SHADOW,
                ),
            );
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 20.0, 20.0);
            palette::shadowed(&palette::shadow(16.0, -4.0, 0.12, 0.45, dark), || {
                palette::GROUND.resolve(dark).setFill();
                path.fill();
            });
            palette::stroke_hairline(&path, dark);
        }
    }
);

impl Pill {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

/// For the bench: what the list under a box shows, and what the bar says.
pub fn snapshot() -> serde_json::Value {
    STATE.with_borrow(|s| {
        serde_json::json!({
            "list": s.up.as_ref().map(|u| u.logins.iter().map(|k| serde_json::json!({
                "host": k.host, "user": k.user,
            })).collect::<Vec<_>>()),
            "nameOnly": s.up.as_ref().map(|u| u.name_only),
            "offer": s.offer.as_ref().map(|o| serde_json::json!({
                "host": o.host, "user": o.user, "changed": o.changed,
            })),
            "bar": s.bar.as_ref().filter(|b| !b.pill.isHidden()).map(|b| b.words.stringValue().to_string()),
        })
    })
}

/// For the bench: a row picked as a click would, past the half second.
pub fn bench_pick(row: usize) {
    STATE.with_borrow_mut(|s| {
        if let Some(up) = s.up.as_mut() {
            up.shown = Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap_or(up.shown);
        }
    });
    pick(row);
}

/// For the bench: the bar's buttons, pressed.
pub fn bench_answer(answer: &str) {
    answered(match answer {
        "save" => Answer::Keep,
        "never" => Answer::Never,
        _ => Answer::Later,
    });
}
