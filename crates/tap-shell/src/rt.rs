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

use tap_core::browser::{Browser, BrowserFields, Effect, TabFields, TabId};

use crate::{web, window};

type Change = Box<dyn FnOnce(&mut Browser)>;

thread_local! {
    static BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static PENDING: RefCell<VecDeque<Change>> = const { RefCell::new(VecDeque::new()) };
    // Launch can consume the initial dirty marks before the window exists.
    static WINDOW_UPDATED: Cell<bool> = const { Cell::new(false) };
}

/// Hands the model over. Once, at launch.
pub fn install(browser: Browser) {
    BROWSER.with_borrow_mut(|slot| *slot = Some(browser));
    WINDOW_UPDATED.set(false);
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

type Flush = (
    BrowserFields,
    Vec<(TabId, TabFields)>,
    Vec<Effect>,
    window::View,
);

fn take_flush(browser: &mut Browser, window_updated: bool) -> Option<Flush> {
    let (window_dirty, tab_dirty) = browser.take_dirty();
    let effects = browser.take_effects();
    if window_updated && window_dirty.is_empty() && tab_dirty.is_empty() && effects.is_empty() {
        return None;
    }
    Some((window_dirty, tab_dirty, effects, window::View::of(browser)))
}

fn flush() {
    let Some((window_dirty, tab_dirty, effects, view)) = BROWSER.with_borrow_mut(|slot| {
        slot.as_mut()
            .and_then(|browser| take_flush(browser, WINDOW_UPDATED.get()))
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
    if !WINDOW_UPDATED.get() && window::main_window().is_some() {
        WINDOW_UPDATED.set(true);
    }
    for effect in focus {
        web::perform(effect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_load_notifications_skip_snapshots_after_window_update() {
        let mut browser = Browser::new();
        let tab = browser.active().unwrap();
        browser.take_dirty();
        browser.take_effects();

        for _ in 0..100 {
            browser.page_loading(tab, false, 0.0);
            assert!(take_flush(&mut browser, true).is_none());
        }
    }

    #[test]
    fn clean_flush_still_initializes_window() {
        let mut browser = Browser::new();
        browser.take_dirty();
        browser.take_effects();

        assert!(take_flush(&mut browser, false).is_some());
    }

    #[test]
    fn dirty_load_notification_updates_window() {
        let mut browser = Browser::new();
        let tab = browser.active().unwrap();
        browser.take_dirty();
        browser.take_effects();
        browser.page_loading(tab, true, 0.1);

        let (_, tabs, _, _) = take_flush(&mut browser, true).unwrap();
        assert!(tabs.iter().any(|(id, fields)| {
            *id == tab && fields.contains(TabFields::LOADING | TabFields::PROGRESS)
        }));
    }

    #[test]
    fn dirty_window_is_not_skipped() {
        let mut browser = Browser::new();
        browser.take_dirty();
        browser.take_effects();
        browser.begin_editing(false);

        let (dirty, _, _, _) = take_flush(&mut browser, true).unwrap();
        assert!(dirty.contains(BrowserFields::EDITING));
    }

    #[test]
    fn queued_changes_run_in_order_after_clean_flushes() {
        use std::rc::Rc;

        install(Browser::new());
        update(|browser| {
            browser.take_dirty();
            browser.take_effects();
        });
        WINDOW_UPDATED.set(true);
        let order = Rc::new(RefCell::new(Vec::new()));
        let first = Rc::clone(&order);
        let second = Rc::clone(&order);
        dispatch(move |_| {
            dispatch(move |_| first.borrow_mut().push(1));
            dispatch(move |_| second.borrow_mut().push(2));
        });

        assert_eq!(*order.borrow(), [1, 2]);
    }

    #[test]
    fn clean_flush_keeps_focus_effects() {
        let mut browser = Browser::new();
        browser.take_dirty();
        browser.take_effects();
        browser.focus_page();

        let (_, _, effects, _) = take_flush(&mut browser, true).unwrap();
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::FocusPage(_)))
        );
    }
}
