//! The keys and the clicks that move the cards under the chrome: the list of what is
//! kept, and the downloads door's card.
//!
//! The keys are ⇧⌘B, which keeps the page in front, and ⇧⌘J, which opens the
//! downloads; Escape puts a card away, and so does a click anywhere but on the card
//! or on the button it hangs from.

use std::{cell::RefCell, ptr::null_mut};

use block2::{DynBlock, RcBlock};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSTextField, NSTextView};

/// Which card is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The list of what is kept.
    Bookmarks,
    /// The downloads door's card.
    Downloads,
}

/// Escape's key code, as every Mac keyboard gives it.
const ESCAPE: u16 = 53;

thread_local! {
    /// The card that is up, if one is.
    static OPEN: RefCell<Option<Kind>> = const { RefCell::new(None) };
}

/// The card that is up, if one is.
#[must_use]
pub fn opened() -> Option<Kind> {
    OPEN.with_borrow(|slot| *slot)
}

/// A card is up.
pub fn mark_open(kind: Kind) {
    OPEN.with_borrow_mut(|slot| *slot = Some(kind));
}

/// No card is up.
pub fn mark_closed() {
    OPEN.with_borrow_mut(|slot| *slot = None);
}

/// Watches the keys and the clicks that move the cards.
pub fn install() {
    keys();
    clicks();
}

/// The keys: ⇧⌘B keeps the page, ⇧⌘J opens the downloads, and Escape puts a card
/// away.
fn keys() {
    let block = RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands a live event, for the call's length.
        let event = unsafe { event.as_ref() };
        let flags = event.modifierFlags();
        let command = flags.contains(NSEventModifierFlags::Command);
        let shift = flags.contains(NSEventModifierFlags::Shift);
        if command && shift {
            let keys = event
                .charactersIgnoringModifiers()
                .map(|keys| keys.to_string())
                .unwrap_or_default();
            match keys.to_lowercase().as_str() {
                "b" => {
                    crate::bookmarks::keep_current();
                    return null_mut();
                }
                "j" => {
                    crate::window::toggle_downloads();
                    return null_mut();
                }
                _ => {}
            }
        }
        if event.keyCode() == ESCAPE && opened().is_some() && !a_field_has_it(event) {
            crate::window::close_panels();
            return null_mut();
        }
        std::ptr::NonNull::from(event).as_ptr()
    });
    let block: &DynBlock<dyn Fn(std::ptr::NonNull<NSEvent>) -> *mut NSEvent> = &block;
    // SAFETY: a key-down monitor for this app's own events; the returned monitor is
    // forgotten on purpose, so it lives as long as the app does.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, block)
    };
    std::mem::forget(monitor);
}

/// Whether a text field has the keyboard: its own Escape is its own.
fn a_field_has_it(event: &NSEvent) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(window) = event.window(mtm) else {
        return false;
    };
    let Some(held) = window.firstResponder() else {
        return false;
    };
    // While a field edits, the window's first responder is the field editor (an
    // NSTextView), not the field itself.
    held.downcast_ref::<NSTextField>().is_some()
        || held
            .downcast_ref::<NSTextView>()
            .is_some_and(NSTextView::isFieldEditor)
}

/// A click anywhere but on the card, or on the button it hangs from, puts the card
/// away.
fn clicks() {
    let block = RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands a live event, for the call's length.
        let event = unsafe { event.as_ref() };
        if opened().is_some() && !crate::window::on_a_panel(event.locationInWindow()) {
            crate::window::close_panels();
        }
        std::ptr::NonNull::from(event).as_ptr()
    });
    let block: &DynBlock<dyn Fn(std::ptr::NonNull<NSEvent>) -> *mut NSEvent> = &block;
    // SAFETY: a mouse-down monitor for this app's own events; the returned monitor is
    // forgotten on purpose, so it lives as long as the app does.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown, block)
    };
    std::mem::forget(monitor);
}
