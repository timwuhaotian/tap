//! The address bar under the tabs (docs/PLAN.md "0.1.6"; Tap's own, Search has none:
//! its live tab says where the page is, `AtRest.swift`). It follows the tab in front:
//! back, forward and reload on its left (the window puts the helm here while the bar
//! is up), then the address, the host in ink and the rest dim, with a badge for the
//! connection, and the load's progress along its foot. A click on the address opens
//! the field there; a click on the badge says what the connection is and whose
//! certificate the site showed. Settings › Show the address bar hides it.

use std::ffi::c_void;

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject},
};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSCursor, NSEvent, NSFontWeightMedium, NSImage,
    NSImageSymbolConfiguration, NSMenu, NSMenuItem, NSResponder, NSView,
};
use objc2_foundation::{
    NSArray, NSDate, NSDateFormatter, NSDateFormatterStyle, NSPoint, NSRect, NSSize, NSString,
};
use serde_json::{Value, json};
use tap_core::{
    browser::{Browser, TabId},
    location::{self, Security},
    settings_index,
};

use crate::{palette, prefs, rt, web};

/// Where the address's capsule starts: after the helm's three doors.
const LEAD: f64 = palette::HELM + 6.0;
const CAPSULE: f64 = 28.0;
/// The badge's square, and where the address starts after it.
const BADGE: f64 = 24.0;
const TEXT: f64 = 30.0;

/// Whether Settings has the bar up.
#[must_use]
pub fn shown() -> bool {
    settings_index::flag(&prefs::Kept::new(), "toolbar.address")
}

/// Where the helm's doors stand on the bar: on its left, centred in its height.
#[must_use]
pub fn door_rect(index: usize) -> NSRect {
    #[allow(clippy::cast_precision_loss)] // three doors
    let x = 8.0 + index as f64 * (palette::DOOR + 2.0);
    NSRect::new(
        NSPoint::new(x, (palette::BAR - palette::DOOR) / 2.0),
        NSSize::new(palette::DOOR, palette::DOOR),
    )
}

fn capsule(width: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(LEAD, (palette::BAR - CAPSULE) / 2.0),
        NSSize::new((width - LEAD - 10.0).max(0.0), CAPSULE),
    )
}

fn badge_rect(capsule: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(capsule.origin.x + 4.0, capsule.origin.y + 2.0),
        NSSize::new(BADGE, BADGE),
    )
}

fn text_rect(capsule: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(capsule.origin.x + TEXT, capsule.origin.y + 5.5),
        NSSize::new((capsule.size.width - TEXT - 10.0).max(0.0), 17.0),
    )
}

/// What the bar shows, read from the model in one go.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    tab: Option<TabId>,
    address: String,
    security: Security,
    loading: bool,
    progress: f64,
    /// The field is open in the bar.
    editing: bool,
}

impl Shown {
    fn of(browser: &Browser) -> Shown {
        let tab = browser.active_tab();
        let address = tab.map(|t| t.url().to_owned()).unwrap_or_default();
        Shown {
            tab: tab.map(|t| t.id),
            security: location::security(&address, tab.is_some_and(tap_core::browser::Tab::secure)),
            address,
            loading: tab.is_some_and(tap_core::browser::Tab::loading),
            progress: tab.map_or(0.0, tap_core::browser::Tab::progress),
            editing: browser.editing().is_some_and(|e| !e.launching),
        }
    }
}

/// The badge's symbol for a connection, and whether it warns.
fn badge(security: Security) -> (&'static str, bool) {
    match security {
        Security::Secure => ("lock.fill", false),
        Security::Mixed => ("lock.slash", true),
        Security::NotSecure => ("exclamationmark.triangle.fill", true),
        Security::File => ("doc", false),
        Security::ThisMac => ("laptopcomputer", false),
        Security::Tap => ("gearshape", false),
        Security::Nothing => ("magnifyingglass", false),
    }
}

/// The site's certificate, as the badge's menu says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    pub subject: String,
    pub issuer: Option<String>,
    pub until: Option<String>,
}

/// What a click on the badge says: a heading and its lines (Chrome's and Safari's
/// page information, in Tap's words). `None` for a page with nothing to say.
#[must_use]
pub fn security_lines(
    security: Security,
    certificate: Option<&Certificate>,
) -> Option<(String, Vec<String>)> {
    let (heading, mut lines): (&str, Vec<String>) = match security {
        Security::Secure => (
            "Connection is secure",
            vec!["What you send this site is private on the way.".to_owned()],
        ),
        Security::Mixed => (
            "Parts of this page are not secure",
            vec!["Some images or scripts on it came over plain HTTP.".to_owned()],
        ),
        Security::NotSecure => (
            "Your connection to this site is not secure",
            vec!["Don't enter passwords or card details here.".to_owned()],
        ),
        Security::File => ("A file on this Mac", Vec::new()),
        Security::ThisMac => ("A page served from this Mac", Vec::new()),
        Security::Tap => ("One of Tap's own pages", Vec::new()),
        Security::Nothing => return None,
    };
    if matches!(security, Security::Secure | Security::Mixed)
        && let Some(certificate) = certificate
    {
        lines.push(format!("Certificate issued to {}", certificate.subject));
        if let Some(issuer) = &certificate.issuer {
            lines.push(format!("Issued by {issuer}"));
        }
        if let Some(until) = &certificate.until {
            lines.push(format!("Valid until {until}"));
        }
    }
    Some((heading.to_owned(), lines))
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    /// `CFArrayRef` of `SecCertificateRef`, leaf first; +1.
    fn SecTrustCopyCertificateChain(trust: *const c_void) -> *mut AnyObject;
    /// `CFStringRef`, +1.
    fn SecCertificateCopySubjectSummary(certificate: *const c_void) -> *mut AnyObject;
    /// `CFDateRef`, +1.
    fn SecCertificateCopyNotValidAfterDate(certificate: *const c_void) -> *mut AnyObject;
}

/// The certificate the tab's page showed, from WebKit's `serverTrust`.
fn certificate(tab: TabId) -> Option<Certificate> {
    let web = web::existing(tab)?;
    // SAFETY: a plain property read on a live web view.
    let trust = unsafe { web.serverTrust() }?;
    // SAFETY: a live trust object; the copy is ours (+1) and toll-free bridged to
    // NSArray, so `from_raw` takes that one reference over.
    let chain: Retained<NSArray<AnyObject>> = unsafe {
        Retained::from_raw(SecTrustCopyCertificateChain(Retained::as_ptr(&trust).cast()).cast())
    }?;
    let summary = |certificate: &AnyObject| -> Option<String> {
        // SAFETY: an element of the chain is a SecCertificateRef; the summary is a
        // +1 CFString, bridged to NSString.
        let text: Retained<NSString> = unsafe {
            Retained::from_raw(
                SecCertificateCopySubjectSummary(std::ptr::from_ref(certificate).cast()).cast(),
            )
        }?;
        Some(text.to_string())
    };
    let leaf = chain.firstObject()?;
    let issuer = (chain.count() > 1)
        .then(|| chain.objectAtIndex(1))
        .and_then(|c| summary(&c));
    // SAFETY: as above, a +1 CFDate bridged to NSDate.
    let until: Option<Retained<NSDate>> = unsafe {
        Retained::from_raw(
            SecCertificateCopyNotValidAfterDate(Retained::as_ptr(&leaf).cast()).cast(),
        )
    };
    let until = until.map(|date| {
        let formatter = NSDateFormatter::new();
        formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
        formatter.stringFromDate(&date).to_string()
    });
    Some(Certificate {
        subject: summary(&leaf)?,
        issuer,
        until,
    })
}

/// What the bar says now, for the bench.
#[must_use]
pub fn describe() -> Value {
    let Some(now) = rt::read(Shown::of) else {
        return json!({});
    };
    json!({
        "shown": shown(),
        "address": now.address,
        "security": format!("{:?}", now.security),
        "editing": now.editing,
        "says": security_lines(now.security, None).map(|(heading, _)| heading),
    })
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapAddressBar"]
    pub struct AddressBar;

    impl AddressBar {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let capsule = capsule(self.bounds().size.width);
            let text = NSRect::new(
                NSPoint::new(capsule.origin.x + TEXT, capsule.origin.y),
                NSSize::new((capsule.size.width - TEXT).max(0.0), capsule.size.height),
            );
            self.addCursorRect_cursor(text, &NSCursor::IBeamCursor());
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(event);
        }
    }
);

impl AddressBar {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Where the field sits while it is open in the bar: over the address.
    pub fn field_rect(&self) -> NSRect {
        let text = text_rect(capsule(self.bounds().size.width));
        NSRect::new(
            NSPoint::new(text.origin.x - 2.0, text.origin.y - 1.5),
            NSSize::new(text.size.width + 4.0, 20.0),
        )
    }

    fn press(&self, event: &NSEvent) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let capsule = capsule(self.bounds().size.width);
        let inside = |rect: NSRect| {
            point.x >= rect.origin.x
                && point.x < rect.origin.x + rect.size.width
                && point.y >= rect.origin.y
                && point.y < rect.origin.y + rect.size.height
        };
        if inside(badge_rect(capsule)) {
            self.security_menu(capsule);
        } else if inside(capsule) {
            rt::dispatch(|browser| {
                if browser.editing().is_none() {
                    browser.begin_editing(false);
                }
            });
        } else if let Some(window) = self.window() {
            // The bar's ends are the window's to move, as a toolbar's are.
            window.performWindowDragWithEvent(event);
        }
    }

    /// What the connection is, under the badge.
    fn security_menu(&self, capsule: NSRect) {
        let Some(shown) = rt::read(Shown::of) else {
            return;
        };
        let certificate = shown.tab.and_then(certificate);
        let Some((heading, lines)) = security_lines(shown.security, certificate.as_ref()) else {
            return;
        };
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.addItem(&NSMenuItem::sectionHeaderWithTitle(
            &NSString::from_str(&heading),
            mtm,
        ));
        for line in lines {
            // SAFETY: a fresh allocation, no action: a line to read.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&line),
                    None,
                    &NSString::from_str(""),
                )
            };
            // VoiceOver skips disabled rows, so these stay enabled with no action to run.
            item.setEnabled(true);
            menu.addItem(&item);
        }
        // Under the capsule, not the badge, so the menu clears the capsule's rounded foot.
        let at = NSPoint::new(
            capsule.origin.x + 4.0,
            capsule.origin.y + capsule.size.height + 4.0,
        );
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(self));
    }

    fn draw(&self) {
        let dark = palette::is_dark(self);
        let bounds = self.bounds();
        palette::GROUND.resolve(dark).setFill();
        NSBezierPath::fillRect(bounds);
        // The line between the bar and the page.
        palette::HAIRLINE.resolve(dark).setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0.0, bounds.size.height - 1.0),
            NSSize::new(bounds.size.width, 1.0),
        ));
        let Some(shown) = rt::read(Shown::of) else {
            return;
        };
        let capsule = capsule(bounds.size.width);
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            capsule,
            palette::RADIUS,
            palette::RADIUS,
        );
        if shown.editing {
            palette::GROUND.resolve(dark).setFill();
            path.fill();
            NSColor::controlAccentColor().setStroke();
            path.setLineWidth(1.5);
            path.stroke();
        } else {
            palette::WASH.resolve(dark).setFill();
            path.fill();
        }
        let (symbol, warns) = badge(shown.security);
        let tint = if warns {
            palette::UNSAFE.resolve(dark)
        } else {
            palette::MUTED.resolve(dark)
        };
        draw_symbol(symbol, badge_rect(capsule), &tint);
        if !shown.editing {
            draw_address(&shown, text_rect(capsule), dark);
        }
        if shown.loading && !shown.editing {
            // Along the capsule's foot, as far as the load has come.
            // From zero, so an early load shows nothing yet rather than a stub.
            let done = shown.progress.clamp(0.0, 1.0);
            if done > 0.0 {
                NSColor::controlAccentColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    NSRect::new(
                        NSPoint::new(
                            capsule.origin.x + palette::RADIUS,
                            capsule.origin.y + capsule.size.height - 2.5,
                        ),
                        NSSize::new((capsule.size.width - 2.0 * palette::RADIUS) * done, 2.0),
                    ),
                    1.0,
                    1.0,
                )
                .fill();
            }
        }
    }
}

/// The address in its runs: "Not secure" first where it must be said, then the
/// scheme dim, the host in ink, the rest dim, cut short at the capsule's end.
fn draw_address(shown: &Shown, rect: NSRect, dark: bool) {
    let muted = palette::MUTED.resolve(dark);
    if shown.address.is_empty() {
        palette::draw_text("Search or enter address", rect, &muted);
        return;
    }
    let mut x = rect.origin.x;
    let end = rect.origin.x + rect.size.width;
    let mut run = |text: &str, colour: &NSColor| {
        if text.is_empty() || x >= end {
            return;
        }
        let width = palette::text_width(text);
        palette::draw_text(
            text,
            NSRect::new(
                NSPoint::new(x, rect.origin.y),
                NSSize::new((end - x).max(0.0), rect.size.height),
            ),
            colour,
        );
        x += width;
    };
    if shown.security == Security::NotSecure {
        run("Not secure", &palette::UNSAFE.resolve(dark));
        run("  ", &muted);
    }
    let (lead, host, tail) = location::split(&shown.address);
    run(lead, &muted);
    run(host, &palette::INK.resolve(dark));
    run(tail, &muted);
}

/// An SF Symbol in `colour`, centred in `rect`.
fn draw_symbol(name: &str, rect: NSRect, colour: &NSColor) {
    let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    ) else {
        return;
    };
    // SAFETY: a valid point size and AppKit's own weight constant.
    let size = unsafe {
        NSImageSymbolConfiguration::configurationWithPointSize_weight(12.0, NSFontWeightMedium)
    };
    let config = size.configurationByApplyingConfiguration(
        &NSImageSymbolConfiguration::configurationWithHierarchicalColor(colour),
    );
    let Some(image) = image.imageWithSymbolConfiguration(&config) else {
        return;
    };
    let size = image.size();
    let square = NSRect::new(
        NSPoint::new(
            rect.origin.x + (rect.size.width - size.width) / 2.0,
            rect.origin.y + (rect.size.height - size.height) / 2.0,
        ),
        size,
    );
    // SAFETY: drawing a symbol image into the view's own context, no hints.
    unsafe {
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            square,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_warns_only_where_it_must() {
        assert_eq!(badge(Security::Secure), ("lock.fill", false));
        assert!(badge(Security::Mixed).1 && badge(Security::NotSecure).1);
        assert!(!badge(Security::ThisMac).1 && !badge(Security::File).1);
    }

    #[test]
    fn the_badge_menu_says_the_connection_and_the_certificate() {
        let certificate = Certificate {
            subject: "example.com".to_owned(),
            issuer: Some("R11".to_owned()),
            until: Some("Jan 1, 2027".to_owned()),
        };
        let (heading, lines) =
            security_lines(Security::Secure, Some(&certificate)).expect("something to say");
        assert_eq!(heading, "Connection is secure");
        assert_eq!(
            lines[1..],
            [
                "Certificate issued to example.com",
                "Issued by R11",
                "Valid until Jan 1, 2027"
            ]
        );
        let (heading, lines) =
            security_lines(Security::NotSecure, Some(&certificate)).expect("a warning");
        assert!(heading.contains("not secure"));
        assert_eq!(lines.len(), 1, "no certificate on plain http");
        assert!(security_lines(Security::Nothing, None).is_none());
    }
}
