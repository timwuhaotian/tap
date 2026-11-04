//! Downloads: a link, a response or a file WebKit can't show goes to disk instead
//! (the `WKDownloadDelegate` half of Search's `Browser.swift` and `Fetching.swift`).
//!
//! Into Downloads, under its own name, numbered if the name is taken; on a test run into
//! the test world's own `Downloads` folder, so a script never writes into yours. A
//! finished file bounces the Downloads stack in the Dock, as Safari's do.
//!
//! Not yet: the list of downloads (⇧⌘J) and `downloads.json`, progress in the strip,
//! cancel and resume, asking where to save, and the PDF bar's own save button.

use std::{cell::RefCell, path::PathBuf, ptr};

use block2::DynBlock;
use objc2::{
    MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::{
    NSData, NSDistributedNotificationCenter, NSError, NSString, NSURL, NSURLResponse,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate};
use tap_core::{fetching, identity};

use crate::store;

thread_local! {
    /// Every download going: WebKit holds them weakly, so they are kept here until they
    /// end, with where each is being written.
    static GOING: RefCell<Vec<(Retained<WKDownload>, Option<PathBuf>)>> = const { RefCell::new(Vec::new()) };
    /// Finished files, newest last (bench `downloads`).
    static DONE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static DELEGATE: RefCell<Option<Retained<DownloadDelegate>>> = const { RefCell::new(None) };
}

fn testing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// Where downloads go: the user's Downloads, or a test world's own.
fn folder() -> PathBuf {
    if testing() {
        let folder = store::file("Downloads");
        let _ = std::fs::create_dir_all(&folder);
        return folder;
    }
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
        .join("Downloads")
}

/// Files finished so far, for the bench.
pub fn done() -> Vec<String> {
    DONE.with_borrow(|done| done.iter().map(|p| p.display().to_string()).collect())
}

/// How many are still going.
pub fn going() -> usize {
    GOING.with_borrow(Vec::len)
}

/// WebKit turned a navigation into a download: it is heard from, and kept, until it ends.
pub fn keep(mtm: MainThreadMarker, download: &WKDownload) {
    let delegate = DELEGATE.with_borrow_mut(|slot| {
        slot.get_or_insert_with(|| DownloadDelegate::new(mtm))
            .clone()
    });
    // SAFETY: the delegate lives for the app's life in `DELEGATE`, longer than any download.
    unsafe { download.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    GOING.with_borrow_mut(|going| going.push((download.retain(), None)));
}

fn same(a: &WKDownload, b: &WKDownload) -> bool {
    ptr::eq(a, b)
}

/// The download is over, one way or the other: let it go, and answer where it went.
fn release(download: &WKDownload) -> Option<PathBuf> {
    GOING.with_borrow_mut(|going| {
        let index = going.iter().position(|(d, _)| same(d, download))?;
        going.remove(index).1
    })
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadDelegate"]
    struct DownloadDelegate;

    unsafe impl NSObjectProtocol for DownloadDelegate {}

    unsafe impl WKDownloadDelegate for DownloadDelegate {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        fn destination(
            &self,
            download: &WKDownload,
            _response: &NSURLResponse,
            suggested: &NSString,
            handler: &DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            let file = fetching::free(&fetching::file_name(&suggested.to_string()), &folder());
            GOING.with_borrow_mut(|going| {
                if let Some(entry) = going.iter_mut().find(|(d, _)| same(d, download)) {
                    entry.1 = Some(file.clone());
                }
            });
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.display().to_string()));
            handler.call((Retained::as_ptr(&url).cast_mut(),));
        }

        #[unsafe(method(downloadDidFinish:))]
        fn finished(&self, download: &WKDownload) {
            let Some(file) = release(download) else {
                return;
            };
            if !testing() {
                // The Downloads stack in the Dock bounces, as it does for Safari's.
                let path = NSString::from_str(&file.display().to_string());
                // SAFETY: a notification name and its object, the file's path, as the
                // Dock reads them.
                unsafe {
                    NSDistributedNotificationCenter::defaultCenter().postNotificationName_object(
                        &NSString::from_str("com.apple.DownloadFileFinished"),
                        Some(&path),
                    );
                }
            }
            DONE.with_borrow_mut(|done| done.push(file));
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn failed(&self, download: &WKDownload, error: &NSError, _resume: Option<&NSData>) {
            let file = release(download);
            eprintln!(
                "A download failed ({}): {}",
                file.map(|f| f.display().to_string()).unwrap_or_default(),
                error.localizedDescription()
            );
        }
    }
);

impl DownloadDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}
