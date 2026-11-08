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
//! The strip's door says what is happening (Search's `FetchDoor`): a ring filling with
//! every running file's bytes together, turning while none says its size; the arrow down
//! a moment once the last one is in, a warning a moment when one failed. Each file also
//! carries its own bar in the Finder while it comes in.
//!
//! Not yet: the list of downloads (⇧⌘J) and `downloads.json`, pause, cancel and resume,
//! and the PDF bar's own save button. Until the list lands, the door opens the folder
//! downloads are saved in, and says a failure itself rather than in a row.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    ptr::{self, NonNull},
};

use block2::{DynBlock, RcBlock};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSBezierPath, NSButton, NSImage, NSLineCapStyle, NSModalResponseOK, NSResponder, NSSavePanel,
    NSView, NSWorkspace,
};
use objc2_foundation::{
    NSData, NSDictionary, NSDistributedNotificationCenter, NSError, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSPoint, NSProgress,
    NSProgressFileOperationKindDownloading, NSProgressKindFile, NSProgressReporting, NSRect,
    NSSize, NSString, NSTimer, NSURL, NSURLResponse,
};
use objc2_web_kit::{WKDownload, WKDownloadDelegate};
use tap_core::{browser::TabId, fetching, identity, settings_index};

use crate::{palette, store};

/// What the door says now.
#[derive(Clone, Debug, PartialEq)]
pub enum Door {
    /// Nothing going, nothing to say: the arrow at rest.
    Idle,
    /// Files coming in, together (Search's `Fetches.fraction`); `None` while any of
    /// them says no size, and the ring turns instead of filling.
    Going { fraction: Option<f64> },
    /// The last file in, for a moment (Search's `Fetches.linger`).
    Done { name: String },
    /// The last one failed, for a moment. Search says it in its panel; Tap has none
    /// yet, so the door says it instead.
    Failed { name: String, reason: String },
}

/// A download going: the download, where it is written, the tab it came from, and the
/// Finder's own bar for it, once its destination is known.
struct Going {
    download: Retained<WKDownload>,
    file: Option<PathBuf>,
    from: Option<TabId>,
    shown: Option<Retained<NSProgress>>,
}

/// How long the door says Done or Failed once nothing is going (Search's
/// `Fetches.linger`).
const LINGER_NS: i64 = 5_000_000_000;

thread_local! {
    /// Every download going: WebKit holds them weakly, so they are kept here until they
    /// end, with where each is being written.
    static GOING: RefCell<Vec<Going>> = const { RefCell::new(Vec::new()) };
    /// Finished files, newest last (bench `downloads`).
    static DONE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    /// How far the running ones are, together (`fetching::aggregate`); stale while none.
    static FRACTION: RefCell<Option<f64>> = const { RefCell::new(None) };
    /// What the door was last told (fraction, how many are going): KVO speaks far
    /// oftener than the ring's whole-percent steps move, and each saying had
    /// `sync_door` rebuild the door whole.
    static SAID: Cell<(Option<f64>, usize)> = const { Cell::new((None, 0)) };
    /// What the door says once nothing is going, until its round is over.
    static LINGER: RefCell<Option<Door>> = const { RefCell::new(None) };
    /// Which saying is the latest, so a late one lands nowhere.
    static ROUND: Cell<u64> = const { Cell::new(0) };
    static DELEGATE: RefCell<Option<Retained<DownloadDelegate>>> = const { RefCell::new(None) };
    static OBSERVER: RefCell<Option<Retained<DownloadObserver>>> = const { RefCell::new(None) };
    /// The ring's turn, while some download says no size.
    static SPIN: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    /// The door's symbol as last put up, so it is rebuilt only on change.
    static SYMBOL: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    /// The door's symbol is put away while the ring is over it.
    static BARE: Cell<bool> = const { Cell::new(false) };
}

/// Where a download is written: a free name in the folder, or wherever the save panel
/// says when Settings asks for it (Search's `whereToSave`). `None`: the panel was closed.
fn where_to_save(mtm: MainThreadMarker, name: &str) -> Option<PathBuf> {
    let asks = settings_index::flag(&crate::prefs::Kept::new(), "downloads.ask");
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

/// What the door says now: the running files together, else what lingered, else rest.
#[must_use]
pub fn door() -> Door {
    if going() > 0 {
        Door::Going {
            fraction: FRACTION.with_borrow(|fraction| *fraction),
        }
    } else {
        LINGER.with_borrow(|linger| linger.clone().unwrap_or(Door::Idle))
    }
}

/// Whether the strip's downloads door is in the row (Search's `FetchDoor.shows`):
/// always with Settings › Always show the downloads button, otherwise only while
/// a file is coming in or what one said still lingers.
#[must_use]
pub fn shows_button(always: bool) -> bool {
    always || going() > 0 || LINGER.with_borrow(Option::is_some)
}

/// How far the running downloads are, together, for the bench.
#[must_use]
pub fn fraction() -> Option<f64> {
    FRACTION.with_borrow(|fraction| *fraction)
}

/// The folder downloads are saved in, in the Finder: what the door opens, until
/// Tap has a downloads list of its own to open instead.
fn open_folder() {
    let path = folder().display().to_string();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
    NSWorkspace::sharedWorkspace().openURL(&url);
}

define_class!(
    /// The downloads door's target: the one click it takes.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadsDoor"]
    struct DoorTarget;

    unsafe impl NSObjectProtocol for DoorTarget {}

    impl DoorTarget {
        #[unsafe(method(open:))]
        fn open_action(&self, _sender: Option<&objc2::runtime::AnyObject>) {
            open_folder();
        }
    }
);

impl DoorTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// The door's target, kept for the app's life beside the delegates below.
    static DOOR: RefCell<Option<Retained<DoorTarget>>> = const { RefCell::new(None) };
}

/// The strip's downloads door (Search's `FetchDoor`): an arrow at rest, the ring over it
/// while files come in. The window shows and hides it (`window::refresh_fetch_door`);
/// a click opens the folder.
pub fn button(mtm: MainThreadMarker) -> Retained<NSButton> {
    let target =
        DOOR.with_borrow_mut(|slot| slot.get_or_insert_with(|| DoorTarget::new(mtm)).clone());
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("arrow.down.circle"),
        Some(&NSString::from_str("Downloads")),
    );
    // SAFETY: the target outlives the button (both live for the app's life in `DOOR`
    // and the window), and `open:` takes the sender, as an action must.
    let button = unsafe {
        NSButton::buttonWithImage_target_action(
            &image.unwrap_or_default(),
            Some(&target),
            Some(sel!(open:)),
            mtm,
        )
    };
    button.setBordered(false);
    button.setToolTip(Some(&NSString::from_str("Downloads")));
    button
}

/// The door says what `door()` says: the ring over it while files come in, the arrow
/// down a moment once they are in, a warning a moment when one failed.
pub fn sync_door(button: &NSButton, ring: &Ring) {
    let state = door();
    let always = settings_index::flag(&crate::prefs::Kept::new(), "downloads.button");
    button.setHidden(!shows_button(always));
    let symbol = match &state {
        Door::Idle | Door::Going { .. } => "arrow.down.circle",
        Door::Done { .. } => "arrow.down",
        Door::Failed { .. } => "exclamationmark.circle",
    };
    // The ring stands alone while files come in, as Search's does; the symbol is put
    // away and hung back up as it comes and goes.
    let bare = matches!(state, Door::Going { .. });
    if BARE.replace(bare) != bare {
        if bare {
            button.setImage(None);
        } else {
            SYMBOL.with_borrow_mut(|last| *last = None);
        }
    }
    if !bare {
        SYMBOL.with_borrow_mut(|last| {
            if *last != Some(symbol) {
                *last = Some(symbol);
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&NSString::from_str("Downloads")),
                ) {
                    button.setImage(Some(&image));
                }
            }
        });
    }
    let tip = match &state {
        Door::Idle => "Downloads — open the folder downloads are saved in".to_owned(),
        Door::Going { .. } => {
            format!(
                "Downloading {} — open the folder downloads are saved in",
                going()
            )
        }
        Door::Failed { name, reason } => format!("{name}: {reason}"),
        Door::Done { name } => {
            format!("Finished {name} — open the folder downloads are saved in")
        }
    };
    let tip = NSString::from_str(&tip);
    button.setToolTip(Some(&tip));
    ring.setToolTip(Some(&tip));
    ring.setHidden(!matches!(state, Door::Going { .. }));
    ring.setNeedsDisplay(true);
    sync_spin(ring);
}

/// The ring turns only while some download says no size; otherwise it fills, or rests.
fn sync_spin(ring: &Ring) {
    let turning = matches!(door(), Door::Going { fraction: None });
    SPIN.with_borrow_mut(|slot| {
        if !turning {
            if let Some(timer) = slot.take() {
                timer.invalidate();
            }
            return;
        }
        if slot.is_some() {
            return;
        }
        let held = ring.retain();
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            held.turn();
        });
        // SAFETY: a repeating timer on the main run loop; its block is kept by the
        // timer, and the timer is kept here until the ring rests again.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0 / 30.0, true, &block)
        };
        timer.setTolerance(1.0 / 120.0);
        *slot = Some(timer);
    });
}

/// Each running download's own bytes, together, and onto its Finder bar on the way
/// (Search's `Fetches.measure`).
fn measure() {
    let parts: Vec<(i64, i64)> = GOING.with_borrow_mut(|going| {
        going
            .iter_mut()
            .map(|entry| {
                let progress = entry.download.progress();
                let (done, size) = (progress.completedUnitCount(), progress.totalUnitCount());
                if let Some(shown) = &entry.shown {
                    if size > 0 && shown.totalUnitCount() != size {
                        shown.setTotalUnitCount(size);
                    }
                    shown.setCompletedUnitCount(done.max(0));
                }
                (done, size)
            })
            .collect()
    });
    let fraction = fetching::aggregate(&parts);
    FRACTION.with_borrow_mut(|slot| *slot = fraction);
    if SAID.get() != (fraction, parts.len()) {
        SAID.set((fraction, parts.len()));
        crate::window::downloads_moved();
    }
}

/// The door says `state` until a download starts or five seconds pass with none.
fn linger(state: Door) {
    let round = ROUND.with(|round| {
        round.set(round.get() + 1);
        round.get()
    });
    LINGER.with_borrow_mut(|slot| *slot = Some(state));
    let later = dispatch2::DispatchTime::NOW.time(LINGER_NS);
    let _ = dispatch2::DispatchQueue::main().after(later, move || {
        if ROUND.with(Cell::get) != round {
            return;
        }
        LINGER.with_borrow_mut(Option::take);
        // Nothing to measure any more: the last fraction goes with the saying.
        FRACTION.with_borrow_mut(|slot| *slot = None);
        // The door was told, so what it was told is what the next measure must
        // differ from: a download still going would otherwise say the same
        // fraction again and the ring would keep turning for it.
        SAID.set((None, going()));
        crate::window::downloads_moved();
    });
}

/// Files finished so far, for the bench.
pub fn done() -> Vec<String> {
    DONE.with_borrow(|done| done.iter().map(|p| p.display().to_string()).collect())
}

/// Whether the tab is still sending a download to disk: it stays awake until it is done.
pub fn sending(tab: TabId) -> bool {
    GOING.with_borrow(|going| going.iter().any(|entry| entry.from == Some(tab)))
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
    let observer = OBSERVER.with_borrow_mut(|slot| {
        slot.get_or_insert_with(|| DownloadObserver::new(mtm))
            .clone()
    });
    // SAFETY: the observer lives for the app's life in `OBSERVER`, and is removed from
    // this same progress in `release`, before the download is let go.
    unsafe {
        download.progress().addObserver_forKeyPath_options_context(
            &observer,
            &NSString::from_str("fractionCompleted"),
            NSKeyValueObservingOptions::New,
            ptr::null_mut(),
        );
    }
    GOING.with_borrow_mut(|going| {
        going.push(Going {
            download: download.retain(),
            file: None,
            from: tab,
            shown: None,
        });
    });
    // Something is coming in again: what lingered is said no more.
    ROUND.with(|round| round.set(round.get() + 1));
    LINGER.with_borrow_mut(Option::take);
    measure();
}

fn same(a: &WKDownload, b: &WKDownload) -> bool {
    ptr::eq(a, b)
}

/// The download is over, one way or the other: let it go, and answer where it went.
fn release(download: &WKDownload) -> Option<PathBuf> {
    let entry = GOING.with_borrow_mut(|going| {
        going
            .iter()
            .position(|entry| same(&entry.download, download))
            .map(|index| going.remove(index))
    })?;
    OBSERVER.with_borrow(|slot| {
        if let Some(observer) = slot.as_ref() {
            // SAFETY: added exactly once in `keep`, on this same progress.
            unsafe {
                entry
                    .download
                    .progress()
                    .removeObserver_forKeyPath(observer, &NSString::from_str("fractionCompleted"));
            }
        }
    });
    if let Some(shown) = entry.shown {
        shown.unpublish();
    }
    entry.file
}

define_class!(
    /// Hears every download's progress move, so the door's ring follows it.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadObserver"]
    struct DownloadObserver;

    unsafe impl NSObjectProtocol for DownloadObserver {}

    impl DownloadObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            key: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut std::ffi::c_void,
        ) {
            if key.is_some_and(|key| key.to_string() == "fractionCompleted") {
                measure();
            }
        }
    }
);

impl DownloadObserver {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
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
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.display().to_string()));
            GOING.with_borrow_mut(|going| {
                if let Some(entry) = going
                    .iter_mut()
                    .find(|entry| same(&entry.download, download))
                {
                    entry.file = Some(file.clone());
                    // The file's own bar in the Finder and the Dock's stack (Search's
                    // `Fetches.going`): published now its destination is known,
                    // unpublished as it ends. Not cancellable: Tap cannot stop a
                    // download yet.
                    let progress = download.progress();
                    let shown =
                        NSProgress::progressWithTotalUnitCount(progress.totalUnitCount().max(1));
                    // SAFETY: Apple's own immutable progress-kind constants.
                    unsafe {
                        shown.setKind(Some(NSProgressKindFile));
                        shown.setFileOperationKind(Some(NSProgressFileOperationKindDownloading));
                    }
                    shown.setFileURL(Some(&url));
                    shown.setCompletedUnitCount(progress.completedUnitCount().max(0));
                    shown.publish();
                    entry.shown = Some(shown);
                }
            });
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
            DONE.with_borrow_mut(|done| done.push(file.clone()));
            linger(Door::Done {
                name: file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            });
            measure();
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn failed(&self, download: &WKDownload, error: &NSError, _resume: Option<&NSData>) {
            // Closed in the save panel: no file was ever named, so there is nothing
            // to say, as Search's `destinationCancelled` has it.
            let Some(file) = release(download) else {
                measure();
                return;
            };
            let reason = {
                let text = error.localizedDescription().to_string();
                let clean = text.trim();
                if clean.is_empty() {
                    "The download failed for an unknown reason.".to_owned()
                } else {
                    clean.to_owned()
                }
            };
            eprintln!("A download failed ({}): {}", file.display(), reason);
            linger(Door::Failed {
                name: file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                reason,
            });
            measure();
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

/// The ring over the door while files come in (Search's `FetchDoor` circle and `Ring`):
/// filling with every running file's bytes together, turning while none says its size.
/// Seen, never pressed: clicks fall through to the door beneath.
pub struct RingIvars {
    angle: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadRing"]
    #[ivars = RingIvars]
    pub struct Ring;

    impl Ring {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            ptr::null_mut()
        }
    }
);

impl Ring {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RingIvars {
            angle: Cell::new(90.0),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setHidden(true);
        view
    }

    /// One step of the turn, about once round a second.
    fn turn(&self) {
        self.ivars().angle.set(self.ivars().angle.get() - 12.0);
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let Door::Going { fraction } = door() else {
            return;
        };
        let bounds = self.bounds();
        let center = NSPoint::new(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        // Search's ring is 12 points across on a 1.5-point line.
        let radius = 6.0 - 0.75;
        let dark = palette::drawing_dark();
        palette::MUTED
            .resolve(dark)
            .colorWithAlphaComponent(0.3)
            .set();
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(center.x - radius, center.y - radius),
            NSSize::new(radius * 2.0, radius * 2.0),
        ))
        .stroke();
        // From the top, clockwise: a share of the circle filled, or most of it
        // turning (Search's `strokeEnd` 0.78).
        let (start, end) = if let Some(fraction) = fraction {
            (90.0, 90.0 - 360.0 * fraction.max(0.02))
        } else {
            let angle = self.ivars().angle.get();
            (angle, angle - 360.0 * 0.78)
        };
        let path = NSBezierPath::bezierPath();
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            center, radius, start, end, true,
        );
        path.setLineWidth(1.5);
        path.setLineCapStyle(NSLineCapStyle::Round);
        palette::INK
            .resolve(dark)
            .colorWithAlphaComponent(0.7)
            .set();
        path.stroke();
    }
}

/// The ring over the door, made once and hung from it by the window.
pub fn ring(mtm: MainThreadMarker) -> Retained<Ring> {
    Ring::new(
        mtm,
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(26.0, 26.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_door_is_always_there_only_when_asked_and_otherwise_while_downloading() {
        assert!(shows_button(true));
        // Nothing going and nothing lingered in a test.
        assert!(!shows_button(false));
    }

    #[test]
    fn the_door_rests_in_a_test() {
        assert_eq!(door(), Door::Idle);
        assert_eq!(fraction(), None);
    }
}
