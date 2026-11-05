//! Site icons for the tabs (Search's `Icons.swift`, the app's side): asked of the page,
//! fetched, kept as a 32-pixel PNG in `icons/` beside the history, found again by site
//! and page through `tap_core::icons::Index` (Search's `icons/pages.json`).
//!
//! Only the raster kinds a tab needs are decoded in the app (PNG, ICO, JPEG, GIF, WebP),
//! each looked at before it is opened. An SVG icon is drawn in an isolated web view of
//! its own (`svg`, Search's `SVGIcons`) and kept the same way. Not yet: inline icons,
//! a second icon for the dark look; the letter shown before an icon arrives; icons in
//! the field's rows.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, ptr, time::Duration};

use block2::RcBlock;
use objc2::{AllocAnyThread, rc::Retained};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage,
};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLResponse,
    NSURLSession,
};
use objc2_web_kit::WKWebView;
use tap_core::{
    browser::TabId,
    fnd::Url,
    history,
    icons::{self, Declared, Index},
};

use crate::{rt, store, svg, web};

/// The side an icon is kept at, in pixels: crisp at 16 points on a Retina screen.
const SIDE: isize = 32;
const EDGE: f64 = 32.0;
/// An icon file bigger than this is not an icon.
const LARGEST: usize = 1_000_000;
/// How long a kept icon stands for the page (Search's `Favicons.fresh`): after this, a
/// finished page is asked again, in case the site has changed what it wears.
#[allow(clippy::duration_suboptimal_units)] // `Duration::from_days` is unstable
const FRESH: Duration = Duration::from_secs(7 * 86_400);

/// What the page declares, read in Tap's own world (Search's `Favicons.probe`).
const PROBE: &str = r"(() => {
  const out = [];
  for (const l of document.querySelectorAll('link[rel]')) {
    const rel = (l.getAttribute('rel') || '').toLowerCase();
    if (!rel.includes('icon')) continue;
    out.push({ href: l.href, rel, sizes: (l.getAttribute('sizes') || '').toLowerCase(),
               type: (l.getAttribute('type') || '').toLowerCase(), media: (l.getAttribute('media') || '').toLowerCase() });
  }
  return out;
})()";

thread_local! {
    static INDEX: RefCell<Option<Index>> = const { RefCell::new(None) };
    static IMAGES: RefCell<HashMap<String, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

fn folder() -> PathBuf {
    let folder = store::file("icons");
    let _ = std::fs::create_dir_all(&folder);
    folder
}

fn with_index<R>(f: impl FnOnce(&mut Index) -> R) -> R {
    INDEX.with_borrow_mut(|slot| {
        let index = slot.get_or_insert_with(|| {
            std::fs::read(folder().join("pages.json"))
                .ok()
                .and_then(|data| Index::load(&data))
                .unwrap_or_default()
        });
        f(index)
    })
}

/// The icon a page wears, if one is kept: the tab strip draws it.
pub fn image_for(address: &str) -> Option<Retained<NSImage>> {
    let url = Url::parse(address)?;
    let name = with_index(|index| index.lookup(&url, false))?;
    image_named(&name)
}

fn image_named(name: &str) -> Option<Retained<NSImage>> {
    cached_image(name, || {
        let path = folder().join(format!("{name}.png"));
        let image = NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&path.display().to_string()),
        )?;
        square_png(&image)?;
        Some(image)
    })
}

fn cached_image(
    name: &str,
    load: impl FnOnce() -> Option<Retained<NSImage>>,
) -> Option<Retained<NSImage>> {
    if let Some(image) = IMAGES.with_borrow(|images| images.get(name).cloned()) {
        return Some(image);
    }
    let image = load()?;
    IMAGES.with_borrow_mut(|images| images.insert(name.to_owned(), image.clone()));
    Some(image)
}

/// A page finished loading in the tab: it is asked what it declares, and the best icon
/// that comes back is kept.
pub fn fetch(tab: TabId, web: &WKWebView) {
    let Some(page) = web::url_of(web).and_then(|u| Url::parse(&u)) else {
        return;
    };
    if !page
        .scheme()
        .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
    {
        return;
    }
    // Fresh and right for this look: nothing to do (Search's `Icons.fetch`). Without
    // this every visit re-probed the page, re-downloaded and redrew its icon, and
    // rewrote the PNG and the whole index JSON on the main thread. Only the page's
    // own record counts: a neighbour's fresh icon is no proof this page was probed.
    if let Some(name) = with_index(|index| index.exact(&page, false))
        && std::fs::metadata(folder().join(format!("{name}.png")))
            .and_then(|kept| kept.modified())
            .is_ok_and(|kept| kept.elapsed().is_ok_and(|age| age < FRESH))
        && image_named(&name).is_some()
    {
        return;
    }
    web::evaluate_in_world(web, PROBE, move |value| {
        let declared: Vec<Declared> = value
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let candidates = icons::rank(&declared, &page, false);
        try_next(tab, page.clone(), candidates, 0);
    });
}

/// Fetches the candidates in order until one is an icon.
fn try_next(tab: TabId, page: Url, candidates: Vec<String>, at: usize) {
    let Some(address) = candidates.get(at).cloned() else {
        return;
    };
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&address)) else {
        return try_next(tab, page, candidates, at + 1);
    };
    let block = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, _error: *mut NSError| {
            // SAFETY: the session hands valid objects or null, for the call's length.
            let bytes = unsafe { data.as_ref() }
                .map(NSData::to_vec)
                .unwrap_or_default();
            // SAFETY: as above, for the response.
            let mime = unsafe { response.as_ref() }
                .and_then(NSURLResponse::MIMEType)
                .map(|mime| mime.to_string());
            let (page, candidates, address) = (page.clone(), candidates.clone(), address.clone());
            // Back on the main thread, where images and the model live.
            dispatch2::DispatchQueue::main().exec_async(move || {
                if keep(&page, &address, &bytes) {
                    rt::dispatch(move |browser| browser.icon_arrived(tab));
                    return;
                }
                // No raster in it, but an SVG by its mime or its first bytes:
                // drawn in the isolated view, kept the same way (Search's
                // `SVGIcons`). Anything else moves on to the next candidate.
                if !svg::looks_like_svg(&bytes, mime.as_deref()) {
                    try_next(tab, page, candidates, at + 1);
                    return;
                }
                // The outer block may run again, so the drawing takes its own.
                let (page, candidates, address) =
                    (page.clone(), candidates.clone(), address.clone());
                svg::draw(bytes, move |png| {
                    if png.as_deref().is_some_and(|png| keep(&page, &address, png)) {
                        rt::dispatch(move |browser| browser.icon_arrived(tab));
                    } else {
                        try_next(tab, page, candidates, at + 1);
                    }
                });
            });
        },
    );
    // SAFETY: a valid URL and a block that outlives the request.
    let task =
        unsafe { NSURLSession::sharedSession().dataTaskWithURL_completionHandler(&url, &block) };
    task.resume();
}

/// One of the kinds a tab's icon is: looked at before anything opens it.
fn raster(bytes: &[u8]) -> bool {
    bytes.len() <= LARGEST
        && (bytes.starts_with(b"\x89PNG")
            || bytes.starts_with(&[0, 0, 1, 0])
            || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
            || bytes.starts_with(b"GIF8")
            || (bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"))
}

/// Decodes, draws at 32 pixels, keeps the PNG and indexes it. Answers whether it was an
/// icon.
fn keep(page: &Url, address: &str, bytes: &[u8]) -> bool {
    if !raster(bytes) {
        return false;
    }
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes)) else {
        return false;
    };
    let Some(png) = square_png(&image) else {
        return false;
    };
    let Some(kept) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png)) else {
        return false;
    };
    let name = icons::ident(address);
    // The strip can draw immediately, without racing the asynchronous disk write.
    IMAGES.with_borrow_mut(|images| images.insert(name.clone(), kept));
    store::save(folder().join(format!("{name}.png")), png);
    let data = with_index(|index| {
        index.record(page, &name, false, history::now());
        index.encode()
    });
    store::save(folder().join("pages.json"), data);
    true
}

/// The image drawn into a 32-pixel square, kept to its proportions, as PNG.
fn square_png(image: &NSImage) -> Option<Vec<u8>> {
    // SAFETY: a fresh bitmap of the stated geometry, with no planes of its own (AppKit
    // allocates them); drawn into through its own graphics context, then encoded.
    unsafe {
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            SIDE,
            SIDE,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        let natural = image.size();
        let scale = (EDGE / natural.width.max(1.0)).min(EDGE / natural.height.max(1.0));
        let (w, h) = (natural.width * scale, natural.height * scale);
        image.drawInRect_fromRect_operation_fraction(
            NSRect::new(
                NSPoint::new((EDGE - w) / 2.0, (EDGE - h) / 2.0),
                NSSize::new(w, h),
            ),
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
        );
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let data = rep.bitmapData();
        let row = usize::try_from(rep.bytesPerRow()).ok()?;
        if data.is_null() || row < SIDE as usize * 4 {
            return None;
        }
        let pixels = std::slice::from_raw_parts(data, row * SIDE as usize);
        if !(0..SIDE as usize).any(|y| (0..SIDE as usize).any(|x| pixels[y * row + x * 4 + 3] != 0))
        {
            return None;
        }
        let png = rep
            .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(png.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_with_alpha(alpha: u8) -> Retained<NSImage> {
        // SAFETY: AppKit allocates the bitmap's planes with the stated dimensions.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), ptr::null_mut(), SIDE, SIDE, 8, 4, true, false,
                NSDeviceRGBColorSpace, 0, 0,
            )
        }.unwrap();
        let row = usize::try_from(rep.bytesPerRow()).unwrap();
        // SAFETY: the allocated bitmap has SIDE rows of row bytes.
        let pixels =
            unsafe { std::slice::from_raw_parts_mut(rep.bitmapData(), row * SIDE as usize) };
        pixels.fill(0);
        pixels[3] = alpha;
        // SAFETY: the bitmap is allocated and the PNG properties are empty.
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }
        .unwrap();
        NSImage::initWithData(NSImage::alloc(), &png).unwrap()
    }

    #[test]
    fn transparent_images_are_not_icons() {
        assert!(square_png(&image_with_alpha(0)).is_none());
        assert!(square_png(&image_with_alpha(255)).is_some());
    }

    #[test]
    fn a_cache_miss_does_not_hide_a_later_image() {
        let name = "test-cache-miss";
        assert!(cached_image(name, || None).is_none());
        assert!(cached_image(name, || Some(image_with_alpha(255))).is_some());
        assert!(cached_image(name, || None).is_some());
        IMAGES.with_borrow_mut(|images| images.remove(name));
    }
}
