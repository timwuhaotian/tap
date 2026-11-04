//! Downloads: a link, a response or a file WebKit can't show goes to disk instead
//! (the `WKDownloadDelegate` half of Search's `Browser.swift` and `Fetching.swift`).
//!
//! Into Downloads, under its own name, numbered if the name is taken; on a test run into
//! the test world's own `Downloads` folder, so a script never writes into yours. A
//! finished file bounces the Downloads stack in the Dock, as Safari's do.
//!
//! With Settings › Ask where to save each file, a save panel names each one instead, and
//! closing it cancels the download.
//!
//! Not yet: the list of downloads (⇧⌘J) and `downloads.json`, progress in the strip,
//! cancel and resume, and the PDF bar's own save button.

use std::{cell::RefCell, path::PathBuf, ptr};

use block2::DynBlock;
use objc2::{
    MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use objc2_foundation::{
    NSData, NSDistributedNotificationCenter, NSError, NSString, NSURL, NSURLResponse,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate};
use tap_core::{browser::TabId, fetching, identity};

use crate::store;

/// A download going: the download, where it is written, and the tab it came from.
type Going = (Retained<WKDownload>, Option<PathBuf>, Option<TabId>);

thread_local! {
    /// Every download going: WebKit holds them weakly, so they are kept here until they
    /// end, with where each is being written.
    static GOING: RefCell<Vec<Going>> = const { RefCell::new(Vec::new()) };
    /// Finished files, newest last (bench `downloads`).
    static DONE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    static DELEGATE: RefCell<Option<Retained<DownloadDelegate>>> = const { RefCell::new(None) };
}

/// Where a download is written: a free name in the folder, or wherever the save panel
/// says when Settings asks for it (Search's `whereToSave`). `None`: the panel was closed.
fn where_to_save(mtm: MainThreadMarker, name: &str) -> Option<PathBuf> {
    let asks = tap_core::settings_index::flag(&crate::prefs::Kept::new(), "downloads.ask");
    // A test run never waits on a panel nobody is there to answer.
    if !asks || testing() {
        return Some(fetching::free(name, &folder()));
    }
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(name));
    panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
        &folder().display().to_string(),
    ))));
    panel.setCanCreateDirectories(true);
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
}

fn testing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// Where downloads go: the folder Settings names (Downloads unless changed), or a test
/// world's own.
pub fn folder() -> PathBuf {
    if testing() {
        let folder = store::file("Downloads");
        let _ = std::fs::create_dir_all(&folder);
        return folder;
    }
    // Settings › General › Save to, else Downloads.
    crate::settings::downloads_folder()
}

/// Files finished so far, for the bench.
pub fn done() -> Vec<String> {
    DONE.with_borrow(|done| done.iter().map(|p| p.display().to_string()).collect())
}

/// Whether the tab is still sending a download to disk: it stays awake until it is done.
pub fn sending(tab: TabId) -> bool {
    GOING.with_borrow(|going| going.iter().any(|(_, _, from)| *from == Some(tab)))
}

/// How many are still going.
pub fn going() -> usize {
    GOING.with_borrow(Vec::len)
}

/// WebKit turned a navigation into a download: it is heard from, and kept, until it ends.
pub fn keep(mtm: MainThreadMarker, download: &WKDownload, tab: Option<TabId>) {
    let delegate = DELEGATE.with_borrow_mut(|slot| {
        slot.get_or_insert_with(|| DownloadDelegate::new(mtm))
            .clone()
    });
    // SAFETY: the delegate lives for the app's life in `DELEGATE`, longer than any download.
    unsafe { download.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    GOING.with_borrow_mut(|going| going.push((download.retain(), None, tab)));
}

fn same(a: &WKDownload, b: &WKDownload) -> bool {
    ptr::eq(a, b)
}

/// The download is over, one way or the other: let it go, and answer where it went.
fn release(download: &WKDownload) -> Option<PathBuf> {
    GOING.with_borrow_mut(|going| {
        let index = going.iter().position(|(d, _, _)| same(d, download))?;
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
            let name = fetching::file_name(&suggested.to_string());
            let Some(file) = where_to_save(self.mtm(), &name) else {
                // Cancelled in the panel: no file, so WebKit cancels the download.
                handler.call((ptr::null_mut(),));
                return;
            };
            GOING.with_borrow_mut(|going| {
                if let Some(entry) = going.iter_mut().find(|(d, _, _)| same(d, download)) {
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
