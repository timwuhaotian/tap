//! The places you kept, and the card that lists them (Search's `Bookmarks.swift`).
//!
//! The star in the address bar keeps the page in front, or lets it go; the list beside
//! it opens this card, which says what is kept and takes you there. The file is the
//! one the importers write, so a folder that came from Chrome is here too.

use std::cell::RefCell;

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{NSEvent, NSImage, NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CADisplayLink;
use tap_core::{bookmarks::Entry, location};

use crate::{cards, palette, rt};

/// The card's width.
pub const WIDTH: f64 = 300.0;
/// A row's height.
const ROW: f64 = 28.0;
/// How many rows the card shows before the list scrolls.
#[allow(clippy::cast_precision_loss)] // a handful of rows
const VISIBLE: f64 = 8.0;

/// The card's height for the rows it shows: the rows, their padding, and the shadow's
/// room around the card.
#[must_use]
pub fn height(rows: usize) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    // a handful of rows
    let rows = rows.min(VISIBLE as usize) as f64;
    rows * ROW + 2.0 * cards::PAD + 2.0 * cards::SHADOW
}

/// Keeps the page in front, or lets it go: the star in the address bar, and ⇧⌘B.
pub fn keep_current() {
    rt::dispatch(|browser| {
        browser.toggle_bookmark();
    });
}

/// The star in the address bar: it keeps the page in front, or lets it go.
pub fn star(mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSButton> {
    button(mtm, "bookmark", "Keep this page", sel!(keep:))
}

/// The list of what is kept, beside the star.
pub fn list(mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSButton> {
    button(mtm, "list.bullet", "Bookmarks", sel!(open:))
}

/// The star's face: filled when the page is kept.
pub fn star_image(kept: bool) -> Option<Retained<NSImage>> {
    let symbol = if kept { "bookmark.fill" } else { "bookmark" };
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str("Keep this page")),
    )
}

/// The card of what is kept.
pub fn panel(mtm: MainThreadMarker) -> Retained<Panel> {
    Panel::new(mtm)
}

/// A button in the address bar, with the symbol and the name it answers to.
fn button(
    mtm: MainThreadMarker,
    symbol: &str,
    name: &str,
    action: objc2::runtime::Sel,
) -> Retained<objc2_app_kit::NSButton> {
    use objc2_app_kit::NSButton;
    let target =
        TARGET.with_borrow_mut(|slot| slot.get_or_insert_with(|| Target::new(mtm)).clone());
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(name)),
    );
    // SAFETY: the target outlives the button (both live for the app's life in `TARGET`
    // and the window), and the action takes the sender, as an action must.
    let button = unsafe {
        NSButton::buttonWithImage_target_action(
            &image.unwrap_or_default(),
            Some(&target),
            Some(action),
            mtm,
        )
    };
    button.setBordered(false);
    button.setToolTip(Some(&NSString::from_str(name)));
    button
}

define_class!(
    /// The star's and the list's target: the one click each takes.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBookmarks"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        /// The star: the page in front is kept, or let go.
        #[unsafe(method(keep:))]
        fn keep_action(&self, _sender: Option<&AnyObject>) {
            keep_current();
        }

        /// The list: the card of what is kept.
        #[unsafe(method(open:))]
        fn open_action(&self, _sender: Option<&AnyObject>) {
            crate::window::open_bookmarks();
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

thread_local! {
    /// The buttons' target, kept for the app's life beside the panel below.
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
}

/// The card's own state: how far the list is scrolled, and how far the card has come
/// in.
pub struct Ivars {
    state: cards::State,
}

define_class!(
    /// The card of what is kept: the places you kept, in the order the file keeps
    /// them, a folder's places under it.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapBookmarksPanel"]
    #[ivars = Ivars]
    pub struct Panel;

    impl Panel {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// One frame of the card's coming in.
        #[unsafe(method(panelFrame:))]
        fn panel_frame(&self, _link: &CADisplayLink) {
            let dt = self.ivars().state.dt();
            if !self.ivars().state.tick(self, dt) {
                self.ivars().state.pause();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(event);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.scroll(event);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// Only the card takes clicks; its shadow's room lets them through to whatever
        /// is under it.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: a plain read of a live view's superview.
            let holder = unsafe { self.superview() };
            let local = self.convertPoint_fromView(point, holder.as_deref());
            let card = self.card();
            let inside = palette::inside(card, local);
            // SAFETY: the superclass's own hit test, for a point inside the card.
            inside.then(|| unsafe { msg_send![super(self), hitTest: point] }).flatten()
        }
    }
);

impl Panel {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            state: cards::State::default(),
        });
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// The card comes up under the list in the address bar.
    pub fn show(&self) {
        self.ivars().state.show(self, sel!(panelFrame:));
    }

    /// The card goes away.
    pub fn hide(&self) {
        self.ivars().state.hide(self);
    }

    /// Whether a point in the window is on the card itself.
    pub fn holds(&self, point: NSPoint) -> bool {
        let rect = self.convertRect_toView(self.card(), None);
        palette::inside(rect, point)
    }

    /// The card's own ground, without the shadow's room around it.
    fn card(&self) -> NSRect {
        cards::State::card(self.bounds())
    }

    /// A row's place in the card.
    fn row_rect(&self, row: usize, top: f64) -> NSRect {
        let card = self.card();
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let y = card.origin.y + cards::PAD + row as f64 * ROW - top;
        NSRect::new(
            NSPoint::new(card.origin.x + cards::PAD, y),
            NSSize::new(card.size.width - 2.0 * cards::PAD, ROW),
        )
    }

    /// The row under a point, if it is on one.
    fn row_at(&self, point: NSPoint) -> Option<usize> {
        let card = self.card();
        let y = point.y - card.origin.y - cards::PAD + self.ivars().state.offset();
        if y < 0.0 {
            return None;
        }
        let row = (y / ROW).floor();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a row's number
        let row = row as usize;
        (row < Self::rows().len()).then_some(row)
    }

    /// The rows as they are now.
    fn rows() -> Vec<Entry> {
        rt::read(|browser| browser.bookmarks().entries()).unwrap_or_default()
    }

    /// A click on a place goes there, and the card goes away.
    fn press(&self, event: &NSEvent) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let Some(row) = self.row_at(point) else {
            return;
        };
        let Some(Entry::Site { url, .. }) = Self::rows().get(row).cloned() else {
            return;
        };
        rt::dispatch(move |browser| browser.open_bookmark(&url));
        crate::window::close_panels();
    }

    /// The wheel scrolls the list, when there is more of it than the card shows.
    fn scroll(&self, event: &NSEvent) {
        let rows = Self::rows().len();
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let more = (rows as f64 - VISIBLE).max(0.0) * ROW;
        self.ivars().state.scroll(event.scrollingDeltaY(), more);
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let dark = palette::is_dark(self);
        let rows = Self::rows();
        let card = self.card();
        cards::ground(card, dark);
        if rows.is_empty() {
            palette::draw_text(
                "Nothing kept yet",
                palette::text_line(card, cards::PAD, cards::PAD),
                &palette::MUTED.resolve(dark),
            );
            return;
        }
        // The rows are clipped to the card, so a row never draws over its rounded ends.
        let path = palette::card_path(card);
        path.addClip();
        let top = self.ivars().state.offset();
        for (row, entry) in rows.iter().enumerate() {
            let rect = self.row_rect(row, top);
            // The rows above the card's own ground are not drawn at all.
            if rect.origin.y + rect.size.height < card.origin.y {
                continue;
            }
            if rect.origin.y > card.origin.y + card.size.height {
                break;
            }
            match entry {
                Entry::Folder { depth, title } => {
                    palette::draw_text(
                        title,
                        palette::text_line(rect, indent(*depth), 0.0),
                        &palette::MUTED.resolve(dark),
                    );
                }
                Entry::Site { depth, title, url } => {
                    let host = location::split(url).1;
                    draw_site(title, host, rect, *depth, dark);
                }
            }
        }
    }
}

/// How far a row in a folder stands in from the card's edge.
fn indent(depth: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)] // a handful of folders
    let depth = depth.min(4) as f64;
    depth * 14.0
}

/// One place: its name, then the host it is at, quieter.
fn draw_site(title: &str, host: &str, rect: NSRect, depth: usize, dark: bool) {
    let line = palette::text_line(rect, indent(depth), 0.0);
    let detail = if host.is_empty() {
        String::new()
    } else {
        format!("— {host}")
    };
    let width = palette::text_width(&detail);
    let name = (line.size.width - width - 10.0).max(0.0);
    palette::draw_text(
        title,
        NSRect::new(line.origin, NSSize::new(name, line.size.height)),
        &palette::INK.resolve(dark),
    );
    if !detail.is_empty() {
        palette::draw_text(
            &detail,
            NSRect::new(
                NSPoint::new(line.origin.x + name + 10.0, line.origin.y),
                NSSize::new(width, line.size.height),
            ),
            &palette::MUTED.resolve(dark),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_tall_enough_for_its_rows_and_no_taller() {
        assert!(height(1) > height(0));
        assert_eq!(height(20), height(8));
        // The shadow's room is around the card, not inside it.
        assert!(height(1) > ROW + 2.0 * cards::PAD);
    }
}
