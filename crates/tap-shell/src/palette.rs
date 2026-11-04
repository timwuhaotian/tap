//! Search's colours and measures (`Design.swift`). Every colour is a pair, one for a light
//! window and one for a dark, resolved against whatever appearance the view has; nothing
//! else in the code knows which it is.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSColor, NSView,
};
use objc2_foundation::NSArray;

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

/// Whether a view is drawn dark right now.
#[must_use]
pub fn is_dark(view: &NSView) -> bool {
    is_dark_appearance(&view.effectiveAppearance())
}
