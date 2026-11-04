//! Tap › Check for Updates…: a small window that follows the check from start to end
//! (Kil's `update_window.rs`): checking, the download with its bar, verifying, then
//! ready (Later or Restart Now), up to date, or why it can't update.

use std::{cell::RefCell, sync::Arc};

use block2::RcBlock;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSFont, NSFontWeightSemibold, NSProgressIndicator,
    NSProgressIndicatorStyle, NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer};

use crate::{
    menu::{Command, Commands},
    palette,
    update::{self, Progress, Shown, Step},
};

struct Views {
    window: Retained<NSWindow>,
    headline: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    left: Retained<NSButton>,
    right: Retained<NSButton>,
    shown: Shown,
    timer: Option<Retained<NSTimer>>,
}

thread_local! {
    static VIEWS: RefCell<Option<Views>> = const { RefCell::new(None) };
    static COMMANDS: RefCell<Option<Retained<Commands>>> = const { RefCell::new(None) };
}

/// The target the window's buttons send to: the menus' own.
pub fn install(commands: Retained<Commands>) {
    COMMANDS.with_borrow_mut(|slot| *slot = Some(commands));
}

const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 150.0;

fn make(mtm: MainThreadMarker) -> Option<Views> {
    let commands = COMMANDS.with_borrow(Clone::clone)?;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
    // SAFETY: a fresh allocation, initialised once, on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: kept in `VIEWS` until closed for good; AppKit must not also release it.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Software Update"));
    let content = window.contentView()?;
    let headline = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    // SAFETY: the system font at a valid size and weight.
    let bold = unsafe { NSFont::systemFontOfSize_weight(13.0, NSFontWeightSemibold) };
    headline.setFont(Some(&bold));
    headline.setFrame(NSRect::new(
        NSPoint::new(20.0, 108.0),
        NSSize::new(WIDTH - 40.0, 20.0),
    ));
    let detail = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    detail.setFont(Some(&NSFont::systemFontOfSize(11.5)));
    detail.setTextColor(Some(&palette::MUTED.dynamic()));
    detail.setFrame(NSRect::new(
        NSPoint::new(20.0, 72.0),
        NSSize::new(WIDTH - 40.0, 32.0),
    ));
    let bar = NSProgressIndicator::new(mtm);
    bar.setStyle(NSProgressIndicatorStyle::Bar);
    bar.setFrame(NSRect::new(
        NSPoint::new(20.0, 56.0),
        NSSize::new(WIDTH - 40.0, 12.0),
    ));
    let button = |title: &str, command: Command| {
        // SAFETY: the target outlives the button (both live for the app's life), and
        // `run:` takes the sender, as an action must.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(&commands),
                Some(sel!(run:)),
                mtm,
            )
        };
        button.setTag(command as isize);
        button
    };
    let left = button("Later", Command::CloseUpdate);
    left.setFrame(NSRect::new(
        NSPoint::new(WIDTH - 236.0, 14.0),
        NSSize::new(104.0, 32.0),
    ));
    let right = button("Restart Now", Command::RestartToUpdate);
    right.setFrame(NSRect::new(
        NSPoint::new(WIDTH - 124.0, 14.0),
        NSSize::new(104.0, 32.0),
    ));
    content.addSubview(&headline);
    content.addSubview(&detail);
    content.addSubview(&bar);
    content.addSubview(&left);
    content.addSubview(&right);
    window.center();
    Some(Views {
        window,
        headline,
        detail,
        bar,
        left,
        right,
        shown: Shown::UpToDate,
        timer: None,
    })
}

/// Opens the window, or brings it forward, showing `shown`.
pub fn show(shown: Shown) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let made = VIEWS.with_borrow(Option::is_some);
    if !made && let Some(views) = make(mtm) {
        VIEWS.with_borrow_mut(|slot| *slot = Some(views));
    }
    set(shown);
    VIEWS.with_borrow(|views| {
        if let Some(views) = views {
            views.window.makeKeyAndOrderFront(None);
        }
    });
    crate::links::bring_app_forward(mtm);
}

/// Shows a new outcome if the window is open, and does nothing if it isn't: a check
/// nobody asked for says nothing.
pub fn refresh(shown: Shown) {
    if is_open() {
        set(shown);
    }
}

pub fn is_open() -> bool {
    VIEWS.with_borrow(|views| views.as_ref().is_some_and(|v| v.window.isVisible()))
}

/// Puts it away; a check under way goes on.
pub fn close() {
    VIEWS.with_borrow_mut(|views| {
        if let Some(views) = views {
            if let Some(timer) = views.timer.take() {
                timer.invalidate();
            }
            views.window.orderOut(None);
        }
    });
}

fn set(shown: Shown) {
    let progress = match &shown {
        Shown::Working(progress) => Some(progress.clone()),
        _ => None,
    };
    VIEWS.with_borrow_mut(|views| {
        let Some(views) = views else { return };
        views.shown = shown;
        if let Some(timer) = views.timer.take() {
            timer.invalidate();
        }
        if let Some(progress) = progress {
            views.timer = Some(poll(progress));
        }
        draw(views);
    });
}

/// While working: the step is read again every tenth of a second.
fn poll(progress: Arc<Progress>) -> Retained<NSTimer> {
    let block = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        let _ = &progress;
        VIEWS.with_borrow(|views| {
            if let Some(views) = views {
                draw(views);
            }
        });
    });
    // SAFETY: a repeating timer on the main run loop, its block kept by the timer.
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(
            update::POLL.as_secs_f64(),
            true,
            &block,
        )
    }
}

fn megabytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // a DMG is megabytes, not petabytes
    let mb = bytes as f64 / 1_000_000.0;
    format!("{mb:.1} MB")
}

fn draw(views: &Views) {
    let current = update::current();
    let step = match &views.shown {
        Shown::Working(progress) => Some(progress.step()),
        _ => None,
    };
    let (headline, detail) = match (&views.shown, &step) {
        (Shown::Working(_), Some(Step::Checking) | None) => (
            "Checking for updates…".to_owned(),
            format!("You have Tap {current}."),
        ),
        (
            Shown::Working(_),
            Some(Step::Downloading {
                version,
                got,
                total,
            }),
        ) => (
            format!("Downloading Tap {version}…"),
            match total {
                Some(total) => format!("{} of {}", megabytes(*got), megabytes(*total)),
                None => megabytes(*got),
            },
        ),
        (Shown::Working(_), Some(Step::Verifying { version })) => (
            format!("Verifying Tap {version}…"),
            "Checking its signature and Apple's notarization.".to_owned(),
        ),
        (Shown::Ready(version), _) => (
            format!("Tap {version} is ready"),
            "Restart now, or it installs when Tap quits. Your tabs come back as they are."
                .to_owned(),
        ),
        (Shown::UpToDate, _) => (
            "Tap is up to date".to_owned(),
            format!("{current} is the newest version."),
        ),
        (Shown::Failed(why), _) => ("Tap can't update right now".to_owned(), why.clone()),
    };
    views
        .headline
        .setStringValue(&NSString::from_str(&headline));
    views.detail.setStringValue(&NSString::from_str(&detail));
    match (&views.shown, &step) {
        (
            Shown::Working(_),
            Some(Step::Downloading {
                got,
                total: Some(total),
                ..
            }),
        ) => {
            views.bar.setHidden(false);
            views.bar.setIndeterminate(false);
            views.bar.setMaxValue(1.0);
            #[allow(clippy::cast_precision_loss)]
            views
                .bar
                .setDoubleValue(*got as f64 / (*total).max(1) as f64);
        }
        (Shown::Working(_), _) => {
            views.bar.setHidden(false);
            views.bar.setIndeterminate(true);
            // SAFETY: a plain call on a live indicator.
            unsafe { views.bar.startAnimation(None) };
        }
        _ => {
            // SAFETY: as above.
            unsafe { views.bar.stopAnimation(None) };
            views.bar.setHidden(true);
        }
    }
    let ready = matches!(views.shown, Shown::Ready(_));
    let working = matches!(views.shown, Shown::Working(_));
    views.right.setHidden(!ready);
    views.left.setTitle(&NSString::from_str(if ready {
        "Later"
    } else if working {
        "Hide"
    } else {
        "OK"
    }));
    // The one button there is sits at the right; Restart Now takes Return when it shows.
    let x = if ready { WIDTH - 236.0 } else { WIDTH - 124.0 };
    views.left.setFrameOrigin(NSPoint::new(x, 14.0));
    views
        .right
        .setKeyEquivalent(&NSString::from_str(if ready { "\r" } else { "" }));
    views
        .left
        .setKeyEquivalent(&NSString::from_str(if ready { "\u{1b}" } else { "\r" }));
}
