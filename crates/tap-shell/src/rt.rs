//! The one owner of the model, on the main thread (docs/PLAN.md §5.1).
//!
//! Everything that changes the model goes through [`dispatch`]: the change runs with the
//! model borrowed, then — the borrow released — the shell carries out the effects it
//! asked for and redraws what its marks name. AppKit and WebKit are never called while
//! the model is borrowed; a callback they make during a flush that wants to change the
//! model is queued and run right after, never nested.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
};

use tap_core::browser::{Browser, Effect};

use crate::{web, window};

type Change = Box<dyn FnOnce(&mut Browser)>;

thread_local! {
    static BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static PENDING: RefCell<VecDeque<Change>> = const { RefCell::new(VecDeque::new()) };
}

/// Hands the model over. Once, at launch.
pub fn install(browser: Browser) {
    BROWSER.with_borrow_mut(|slot| *slot = Some(browser));
}

/// Changes the model, then carries out its effects and redraws.
pub fn dispatch(change: impl FnOnce(&mut Browser) + 'static) {
    if DEPTH.get() > 0 {
        PENDING.with_borrow_mut(|queue| queue.push_back(Box::new(change)));
        return;
    }
    DEPTH.set(1);
    apply(Box::new(change));
    settle_inner();
    DEPTH.set(0);
}

/// Changes the model and answers, without flushing: for a WebKit callback that must
/// return something it can only make once the model has changed (a new tab's web
/// view). The caller calls [`settle`] when it is ready for the views to catch up.
pub fn update<R>(change: impl FnOnce(&mut Browser) -> R) -> Option<R> {
    BROWSER.with_borrow_mut(|slot| slot.as_mut().map(change))
}

/// Carries out what [`update`] left waiting, unless a dispatch already will.
pub fn settle() {
    if DEPTH.get() > 0 {
        return;
    }
    DEPTH.set(1);
    settle_inner();
    DEPTH.set(0);
}

/// Reads the model, for drawing. `None` while it is being changed, which only a bug
/// could make happen: a view is never asked to draw in the middle of a change.
pub fn read<R>(look: impl FnOnce(&Browser) -> R) -> Option<R> {
    BROWSER.with(|slot| slot.try_borrow().ok().and_then(|b| b.as_ref().map(look)))
}

fn apply(change: Change) {
    BROWSER.with_borrow_mut(|slot| {
        if let Some(browser) = slot.as_mut() {
            change(browser);
        }
    });
}

fn settle_inner() {
    loop {
        flush();
        let Some(next) = PENDING.with_borrow_mut(VecDeque::pop_front) else {
            break;
        };
        apply(next);
    }
}

fn flush() {
    let Some((window_dirty, tab_dirty, effects, view)) = BROWSER.with_borrow_mut(|slot| {
        let browser = slot.as_mut()?;
        let (window_dirty, tab_dirty) = browser.take_dirty();
        let effects = browser.take_effects();
        Some((window_dirty, tab_dirty, effects, window::View::of(browser)))
    }) else {
        return;
    };
    // Focus last: a page can only take the keyboard once it is in the window, and the
    // views are brought up to date (a newly shown page added) in between.
    let (focus, rest): (Vec<Effect>, Vec<Effect>) = effects
        .into_iter()
        .partition(|e| matches!(e, Effect::FocusPage(_)));
    for effect in rest {
        web::perform(effect);
    }
    window::update(window_dirty, &tab_dirty, &view);
    for effect in focus {
        web::perform(effect);
    }
}
