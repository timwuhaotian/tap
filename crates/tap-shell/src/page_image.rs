//! Copy Page as Image (the page's menu): the whole page, scrolled-out parts and all, as
//! one picture on the pasteboard.
//!
//! WebKit draws the entire document into a single-page PDF (`createPDF` with no rect);
//! that is drawn into a bitmap at the window's backing scale and put on the pasteboard
//! as PNG. Only the document's own scroll is unrolled: a box that scrolls inside the
//! page shows what it shows on screen.

use std::ptr;

use block2::RcBlock;
use objc2::{AnyThread, rc::Retained};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSCompositingOperation,
    NSDeviceRGBColorSpace, NSGraphicsContext, NSPDFImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSView,
};
use objc2_foundation::{NSData, NSDictionary, NSError, NSPoint, NSRect};
use objc2_web_kit::WKWebView;

/// The most pixels a picture is given (about 200 MB while it is drawn); a longer page
/// is drawn smaller to fit.
const MOST_PIXELS: f64 = 50_000_000.0;

/// The scale a page of `width`×`height` points is drawn at: the screen's, unless that
/// would pass the pixel budget.
fn fit_scale(width: f64, height: f64, backing: f64) -> f64 {
    let backing = if backing > 0.0 { backing } else { 1.0 };
    let area = width * height;
    if area <= 0.0 {
        return backing;
    }
    backing.min((MOST_PIXELS / area).sqrt())
}

/// Captures `web`'s whole page and puts it on the pasteboard; a beep when there was
/// nothing to capture.
pub fn copy(web: &WKWebView) {
    let view: &NSView = web.as_ref();
    let backing = view.window().map_or(2.0, |w| w.backingScaleFactor());
    let done = RcBlock::new(move |data: *mut NSData, _error: *mut NSError| {
        // SAFETY: WebKit hands either a live NSData or null, for the block's duration.
        let png = unsafe { data.as_ref() }.and_then(|pdf| png_of(pdf, backing));
        match png {
            Some(png) => {
                let board = NSPasteboard::generalPasteboard();
                board.clearContents();
                // SAFETY: the system's own pasteboard type constant.
                let kind = unsafe { NSPasteboardTypePNG };
                board.setData_forType(Some(&NSData::with_bytes(&png)), kind);
            }
            None => objc2_app_kit::NSBeep(),
        }
    });
    // SAFETY: no configuration captures the entire web content; the handler runs on
    // the main thread and takes nullable arguments.
    unsafe { web.createPDFWithConfiguration_completionHandler(None, &done) };
}

/// The one-page PDF drawn into a bitmap over white, as PNG.
fn png_of(pdf: &NSData, backing: f64) -> Option<Vec<u8>> {
    let page = NSPDFImageRep::imageRepWithData(pdf)?;
    let size = page.bounds().size;
    if size.width < 1.0 || size.height < 1.0 {
        return None;
    }
    let scale = fit_scale(size.width, size.height, backing);
    #[allow(clippy::cast_possible_truncation)] // bounded by MOST_PIXELS
    let (wide, high) = (
        (size.width * scale).round().max(1.0) as isize,
        (size.height * scale).round().max(1.0) as isize,
    );
    // SAFETY: a fresh bitmap of the stated geometry, with no planes of its own (AppKit
    // allocates them); drawn into through its own graphics context, then encoded.
    unsafe {
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            wide,
            high,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        // Points, so the PNG says its resolution and pastes at the page's size.
        rep.setSize(size);
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        let whole = NSRect::new(NSPoint::new(0.0, 0.0), size);
        NSColor::whiteColor().setFill();
        objc2_app_kit::NSRectFillUsingOperation(whole, NSCompositingOperation::Copy);
        let drawn = page.drawInRect(whole);
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        if !drawn {
            return None;
        }
        let png: Option<Retained<NSData>> = rep
            .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new());
        png.map(|data| data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_drawn_at_the_screen_s_scale_until_it_would_pass_the_pixel_budget() {
        assert!((fit_scale(1440.0, 5000.0, 2.0) - 2.0).abs() < f64::EPSILON);
        let long = fit_scale(1440.0, 100_000.0, 2.0);
        assert!(long < 2.0);
        assert!(1440.0 * long * 100_000.0 * long <= MOST_PIXELS + 1.0);
        assert!((fit_scale(800.0, 600.0, 0.0) - 1.0).abs() < f64::EPSILON);
    }
}
