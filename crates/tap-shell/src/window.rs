//! The window: the strip across the top, the page under it, and the field (Search's
//! `App.swift` window content, `Stage.swift` and `Omnibox.swift`).

use std::cell::RefCell;

use block2::RcBlock;
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBezierPath, NSButton, NSColor,
    NSControlTextEditingDelegate, NSFocusRingType, NSFont, NSImage, NSResponder, NSShadow,
    NSTextField, NSTextFieldDelegate, NSTextInputClient, NSTextView, NSView,
    NSViewFrameDidChangeNotification, NSWindow, NSWindowButton, NSWindowDidBecomeKeyNotification,
    NSWindowDidEndLiveResizeNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::WKWebView;
use tap_core::{
    browser::{Browser, BrowserFields, Editing, TabFields, TabId},
    internal::Page,
};

use crate::{
    dialogs, findbar,
    menu::{Command, Commands},
    offers::{self, Offers},
    palette, rt, settings,
    strip::Strip,
    web,
};

/// What the views need from the model, read in one go while it is borrowed.
pub struct View {
    active: Option<TabId>,
    /// One of Tap's own pages, in place of a web page.
    page: Option<Page>,
    failure: Option<String>,
    editing: Option<Editing>,
    can_back: bool,
    can_forward: bool,
    loading: bool,
    title: String,
    find: findbar::State,
}

impl View {
    pub fn of(browser: &Browser) -> View {
        let tab = browser.active_tab();
        View {
            active: browser.active(),
            page: tab.and_then(tap_core::browser::Tab::page),
            failure: tab.and_then(|t| t.failure().map(str::to_owned)),
            editing: browser.editing().cloned(),
            can_back: tab.is_some_and(tap_core::browser::Tab::can_back),
            can_forward: tab.is_some_and(tap_core::browser::Tab::can_forward),
            loading: tab.is_some_and(tap_core::browser::Tab::loading),
            title: tab.map_or_else(|| "Tap".to_owned(), tap_core::browser::Tab::label),
            find: findbar::State::of(browser),
        }
    }
}

struct Views {
    window: Retained<NSWindow>,
    strip: Retained<Strip>,
    stage: Retained<Stage>,
    field: Retained<NSTextField>,
    plate: Retained<Plate>,
    /// The rows under the field, and the grey rest of the address after its text.
    offers: Retained<Offers>,
    ending: Retained<NSTextField>,
    trouble: Retained<NSView>,
    trouble_text: Retained<NSTextField>,
    helm: [Retained<NSButton>; 3],
    shown: RefCell<Option<TabId>>,
    _field_delegate: Retained<FieldDelegate>,
}

thread_local! {
    static VIEWS: RefCell<Option<Views>> = const { RefCell::new(None) };
}

fn with_views<R>(f: impl FnOnce(&Views) -> R) -> Option<R> {
    VIEWS.with_borrow(|views| views.as_ref().map(f))
}

/// Opens the one window.
pub fn open(mtm: MainThreadMarker, commands: &Commands) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 820.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    // SAFETY: a fresh allocation, initialised once, on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is kept in `VIEWS` for the app's life; AppKit must not also
    // release it when it closes.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Tap"));
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    window.setTitlebarAppearsTransparent(true);
    window.setBackgroundColor(Some(&palette::GROUND.dynamic()));
    window.setMinSize(NSSize::new(560.0, 360.0));
    window.center();
    window.setFrameAutosaveName(&NSString::from_str("Tap"));

    let root = Root::new(mtm, frame);
    let width = frame.size.width;
    let strip = Strip::new(
        mtm,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, palette::STRIP)),
    );
    strip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    let stage = Stage::new(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, palette::STRIP),
            NSSize::new(width, frame.size.height - palette::STRIP),
        ),
    );
    stage.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    root.addSubview(&stage);
    root.addSubview(&strip);

    let helm = helm(mtm, commands, width);
    for door in &helm {
        strip.addSubview(door);
    }

    let field_delegate = FieldDelegate::new(mtm);
    let field = field(mtm, &field_delegate);
    let plate = Plate::new(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(palette::FIELD + 40.0, 84.0),
        ),
    );
    plate.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxXMargin,
    );
    plate.setHidden(true);
    root.addSubview(&plate);
    let offers = Offers::new(mtm);
    offers.setHidden(true);
    root.addSubview(&offers);
    let ending = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    ending.setFont(Some(&palette::font()));
    ending.setTextColor(Some(&palette::MUTED.dynamic()));
    ending.setHidden(true);

    let (trouble, trouble_text) = trouble(mtm, commands);
    findbar::install(mtm, &stage, commands);
    stage.addSubview(&trouble);

    window.setContentView(Some(&root));
    keep_lights(&window);

    VIEWS.with_borrow_mut(|views| {
        *views = Some(Views {
            window: window.clone(),
            strip,
            stage,
            field,
            plate,
            offers,
            ending,
            trouble,
            trouble_text,
            helm,
            shown: RefCell::new(None),
            _field_delegate: field_delegate,
        });
    });
    window.makeKeyAndOrderFront(None);
}

/// Back, forward and reload, at the far end of the row.
fn helm(mtm: MainThreadMarker, commands: &Commands, width: f64) -> [Retained<NSButton>; 3] {
    let doors = [
        ("chevron.left", "Back", Command::Back),
        ("chevron.right", "Forward", Command::Forward),
        ("arrow.clockwise", "Reload", Command::ReloadOrStop),
    ];
    doors
        .map(|(symbol, name, command)| {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(name)),
            );
            // SAFETY: the target outlives the button (both live for the app's life), and
            // `run:` takes the sender, as an action must.
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
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            button
        })
        .into_iter()
        .enumerate()
        .map(|(index, button)| {
            #[allow(clippy::cast_precision_loss)] // three doors
            let x = width - palette::HELM + 4.0 + index as f64 * (palette::DOOR + 2.0);
            button.setFrame(NSRect::new(
                NSPoint::new(x, (palette::STRIP - palette::DOOR) / 2.0),
                NSSize::new(palette::DOOR, palette::DOOR),
            ));
            button
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap_or_else(|_| unreachable!("three doors in, three out"))
}

fn field(mtm: MainThreadMarker, delegate: &FieldDelegate) -> Retained<NSTextField> {
    let field = NSTextField::new(mtm);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    let font = NSFont::systemFontOfSize(13.0);
    field.setFont(Some(&font));
    field.setPlaceholderString(Some(&NSString::from_str("Search or enter address")));
    if let Some(cell) = field.cell() {
        cell.setScrollable(true);
    }
    // SAFETY: the delegate is kept in `VIEWS` beside the field, for the app's life.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(delegate))) };
    field
}

/// Said in place of a page that couldn't be shown, with Try Again.
fn trouble(
    mtm: MainThreadMarker,
    commands: &Commands,
) -> (Retained<NSView>, Retained<NSTextField>) {
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 110.0)),
    );
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewMinXMargin
            | NSAutoresizingMaskOptions::ViewMaxXMargin
            | NSAutoresizingMaskOptions::ViewMinYMargin
            | NSAutoresizingMaskOptions::ViewMaxYMargin,
    );
    let text = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    text.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    let font = NSFont::systemFontOfSize(15.0);
    text.setFont(Some(&font));
    text.setFrame(NSRect::new(
        NSPoint::new(0.0, 64.0),
        NSSize::new(420.0, 22.0),
    ));
    // SAFETY: the target outlives the button, and `run:` takes the sender.
    let again = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Try Again"),
            Some(commands),
            Some(sel!(run:)),
            mtm,
        )
    };
    again.setTag(Command::Reload as isize);
    again.setFrame(NSRect::new(
        NSPoint::new(160.0, 16.0),
        NSSize::new(100.0, 32.0),
    ));
    view.addSubview(&text);
    view.addSubview(&again);
    view.setHidden(true);
    (view, text)
}

/// Brings the views up to date with what changed.
pub fn update(dirty: BrowserFields, tabs: &[(TabId, TabFields)], view: &View) {
    let active_dirty = tabs
        .iter()
        .find(|(id, _)| Some(*id) == view.active)
        .map(|(_, f)| *f)
        .unwrap_or_default();
    with_views(|views| {
        if !dirty.is_empty() || !tabs.is_empty() {
            views.strip.setNeedsDisplay(true);
        }
        views.helm[0].setEnabled(view.can_back);
        views.helm[1].setEnabled(view.can_forward);
        let (symbol, name) = if view.loading {
            ("xmark", "Stop")
        } else {
            ("arrow.clockwise", "Reload")
        };
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(name)),
        ) {
            views.helm[2].setImage(Some(&image));
        }
        views.window.setTitle(&NSString::from_str(&view.title));

        if dirty.intersects(BrowserFields::ACTIVE | BrowserFields::TABS)
            || active_dirty.intersects(TabFields::AWAKE | TabFields::FAILURE | TabFields::URL)
            || *views.shown.borrow() != view.active
        {
            show_page(views, view);
        }
        // What the tab asked while it was behind another is put to you now.
        if dirty.contains(BrowserFields::ACTIVE)
            && let Some(active) = view.active
        {
            dialogs::present_held(active);
        }
        if dirty.intersects(BrowserFields::EDITING | BrowserFields::TABS | BrowserFields::ACTIVE) {
            place_field(views, view, dirty.contains(BrowserFields::EDITING));
        }
        if dirty.intersects(BrowserFields::FIND | BrowserFields::FIND_FOCUS) {
            findbar::update(&view.find, dirty.contains(BrowserFields::FIND_FOCUS));
        }
        if dirty.contains(BrowserFields::RETYPED)
            && let Some(editing) = &view.editing
        {
            // Tab took the ending: the field shows it, the caret at its end.
            views
                .field
                .setStringValue(&NSString::from_str(&editing.typed));
            if let Some(editor) = views.field.currentEditor() {
                let end = editing.typed.encode_utf16().count();
                editor.setSelectedRange(objc2_foundation::NSRange::new(end, 0));
            }
        }
        if dirty.intersects(
            BrowserFields::EDITING
                | BrowserFields::OFFERS
                | BrowserFields::TYPED
                | BrowserFields::TABS
                | BrowserFields::ACTIVE,
        ) {
            place_offers(views, view);
        }
    });
}

/// The active tab's page in the stage, or what it says in place of the page.
fn show_page(views: &Views, view: &View) {
    show_own_page(views, view.page);
    let web = view.active.and_then(web::existing);
    let previous = views.shown.replace(view.active);
    if previous != view.active
        && let Some(old) = previous.and_then(web::existing)
    {
        old.removeFromSuperview();
    }
    if let Some(web) = &web {
        // SAFETY: a plain read of a live view's superview.
        if unsafe { web.superview() }.is_none() {
            web.setFrame(views.stage.bounds());
            web.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            views.stage.addSubview_positioned_relativeTo(
                web,
                objc2_app_kit::NSWindowOrderingMode::Below,
                None,
            );
        }
        web.setHidden(view.failure.is_some());
    }
    match &view.failure {
        Some(message) => {
            views
                .trouble_text
                .setStringValue(&NSString::from_str(message));
            let stage = views.stage.bounds();
            let size = views.trouble.frame().size;
            views.trouble.setFrameOrigin(NSPoint::new(
                (stage.size.width - size.width) / 2.0,
                (stage.size.height - size.height) / 2.0,
            ));
            views.trouble.setHidden(false);
        }
        None => views.trouble.setHidden(true),
    }
}

/// One of Tap's own pages in the stage, below the find bar and the field like a web page,
/// or taken away when the tab in front is a web page.
fn show_own_page(views: &Views, page: Option<Page>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    match page {
        Some(Page::Settings) => {
            let own = settings::view(mtm, views.stage.bounds());
            // SAFETY: a plain read of a live view's superview.
            if unsafe { own.superview() }.as_deref() != Some(&**views.stage) {
                own.setFrame(views.stage.bounds());
                views.stage.addSubview_positioned_relativeTo(
                    &own,
                    objc2_app_kit::NSWindowOrderingMode::Below,
                    None,
                );
            }
            own.setHidden(false);
        }
        None => {
            let own = settings::view(mtm, views.stage.bounds());
            own.setHidden(true);
        }
    }
}

/// The field: on the live tab, floating over the page for ⌘T, or put away.
fn place_field(views: &Views, view: &View, opened: bool) {
    let field = &views.field;
    match &view.editing {
        None => {
            views.plate.setHidden(true);
            field.removeFromSuperview();
            views.ending.removeFromSuperview();
        }
        Some(editing) => {
            if editing.launching {
                let root = views
                    .window
                    .contentView()
                    .map(|v| v.bounds())
                    .unwrap_or_default();
                let size = views.plate.frame().size;
                views.plate.setFrameOrigin(NSPoint::new(
                    (root.size.width - size.width) / 2.0,
                    palette::STRIP + 64.0,
                ));
                views.plate.setHidden(false);
                // SAFETY: a plain read of a live view's superview.
                if unsafe { field.superview() }.as_deref() != Some(&**views.plate) {
                    field.removeFromSuperview();
                    views.plate.addSubview(field);
                }
                field.setFrame(NSRect::new(
                    NSPoint::new(36.0, 31.0),
                    NSSize::new(size.width - 72.0, 22.0),
                ));
            } else {
                views.plate.setHidden(true);
                let Some(rect) = views.strip.field_rect() else {
                    return;
                };
                // SAFETY: a plain read of a live view's superview.
                if unsafe { field.superview() }.as_deref() != Some(&**views.strip) {
                    field.removeFromSuperview();
                    views.strip.addSubview(field);
                }
                field.setFrame(rect);
            }
            if opened {
                field.setStringValue(&NSString::from_str(&editing.typed));
                // A text field taking the keyboard selects all it holds. `selectText:`
                // would end that editing session to start another, and the end would
                // read as the field being put away.
                views.window.makeFirstResponder(Some(field));
            }
        }
    }
}

/// The rows under the field, and the grey ending after the text in it. The card lines
/// up with the live tab's pill, or with the floating field's card.
fn place_offers(views: &Views, view: &View) {
    let Some(editing) = view.editing.as_ref().filter(|_| view_has_field(views)) else {
        views.offers.setHidden(true);
        views.ending.setHidden(true);
        return;
    };
    // The card's left edge and width, in the window's content.
    let (x, top, width) = if editing.launching {
        let plate = views.plate.frame();
        (
            plate.origin.x + 20.0,
            plate.origin.y + 20.0 + 44.0 + 4.0,
            plate.size.width - 40.0,
        )
    } else {
        let field = views.field.frame();
        (
            field.origin.x - 10.0,
            palette::STRIP + 4.0,
            field.size.width + 20.0,
        )
    };
    if editing.offers.is_empty() {
        views.offers.setHidden(true);
    } else {
        views.offers.setFrame(NSRect::new(
            NSPoint::new(x - offers::SHADOW, top - offers::SHADOW),
            NSSize::new(
                width + 2.0 * offers::SHADOW,
                offers::height(editing.offers.len()),
            ),
        ));
        views.offers.setHidden(false);
        views.offers.setNeedsDisplay(true);
    }

    // The ending sits right after the typed text, in the field's own font; a label and
    // a borderless field put their text at the same inset. Not while a row is picked,
    // and not once the text runs past the field.
    let field = views.field.frame();
    let typed = palette::text_width(&editing.typed);
    match editing
        .ending
        .as_deref()
        .filter(|_| editing.picked.is_none())
    {
        Some(ending) if typed + palette::text_width(ending) < field.size.width - 4.0 => {
            // SAFETY: a plain read of a live view's superview.
            if let Some(holder) = unsafe { views.field.superview() }
                // SAFETY: as above.
                && unsafe { views.ending.superview() }.as_deref() != Some(&*holder)
            {
                views.ending.removeFromSuperview();
                holder.addSubview(&views.ending);
            }
            views.ending.setStringValue(&NSString::from_str(ending));
            views.ending.setFrame(NSRect::new(
                NSPoint::new(field.origin.x + typed, field.origin.y),
                NSSize::new(field.size.width - typed, field.size.height),
            ));
            views.ending.setHidden(false);
        }
        _ => views.ending.setHidden(true),
    }
}

fn view_has_field(views: &Views) -> bool {
    // SAFETY: a plain read of a live view's superview.
    unsafe { views.field.superview() }.is_some()
}

/// The traffic lights for the bench: each one's left edge and its centre's height from
/// the top, in the window's points (Search's `Bench.lights`).
pub fn lights() -> Vec<[i64; 2]> {
    with_views(|views| {
        let height = views.window.frame().size.height;
        [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .filter_map(|kind| views.window.standardWindowButton(kind))
        .map(|button| {
            let frame = button.convertRect_toView(button.bounds(), None);
            #[allow(clippy::cast_possible_truncation)] // points on a screen
            [
                frame.origin.x.round() as i64,
                (height - (frame.origin.y + frame.size.height / 2.0)).round() as i64,
            ]
        })
        .collect()
    })
    .unwrap_or_default()
}

/// What the field shows, while it is up.
pub fn field_text() -> Option<String> {
    with_views(|views| {
        // SAFETY: a plain read of a live view's superview.
        unsafe { views.field.superview() }.map(|_| views.field.stringValue().to_string())
    })
    .flatten()
}

/// The field's editor, while the field has the keyboard.
pub fn field_editor() -> Option<Retained<objc2_app_kit::NSText>> {
    with_views(|views| views.field.currentEditor()).flatten()
}

/// The text editor that has the keyboard in the window, whichever field it serves.
pub fn key_editor() -> Option<Retained<NSTextView>> {
    with_views(|views| {
        views
            .window
            .firstResponder()
            .and_then(|r| r.downcast::<NSTextView>().ok())
    })
    .flatten()
}

/// Who has the keyboard: the field, the page, or the class of whatever else.
pub fn first_responder() -> String {
    with_views(|views| {
        if views.field.currentEditor().is_some() {
            return "field".to_owned();
        }
        let Some(responder) = views.window.firstResponder() else {
            return "none".to_owned();
        };
        let mut view = responder.downcast::<NSView>().ok();
        while let Some(current) = view {
            if current.downcast_ref::<WKWebView>().is_some() {
                return "page".to_owned();
            }
            // SAFETY: a plain read of a live view's superview.
            view = unsafe { current.superview() };
        }
        "other".to_owned()
    })
    .unwrap_or_default()
}

/// The one window, for a sheet or a picker to hang from.
pub fn main_window() -> Option<Retained<NSWindow>> {
    with_views(|views| views.window.clone())
}

/// The window in front, out of the Dock if it was put there.
pub fn bring_forward() {
    with_views(|views| {
        if views.window.isMiniaturized() {
            views.window.deminiaturize(None);
        }
        views.window.makeKeyAndOrderFront(None);
    });
}

/// The window's number, for events made for it.
pub fn number() -> Option<isize> {
    with_views(|views| views.window.windowNumber())
}

/// Gives the page the keyboard, unless the field has it on purpose.
pub fn focus_page(web: Option<&WKWebView>) {
    with_views(|views| {
        if let Some(web) = web {
            views.window.makeFirstResponder(Some(web));
        }
    });
}

/// The traffic lights where a window with a toolbar has them, without the toolbar: on
/// macOS 26 a toolbar also rounds the window's corners almost twice as much (Search's
/// `Lights.swift`). AppKit lays its title bar out again when it sees fit, so they are
/// put back each time.
fn keep_lights(window: &Retained<NSWindow>) {
    place_lights(window);
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: AppKit's own notification names, read as the constants they are.
    let window_names = unsafe {
        [
            NSWindowDidResizeNotification,
            NSWindowDidEndLiveResizeNotification,
            NSWindowDidBecomeKeyNotification,
            NSWindowDidResignKeyNotification,
            NSWindowDidExitFullScreenNotification,
        ]
    };
    let watch = |name: &NSString, object: &AnyObject| {
        let block = RcBlock::new(|_note: std::ptr::NonNull<NSNotification>| request_lights());
        // SAFETY: the centre keeps the observer for the app's life; the block holds
        // nothing, the window is found again when the placement runs.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(object),
                None,
                &block,
            )
        };
        std::mem::forget(token);
    };
    for name in window_names {
        watch(name, window.as_ref());
    }
    // The title bar's own views moving is the surest sign AppKit has just laid them out
    // again, which it does without telling the window (on first display, among others).
    let Some(close) = window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: plain reads of the title bar's own views.
    let bar = unsafe { close.superview() };
    // SAFETY: as above.
    let container = bar.as_ref().and_then(|bar| unsafe { bar.superview() });
    let mut watched: Vec<Retained<NSView>> = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .filter_map(|kind| {
        window
            .standardWindowButton(kind)
            .map(Retained::into_super)
            .map(Retained::into_super)
    })
    .collect();
    watched.extend(bar);
    watched.extend(container);
    // SAFETY: AppKit's own notification name.
    let moved = unsafe { NSViewFrameDidChangeNotification };
    for view in &watched {
        view.setPostsFrameChangedNotifications(true);
        watch(moved, view.as_ref());
    }
}

thread_local! {
    /// Set while the lights are being moved: moving them posts the very notifications
    /// that ask for them to be moved.
    static PLACING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SPACING: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
    static SCHEDULED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Asks for the lights to be placed once AppKit has finished the layout pass that moved
/// them. Placed from inside that pass, the last button AppKit moved was moved straight
/// back: AppKit lays the three out one after another, posting as it goes.
fn request_lights() {
    if PLACING.get() || SCHEDULED.get() {
        return;
    }
    SCHEDULED.set(true);
    dispatch2::DispatchQueue::main().exec_async(|| {
        SCHEDULED.set(false);
        let window = with_views(|views| views.window.clone());
        if let Some(window) = window {
            place_lights(&window);
        }
    });
}

fn place_lights(window: &NSWindow) {
    if PLACING.get() || window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    PLACING.set(true);
    place_lights_now(window);
    PLACING.set(false);
}

fn place_lights_now(window: &NSWindow) {
    let buttons: Vec<Retained<NSButton>> = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .filter_map(|kind| window.standardWindowButton(kind))
    .collect();
    let [close, mini, _] = buttons.as_slice() else {
        return;
    };
    // SAFETY: plain reads of the title bar's own views.
    let Some(bar) = (unsafe { close.superview() }) else {
        return;
    };
    // SAFETY: as above.
    let Some(container) = (unsafe { bar.superview() }) else {
        return;
    };
    // AppKit's own spacing, read once from its first layout and kept. Read on every
    // pass, it was caught halfway through AppKit putting the buttons back, and the three
    // closed up on each other (Search's Lights.swift, 23 Sep 2026).
    let spacing = SPACING.get().unwrap_or_else(|| {
        let measured = mini.frame().origin.x - close.frame().origin.x;
        let spacing = if (16.0..=32.0).contains(&measured) {
            measured
        } else {
            20.0
        };
        SPACING.set(Some(spacing));
        spacing
    });
    let height = palette::STRIP;
    let window_height = window.frame().size.height;
    let mut frame = container.frame();
    if (frame.size.height - height).abs() > 0.5
        || (frame.origin.y + frame.size.height - window_height).abs() > 0.5
    {
        frame.size.height = height;
        frame.origin.y = window_height - height;
        container.setFrame(frame);
    }
    for (index, button) in buttons.iter().enumerate() {
        let size = button.frame().size;
        #[allow(clippy::cast_precision_loss)] // three buttons
        let origin = NSPoint::new(
            26.0 - size.width / 2.0 + index as f64 * spacing,
            bar.bounds().size.height - 26.0 - size.height / 2.0,
        );
        if button.frame().origin != origin {
            button.setFrameOrigin(origin);
        }
    }
}

define_class!(
    /// The window's content: flipped, so everything is laid out from the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapRoot"]
    struct Root;

    impl Root {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl Root {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    /// Where the page goes: the ground, under whichever tab's web view is in front.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapStage"]
    struct Stage;

    impl Stage {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            palette::GROUND.resolve(palette::is_dark(self)).setFill();
            NSBezierPath::fillRect(self.bounds());
        }
    }
);

impl Stage {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    /// The floating field's card for ⌘T, over the page.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapPlate"]
    struct Plate;

    impl Plate {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            let card = NSRect::new(NSPoint::new(20.0, 20.0), NSSize::new(self.bounds().size.width - 40.0, 44.0));
            let shadow = NSShadow::new();
            shadow.setShadowBlurRadius(16.0);
            shadow.setShadowOffset(NSSize::new(0.0, -4.0));
            shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0.0, if dark { 0.5 } else { 0.18 })));
            shadow.set();
            palette::GROUND.resolve(dark).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(card, 12.0, 12.0).fill();
        }
    }
);

impl Plate {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

define_class!(
    /// The field's delegate: what is typed goes to the model, Return submits, Esc and a
    /// click away put the field away.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapFieldDelegate"]
    struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn changed(&self, note: &NSNotification) {
            let Some(field) = note.object().and_then(|o| o.downcast::<NSTextField>().ok()) else {
                return;
            };
            let typed = field.stringValue().to_string();
            // An input method still composing (pinyin, kana) leaves marked text: nothing
            // is completed under it.
            let composing = field
                .currentEditor()
                .and_then(|editor| editor.downcast::<NSTextView>().ok())
                .is_some_and(|editor| editor.hasMarkedText());
            rt::dispatch(move |browser| browser.set_typed(&typed, composing));
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn command(
            &self,
            _control: &objc2_app_kit::NSControl,
            _text: &NSTextView,
            selector: Sel,
        ) -> Bool {
            if selector == sel!(insertNewline:) {
                rt::dispatch(Browser::submit);
                return Bool::YES;
            }
            if selector == sel!(cancelOperation:) {
                rt::dispatch(Browser::cancel_editing);
                return Bool::YES;
            }
            if selector == sel!(moveDown:) || selector == sel!(moveUp:) {
                let down = selector == sel!(moveDown:);
                rt::dispatch(move |browser| browser.pick(down));
                return Bool::YES;
            }
            if selector == sel!(insertTab:) {
                // Tab takes the grey ending; with none, Tab does what Tab does.
                let took = rt::update(Browser::accept_ending).unwrap_or(false);
                rt::settle();
                return Bool::new(took);
            }
            Bool::NO
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn ended(&self, _note: &NSNotification) {
            // A click away puts the field away. AppKit also ends an editing session
            // in passing (moving the field, starting another), so it is put away only
            // if, once AppKit is done, the field really has lost the keyboard.
            dispatch2::DispatchQueue::main().exec_async(|| {
                let editing =
                    with_views(|views| views.field.currentEditor().is_some()).unwrap_or(false);
                if !editing {
                    rt::dispatch(Browser::cancel_editing);
                }
            });
        }
    }

    unsafe impl NSTextFieldDelegate for FieldDelegate {}
);

impl FieldDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}
