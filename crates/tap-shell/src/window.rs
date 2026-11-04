//! The window: the strip across the top, the page under it, and the field (Search's
//! `App.swift` window content, `Stage.swift` and `Omnibox.swift`).
//!
//! Not yet: `links.peek` and `swipe.history`. Tap's pages are `WKWebView`s owned by
//! `web.rs`, whose public surface makes one view per tab in the row and steps back or
//! forward one page (`Effect::Back`/`Forward`); there is no off-row view to put in a
//! peek panel, no way to stop WebKit's own swipe (`register` turns its back/forward
//! gestures on), and no back-forward list to pick from. Both wait on hooks there.

use std::cell::RefCell;

use block2::{DynBlock, RcBlock};
use objc2::{
    AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBezierPath, NSButton, NSColor,
    NSControlTextEditingDelegate, NSEvent, NSFocusRingType, NSFont, NSImage, NSResponder, NSShadow,
    NSTextField, NSTextFieldDelegate, NSTextInputClient, NSTextView, NSView,
    NSViewFrameDidChangeNotification, NSWindow, NSWindowButton, NSWindowDidBecomeKeyNotification,
    NSWindowDidEndLiveResizeNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSData, NSHTTPURLResponse, NSNotification, NSNotificationCenter, NSOperationQueue, NSPoint,
    NSRect, NSSize, NSString, NSURL, NSURLRequest, NSURLRequestCachePolicy, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSURLSessionDataTask, NSURLSessionDelegate,
    NSURLSessionTask, NSURLSessionTaskDelegate,
};
use objc2_web_kit::WKWebView;
use serde_json::Value;
use tap_core::{
    address,
    browser::{Browser, BrowserFields, Editing, TabFields, TabId},
    engine::{self, Engine},
    identity,
    internal::Page,
    keyword,
    settings_index::{flag, text},
};

use crate::{
    address_bar::{self, AddressBar},
    dialogs, downloads, findbar,
    menu::{Command, Commands},
    offers::{self, Offers},
    palette,
    prefs::Kept,
    rt, settings, signin,
    strip::Strip,
    web,
};

/// What the views need from the model, read in one go while it is borrowed.
#[allow(clippy::struct_excessive_bools)] // one flag per thing the views ask, as read
pub struct View {
    active: Option<TabId>,
    /// One of Tap's own pages, in place of a web page.
    page: Option<Page>,
    failure: Option<String>,
    editing: Option<Editing>,
    can_back: bool,
    can_forward: bool,
    loading: bool,
    blank: bool,
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
            blank: tab.is_some_and(tap_core::browser::Tab::is_blank),
            title: tab.map_or_else(|| "Tap".to_owned(), tap_core::browser::Tab::label),
            find: findbar::State::of(browser),
        }
    }
}

struct Views {
    window: Retained<NSWindow>,
    strip: Retained<Strip>,
    /// The address bar under the strip, while Settings has it up.
    bar: Retained<AddressBar>,
    stage: Retained<Stage>,
    field: Retained<NSTextField>,
    plate: Retained<Plate>,
    /// The rows under the field, and the grey rest of the address after its text.
    offers: Retained<Offers>,
    /// The search engine's rows, in their own card under the field's rows: the model
    /// owns the field's rows and has no room in them for words that arrive later.
    suggest: Retained<Suggest>,
    /// The downloads door, at the strip's far end beside the helm.
    fetch_door: Retained<NSButton>,
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
#[allow(clippy::too_many_lines)] // every view of the window, made and placed in turn
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
        commands,
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
    let bar = AddressBar::new(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, palette::STRIP),
            NSSize::new(width, palette::BAR),
        ),
    );
    bar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    root.addSubview(&bar);
    root.addSubview(&strip);

    let helm = helm(mtm, commands, width);
    for door in &helm {
        strip.addSubview(door);
    }
    let fetch_door = fetch_door(mtm, width);
    strip.addSubview(&fetch_door);

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
    let suggest = Suggest::new(mtm);
    suggest.setHidden(true);
    root.addSubview(&suggest);
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
            bar,
            stage,
            field,
            plate,
            offers,
            suggest,
            fetch_door,
            ending,
            trouble,
            trouble_text,
            helm,
            shown: RefCell::new(None),
            _field_delegate: field_delegate,
        });
    });
    with_views(arrange);
    window.makeKeyAndOrderFront(None);
}

/// Settings moved: the address bar comes up or goes, with all that follows from it.
pub fn bar_changed() {
    with_views(arrange);
}

/// The address bar up or down (Settings › Show the address bar), and what follows from
/// it: back, forward and reload on the bar's left or at the strip's far end, the
/// downloads door at the strip's end, the page under whichever is lower, and the field
/// in the bar or on the live tab.
fn arrange(views: &Views) {
    let up = address_bar::shown();
    let root = views
        .window
        .contentView()
        .map(|v| v.bounds())
        .unwrap_or_default();
    let width = root.size.width;
    views.bar.setHidden(!up);
    views.bar.setFrame(NSRect::new(
        NSPoint::new(0.0, palette::STRIP),
        NSSize::new(width, palette::BAR),
    ));
    let top = palette::STRIP + if up { palette::BAR } else { 0.0 };
    views.stage.setFrame(NSRect::new(
        NSPoint::new(0.0, top),
        NSSize::new(width, (root.size.height - top).max(0.0)),
    ));
    for (index, door) in views.helm.iter().enumerate() {
        let (holder, frame, mask): (&NSView, NSRect, NSAutoresizingMaskOptions) = if up {
            (
                &views.bar,
                address_bar::door_rect(index),
                NSAutoresizingMaskOptions::ViewMaxXMargin,
            )
        } else {
            (
                &views.strip,
                helm_rect(width, index),
                NSAutoresizingMaskOptions::ViewMinXMargin,
            )
        };
        // SAFETY: a plain read of a live view's superview.
        if unsafe { door.superview() }.as_deref() != Some(holder) {
            door.removeFromSuperview();
            holder.addSubview(door);
        }
        door.setFrame(frame);
        door.setAutoresizingMask(mask);
    }
    let fetch = views.fetch_door.frame();
    let x = if up {
        width - palette::DOOR - 8.0
    } else {
        width - palette::HELM - palette::DOOR - 4.0
    };
    views
        .fetch_door
        .setFrameOrigin(NSPoint::new(x, fetch.origin.y));
    views.window.invalidateCursorRectsForView(&views.bar);
    views.strip.setNeedsDisplay(true);
    views.bar.setNeedsDisplay(true);
    if let Some(view) = rt::read(View::of) {
        place_field(views, &view, false);
        place_offers(views, &view);
    }
}

/// A helm door's place at the strip's far end.
fn helm_rect(width: f64, index: usize) -> NSRect {
    #[allow(clippy::cast_precision_loss)] // three doors
    let x = width - palette::HELM + 4.0 + index as f64 * (palette::DOOR + 2.0);
    NSRect::new(
        NSPoint::new(x, (palette::STRIP - palette::DOOR) / 2.0),
        NSSize::new(palette::DOOR, palette::DOOR),
    )
}

/// The downloads door, just before the helm at the strip's far end; the window
/// shows it as `downloads::shows_button` says.
fn fetch_door(mtm: MainThreadMarker, width: f64) -> Retained<NSButton> {
    let door = downloads::button(mtm);
    door.setFrame(NSRect::new(
        NSPoint::new(
            width - palette::HELM - palette::DOOR - 4.0,
            (palette::STRIP - palette::DOOR) / 2.0,
        ),
        NSSize::new(palette::DOOR, palette::DOOR),
    ));
    door.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
    door.setHidden(true);
    door
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
            button.setFrame(helm_rect(width, index));
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
    // ⌘T asked for the floating field whatever the setting says (the menu and the
    // strip's plus both call `begin_editing(true)`): with `newtab.floating` off, or on
    // a tab that is already blank, Search's `launch` is an ordinary new tab instead,
    // so the launching field is turned into one here, where the setting is read.
    if dirty.contains(BrowserFields::EDITING)
        && let Some(editing) = &view.editing
        && editing.launching
        && launch_action(flag(&Kept::new(), "newtab.floating"), true, view.blank) != Launch::Keep
    {
        queue_launch_conversion();
    }
    with_views(|views| {
        refresh_fetch_door(views);
        if !dirty.is_empty() || !tabs.is_empty() {
            views.strip.setNeedsDisplay(true);
            views.bar.setNeedsDisplay(true);
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
        // The accounts under a sign-in box belong to the tab they were hung from.
        if dirty.intersects(BrowserFields::ACTIVE | BrowserFields::TABS) {
            signin::tab_changed();
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
            } else if !views.bar.isHidden() {
                // The address bar holds the field while it is up.
                views.plate.setHidden(true);
                // SAFETY: a plain read of a live view's superview.
                if unsafe { field.superview() }.as_deref() != Some(&**views.bar) {
                    field.removeFromSuperview();
                    views.bar.addSubview(field);
                }
                field.setFrame(views.bar.field_rect());
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

/// The card's left edge, its top and its width, in the window's content: lined up
/// with the live tab's pill, or with the floating field's card.
fn offers_geometry(views: &Views, editing: &Editing) -> (f64, f64, f64) {
    if editing.launching {
        let plate = views.plate.frame();
        (
            plate.origin.x + 20.0,
            plate.origin.y + 20.0 + 44.0 + 4.0,
            plate.size.width - 40.0,
        )
    } else if !views.bar.isHidden() {
        // Hung from the address bar, as wide as the field in it.
        let field = views.field.frame();
        (
            field.origin.x - 28.0,
            palette::STRIP + palette::BAR + 2.0,
            field.size.width + 36.0,
        )
    } else {
        let field = views.field.frame();
        (
            field.origin.x - 10.0,
            palette::STRIP + 4.0,
            field.size.width + 20.0,
        )
    }
}

/// The rows under the field, and the grey ending after the text in it. The card lines
/// up with the live tab's pill, or with the floating field's card.
fn place_offers(views: &Views, view: &View) {
    let Some(editing) = view.editing.as_ref().filter(|_| view_has_field(views)) else {
        views.offers.setHidden(true);
        views.suggest.setHidden(true);
        views.ending.setHidden(true);
        return;
    };
    let (x, top, width) = offers_geometry(views, editing);
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
    place_suggest(views, view, x, top, width);

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

/// What a launching (⌘T) field becomes, once `newtab.floating` has its say
/// (Search's `Browser.launch`): kept floating only over a page that is showing
/// something; anywhere else ⌘T is an ordinary new tab, made at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    Keep,
    /// The tab in front is already blank: the field opens on it, in the strip.
    ReuseBlank,
    /// A new blank tab is made now, and the field opens on it, in the strip.
    OpenBlank,
}

fn launch_action(floating: bool, launching: bool, blank: bool) -> Launch {
    if !launching || (floating && !blank) {
        Launch::Keep
    } else if blank {
        Launch::ReuseBlank
    } else {
        Launch::OpenBlank
    }
}

/// Turns a launching field into the ordinary new tab `launch_action` names. Queued
/// through `rt`, as `update` runs inside a flush: the change lands right after it.
fn queue_launch_conversion() {
    rt::dispatch(|browser| {
        let Some(editing) = browser.editing().cloned() else {
            return;
        };
        let blank = browser
            .active_tab()
            .is_some_and(tap_core::browser::Tab::is_blank);
        let typed = editing.typed.clone();
        match launch_action(
            flag(&Kept::new(), "newtab.floating"),
            editing.launching,
            blank,
        ) {
            Launch::Keep => {}
            Launch::ReuseBlank => {
                browser.cancel_editing();
                browser.begin_editing(false);
            }
            Launch::OpenBlank => {
                browser.open(None, true, None);
                browser.begin_editing(false);
            }
        }
        if !typed.is_empty() {
            browser.set_typed(&typed, false);
        }
    });
}

/// The downloads door, shown and named for what is coming in (Search's `FetchDoor.shows`:
/// always with the setting on, otherwise only while a file comes in).
fn refresh_fetch_door(views: &Views) {
    let going = downloads::going();
    let always = flag(&Kept::new(), "downloads.button");
    views
        .fetch_door
        .setHidden(!downloads::shows_button(always, going));
    let tip = if going > 0 {
        format!("Downloading {going} \u{2014} open the folder downloads are saved in")
    } else {
        "Downloads \u{2014} open the folder downloads are saved in".to_owned()
    };
    views.fetch_door.setToolTip(Some(&NSString::from_str(&tip)));
}

/// A download started or ended: the door follows, without waiting for the model.
pub fn downloads_moved() {
    with_views(refresh_fetch_door);
}

// MARK: the engine's suggestions (`search.suggest`, Search's `EngineSuggest.swift`)

/// How long typing has to pause before a question goes out, and how long an answer
/// is worth waiting for.
const SUGGEST_PAUSE_NS: i64 = 120_000_000;
const SUGGEST_PATIENCE: f64 = 0.8;
/// At most this many rows from the engine; longer than this is a paste, not a search.
const SUGGEST_MOST: usize = 6;
const SUGGEST_LONGEST: usize = 80;

struct SuggestState {
    /// What the words on show answered, trimmed.
    typed: String,
    words: Vec<String>,
    /// Which question is the latest, so a late answer lands nowhere.
    round: u64,
    task: Option<Retained<NSURLSessionDataTask>>,
    session: Option<Retained<NSURLSession>>,
    delegate: Option<Retained<SuggestDelegate>>,
}

thread_local! {
    static SUGGEST: RefCell<SuggestState> = const {
        RefCell::new(SuggestState {
            typed: String::new(),
            words: Vec::new(),
            round: 0,
            task: None,
            session: None,
            delegate: None,
        })
    };
}

/// Where each engine answers, `OpenSearch`'s JSON `["words", ["…"]]`, as Search
/// checked them; a custom engine has none we could know, and so shows nothing.
fn suggest_endpoint(engine: Engine) -> Option<&'static str> {
    Some(match engine {
        Engine::Google => "https://suggestqueries.google.com/complete/search?client=firefox&q=%s",
        Engine::DuckDuckGo => "https://duckduckgo.com/ac/?q=%s&type=list",
        Engine::Bing => "https://www.bing.com/osjson.aspx?query=%s",
        Engine::Ecosia => "https://ac.ecosia.org/autocomplete?q=%s&type=list",
        Engine::Startpage => "https://www.startpage.com/osuggestions?q=%s",
        Engine::Kagi => "https://kagi.com/api/autosuggest?q=%s",
        Engine::Brave => "https://search.brave.com/api/suggest?q=%s",
        Engine::Qwant => "https://api.qwant.com/api/suggest/?q=%s&client=opensearch",
        Engine::Custom => return None,
    })
}

/// The question for these words, or `None` when there is nowhere to ask. Only ever
/// https — except, in a test run, a fake engine on this Mac (`suggest.testURL`).
fn suggest_url(words: &str, engine: Engine, test: Option<&str>) -> Option<String> {
    let template = test.or_else(|| suggest_endpoint(engine))?;
    let url = engine::url_for(words, template)?;
    let scheme = url.scheme()?.to_lowercase();
    if scheme == "https" {
        return Some(url.as_str().to_owned());
    }
    let host = url.host().unwrap_or_default();
    if test.is_some() && scheme == "http" && matches!(host.as_str(), "127.0.0.1" | "localhost") {
        return Some(url.as_str().to_owned());
    }
    None
}

/// A long run with no space that mixes letters and digits, or reads as a key's
/// encoding: a password, a token, a card or account number (Search's `secretLike`).
fn suggest_secret_like(token: &str) -> bool {
    let count = token.chars().count();
    let letters = token.chars().any(char::is_alphabetic);
    let digits = token.chars().any(char::is_numeric);
    if count >= 12 && letters && digits {
        return true;
    }
    if count >= 9 && digits && token.chars().all(|c| c.is_numeric() || c == '-') {
        return true;
    }
    count >= 24
        && token
            .chars()
            .all(|c| c.is_ascii() && (c.is_alphanumeric() || "+/=_-".contains(c)))
}

/// Whether these words may leave the Mac at all (Search's `EngineSuggest.sendable`).
/// Only plain words do: nothing the field would go to as an address, no scheme, no
/// path, no email, nothing long, nothing that reads like a password or a key. Tap has
/// no private tabs yet, so Search's private-tab refusal has nothing to refuse here;
/// the moment it grows them, this is where they are checked.
fn suggest_sendable(typed: &str) -> bool {
    let text = typed.trim();
    if text.is_empty() || text.chars().count() > SUGGEST_LONGEST || text.contains('\n') {
        return false;
    }
    if address::url(text).is_some() {
        return false;
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let first = tokens.first().copied().unwrap_or(text);
    if let Some(colon) = first.find(':') {
        let head = &first[..colon];
        if !head.is_empty()
            && head.chars().next().is_some_and(char::is_alphabetic)
            && !first[colon + 1..].is_empty()
            && head
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '+' | '.' | '-'))
        {
            return false;
        }
    }
    if first.starts_with('/')
        || first.starts_with('~')
        || first.starts_with("./")
        || first.starts_with("../")
        || text.contains('\\')
    {
        return false;
    }
    for token in &tokens {
        if tokens.len() == 1 && (token.contains('.') || token.contains('/')) {
            return false;
        }
        if token.contains('@') && token.contains('.') {
            return false;
        }
        if suggest_secret_like(token) {
            return false;
        }
    }
    true
}

/// The words in an `OpenSearch` answer: trimmed, each once, none the same as what was
/// typed, at most `SUGGEST_MOST` (Search's `EngineSuggest.parse`).
fn suggest_parse(data: &[u8], asked: &str) -> Vec<String> {
    let Ok(Value::Array(top)) = serde_json::from_slice::<Value>(data) else {
        return Vec::new();
    };
    let Some(Value::Array(words)) = top.get(1) else {
        return Vec::new();
    };
    let typed = asked.trim().to_lowercase();
    let mut seen = std::collections::HashSet::from([typed]);
    let mut out = Vec::new();
    for word in words {
        let Some(word) = word.as_str() else { continue };
        let clean = word.trim();
        if clean.is_empty()
            || clean.chars().count() > 200
            || clean.contains('\n')
            || !seen.insert(clean.to_lowercase())
        {
            continue;
        }
        out.push(clean.to_owned());
        if out.len() == SUGGEST_MOST {
            break;
        }
    }
    out
}

/// Last question's words that still fit what is typed now, while the new answer is
/// on its way (Search's `askEngine`): rows that came and went with every key would
/// be a list nobody could read.
fn suggest_still(previous: &[String], typed: &str, offered: &[String]) -> Vec<String> {
    let lower = typed.trim().to_lowercase();
    previous
        .iter()
        .filter(|word| {
            let word = word.to_lowercase();
            word.starts_with(&lower) && word != lower && !offered.iter().any(|o| o == &word)
        })
        .take(SUGGEST_MOST)
        .cloned()
        .collect()
}

/// The fake engine a test run asks instead, if it set one; never read outside one.
fn suggest_test_endpoint() -> Option<String> {
    std::env::var_os(identity::PROBE_ENV)?;
    let raw = text(&Kept::new(), "suggest.testURL");
    (!raw.trim().is_empty()).then_some(raw)
}

/// The words on show for this editing: the state's, if they answer what is typed,
/// and none the field's own rows already say.
fn suggest_words(editing: &Editing) -> Vec<String> {
    SUGGEST.with_borrow(|state| {
        if state.typed != editing.typed.trim() {
            return Vec::new();
        }
        let offered: Vec<String> = editing
            .offers
            .iter()
            .map(|o| o.key.to_lowercase())
            .collect();
        state
            .words
            .iter()
            .filter(|word| !offered.contains(&word.to_lowercase()))
            .take(SUGGEST_MOST)
            .cloned()
            .collect()
    })
}

fn place_suggest(views: &Views, view: &View, x: f64, top: f64, width: f64) {
    let Some(editing) = view.editing.as_ref() else {
        views.suggest.setHidden(true);
        return;
    };
    let words = suggest_words(editing);
    if words.is_empty() {
        views.suggest.setHidden(true);
        return;
    }
    // Its card starts where the field's own card ends; with no rows of its own, it
    // takes their place under the field.
    let y = if editing.offers.is_empty() {
        top - offers::SHADOW
    } else {
        top - offers::SHADOW + offers::height(editing.offers.len()) - 2.0 * offers::SHADOW
    };
    views.suggest.setFrame(NSRect::new(
        NSPoint::new(x - offers::SHADOW, y),
        NSSize::new(width + 2.0 * offers::SHADOW, offers::height(words.len())),
    ));
    views.suggest.setHidden(false);
    views.suggest.setNeedsDisplay(true);
}

/// The card caught up after an answer landed, outside a model change.
fn refresh_suggest() {
    with_views(|views| {
        let Some(view) = rt::read(View::of) else {
            views.suggest.setHidden(true);
            return;
        };
        let Some(editing) = view.editing.as_ref().filter(|_| view_has_field(views)) else {
            views.suggest.setHidden(true);
            return;
        };
        let (x, top, width) = offers_geometry(views, editing);
        place_suggest(views, &view, x, top, width);
    });
}

fn suggest_cancel_task() {
    SUGGEST.with_borrow_mut(|state| {
        state.round += 1;
        if let Some(task) = state.task.take() {
            task.cancel();
        }
    });
}

/// Nothing is to be asked, or shown, any more: the switch off, the field gone, or
/// words that stay on this Mac.
fn suggest_clear() {
    suggest_cancel_task();
    SUGGEST.with_borrow_mut(|state| {
        state.typed.clear();
        state.words.clear();
    });
    refresh_suggest();
}

/// What is typed may be asked about, and the asking itself, a pause after the last
/// key (Search's `Browser.askEngine` and `EngineSuggester`).
fn ask_engine_suggest(typed: &str, composing: bool) {
    if composing {
        return;
    }
    let kept = Kept::new();
    let engine = Engine::from_raw(&text(&kept, "search.engine")).unwrap_or_default();
    let test = suggest_test_endpoint();
    let allowed = flag(&kept, "search.suggest")
        && suggest_sendable(typed)
        && rt::read(|browser| {
            browser.editing().is_some() && keyword::find(typed, &browser.keywords).is_none()
        })
        .unwrap_or(false);
    let Some(url) = allowed
        .then(|| suggest_url(typed.trim(), engine, test.as_deref()))
        .flatten()
    else {
        suggest_clear();
        return;
    };
    let words = typed.trim().to_owned();
    let round = SUGGEST.with_borrow_mut(|state| {
        state.round += 1;
        if let Some(task) = state.task.take() {
            task.cancel();
        }
        let offered = rt::read(|browser| {
            browser
                .editing()
                .map(|e| {
                    e.offers
                        .iter()
                        .map(|o| o.key.to_lowercase())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
        state.words = suggest_still(&state.words, &words, &offered);
        state.typed.clone_from(&words);
        state.round
    });
    refresh_suggest();
    let later = dispatch2::DispatchTime::NOW.time(SUGGEST_PAUSE_NS);
    let _ = dispatch2::DispatchQueue::main().after(later, move || {
        let current = SUGGEST.with_borrow(|state| state.round) == round
            && rt::read(|browser| browser.editing().is_some_and(|e| e.typed.trim() == words))
                .unwrap_or(false);
        if current {
            start_suggest_fetch(&url, &words, round);
        }
    });
}

/// One question, on a session that keeps nothing: no cookies either way, no
/// credentials, no cache, and no following an engine somewhere else (Search's
/// `EngineSuggest.session` and `NoRedirects`).
fn start_suggest_fetch(url: &str, asked: &str, round: u64) {
    // The session and its state live on the main thread's `SUGGEST`.
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Some(target) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    let session = SUGGEST.with_borrow_mut(|state| {
        if state.session.is_none() {
            let delegate = SuggestDelegate::new();
            let configuration = NSURLSessionConfiguration::ephemeralSessionConfiguration();
            // SAFETY: AppKit's own cookie policy constant, read as the value it is.
            configuration
                .setHTTPCookieAcceptPolicy(objc2_foundation::NSHTTPCookieAcceptPolicy::Never);
            configuration.setURLCache(None);
            configuration
                .setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
            configuration.setTimeoutIntervalForRequest(SUGGEST_PATIENCE);
            configuration.setTimeoutIntervalForResource(SUGGEST_PATIENCE);
            // SAFETY: the delegate is kept in `SUGGEST` for the app's life, and every
            // answer is delivered on the main queue this session is given.
            let session = unsafe {
                NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                    &configuration,
                    Some(ProtocolObject::from_ref(&*delegate)),
                    Some(&NSOperationQueue::mainQueue()),
                )
            };
            state.delegate = Some(delegate);
            state.session = Some(session);
        }
        state.session.clone()
    });
    let Some(session) = session else { return };
    let request = NSURLRequest::requestWithURL_cachePolicy_timeoutInterval(
        &target,
        NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
        SUGGEST_PATIENCE,
    );
    let asked = asked.to_owned();
    let answer = RcBlock::new(
        move |data: *mut NSData,
              response: *mut NSURLResponse,
              _error: *mut objc2_foundation::NSError| {
            // SAFETY: Foundation hands a valid object or null, for the call's length;
            // the bytes are copied before anything else is done with them.
            let bytes = unsafe { data.as_ref() }
                .map(NSData::to_vec)
                .unwrap_or_default();
            // SAFETY: as above, a plain read of the response's status.
            let status = unsafe { response.as_ref() }
                .and_then(|r| r.downcast_ref::<NSHTTPURLResponse>())
                .map(NSHTTPURLResponse::statusCode);
            if status != Some(200) {
                return;
            }
            let words = suggest_parse(&bytes, &asked);
            // Typed on since, or the field gone: nothing to answer.
            let current = SUGGEST.with_borrow(|state| state.round) == round
                && rt::read(|browser| browser.editing().is_some_and(|e| e.typed.trim() == asked))
                    .unwrap_or(false);
            if !current {
                return;
            }
            SUGGEST.with_borrow_mut(|state| {
                state.task = None;
                state.words = words;
            });
            refresh_suggest();
        },
    );
    // SAFETY: a live session and request, and a block kept by the session until the
    // task ends; the task is kept in `SUGGEST` until it ends or is cancelled.
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &answer) };
    SUGGEST.with_borrow_mut(|state| state.task = Some(task.clone()));
    task.resume();
}

/// A row of the engine's card was clicked: it is searched for, as Return on that row
/// in Search. Not through `submit`: the grey ending the field holds for a visited
/// place would take the words to that place instead of searching them.
fn pick_suggest_word(word: &str) {
    suggest_cancel_task();
    SUGGEST.with_borrow_mut(|state| state.words.clear());
    refresh_suggest();
    let word = word.to_owned();
    rt::dispatch(move |browser| {
        let Some(editing) = browser.editing().cloned() else {
            return;
        };
        let Some(url) = browser.destination(&word) else {
            return;
        };
        let blank = browser
            .active_tab()
            .is_some_and(tap_core::browser::Tab::is_blank);
        browser.cancel_editing();
        if editing.launching && !blank {
            browser.open(Some(&url), true, None);
        } else if let Some(id) = browser.active() {
            browser.go(id, &url);
        }
    });
}

define_class!(
    /// The search engine's rows, in a card of their own under the field's rows. It
    /// never takes the keyboard: a click on a row is a choice, not a click away.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSuggest"]
    struct Suggest;

    impl Suggest {
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
            if let Some(row) = self.row_at(event)
                && let Some(word) = rt::read(|browser| {
                    browser.editing().and_then(|e| suggest_words(e).get(row).cloned())
                })
                .flatten()
            {
                pick_suggest_word(&word);
            }
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

impl Suggest {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn card(&self) -> NSRect {
        let bounds = self.bounds();
        NSRect::new(
            NSPoint::new(offers::SHADOW, offers::SHADOW),
            NSSize::new(
                bounds.size.width - 2.0 * offers::SHADOW,
                bounds.size.height - 2.0 * offers::SHADOW,
            ),
        )
    }

    fn row_rect(&self, row: usize) -> NSRect {
        let card = self.card();
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let top = card.origin.y + 6.0 + row as f64 * offers::ROW;
        NSRect::new(
            NSPoint::new(card.origin.x + 6.0, top),
            NSSize::new(card.size.width - 12.0, offers::ROW),
        )
    }

    fn row_at(&self, event: &NSEvent) -> Option<usize> {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let count = rt::read(|browser| browser.editing().map_or(0, |e| suggest_words(e).len()))
            .unwrap_or(0);
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
        let Some(words) = rt::read(|browser| browser.editing().map(suggest_words)).flatten() else {
            return;
        };
        if words.is_empty() {
            return;
        }
        let shadow = NSShadow::new();
        shadow.setShadowBlurRadius(14.0);
        shadow.setShadowOffset(NSSize::new(0.0, -3.0));
        shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(
            0.0,
            if dark { 0.5 } else { 0.16 },
        )));
        shadow.set();
        palette::GROUND.resolve(dark).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.card(), 12.0, 12.0).fill();
        NSShadow::new().set();
        let engine = Engine::from_raw(&text(&Kept::new(), "search.engine")).unwrap_or_default();
        let detail = format!(
            "Search {}",
            engine.name(&text(&Kept::new(), "search.custom"))
        );
        for (row, word) in words.iter().enumerate() {
            let rect = self.row_rect(row);
            let text_top = rect.origin.y + (offers::ROW - 17.0) / 2.0;
            let left = rect.origin.x + 12.0;
            let width = rect.size.width - 24.0;
            let main_width = palette::text_width(word).min(width * 0.62);
            palette::draw_text(
                word,
                NSRect::new(
                    NSPoint::new(left, text_top),
                    NSSize::new(main_width + 1.0, 17.0),
                ),
                &palette::INK.resolve(dark),
            );
            let x = left + main_width + 10.0;
            palette::draw_text(
                &format!("\u{2014} {detail}"),
                NSRect::new(
                    NSPoint::new(x, text_top),
                    NSSize::new((left + width - x).max(0.0), 17.0),
                ),
                &palette::MUTED.resolve(dark),
            );
        }
    }
}

define_class!(
    /// The suggestions session's delegate: an engine that answers by sending us
    /// somewhere else isn't followed there — the words would go to a host
    /// nobody picked (Search's `NoRedirects`).
    // No thread kind: a session delegate is handed to Foundation, which asks that
    // it be sendable; its one answer is always given on the main queue below.
    #[unsafe(super(NSObject))]
    #[thread_kind = AllocAnyThread]
    #[name = "TapSuggestDelegate"]
    struct SuggestDelegate;

    unsafe impl NSObjectProtocol for SuggestDelegate {}

    unsafe impl NSURLSessionDelegate for SuggestDelegate {}

    unsafe impl NSURLSessionTaskDelegate for SuggestDelegate {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        #[allow(non_snake_case)] // Foundation's own name for it
        fn URLSession_task_willPerformHTTPRedirection_newRequest_completionHandler(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            _response: &NSHTTPURLResponse,
            _request: &NSURLRequest,
            handler: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            handler.call((std::ptr::null_mut(),));
        }
    }
);

impl SuggestDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
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
/// The stage the page is on, for what hangs over a page: the accounts under a sign-in
/// box, the offer to keep a password.
pub fn stage() -> Option<Retained<NSView>> {
    with_views(|views| Retained::into_super(views.stage.clone()))
}

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
            let asked = typed.clone();
            rt::dispatch(move |browser| browser.set_typed(&typed, composing));
            ask_engine_suggest(&asked, composing);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_only_floats_over_a_page_with_something_on_it() {
        assert_eq!(launch_action(true, true, false), Launch::Keep);
        assert_eq!(launch_action(true, true, true), Launch::ReuseBlank);
        assert_eq!(launch_action(false, true, false), Launch::OpenBlank);
        assert_eq!(launch_action(false, true, true), Launch::ReuseBlank);
        assert_eq!(launch_action(false, false, false), Launch::Keep);
    }

    #[test]
    fn only_plain_words_are_sent_for_suggestions() {
        for sent in ["coffee", "coffee near me", "how do i boil eggs", "café"] {
            assert!(suggest_sendable(sent), "{sent}");
        }
        for kept in [
            "",
            "example.com",
            "https://example.com/x",
            "localhost:3000",
            "person@example.com",
            "mailto:person@example.com",
            "/Users/me/report.pdf",
            "~/notes.txt",
            "./report.pdf",
            "hunter2hunter2",
            "4111111111111111",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
        ] {
            assert!(!suggest_sendable(kept), "{kept}");
        }
        assert!(!suggest_sendable(&"word ".repeat(30)));
    }

    #[test]
    fn suggestion_answers_are_cleaned_and_capped() {
        let data =
            br#"["coff",[" Coffee ","coffee","coffee near me","coffee","a","b","c","d","e"]]"#;
        assert_eq!(
            suggest_parse(data, "coff"),
            ["Coffee", "coffee near me", "a", "b", "c", "d"]
        );
        assert_eq!(suggest_parse(b"not json", "coff"), Vec::<String>::new());
        assert_eq!(suggest_parse(br#"["coff"]"#, "coff"), Vec::<String>::new());
        assert_eq!(
            suggest_parse(br#"["coff",[1,2]]"#, "coff"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn suggestion_questions_go_to_the_engine_chosen_and_nowhere_else() {
        assert_eq!(
            suggest_url("cats & dogs", Engine::Google, None).as_deref(),
            Some(
                "https://suggestqueries.google.com/complete/search?client=firefox&q=cats%20%26%20dogs"
            )
        );
        assert_eq!(
            suggest_url("cats", Engine::DuckDuckGo, None).as_deref(),
            Some("https://duckduckgo.com/ac/?q=cats&type=list")
        );
        assert_eq!(suggest_url("cats", Engine::Custom, None), None);
        assert_eq!(
            suggest_url("cats", Engine::Custom, Some("http://127.0.0.1:9/s?q=%s")).as_deref(),
            Some("http://127.0.0.1:9/s?q=cats"),
            "a test run's fake engine on this Mac"
        );
        assert_eq!(
            suggest_url("cats", Engine::Custom, Some("http://example.com/s?q=%s")),
            None,
            "plain http is never asked"
        );
    }

    #[test]
    fn words_that_still_fit_stay_while_the_next_answer_comes() {
        let previous = vec![
            "coffee".to_owned(),
            "coffee near me".to_owned(),
            "tea".to_owned(),
        ];
        assert_eq!(
            suggest_still(&previous, "coff", &[]),
            ["coffee", "coffee near me"]
        );
        assert_eq!(
            suggest_still(&previous, "coffee", &[]),
            ["coffee near me"],
            "not the words themselves again"
        );
        assert_eq!(
            suggest_still(&previous, "coff", &["coffee".to_owned()]),
            ["coffee near me"],
            "and not a row the field already shows"
        );
    }
}
