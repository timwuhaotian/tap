//! Search's colours and measures (`Design.swift`). Every colour is a pair, one for a light
//! window and one for a dark, resolved against whatever appearance the view has; nothing
//! else in the code knows which it is.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSFontWeightRegular,
    NSForegroundColorAttributeName, NSLineBreakMode, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSShadow, NSStringDrawing, NSView,
};
use objc2_foundation::{
    NSArray, NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};

/// A grey for a light window and one for a dark.
#[derive(Clone, Copy, Debug)]
pub struct Grey(pub f64, pub f64);

pub const GROUND: Grey = Grey(1.0, 0.11);
pub const INK: Grey = Grey(0.09, 0.93);
pub const MUTED: Grey = Grey(0.55, 0.58);
pub const HAIRLINE: Grey = Grey(0.91, 0.20);
/// The live tab.
pub const WASH: Grey = Grey(0.937, 0.175);
/// The one under the pointer.
pub const HOVER: Grey = Grey(0.965, 0.15);

/// The strip's height; the traffic lights sit centred in it.
pub const STRIP: f64 = 52.0;
/// The address bar's height, under the strip.
pub const BAR: f64 = 38.0;
/// Where the first tab starts: the lights run from 19 to 79, and get the same air on
/// their right that the window gives them on their left.
pub const LIGHTS: f64 = 100.0;
/// Tabs are a fixed width so the cross always lands in the same place, giving way only
/// when there are too many.
pub const TAB_WIDTH: f64 = 186.0;
pub const TAB_TITLED: f64 = 80.0;
pub const TAB_MIN: f64 = 36.0;
pub const TAB_GAP: f64 = 2.0;
/// The row the tabs sit in, and the plus at its end.
pub const ROW: f64 = 30.0;
pub const PLUS: f64 = 30.0;
/// Back, forward and reload at the far end: three doors and the air before the next.
pub const DOOR: f64 = 26.0;
pub const HELM: f64 = 3.0 * DOOR + 2.0 * 2.0 + 8.0;
/// The address field, in both the places it shows up.
pub const FIELD: f64 = 560.0;
pub const RADIUS: f64 = 8.0;
/// Every floating card's radius: offers, suggestions, sign-in, bring, Settings.
/// One radius so the chrome reads as one piece.
pub const CARD_RADIUS: f64 = 12.0;

/// A card's outline at the shared radius.
#[must_use]
pub fn card_path(rect: NSRect) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, CARD_RADIUS, CARD_RADIUS)
}

/// Whether two rects share any ground: what a draw can skip when AppKit has only
/// dirtied part of a view (the load line's frames dirty a sliver, once a frame).
#[must_use]
pub fn overlaps(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

/// A pill's outline at the chrome radius: tabs, picked rows, the address capsule.
#[must_use]
pub fn pill_path(rect: NSRect) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, RADIUS, RADIUS)
}

/// One shadow, described once: `blur` points of blur, `dy` down, ink at `light`
/// in a light window and at `dark_alpha` in a dark one.
#[must_use]
pub fn shadow(blur: f64, dy: f64, light: f64, dark_alpha: f64, dark: bool) -> Retained<NSShadow> {
    let shadow = NSShadow::new();
    shadow.setShadowBlurRadius(blur);
    shadow.setShadowOffset(NSSize::new(0.0, dy));
    shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(
        0.0,
        if dark { dark_alpha } else { light },
    )));
    shadow
}

/// The shadow under a floating card: offers, suggestions, sign-in.
#[must_use]
pub fn float_shadow(dark: bool) -> Retained<NSShadow> {
    shadow(14.0, -3.0, 0.16, 0.5, dark)
}

/// The shadow under a carried tab: Search's lift.
#[must_use]
pub fn lift_shadow() -> Retained<NSShadow> {
    shadow(12.0, -4.0, 0.14, 0.14, false)
}

/// The shadow under the live tab's pill in a light window; in a dark one the
/// pill wears an edge instead, where a shadow wouldn't show.
#[must_use]
pub fn live_shadow() -> Retained<NSShadow> {
    shadow(3.0, -1.0, 0.10, 0.10, false)
}

/// Draws `draw` under `shadow`, then takes the shadow back off.
pub fn shadowed(shadow: &NSShadow, draw: impl FnOnce()) {
    shadow.set();
    draw();
    NSShadow::new().set();
}

/// A 1-point hairline round `path`: the edge that makes a filled pill read as
/// a plate rather than a flat mark.
pub fn stroke_hairline(path: &NSBezierPath, dark: bool) {
    HAIRLINE.resolve(dark).setStroke();
    path.setLineWidth(1.0);
    path.stroke();
}

/// A colour for a light window and one for a dark, as red, green and blue.
#[derive(Clone, Copy, Debug)]
pub struct Tint(pub (f64, f64, f64), pub (f64, f64, f64));

impl Tint {
    #[must_use]
    pub fn resolve(self, dark: bool) -> Retained<NSColor> {
        let (r, g, b) = if dark { self.1 } else { self.0 };
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
    }
}

/// Said with care: plain http, a page that isn't all secure (Search's `unsafe`).
pub const UNSAFE: Tint = Tint((0.71, 0.33, 0.04), (0.98, 0.75, 0.14));

/// A tab group's colour by its name in the session (`tap_core::groups::COLOURS`),
/// Chrome's nine; anything unknown is grey.
#[must_use]
pub fn group_colour(name: Option<&str>) -> Tint {
    const fn hex(rgb: u32) -> (f64, f64, f64) {
        (
            ((rgb >> 16) & 0xff) as f64 / 255.0,
            ((rgb >> 8) & 0xff) as f64 / 255.0,
            (rgb & 0xff) as f64 / 255.0,
        )
    }
    let (light, dark) = match name.unwrap_or("grey") {
        "blue" => (0x1A_73E8, 0x8A_B4F8),
        "red" => (0xD9_3025, 0xF2_8B82),
        "yellow" => (0xE3_7400, 0xFD_D663),
        "green" => (0x1E_8E3E, 0x81_C995),
        "pink" => (0xD0_1884, 0xFF_8BCB),
        "purple" => (0xA1_42F4, 0xD7_AEFB),
        "cyan" => (0x00_7B83, 0x78_D9EC),
        "orange" => (0xE8_710A, 0xFC_AD70),
        _ => (0x5F_6368, 0xDA_DCE0),
    };
    Tint(hex(light), hex(dark))
}

/// The words on a group's chip: white on the deep light-window colours, near black on
/// the pale dark-window ones.
#[must_use]
pub fn on_group(dark: bool) -> Retained<NSColor> {
    if dark {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.125, 0.13, 0.14, 1.0)
    } else {
        NSColor::colorWithWhite_alpha(1.0, 1.0)
    }
}

impl Grey {
    #[must_use]
    pub fn resolve(self, dark: bool) -> Retained<NSColor> {
        NSColor::colorWithWhite_alpha(if dark { self.1 } else { self.0 }, 1.0)
    }

    /// A colour that resolves itself, for AppKit corners that keep an `NSColor`, such as
    /// the window's background.
    #[must_use]
    pub fn dynamic(self) -> Retained<NSColor> {
        let light = self.resolve(false);
        let dim = self.resolve(true);
        let provider = RcBlock::new(
            move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
                // SAFETY: AppKit hands the provider a live appearance for the call's length.
                let appearance = unsafe { appearance.as_ref() };
                let chosen = if is_dark_appearance(appearance) {
                    &dim
                } else {
                    &light
                };
                // The block owns both colours for as long as AppKit keeps the colour, so a
                // borrowed one is a valid +0 return.
                NonNull::from(&**chosen)
            },
        );
        // SAFETY: the provider always returns a valid colour, kept alive by the block.
        unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
    }
}

fn is_dark_appearance(appearance: &NSAppearance) -> bool {
    // SAFETY: the two appearance names are AppKit's own constants.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let names = NSArray::from_slice(&[aqua, dark]);
    appearance
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|name| &*name == dark)
}

/// Whether what is being drawn right now is drawn dark: for images that draw
/// themselves, such as a menu's.
#[must_use]
pub fn drawing_dark() -> bool {
    is_dark_appearance(&NSAppearance::currentDrawingAppearance())
}

/// Whether a view is drawn dark right now.
#[must_use]
pub fn is_dark(view: &NSView) -> bool {
    is_dark_appearance(&view.effectiveAppearance())
}

// The one text style of the chrome: the system font at 13 points, the field's too.
// Made once: a load redraws the chrome dozens of times, and rebuilding the font and
// the paragraph style on each draw showed.
thread_local! {
    static STYLE: (Retained<NSFont>, Retained<NSMutableParagraphStyle>) = {
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        // SAFETY: the system font at a valid size and weight.
        (unsafe { NSFont::systemFontOfSize_weight(13.0, NSFontWeightRegular) }, paragraph)
    };
}

#[must_use]
pub fn font() -> Retained<NSFont> {
    STYLE.with(|(font, _)| font.clone())
}

fn attributes(color: Option<&NSColor>) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    STYLE.with(|(font, paragraph)| {
        // SAFETY: AppKit's own attribute names, read as the constants they are.
        let (font_key, color_key, paragraph_key) = unsafe {
            (
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            )
        };
        match color {
            Some(color) => NSDictionary::from_slices(
                &[font_key, color_key, paragraph_key],
                &[font.as_ref(), color.as_ref(), paragraph.as_ref()],
            ),
            None => NSDictionary::from_slices(
                &[font_key, paragraph_key],
                &[font.as_ref(), paragraph.as_ref()],
            ),
        }
    })
}

/// Draws one line of chrome text in `rect`, cut short with an ellipsis if it won't fit.
pub fn draw_text(text: &str, rect: NSRect, color: &NSColor) {
    // SAFETY: each attribute holds the type AppKit expects for its name.
    unsafe {
        NSString::from_str(text).drawInRect_withAttributes(rect, Some(&attributes(Some(color))));
    }
}

/// How tall one line of chrome text is as `draw_text` lays it out: the font's
/// line, not a guess, so a line can be centred in a row whatever its height.
#[must_use]
pub fn line_height() -> f64 {
    thread_local! {
        static HEIGHT: f64 = {
            let attributes = attributes(None);
            // SAFETY: each attribute holds the type AppKit expects for its name.
            unsafe { NSString::from_str("Ag").sizeWithAttributes(Some(&attributes)) }.height
        };
    }
    HEIGHT.with(|h| *h)
}

/// The rect one line of chrome text is drawn in so it sits centred in `rect`'s
/// height, `left` and `right` in from its ends. Every line of the chrome goes
/// through here, so text in a tab, a chip, the address bar and a suggestion
/// shares one baseline rule instead of each its own offset.
#[must_use]
pub fn text_line(rect: NSRect, left: f64, right: f64) -> NSRect {
    centred_line(rect, line_height(), left, right)
}

fn centred_line(rect: NSRect, line: f64, left: f64, right: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(
            rect.origin.x + left,
            rect.origin.y + (rect.size.height - line) / 2.0,
        ),
        NSSize::new((rect.size.width - left - right).max(0.0), line),
    )
}

/// An editing field's frame that shows its text where `draw_text` drew it in
/// `line`: the borderless field's cell keeps two points of air at either end and
/// none above, so the text doesn't jump when the field opens over a drawn line.
#[must_use]
pub fn field_frame(line: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(line.origin.x - FIELD_INSET, line.origin.y),
        NSSize::new(line.size.width + 2.0 * FIELD_INSET, line.size.height),
    )
}

/// The air a borderless `NSTextField`'s cell keeps at each end of its text.
const FIELD_INSET: f64 = 2.0;

/// How wide a line of chrome text is, in points.
#[must_use]
pub fn text_width(text: &str) -> f64 {
    // SAFETY: as above.
    unsafe { NSString::from_str(text).sizeWithAttributes(Some(&attributes(None))) }.width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_sits_centred_in_any_row() {
        let row = NSRect::new(NSPoint::new(100.0, 11.0), NSSize::new(186.0, 30.0));
        let line = centred_line(row, 16.0, 12.0, 30.0);
        assert!((line.origin.y - 18.0).abs() < 1e-9, "7 above and 7 below");
        assert!((line.origin.x - 112.0).abs() < 1e-9);
        assert!((line.size.width - 144.0).abs() < 1e-9);
        let narrow = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 28.0));
        assert!(centred_line(narrow, 16.0, 12.0, 12.0).size.width.abs() < 1e-9);
    }

    #[test]
    fn the_field_opens_on_the_drawn_line() {
        let line = NSRect::new(NSPoint::new(126.0, 11.0), NSSize::new(300.0, 16.0));
        let frame = field_frame(line);
        // Same top and height: the text doesn't move up or down as the field opens.
        assert!((frame.origin.y - line.origin.y).abs() < 1e-9);
        assert!((frame.size.height - line.size.height).abs() < 1e-9);
        // And the cell's own inset puts the first letter where the drawn one was.
        assert!((frame.origin.x + FIELD_INSET - line.origin.x).abs() < 1e-9);
    }
}
