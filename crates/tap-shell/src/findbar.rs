//! The find bar (Search's `Find.swift`): a pill in the page's top corner, the same white
//! and hairline as everything else that floats, gone the moment it isn't wanted. The
//! field, "3 of 17" as the page counted it, the previous and next match, Match Case and
//! Whole Words, and Done.

use std::cell::RefCell;

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSEventModifierFlags, NSFocusRingType, NSFont, NSFontWeightMedium, NSImage, NSMenu, NSMenuItem,
    NSResponder, NSShadow, NSTextField, NSTextFieldDelegate, NSTextView, NSView,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};
use tap_core::browser::Browser;

use crate::{
    menu::{Command, Commands},
    palette, rt,
};

const WIDTH: f64 = 330.0;
const HEIGHT: f64 = 34.0;
/// Room around the pill for its shadow.
const SHADOW: f64 = 12.0;
const FIELD: f64 = 150.0;

/// What the bar shows, read from the model in one go.
#[allow(clippy::struct_excessive_bools)] // the bar's own switches
pub struct State {
    pub open: bool,
    pub needle: String,
    pub status: Option<String>,
    pub missed: bool,
    pub match_case: bool,
    pub whole_words: bool,
}

impl State {
    pub fn of(browser: &Browser) -> State {
        let find = browser.find();
        State {
            open: find.open(),
            needle: find.needle().to_owned(),
            status: find.status(),
            missed: find.missed(),
            match_case: find.match_case(),
            whole_words: find.whole_words(),
        }
    }
}

struct Bar {
    pill: Retained<Pill>,
    field: Retained<NSTextField>,
    status: Retained<NSTextField>,
    options: Retained<NSButton>,
    menu: Retained<NSMenu>,
    _delegate: Retained<FindFieldDelegate>,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// Builds the bar, hidden, in the top corner of `stage`.
#[allow(clippy::too_many_lines)] // one bar, its parts laid out left to right
pub fn install(mtm: MainThreadMarker, stage: &NSView, commands: &Commands) {
    let bounds = stage.bounds();
    let pill = Pill::new(
        mtm,
        NSRect::new(
            NSPoint::new(bounds.size.width - WIDTH - 2.0 * SHADOW, 0.0),
            NSSize::new(WIDTH + 2.0 * SHADOW, HEIGHT + 2.0 * SHADOW),
        ),
    );
    pill.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
    );
    pill.setHidden(true);

    let delegate = FindFieldDelegate::new(mtm);
    let field = NSTextField::new(mtm);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    field.setFont(Some(&NSFont::systemFontOfSize(12.5)));
    field.setPlaceholderString(Some(&NSString::from_str("Find on page")));
    if let Some(cell) = field.cell() {
        cell.setScrollable(true);
    }
    // SAFETY: the delegate is kept beside the field, in `BAR`, for the app's life.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    field.setFrame(NSRect::new(
        NSPoint::new(SHADOW + 12.0, SHADOW + 8.0),
        NSSize::new(FIELD, 18.0),
    ));

    let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    // SAFETY: the system font at a valid size and weight.
    let digits =
        unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(10.0, NSFontWeightMedium) };
    status.setFont(Some(&digits));
    status.setFrame(NSRect::new(
        NSPoint::new(SHADOW + 12.0 + FIELD + 6.0, SHADOW + 10.0),
        NSSize::new(64.0, 14.0),
    ));

    let button = |symbol: &str, name: &str, command: Command, x: f64| {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(name)),
        );
        // SAFETY: the target outlives the button, and `run:` takes the sender.
        let button = unsafe {
            NSButton::buttonWithImage_target_action(
                &image.unwrap_or_default(),
                Some(commands),
                Some(sel!(run:)),
                mtm,
            )
        };
        button.setBordered(false);
        button.setTag(command as isize);
        button.setToolTip(Some(&NSString::from_str(name)));
        button.setFrame(NSRect::new(
            NSPoint::new(x, SHADOW + 6.0),
            NSSize::new(22.0, 22.0),
        ));
        button
    };
    let right = SHADOW + WIDTH - 10.0;
    let done = button("xmark", "Done", Command::CloseFind, right - 22.0);
    let options = button(
        "ellipsis.circle",
        "Options",
        Command::FindOptions,
        right - 48.0,
    );
    let next = button(
        "chevron.down",
        "Next match",
        Command::FindNext,
        right - 74.0,
    );
    let previous = button(
        "chevron.up",
        "Previous match",
        Command::FindPrevious,
        right - 98.0,
    );

    let menu = NSMenu::new(mtm);
    for (title, command) in [
        ("Match Case", Command::MatchCase),
        ("Whole Words", Command::WholeWords),
    ] {
        // SAFETY: a fresh allocation; `run:` takes the sender.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(run:)),
                &NSString::from_str(""),
            )
        };
        item.setTag(command as isize);
        // SAFETY: the commands object lives for the app's life, longer than the menu.
        unsafe { item.setTarget(Some(commands)) };
        menu.addItem(&item);
    }

    for view in [
        &*field as &NSView,
        &status,
        &previous,
        &next,
        &options,
        &done,
    ] {
        pill.addSubview(view);
    }
    stage.addSubview(&pill);
    BAR.with_borrow_mut(|bar| {
        *bar = Some(Bar {
            pill,
            field,
            status,
            options,
            menu,
            _delegate: delegate,
        });
    });
}

/// Brings the bar up to date. `focus`: ⌘F asked for the keys.
pub fn update(state: &State, focus: bool) {
    BAR.with_borrow(|bar| {
        let Some(bar) = bar.as_ref() else { return };
        bar.pill.setHidden(!state.open);
        if !state.open {
            bar.field.setStringValue(&NSString::from_str(""));
            bar.status.setStringValue(&NSString::from_str(""));
            return;
        }
        bar.status
            .setStringValue(&NSString::from_str(state.status.as_deref().unwrap_or("")));
        let color = if state.missed {
            NSColor::systemRedColor()
        } else {
            palette::MUTED.dynamic()
        };
        bar.status.setTextColor(Some(&color));
        for (index, on) in [state.match_case, state.whole_words]
            .into_iter()
            .enumerate()
        {
            if let Some(item) = bar.menu.itemAtIndex(isize::try_from(index).unwrap_or(0)) {
                item.setState(if on {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }
        }
        if focus {
            if bar.field.stringValue().to_string() != state.needle {
                bar.field.setStringValue(&NSString::from_str(&state.needle));
            }
            if let Some(window) = bar.field.window() {
                // Taking the keys selects all the field holds: ⌘F again retypes.
                window.makeFirstResponder(Some(&bar.field));
            }
        }
    });
}

/// The options button: Match Case and Whole Words, under it.
pub fn show_options() {
    BAR.with_borrow(|bar| {
        let Some(bar) = bar.as_ref() else { return };
        let below = NSPoint::new(0.0, bar.options.bounds().size.height + 4.0);
        bar.menu
            .popUpMenuPositioningItem_atLocation_inView(None, below, Some(&bar.options));
    });
}

define_class!(
    /// The pill the bar sits on.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFindPill"]
    struct Pill;

    impl Pill {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            let card = NSRect::new(NSPoint::new(SHADOW, SHADOW), NSSize::new(WIDTH, HEIGHT));
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(card, HEIGHT / 2.0, HEIGHT / 2.0);
            let shadow = NSShadow::new();
            shadow.setShadowBlurRadius(10.0);
            shadow.setShadowOffset(NSSize::new(0.0, -2.0));
            shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0.0, if dark { 0.45 } else { 0.12 })));
            shadow.set();
            palette::GROUND.resolve(dark).setFill();
            path.fill();
            NSShadow::new().set();
            palette::HAIRLINE.resolve(dark).setStroke();
            path.setLineWidth(1.0);
            path.stroke();
        }
    }
);

impl Pill {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    /// The find field's delegate: what is typed is looked for at once, Return is the next
    /// match and ⇧Return the one before, Esc puts the bar away.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFindFieldDelegate"]
    struct FindFieldDelegate;

    unsafe impl NSObjectProtocol for FindFieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FindFieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn changed(&self, note: &NSNotification) {
            let Some(field) = note.object().and_then(|o| o.downcast::<NSTextField>().ok()) else {
                return;
            };
            let needle = field.stringValue().to_string();
            rt::dispatch(move |browser| browser.set_find_needle(&needle));
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn command(
            &self,
            _control: &objc2_app_kit::NSControl,
            _text: &NSTextView,
            selector: Sel,
        ) -> Bool {
            if selector == sel!(insertNewline:) {
                let shift = NSApplication::sharedApplication(self.mtm())
                    .currentEvent()
                    .is_some_and(|event| {
                        event.modifierFlags().contains(NSEventModifierFlags::Shift)
                    });
                rt::dispatch(move |browser| browser.find_next(!shift));
                return Bool::YES;
            }
            if selector == sel!(cancelOperation:) {
                rt::dispatch(Browser::close_find);
                return Bool::YES;
            }
            Bool::NO
        }
    }

    unsafe impl NSTextFieldDelegate for FindFieldDelegate {}
);

impl FindFieldDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}
