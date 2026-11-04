//! Site icons for the tabs (Search's `Icons.swift`, the app's side): asked of the page,
//! fetched, kept as a 32-pixel PNG in `icons/` beside the history, found again by site
//! and page through `tap_core::icons::Index` (Search's `icons/pages.json`).
//!
//! Only the raster kinds a tab needs are decoded in the app (PNG, ICO, JPEG, GIF, WebP),
//! each looked at before it is opened. Not yet: SVG icons, which Search draws in a web
//! view of its own; a second icon for the dark look; the letter shown before an icon
//! arrives; icons in the field's rows.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, ptr};

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

use crate::{rt, store, web};

/// The side an icon is kept at, in pixels: crisp at 16 points on a Retina screen.
const SIDE: isize = 32;
const EDGE: f64 = 32.0;
/// An icon file bigger than this is not an icon.
const LARGEST: usize = 1_000_000;

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
    static IMAGES: RefCell<HashMap<String, Option<Retained<NSImage>>>> = RefCell::new(HashMap::new());
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
    IMAGES.with_borrow_mut(|images| {
        images
            .entry(name.clone())
            .or_insert_with(|| {
                let path = folder().join(format!("{name}.png"));
                NSImage::initWithContentsOfFile(
                    NSImage::alloc(),
                    &NSString::from_str(&path.display().to_string()),
                )
            })
            .clone()
    })
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
        move |data: *mut NSData, _response: *mut NSURLResponse, _error: *mut NSError| {
            // SAFETY: the session hands a valid object or null, for the call's length.
            let bytes = unsafe { data.as_ref() }
                .map(NSData::to_vec)
                .unwrap_or_default();
            let (page, candidates, address) = (page.clone(), candidates.clone(), address.clone());
            // Back on the main thread, where images and the model live.
            dispatch2::DispatchQueue::main().exec_async(move || {
                if !keep(&page, &address, &bytes) {
                    try_next(tab, page, candidates, at + 1);
                    return;
                }
                rt::dispatch(move |browser| browser.icon_arrived(tab));
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
    let name = icons::ident(address);
    if std::fs::write(folder().join(format!("{name}.png")), &png).is_err() {
        return false;
    }
    IMAGES.with_borrow_mut(|images| images.remove(&name));
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
        let png = rep
            .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(png.to_vec())
    }
}
