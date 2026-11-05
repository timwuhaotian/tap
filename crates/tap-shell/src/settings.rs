//! Settings (⌘,), Tap's own page in a tab (Search's `Settings.swift`, drawn from
//! `tap_core::settings_index`, docs/PLAN.md §5.1.8): the pages down a rail with the find
//! field over them, and the page down the middle of the rest, group by group, each row
//! with its icon, its words and its control.
//!
//! Search's rows that Tap acts on, reading and writing Search's keys; a change takes
//! effect at once. Rows Tap doesn't act on yet aren't shown: their values are still kept
//! and still travel in a profile, and a page with none of them isn't in the rail. The
//! page is built again whenever something changes; it is a few dozen views.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};

use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor,
    NSControl, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightSemibold, NSForegroundColorAttributeName, NSImage,
    NSImageAlignment, NSImageView, NSKernAttributeName, NSModalResponseOK, NSOpenPanel,
    NSPopUpButton, NSResponder, NSScrollView, NSSearchField, NSSegmentSwitchTracking,
    NSSegmentedControl, NSSwitch, NSTextField, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDate, NSDictionary, NSNumber, NSPoint, NSProcessInfo, NSRect,
    NSSet, NSSize, NSString, NSURL,
};
use objc2_web_kit::{WKWebsiteDataStore, WKWebsiteDataTypeDiskCache, WKWebsiteDataTypeMemoryCache};
use serde_json::Value;
use tap_core::{
    engine::Engine,
    fnd::Url,
    identity,
    settings_index::{self as index, Control, Item, Kind, Merged, Page, Prefs},
};

use crate::{bench, bring, palette, prefs, profile, rt, update};

const RAIL: f64 = 212.0;
const COLUMN: f64 = 600.0;
/// Where a row's words start, past its icon.
const TEXT_INSET: f64 = 52.0;
/// The rail's icon column: each icon at the left of a box this wide, at this x, and
/// every page's name starting at `RAIL_TEXT_X` whatever its icon's width.
const RAIL_ICON: f64 = 20.0;
const RAIL_ICON_X: f64 = 18.0;
const RAIL_TEXT_X: f64 = 46.0;
const ROW_MIN: f64 = 52.0;
const CARD: Grey = palette::Grey(0.975, 0.145);

use palette::Grey;

/// What a control is bound to, by its tag.
#[derive(Clone, Debug)]
enum Binding {
    Switch(&'static str),
    SwitchOff(&'static str),
    Segments(&'static str, &'static [(&'static str, &'static str)]),
    Menu(&'static str, &'static [(&'static str, &'static str)]),
    Hours(&'static str, &'static [(i64, &'static str)]),
    Zoom,
    Merged(Merged),
    Button(&'static str),
    Engine,
    Custom,
    Page(Page),
}

struct State {
    root: Retained<Root>,
    rail: Retained<Column>,
    scroll: Retained<NSScrollView>,
    find: Retained<NSSearchField>,
    target: Retained<Target>,
    page: Cell<Page>,
    query: RefCell<String>,
    bindings: RefCell<Vec<Binding>>,
    width: Cell<f64>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

define_class!(
    /// The page's own view: the rail and the column, laid out again when it is resized.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSettings"]
    pub struct Root;

    impl Root {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resize_subviews(&self, _old: NSSize) {
            later_rebuild_if_resized();
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            palette::GROUND.resolve(dark).setFill();
            NSBezierPath::fillRect(self.bounds());
            palette::HAIRLINE.resolve(dark).setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(RAIL, 0.0),
                NSSize::new(1.0, self.bounds().size.height),
            ));
        }
    }
);

define_class!(
    /// A card: the rounded plate a group's rows sit on, with a hairline between rows.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSettingsCard"]
    #[ivars = RefCell<Vec<f64>>]
    struct Card;

    impl Card {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let dark = palette::is_dark(self);
            let bounds = self.bounds();
            CARD.resolve(dark).setFill();
            palette::card_path(bounds).fill();
            palette::HAIRLINE.resolve(dark).setFill();
            for y in self.ivars().borrow().iter() {
                NSBezierPath::fillRect(NSRect::new(
                    NSPoint::new(TEXT_INSET, *y),
                    NSSize::new(bounds.size.width - TEXT_INSET, 1.0),
                ));
            }
        }
    }
);

define_class!(
    /// The column's document view, flipped so it is laid out top down.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSettingsColumn"]
    struct Column;

    impl Column {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

define_class!(
    /// Every control's target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSettingsTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(changed:))]
        fn changed_action(&self, sender: Option<&AnyObject>) {
            if let Some(control) = sender.and_then(|s| s.downcast_ref::<NSControl>()) {
                act(control);
            }
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

/// The Settings page, made the first time it is shown.
pub fn view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    if let Some(root) = STATE.with_borrow(|s| s.as_ref().map(|s| s.root.clone())) {
        return Retained::into_super(root);
    }
    let root: Retained<Root> = {
        let this = Root::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    };
    root.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let target = Target::new(mtm);
    // Flipped, as the column is, so the pages go top down.
    let rail: Retained<Column> = {
        let this = Column::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(RAIL, frame.size.height))]
        }
    };
    rail.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
    let find = NSSearchField::new(mtm);
    find.setPlaceholderString(Some(&NSString::from_str("Find a setting")));
    find.setSendsSearchStringImmediately(true);
    // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
    unsafe {
        find.setTarget(Some(&target));
        find.setAction(Some(sel!(changed:)));
    }
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(mtm),
        NSRect::new(
            NSPoint::new(RAIL + 1.0, 0.0),
            NSSize::new(frame.size.width - RAIL - 1.0, frame.size.height),
        ),
    );
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    scroll.setAutohidesScrollers(true);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    root.addSubview(&rail);
    root.addSubview(&scroll);
    let page = prefs::defaults()
        .stringForKey(&NSString::from_str("settings.page"))
        .and_then(|p| Page::named(&p.to_string()))
        .unwrap_or(Page::General);
    STATE.with_borrow_mut(|state| {
        *state = Some(State {
            root: root.clone(),
            rail,
            scroll,
            find,
            target,
            page: Cell::new(page),
            query: RefCell::new(String::new()),
            bindings: RefCell::new(Vec::new()),
            width: Cell::new(0.0),
        });
    });
    rebuild();
    Retained::into_super(root)
}

/// Something changed elsewhere (a profile came in, an update was found): the page shows
/// it, if it is open.
pub fn changed() {
    if STATE.with_borrow(Option::is_some) {
        later(rebuild);
    }
}

/// The page to show, by the bench or a `tap://settings/<page>` address.
pub fn show_page(page: Page) {
    STATE.with_borrow(|state| {
        if let Some(state) = state {
            state.page.set(page);
            state.query.borrow_mut().clear();
            state.find.setStringValue(&NSString::from_str(""));
        }
    });
    changed();
}

/// Words in the find field, as typing them there does.
pub fn find(typed: &str) {
    STATE.with_borrow(|state| {
        if let Some(state) = state {
            state.find.setStringValue(&NSString::from_str(typed));
            typed.clone_into(&mut state.query.borrow_mut());
        }
    });
    changed();
}

/// What the page shows now, for the bench: its page, its rows and what each control says.
pub fn describe() -> Value {
    let kept = prefs::Kept::new();
    let rows: Vec<Value> = index::items()
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": item.id,
                "page": item.page().raw(),
                "group": item.group,
                "title": item.title,
                "detail": detail_of(item, &kept),
                "keys": item.keys,
                "live": index::live(item.id),
                "shown": shown(item),
                "findOnly": item.find_only,
                "value": shown_value(item, &kept),
            })
        })
        .collect();
    let page = STATE.with_borrow(|s| s.as_ref().map(|s| s.page.get().raw()));
    serde_json::json!({ "page": page, "rows": rows })
}

/// Whether the page shows a row: Tap acts on it, and this Mac's WebKit can do what it
/// asks.
fn shown(item: &Item) -> bool {
    index::live(item.id) && crate::web::offers(item.id)
}

/// What a row's control says, as the bench reads it back.
fn shown_value(item: &Item, kept: &prefs::Kept) -> Value {
    match item.control {
        Control::Switch(key) => index::flag(kept, key).into(),
        Control::SwitchOff(key) => (!index::flag(kept, key)).into(),
        Control::Segments(key, _) | Control::Menu(key, _) => index::text(kept, key).into(),
        Control::Hours(key, _) => index::value(kept, key),
        Control::Zoom => index::value(kept, "pageZoom"),
        Control::Merged(which) => index::merged(kept, which).into(),
        Control::Engine => index::text(kept, "search.engine").into(),
        Control::Keywords => prefs::keywords(kept)
            .iter()
            .map(|k| k.keyword.clone())
            .collect::<Vec<_>>()
            .into(),
        Control::Folder => downloads_folder().to_string_lossy().into_owned().into(),
        Control::Buttons(_) | Control::Note | Control::Nothing => Value::Null,
    }
}

fn later(work: fn()) {
    dispatch2::DispatchQueue::main().exec_async(work);
}

fn later_rebuild_if_resized() {
    let resized = STATE.with_borrow(|state| {
        state.as_ref().is_some_and(|s| {
            let width = s.root.bounds().size.width;
            (width - s.width.get()).abs() > 0.5
        })
    });
    if resized {
        later(rebuild);
    }
}

/// The rail and the column, made again from the index and the settings as they are.
fn rebuild() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with_borrow(|state| {
        let Some(state) = state else { return };
        state.width.set(state.root.bounds().size.width);
        state.bindings.borrow_mut().clear();
        build_rail(mtm, state);
        build_column(mtm, state);
    });
}

fn bind(state: &State, binding: Binding) -> isize {
    let mut bindings = state.bindings.borrow_mut();
    bindings.push(binding);
    isize::try_from(bindings.len() - 1).unwrap_or(0)
}

fn medium() -> f64 {
    // SAFETY: AppKit's own weight, read as the constant it is.
    unsafe { NSFontWeightMedium }
}

fn semibold() -> f64 {
    // SAFETY: as above.
    unsafe { NSFontWeightSemibold }
}

fn font(size: f64, weight: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight)
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    weight: f64,
    color: &NSColor,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font = font(size, weight);
    label.setFont(Some(&font));
    label.setTextColor(Some(color));
    label
}

/// A label that wraps at `width`, as tall as its text is there.
fn wrapping(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    color: &NSColor,
    width: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(size)));
    label.setTextColor(Some(color));
    label.setPreferredMaxLayoutWidth(width);
    // Measured by the cell that draws it, at the width it is drawn at.
    let height = label.cell().map_or_else(
        || label.fittingSize().height,
        |cell| {
            cell.cellSizeForBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, 10_000.0),
            ))
            .height
        },
    );
    label.setFrameSize(NSSize::new(width, height.ceil() + 1.0));
    label
}

fn symbol(mtm: MainThreadMarker, name: &str, size: f64) -> Retained<NSImageView> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )
    .or_else(|| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("gearshape"),
            None,
        )
    })
    .unwrap_or_default();
    let view = NSImageView::imageViewWithImage(&image, mtm);
    view.setFrameSize(NSSize::new(size, size));
    view.setContentTintColor(Some(&palette::MUTED.dynamic()));
    view
}

fn build_rail(mtm: MainThreadMarker, state: &State) {
    for view in &state.rail.subviews() {
        view.removeFromSuperview();
    }
    let ink = palette::INK.dynamic();
    let mut y = 52.0;
    state.find.setFrame(NSRect::new(
        NSPoint::new(14.0, y),
        NSSize::new(RAIL - 28.0, 24.0),
    ));
    state.rail.addSubview(&state.find);
    y += 38.0;
    let searching = !state.query.borrow().trim().is_empty();
    for block in Page::BLOCKS {
        for page in block.iter().copied().filter(|p| index::has_rows(*p)) {
            let chosen = !searching && page == state.page.get();
            let tint = if chosen {
                ink.clone()
            } else {
                palette::MUTED.dynamic()
            };
            if chosen {
                let wash = Pill::new(
                    mtm,
                    NSRect::new(NSPoint::new(8.0, y - 1.0), NSSize::new(RAIL - 16.0, 28.0)),
                );
                state.rail.addSubview(&wash);
            }
            // Every icon's left edge on one line and every name starting at the same
            // place: a button's leading image pushed each name over by its own icon's
            // width, so a key's name sat left of a keyboard's.
            let icon = symbol(mtm, page.icon(), RAIL_ICON);
            icon.setImageAlignment(NSImageAlignment::AlignLeft);
            icon.setContentTintColor(Some(&tint));
            // Centred on the pill behind the chosen page, not on the row's text.
            icon.setFrameOrigin(NSPoint::new(
                RAIL_ICON_X,
                y - 1.0 + (28.0 - RAIL_ICON) / 2.0,
            ));
            state.rail.addSubview(&icon);
            let face = if chosen { semibold() } else { medium() };
            let name = label(mtm, page.title(), 13.0, face, &tint);
            name.sizeToFit();
            let height = name.frame().size.height;
            name.setFrame(NSRect::new(
                NSPoint::new(RAIL_TEXT_X, y + (26.0 - height) / 2.0),
                NSSize::new(RAIL - RAIL_TEXT_X - 16.0, height),
            ));
            state.rail.addSubview(&name);
            // The click: a transparent button over the whole row, its title there for
            // VoiceOver and never drawn.
            // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(page.title()),
                    Some(&state.target),
                    Some(sel!(changed:)),
                    mtm,
                )
            };
            button.setBordered(false);
            button.setTransparent(true);
            button.setTag(bind(state, Binding::Page(page)));
            button.setFrame(NSRect::new(
                NSPoint::new(8.0, y - 1.0),
                NSSize::new(RAIL - 16.0, 28.0),
            ));
            state.rail.addSubview(&button);
            y += 28.0;
        }
        y += 14.0;
    }
}

define_class!(
    /// The chosen page's grey behind its name in the rail.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapSettingsPill"]
    struct Pill;

    impl Pill {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            palette::WASH.resolve(palette::is_dark(self)).setFill();
            palette::pill_path(self.bounds()).fill();
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
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

/// The find field's empty state: why nothing matched, and why a search can come
/// up short (rows Tap doesn't act on yet aren't listed anywhere, here included).
fn no_results(
    mtm: MainThreadMarker,
    column: &Column,
    left: f64,
    muted: &objc2_app_kit::NSColor,
    y: f64,
) -> f64 {
    let none = label(
        mtm,
        "No setting called that. Try other words, like dark, tabs or video",
        13.0,
        medium(),
        muted,
    );
    none.sizeToFit();
    none.setFrameOrigin(NSPoint::new(left + 4.0, y));
    column.addSubview(&none);
    let footnote = label(
        mtm,
        "Only settings Tap already handles are listed.",
        11.5,
        medium(),
        muted,
    );
    footnote.sizeToFit();
    footnote.setFrameOrigin(NSPoint::new(left + 4.0, y + 22.0));
    column.addSubview(&footnote);
    y + 62.0
}

fn build_column(mtm: MainThreadMarker, state: &State) {
    let visible = state.scroll.contentView().bounds();
    let keep = visible.origin;
    let area = state.scroll.contentSize().width;
    let width = COLUMN.min(area - 48.0).max(360.0);
    let left = ((area - width) / 2.0).max(16.0);
    let column: Retained<Column> = {
        let this = Column::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(area, 100.0))]
        }
    };
    let kept = prefs::Kept::new();
    let query = state.query.borrow().clone();
    let mut y = 44.0;
    let ink = palette::INK.dynamic();
    let muted = palette::MUTED.dynamic();
    if query.trim().is_empty() {
        let page = state.page.get();
        let title = label(mtm, page.title(), 22.0, semibold(), &ink);
        title.setFrameOrigin(NSPoint::new(left + 4.0, y));
        title.sizeToFit();
        column.addSubview(&title);
        y += 46.0;
        if page == Page::Extensions {
            let card = card(mtm);
            let note = wrapping(
                mtm,
                "Chrome extensions aren't in Tap yet. A profile's extensions are listed in it, and come back once they are.",
                12.0,
                &muted,
                width - 40.0,
            );
            note.setFrameOrigin(NSPoint::new(20.0, 16.0));
            card.setFrame(NSRect::new(
                NSPoint::new(left, y),
                NSSize::new(width, note.frame().size.height + 32.0),
            ));
            card.addSubview(&note);
            column.addSubview(&card);
            y += card.frame().size.height + 24.0;
        }
        for group in index::groups_of(page) {
            let rows: Vec<&'static Item> = index::items_in(group.id)
                .into_iter()
                .filter(|item| shown(item) && index::applies(item, &kept))
                .collect();
            if rows.is_empty() {
                continue;
            }
            y = heading(
                mtm,
                &column,
                group.icon,
                &group.title.to_uppercase(),
                left,
                y,
            );
            y = card_of(mtm, state, &column, &rows, &kept, left, width, y);
        }
    } else {
        let found: Vec<&'static Item> = index::find(&query)
            .into_iter()
            .filter(|item| shown(item))
            .collect();
        if found.is_empty() {
            y = no_results(mtm, &column, left, &muted, y);
        }
        let mut groups: Vec<&'static str> = found.iter().map(|i| i.group).collect();
        groups.dedup();
        for id in groups {
            let Some(group) = index::group(id) else {
                continue;
            };
            let rows: Vec<&'static Item> =
                found.iter().copied().filter(|i| i.group == id).collect();
            let words = format!("{}  ›  {}", group.page.title(), group.title).to_uppercase();
            y = heading(mtm, &column, group.icon, &words, left, y);
            y = card_of(mtm, state, &column, &rows, &kept, left, width, y);
        }
    }
    column.setFrameSize(NSSize::new(area, y + 40.0));
    state.scroll.setDocumentView(Some(&column));
    column.scrollPoint(keep);
}

fn heading(
    mtm: MainThreadMarker,
    column: &Column,
    icon: &str,
    words: &str,
    left: f64,
    y: f64,
) -> f64 {
    let image = symbol(mtm, icon, 12.0);
    image.setFrameOrigin(NSPoint::new(left + 4.0, y + 1.0));
    column.addSubview(&image);
    let ink = palette::MUTED.dynamic();
    let text = label(mtm, words, 10.5, semibold(), &ink);
    // Air between the capitals, as Search sets them: the string is unchanged, only
    // drawn with a touch of kern.
    let string = NSString::from_str(words);
    let kern = NSNumber::new_cgfloat(0.5);
    // SAFETY: AppKit's own attribute names, read as the constants they are.
    let (font_key, color_key, kern_key) = unsafe {
        (
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSKernAttributeName,
        )
    };
    let face = font(10.5, semibold());
    let attrs = NSDictionary::from_slices(
        &[font_key, color_key, kern_key],
        &[face.as_ref(), ink.as_ref(), kern.as_ref()],
    );
    // SAFETY: the dictionary holds the type AppKit expects for each name.
    let rich = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &string,
            Some(&attrs),
        )
    };
    text.setAttributedStringValue(&rich);
    text.sizeToFit();
    text.setFrameOrigin(NSPoint::new(left + 26.0, y));
    column.addSubview(&text);
    y + 24.0
}

fn card(mtm: MainThreadMarker) -> Retained<Card> {
    let this = Card::alloc(mtm).set_ivars(RefCell::new(Vec::new()));
    // SAFETY: NSView's designated initialiser.
    unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
}

#[allow(clippy::too_many_arguments)]
fn card_of(
    mtm: MainThreadMarker,
    state: &State,
    column: &Column,
    rows: &[&'static Item],
    kept: &prefs::Kept,
    left: f64,
    width: f64,
    y: f64,
) -> f64 {
    let card = card(mtm);
    let mut at = 0.0;
    let mut rules = Vec::new();
    for (index, item) in rows.iter().enumerate() {
        if index > 0 {
            rules.push(at);
        }
        at = row(mtm, state, &card, item, kept, width, at);
    }
    *card.ivars().borrow_mut() = rules;
    card.setFrame(NSRect::new(NSPoint::new(left, y), NSSize::new(width, at)));
    column.addSubview(&card);
    y + at + 26.0
}

/// One row: its icon, its title, its line, its control. Answers where the next starts.
#[allow(clippy::too_many_lines)]
fn row(
    mtm: MainThreadMarker,
    state: &State,
    card: &Card,
    item: &'static Item,
    kept: &prefs::Kept,
    width: f64,
    top: f64,
) -> f64 {
    let counts = index::applies(item, kept);
    let ink = palette::INK.dynamic();
    let muted = palette::MUTED.dynamic();
    // A row that means nothing as things are set reads as unavailable all over,
    // not only at its control.
    let faded = NSColor::tertiaryLabelColor();
    let title_ink = if counts { ink.clone() } else { faded.clone() };
    let line_ink = if counts { muted.clone() } else { faded.clone() };
    let control = make_control(mtm, state, item, kept);
    let wide = item.kind == Kind::Wide;
    let control_width = control.as_ref().map_or(0.0, |c| c.frame().size.width);
    let words = if wide || control.is_none() {
        width - TEXT_INSET - 20.0
    } else {
        (width - TEXT_INSET - control_width - 36.0).max(160.0)
    };
    let mut detail = detail_of(item, kept);
    if !counts {
        index::needs(item).clone_into(&mut detail);
    }
    let icon = symbol(mtm, item.icon, 17.0);
    icon.setContentTintColor(Some(if counts { &muted } else { &faded }));
    let title = label(mtm, &title_of(item), 13.0, medium(), &title_ink);
    title.sizeToFit();
    let title_width = title.frame().size.width;
    let line = wrapping(mtm, &detail, 11.5, &line_ink, words);
    let text_height = 18.0 + 2.0 + line.frame().size.height;
    let mut height = (text_height + 24.0).max(ROW_MIN);
    let text_top = top + (height - text_height) / 2.0;
    let icon_y = if wide {
        // A wide row's control sits under its words, so the icon stays with the
        // words rather than the row's middle.
        text_top + 1.0
    } else {
        top + (height - 17.0) / 2.0
    };
    icon.setFrameOrigin(NSPoint::new(18.0, icon_y));
    title.setFrameOrigin(NSPoint::new(TEXT_INSET, text_top));
    title.setFrameSize(NSSize::new(words, 18.0));
    line.setFrameOrigin(NSPoint::new(TEXT_INSET, text_top + 20.0));
    if let Some(more) = item.more {
        for view in [&title, &line] {
            view.setToolTip(Some(&NSString::from_str(more)));
        }
        // The rest of the story gets a seat of its own, for the pointer that never
        // rests on the words; it carries the same tooltip and does nothing else.
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("info.circle"),
            None,
        );
        // SAFETY: no target or action, so nothing is ever sent anywhere.
        let info = unsafe {
            NSButton::buttonWithImage_target_action(&image.unwrap_or_default(), None, None, mtm)
        };
        info.setBordered(false);
        info.setContentTintColor(Some(if counts { &muted } else { &faded }));
        info.setToolTip(Some(&NSString::from_str(more)));
        let info_x = (TEXT_INSET + title_width + 5.0).min(TEXT_INSET + words - 15.0);
        info.setFrame(NSRect::new(
            NSPoint::new(info_x, text_top + 2.0),
            NSSize::new(14.0, 14.0),
        ));
        card.addSubview(&info);
    }
    card.addSubview(&icon);
    card.addSubview(&title);
    card.addSubview(&line);
    if let Some(control) = control {
        let size = control.frame().size;
        if wide {
            let under = top + 12.0 + text_height + 10.0;
            control.setFrameOrigin(NSPoint::new(TEXT_INSET, under));
            height = under - top + size.height + 14.0;
        } else {
            control.setFrameOrigin(NSPoint::new(
                width - size.width - 18.0,
                top + (height - size.height) / 2.0,
            ));
        }
        set_enabled(&control, counts);
        card.addSubview(&control);
    }
    // What goes under a row's words: the custom engine's address, the site shortcuts.
    let mut bottom = top + height;
    if item.control == Control::Engine && index::text(kept, "search.engine") == "custom" {
        let field = NSTextField::new(mtm);
        field.setStringValue(&NSString::from_str(&index::text(kept, "search.custom")));
        field.setPlaceholderString(Some(&NSString::from_str("https://example.com/search?q=%s")));
        field.setFrame(NSRect::new(
            NSPoint::new(TEXT_INSET, bottom - 6.0),
            NSSize::new(width - TEXT_INSET - 20.0, 24.0),
        ));
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        field.setTag(bind(state, Binding::Custom));
        // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
        unsafe {
            field.setTarget(Some(&state.target));
            field.setAction(Some(sel!(changed:)));
        }
        card.addSubview(&field);
        bottom += 30.0;
    }
    if item.control == Control::Keywords {
        let shortcuts = prefs::keywords(kept);
        let text = if shortcuts.is_empty() {
            "None yet. Shortcuts that come in with a profile show here and work at once; adding them here comes later.".to_owned()
        } else {
            shortcuts
                .iter()
                .map(|k| {
                    format!(
                        "{}  →  {}",
                        k.keyword,
                        tap_core::engine::bare_host(&k.template)
                            .unwrap_or_else(|| k.template.clone())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let list = wrapping(mtm, &text, 11.5, &ink, width - TEXT_INSET - 20.0);
        list.setFrameOrigin(NSPoint::new(TEXT_INSET, bottom - 8.0));
        card.addSubview(&list);
        bottom += list.frame().size.height + 6.0;
    }
    bottom
}

fn set_enabled(control: &NSView, on: bool) {
    if let Some(control) = control.downcast_ref::<NSControl>() {
        control.setEnabled(on);
        return;
    }
    // A row of buttons.
    for child in &control.subviews() {
        if let Some(control) = child.downcast_ref::<NSControl>() {
            control.setEnabled(on);
        }
    }
}

fn title_of(item: &Item) -> String {
    match item.id {
        "updates.version" => format!("{} {}", identity::NAME, update::current()),
        _ => item.title.to_owned(),
    }
}

fn detail_of(item: &Item, kept: &prefs::Kept) -> String {
    match item.id {
        "downloads.folder" => crate::downloads::folder().to_string_lossy().into_owned(),
        "updates.version" => {
            let status = update::status();
            match (status["ready"].as_str(), status["said"].as_str()) {
                (Some(version), _) => format!("Tap {version} is ready: it installs when Tap quits"),
                (None, Some(said)) if said != "up to date" => format!(
                    "{}. Checked every six hours on its own",
                    said.trim_end_matches('.')
                ),
                _ => item.detail.to_owned(),
            }
        }
        "general.import" => match bring::listed(&bring::found()) {
            Some(found) => format!(
                "Passwords, bookmarks, history and open tabs from {found} on this Mac, or from a file"
            ),
            None => "No other browser with something to bring is on this Mac; a passwords CSV or a Tap backup comes in here".to_owned(),
        },
        "passwords.open" => {
            match crate::vault::all().len() {
                0 => "None kept by Tap yet".to_owned(),
                1 => "1 kept by Tap in the macOS keychain".to_owned(),
                n => format!("{n} kept by Tap in the macOS keychain"),
            }
        }
        "general.default" if is_default_browser() => {
            "Tap opens the links other apps send".to_owned()
        }
        "tabs.sleep" => match index::merged(kept, Merged::Sleep).as_str() {
            "never" => "Every tab keeps its page, however long you leave it".to_owned(),
            "later" => {
                "After two hours away, a tab rests and wakes when you go back to it".to_owned()
            }
            "sooner" => {
                "After ten minutes away, a tab rests and wakes when you go back to it".to_owned()
            }
            _ => "After half an hour away, a tab rests and wakes when you go back to it".to_owned(),
        },
        _ => item.detail.to_owned(),
    }
}

fn options_titles(options: &[(&'static str, &'static str)]) -> Retained<NSArray<NSString>> {
    let titles: Vec<Retained<NSString>> =
        options.iter().map(|(_, t)| NSString::from_str(t)).collect();
    NSArray::from_retained_slice(&titles)
}

fn segments(
    mtm: MainThreadMarker,
    state: &State,
    options: &[(&'static str, &'static str)],
    chosen: &str,
    binding: Binding,
) -> Retained<NSView> {
    // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
    let control = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &options_titles(options),
            NSSegmentSwitchTracking::SelectOne,
            Some(&state.target),
            Some(sel!(changed:)),
            mtm,
        )
    };
    if let Some(at) = options.iter().position(|(raw, _)| *raw == chosen) {
        control.setSelectedSegment(isize::try_from(at).unwrap_or(0));
    }
    control.setTag(bind(state, binding));
    control.sizeToFit();
    Retained::into_super(Retained::into_super(control))
}

fn menu(
    mtm: MainThreadMarker,
    state: &State,
    titles: &[String],
    chosen: Option<usize>,
    binding: Binding,
) -> Retained<NSView> {
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    for title in titles {
        popup.addItemWithTitle(&NSString::from_str(title));
    }
    if let Some(at) = chosen {
        popup.selectItemAtIndex(isize::try_from(at).unwrap_or(0));
    }
    // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
    unsafe {
        popup.setTarget(Some(&state.target));
        popup.setAction(Some(sel!(changed:)));
    }
    popup.setTag(bind(state, binding));
    popup.sizeToFit();
    Retained::into_super(Retained::into_super(Retained::into_super(popup)))
}

fn switch(mtm: MainThreadMarker, state: &State, on: bool, binding: Binding) -> Retained<NSView> {
    let control = NSSwitch::new(mtm);
    control.setState(if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
    unsafe {
        control.setTarget(Some(&state.target));
        control.setAction(Some(sel!(changed:)));
    }
    control.setTag(bind(state, binding));
    control.sizeToFit();
    Retained::into_super(Retained::into_super(control))
}

#[allow(clippy::too_many_lines)] // one arm per kind of control, as Search's switch is
fn make_control(
    mtm: MainThreadMarker,
    state: &State,
    item: &'static Item,
    kept: &prefs::Kept,
) -> Option<Retained<NSView>> {
    Some(match item.control {
        Control::Switch(key) => switch(mtm, state, index::flag(kept, key), Binding::Switch(key)),
        Control::SwitchOff(key) => {
            switch(mtm, state, !index::flag(kept, key), Binding::SwitchOff(key))
        }
        Control::Segments(key, options) => {
            let chosen = index::text(kept, key);
            segments(
                mtm,
                state,
                options,
                &chosen,
                Binding::Segments(key, options),
            )
        }
        Control::Menu(key, options) => {
            let titles: Vec<String> = options.iter().map(|(_, t)| (*t).to_owned()).collect();
            let chosen = index::text(kept, key);
            menu(
                mtm,
                state,
                &titles,
                options.iter().position(|(raw, _)| *raw == chosen),
                Binding::Menu(key, options),
            )
        }
        Control::Hours(key, options) => {
            let titles: Vec<String> = options.iter().map(|(_, t)| (*t).to_owned()).collect();
            let chosen = index::value(kept, key).as_i64().unwrap_or(0);
            menu(
                mtm,
                state,
                &titles,
                options.iter().position(|(h, _)| *h == chosen),
                Binding::Hours(key, options),
            )
        }
        Control::Zoom => {
            let stops: Vec<String> = (10..=60).map(|i| format!("{}%", i * 5)).collect();
            let zoom = index::value(kept, "pageZoom").as_f64().unwrap_or(1.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let at = ((zoom * 100.0 / 5.0).round() as usize).saturating_sub(10);
            menu(mtm, state, &stops, Some(at), Binding::Zoom)
        }
        Control::Merged(Merged::PinsKeep) => switch(
            mtm,
            state,
            index::merged(kept, Merged::PinsKeep) == "true",
            Binding::Merged(Merged::PinsKeep),
        ),
        Control::Merged(which) => {
            let chosen = index::merged(kept, which);
            if which.is_menu() {
                let titles: Vec<String> = which
                    .options()
                    .iter()
                    .map(|(_, t)| (*t).to_owned())
                    .collect();
                menu(
                    mtm,
                    state,
                    &titles,
                    which.options().iter().position(|(raw, _)| *raw == chosen),
                    Binding::Merged(which),
                )
            } else {
                segments(mtm, state, which.options(), &chosen, Binding::Merged(which))
            }
        }
        Control::Engine => {
            let titles: Vec<String> = Engine::ALL.iter().map(|e| e.title().to_owned()).collect();
            let chosen = Engine::from_raw(&index::text(kept, "search.engine")).unwrap_or_default();
            menu(
                mtm,
                state,
                &titles,
                Engine::ALL.iter().position(|e| *e == chosen),
                Binding::Engine,
            )
        }
        Control::Buttons(actions) => {
            if item.id == "general.default" && is_default_browser() {
                let mark = symbol(mtm, "checkmark", 16.0);
                mark.setContentTintColor(Some(&palette::INK.dynamic()));
                return Some(Retained::into_super(Retained::into_super(mark)));
            }
            let holder = NSView::new(mtm);
            let mut x = 0.0;
            for (action, title) in actions {
                let button: Retained<NSControl> = {
                    // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
                    let button = unsafe {
                        NSButton::buttonWithTitle_target_action(
                            &NSString::from_str(title),
                            Some(&state.target),
                            Some(sel!(changed:)),
                            mtm,
                        )
                    };
                    button.setTag(bind(state, Binding::Button(action)));
                    Retained::into_super(button)
                };
                button.sizeToFit();
                let size = button.frame().size;
                button.setFrameOrigin(NSPoint::new(x, 0.0));
                x += size.width + 6.0;
                holder.addSubview(&button);
            }
            holder.setFrameSize(NSSize::new((x - 6.0).max(0.0), 30.0));
            holder
        }
        Control::Folder => {
            // SAFETY: the target lives as long as the page, and `changed:` takes the sender.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("Change…"),
                    Some(&state.target),
                    Some(sel!(changed:)),
                    mtm,
                )
            };
            button.setTag(bind(state, Binding::Button("folder")));
            button.sizeToFit();
            Retained::into_super(Retained::into_super(button))
        }
        Control::Note => {
            let always = label(mtm, "Always", 12.0, medium(), &palette::MUTED.dynamic());
            always.sizeToFit();
            Retained::into_super(Retained::into_super(always))
        }
        Control::Keywords | Control::Nothing => return None,
    })
}

/// A control moved: its setting written as Search writes it, then whatever acts on it
/// told, and the page made again.
#[allow(clippy::too_many_lines)] // one arm per kind of control, as Search's switch is
fn act(control: &NSControl) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // The find field: what it holds is the query, and the column follows it.
    if control.downcast_ref::<NSSearchField>().is_some() {
        let typed = control.stringValue().to_string();
        STATE.with_borrow(|state| {
            if let Some(state) = state {
                *state.query.borrow_mut() = typed;
            }
        });
        later(rebuild);
        return;
    }
    let binding = STATE.with_borrow(|state| {
        let state = state.as_ref()?;
        let tag = usize::try_from(control.tag()).ok()?;
        state.bindings.borrow().get(tag).cloned()
    });
    let Some(binding) = binding else { return };
    let bench_toggled = matches!(binding, Binding::Switch("bench"));
    let mut kept = prefs::Kept::new();
    let selected = |control: &NSControl| -> usize {
        if let Some(segments) = control.downcast_ref::<NSSegmentedControl>() {
            return usize::try_from(segments.selectedSegment()).unwrap_or(0);
        }
        if let Some(popup) = control.downcast_ref::<NSPopUpButton>() {
            return usize::try_from(popup.indexOfSelectedItem()).unwrap_or(0);
        }
        0
    };
    let on = control
        .downcast_ref::<NSSwitch>()
        .is_some_and(|s| s.state() == NSControlStateValueOn);
    match binding {
        Binding::Switch(key) => kept.set(key, on.into()),
        Binding::SwitchOff(key) => kept.set(key, (!on).into()),
        Binding::Segments(key, options) | Binding::Menu(key, options) => {
            if let Some((raw, _)) = options.get(selected(control)) {
                let value = match index::default(key) {
                    Value::Bool(_) => Value::Bool(*raw == "true"),
                    _ => Value::String((*raw).to_owned()),
                };
                kept.set(key, value);
            }
        }
        Binding::Hours(key, options) => {
            if let Some((hours, _)) = options.get(selected(control)) {
                kept.set(key, (*hours).into());
            }
        }
        Binding::Zoom => {
            #[allow(clippy::cast_precision_loss)]
            let zoom = (selected(control) + 10) as f64 * 5.0 / 100.0;
            kept.set("pageZoom", zoom.into());
        }
        Binding::Merged(which) => {
            let raw = if which == Merged::PinsKeep {
                on.to_string()
            } else {
                which
                    .options()
                    .get(selected(control))
                    .map(|(r, _)| (*r).to_owned())
                    .unwrap_or_default()
            };
            index::set_merged(&mut kept, which, &raw);
        }
        Binding::Engine => {
            if let Some(engine) = Engine::ALL.get(selected(control)) {
                kept.set("search.engine", engine.raw().into());
            }
        }
        Binding::Custom => {
            let template = control.stringValue().to_string();
            if index::text(&kept, "search.custom") == template {
                return;
            }
            kept.set("search.custom", template.into());
        }
        Binding::Page(page) => {
            STATE.with_borrow(|state| {
                if let Some(state) = state {
                    state.page.set(page);
                    state.query.borrow_mut().clear();
                    state.find.setStringValue(&NSString::from_str(""));
                    state
                        .scroll
                        .contentView()
                        .scrollToPoint(NSPoint::new(0.0, 0.0));
                }
            });
            prefs::Kept::new().set("settings.page", page.raw().into());
        }
        Binding::Button(action) => {
            press(mtm, action);
            return;
        }
    }
    prefs::apply(mtm);
    // The bench socket follows its switch at once.
    if bench_toggled {
        bench::refresh();
    }
    later(rebuild);
}

/// A button: something done now.
fn press(mtm: MainThreadMarker, action: &str) {
    match action {
        // Search's ImportPanel: which browser, which profile, and what of it.
        "import" => bring::open(),
        "export" => profile::export(),
        "update.check" => update::check_now(),
        "feedback" => send_feedback(),
        "passwords" => open_passwords(),
        "forget" => forget_choices(),
        "default" => make_default(),
        "folder" => choose_folder(mtm),
        "notes" => {
            if let Some(url) =
                Url::parse("https://github.com/timwuhaotian/tap/blob/main/CHANGELOG.md")
            {
                rt::dispatch(move |browser| {
                    browser.open(Some(&url), true, None);
                });
            }
        }
        "clear.history" => {
            if sure(
                mtm,
                "Clear your history?",
                "Every address you have been to goes, and the field stops finishing them.",
                "Clear",
            ) {
                rt::dispatch(tap_core::browser::Browser::clear_history);
            }
        }
        "clear.cookies" => {
            if sure(
                mtm,
                "Sign out of every site?",
                "Cookies and what sites keep on this Mac go, so you sign in again.",
                "Sign Out",
            ) {
                // SAFETY: WebKit's own type names, read as the constants they are.
                let types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(mtm) };
                remove(mtm, &types);
            }
        }
        "clear.cache" => {
            // SAFETY: as above.
            let types = unsafe {
                NSSet::from_slice(&[WKWebsiteDataTypeDiskCache, WKWebsiteDataTypeMemoryCache])
            };
            remove(mtm, &types);
        }
        _ => {}
    }
    later(rebuild);
}

fn sure(mtm: MainThreadMarker, title: &str, detail: &str, confirm: &str) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(detail));
    alert.addButtonWithTitle(&NSString::from_str(confirm));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.runModal() == NSAlertFirstButtonReturn
}

fn remove(mtm: MainThreadMarker, types: &NSSet<NSString>) {
    let done = RcBlock::new(|| {});
    // SAFETY: the data store Tap's pages use, every record since the beginning of time.
    unsafe {
        WKWebsiteDataStore::defaultDataStore(mtm)
            .removeDataOfTypes_modifiedSince_completionHandler(
                types,
                &NSDate::distantPast(),
                &done,
            );
    }
}

/// One part of a feedback address, with everything a query value can't carry as it
/// stands written out.
fn percent_encode(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// The feedback draft's address (Search's `Links.writeFeedback`): a new bug report,
/// the version and this Mac already filled in, and nothing sent — the person still
/// writes it and sends it themselves. Search drafts a mail, but Tap's contact address
/// is still an open item (docs/PLAN.md §14), so the draft is on Tap's own tracker
/// rather than addressed to Search's inbox.
fn feedback_url(version: &str, build: &str, macos: &str) -> String {
    format!(
        "https://github.com/timwuhaotian/tap/issues/new?template=bug_report.yml&title={}&build={}&macos={}",
        percent_encode(&format!("Tap feedback — {version}")),
        percent_encode(build),
        percent_encode(macos),
    )
}

fn send_feedback() {
    let version = update::current();
    // The build beside the version, as Search puts both in its draft; a copy run from
    // the tree has no bundle build, and the version alone is what it has.
    let build = objc2_foundation::NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleVersion"))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map_or_else(|| version.clone(), |v| v.to_string());
    let macos = NSProcessInfo::processInfo()
        .operatingSystemVersionString()
        .to_string();
    let Some(url) =
        NSURL::URLWithString(&NSString::from_str(&feedback_url(&version, &build, &macos)))
    else {
        return;
    };
    NSWorkspace::sharedWorkspace().openURL(&url);
}

/// Where Tap's passwords can be seen until Tap has a page of its own: Keychain Access,
/// which lists the login keychain's items (search for "Tap"), wherever this macOS keeps
/// it; Apple's Passwords app, which shows only iCloud's, on a Mac without it.
const PASSWORD_APPS: [&str; 3] = [
    "/System/Library/CoreServices/Applications/Keychain Access.app",
    "/System/Applications/Utilities/Keychain Access.app",
    "/System/Applications/Passwords.app",
];

fn passwords_app(exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    PASSWORD_APPS.iter().copied().find(|app| exists(app))
}

/// Search's Open… leads to its own passwords page. Tap keeps passwords now (`vault`)
/// but has no such page yet, so this opens Keychain Access, where Tap's are listed
/// under its name and shown with your Mac's password.
fn open_passwords() {
    let Some(app) = passwords_app(|path| std::path::Path::new(path).exists()) else {
        return;
    };
    NSWorkspace::sharedWorkspace().openURL(&NSURL::fileURLWithPath(&NSString::from_str(app)));
}

/// A kept site choice, Search's way: the camera, microphone and location answers
/// under `capture.`, a site's don't-float-videos under `nofloat.` (Search's
/// `Browser.forgetCaptureChoices` and `Grounded`). Those answers live in the
/// defaults, not in WebKit — WebKit keeps no say of its own between launches, it
/// asks its delegate each time — so there is no WebKit store to clear for them.
fn is_choice_key(key: &str) -> bool {
    key.starts_with("capture.") || key.starts_with("nofloat.")
}

fn forget_choices() {
    // Read before any are taken out: the dictionary is the defaults' own.
    let keys: Vec<String> = prefs::defaults()
        .dictionaryRepresentation()
        .allKeys()
        .iter()
        .filter_map(|key| key.downcast_ref::<NSString>().map(ToString::to_string))
        .filter(|key| is_choice_key(key))
        .collect();
    let mut kept = prefs::Kept::new();
    for key in &keys {
        kept.set(key, Value::Null);
    }
}

/// Where Settings › Save to points, else Downloads (a test world keeps its own; see
/// `downloads::folder`).
pub fn downloads_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    prefs::defaults()
        .stringForKey(&NSString::from_str("downloads"))
        .map(|p| PathBuf::from(p.to_string()))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| home.join("Downloads"))
}

fn choose_folder(mtm: MainThreadMarker) {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Save Here")));
    if panel.runModal() == NSModalResponseOK
        && let Some(path) = panel.URL().and_then(|u| u.path())
    {
        prefs::Kept::new().set("downloads", path.to_string().into());
    }
}

fn is_default_browser() -> bool {
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(probe) = NSURL::URLWithString(&NSString::from_str("https://example.com")) else {
        return false;
    };
    let Some(app) = workspace.URLForApplicationToOpenURL(&probe) else {
        return false;
    };
    let ours = objc2_foundation::NSBundle::mainBundle().bundleURL();
    app.path().map(|p| p.to_string()) == ours.path().map(|p| p.to_string())
}

/// Open links from other apps: Tap for http and https, as macOS asks you to confirm.
fn make_default() {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = objc2_foundation::NSBundle::mainBundle().bundleURL();
    for scheme in ["http", "https"] {
        let done = RcBlock::new(|_error: *mut objc2_foundation::NSError| {
            later(rebuild);
        });
        workspace.setDefaultApplicationAtURL_toOpenURLsWithScheme_completionHandler(
            &app,
            &NSString::from_str(scheme),
            Some(&done),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_feedback_draft_names_the_version_build_and_mac() {
        assert_eq!(
            feedback_url("0.1.1", "42", "macOS 27.0 (26A100)"),
            "https://github.com/timwuhaotian/tap/issues/new?template=bug_report.yml&title=Tap%20feedback%20%E2%80%94%200.1.1&build=42&macos=macOS%2027.0%20%2826A100%29"
        );
    }

    #[test]
    fn only_a_sites_kept_choices_are_forgotten() {
        assert_eq!(
            [
                is_choice_key("capture.https://example.com|1"),
                is_choice_key("capture.https://example.com|notifications"),
                is_choice_key("nofloat.example.com"),
                is_choice_key("bench"),
                is_choice_key("capture"),
                is_choice_key("look"),
            ],
            [true, true, true, false, false, false]
        );
    }

    #[test]
    fn keychain_access_is_preferred_and_the_passwords_app_is_the_fallback() {
        assert_eq!(
            passwords_app(|_| true),
            Some("/System/Library/CoreServices/Applications/Keychain Access.app")
        );
        assert_eq!(
            passwords_app(|app| app.starts_with("/System/Applications/")),
            Some("/System/Applications/Utilities/Keychain Access.app")
        );
        assert_eq!(
            passwords_app(|app| app.contains("Passwords")),
            Some("/System/Applications/Passwords.app")
        );
        assert_eq!(passwords_app(|_| false), None);
    }
}
