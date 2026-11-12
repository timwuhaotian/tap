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
    path::{Path, PathBuf},
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
    NSBezierPath, NSButton, NSEvent, NSImage, NSLineCapStyle, NSModalResponseOK, NSResponder,
    NSSavePanel, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDateFormatter, NSDateFormatterStyle, NSDictionary,
    NSDistributedNotificationCenter, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSLocale, NSObjectNSKeyValueObserverRegistration, NSPoint, NSProgress,
    NSProgressFileOperationKindDownloading, NSProgressKindFile, NSProgressReporting, NSRect,
    NSSize, NSString, NSTimer, NSURL, NSURLResponse,
};
use objc2_quartz_core::CADisplayLink;
use objc2_web_kit::{WKDownload, WKDownloadDelegate};
use tap_core::{browser::TabId, fetching, history, identity, settings_index};

use crate::{
    cards,
    motion::{self, Curve, Frames, Glide},
    palette, store,
};

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

/// A download going: the download, where it is written, the tab it came from, the
/// Finder's own bar for it, once its destination is known, and the name it is going by.
struct Going {
    download: Retained<WKDownload>,
    file: Option<PathBuf>,
    from: Option<TabId>,
    shown: Option<Retained<NSProgress>>,
    /// The name it is going by, as WebKit says it.
    name: String,
}

/// A file that came in: where it is, and when it did.
#[derive(Clone, Debug)]
struct Done {
    /// The name it has on the disk.
    name: String,
    /// Where it was written.
    path: PathBuf,
    /// Seconds since 2001-01-01.
    at: f64,
    /// Why it did not come in, when it did not.
    error: Option<String>,
}

/// How long the door says Done or Failed once nothing is going (Search's
/// `Fetches.linger`).
const LINGER_NS: i64 = 5_000_000_000;

thread_local! {
    /// Every download going: WebKit holds them weakly, so they are kept here until they
    /// end, with where each is being written.
    static GOING: RefCell<Vec<Going>> = const { RefCell::new(Vec::new()) };
    /// Finished files, newest last (bench `downloads`).
    static DONE: RefCell<Vec<Done>> = const { RefCell::new(Vec::new()) };
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
            crate::window::toggle_downloads();
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
/// Under Reduce Motion it never turns: a fixed arc still says a file is coming.
fn sync_spin(ring: &Ring) {
    let turning = matches!(door(), Door::Going { fraction: None });
    SPIN.with_borrow_mut(|slot| {
        if !turning || motion::reduced() {
            if let Some(timer) = slot.take() {
                timer.invalidate();
            }
            ring.rest();
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
    // The card's rows follow every file's bytes.
    crate::window::downloads_moved();
    if SAID.get() != (fraction, parts.len()) {
        SAID.set((fraction, parts.len()));
        crate::window::downloads_said();
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
        crate::window::downloads_said();
    });
}

/// Files finished so far, for the bench.
pub fn done() -> Vec<String> {
    DONE.with_borrow(|done| done.iter().map(|d| d.path.display().to_string()).collect())
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
    // SAFETY: plain reads on a live download.
    let asked = unsafe { download.isUserInitiated() };
    GOING.with_borrow_mut(|going| {
        going.push(Going {
            download: download.retain(),
            file: None,
            from: tab,
            shown: None,
            name: name_of(download),
        });
    });
    // Something is coming in again: what lingered is said no more.
    ROUND.with(|round| round.set(round.get() + 1));
    LINGER.with_borrow_mut(Option::take);
    measure();
    // A file the person asked for flies from their pointer to the door.
    crate::window::download_started(asked);
}

/// The name a download goes by, before WebKit has named it: the last piece of the
/// address it came from.
fn name_of(download: &WKDownload) -> String {
    // SAFETY: plain reads on a live download.
    unsafe {
        download
            .originalRequest()
            .and_then(|request| request.URL())
            .and_then(|url| url.lastPathComponent().map(|part| part.to_string()))
    }
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| "Download".to_owned())
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
                    // WebKit has named it now.
                    entry.name.clone_from(&name);
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
            DONE.with_borrow_mut(|done| {
                done.push(Done {
                    name: file
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: file.clone(),
                    at: history::now(),
                    error: None,
                });
            });
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
            DONE.with_borrow_mut(|done| {
                done.push(Done {
                    name: file
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: file.clone(),
                    at: history::now(),
                    error: Some(reason.clone()),
                });
            });
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
    /// The door's bounce, as a file lands on it.
    bounce: Cell<f64>,
    /// The spring the bounce rides on.
    glide: RefCell<Glide>,
    /// The frames the bounce moves on.
    frames: Frames,
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

        /// One frame of the door's bounce.
        #[unsafe(method(ringFrame:))]
        fn ring_frame(&self, _link: &CADisplayLink) {
            let dt = self.ivars().frames.dt();
            let moving = self.ivars().glide.borrow_mut().step(dt);
            // The scale: the door pops out and springs back.
            let progress = self.ivars().glide.borrow().progress();
            self.ivars().bounce.set(1.0 - progress);
            self.setNeedsDisplay(true);
            if !moving {
                self.ivars().frames.pause();
                self.ivars().bounce.set(0.0);
            }
        }
    }
);

impl Ring {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RingIvars {
            angle: Cell::new(90.0),
            bounce: Cell::new(0.0),
            glide: RefCell::new(Glide::default()),
            frames: Frames::default(),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setHidden(true);
        view
    }

    /// The door takes the file that flew to it: it bounces.
    pub fn bounce(&self) {
        let curve = Curve::glide_now();
        if curve == Curve::Instant {
            return;
        }
        self.ivars().glide.borrow_mut().restart(curve);
        self.ivars().frames.run(self, sel!(ringFrame:));
    }

    /// One step of the turn, about once round a second.
    fn turn(&self) {
        self.ivars().angle.set(self.ivars().angle.get() - 12.0);
        self.setNeedsDisplay(true);
    }

    /// The ring at rest: a fixed arc, for Reduce Motion.
    fn rest(&self) {
        self.ivars().angle.set(90.0);
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
        let radius = (6.0 - 0.75) * (1.0 + 0.3 * self.ivars().bounce.get());
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

/// The door takes the file that flew to it: it bounces.
pub fn bounce(ring: &Ring) {
    ring.bounce();
}

// MARK: the card of what came down

/// The card's width.
pub const WIDTH: f64 = 340.0;
/// A running file's row: the name and the bytes, and the bar under them.
const GOING_ROW: f64 = 44.0;
/// A heading's row.
const HEADING_ROW: f64 = 26.0;
/// A file that came in: its name, and when it did.
const DONE_ROW: f64 = 28.0;
/// The card's foot.
const FOOT_ROW: f64 = 30.0;
/// How many files that are coming in the card lists.
const RUNNING: usize = 5;
/// How many files that came in the card lists.
const RECENT: usize = 8;

/// One row of the card.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// A file coming in: its name, and how many bytes have come of how many.
    Going { name: String, done: i64, size: i64 },
    /// A heading.
    Heading(&'static str),
    /// A file that came in: its name, when, where, and why it did not, if it did not.
    Done {
        name: String,
        path: PathBuf,
        at: f64,
        error: Option<String>,
    },
    /// The card's foot.
    Foot,
}

/// The card's rows as they are now: the files coming in, then the ones that came.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    GOING.with_borrow(|going| {
        for entry in going.iter().take(RUNNING) {
            let progress = entry.download.progress();
            rows.push(Row::Going {
                name: entry.name.clone(),
                done: progress.completedUnitCount().max(0),
                size: progress.totalUnitCount(),
            });
        }
    });
    let recent: Vec<Done> = DONE.with_borrow(|done| {
        done.iter()
            .rev()
            .take(RECENT)
            .cloned()
            .collect::<Vec<Done>>()
    });
    if !recent.is_empty() {
        rows.push(Row::Heading("Recent"));
        for file in recent {
            rows.push(Row::Done {
                name: file.name,
                path: file.path,
                at: file.at,
                error: file.error,
            });
        }
    }
    rows.push(Row::Foot);
    rows
}

/// How tall a row is.
fn row_height(row: &Row) -> f64 {
    match row {
        Row::Going { .. } => GOING_ROW,
        Row::Heading(_) => HEADING_ROW,
        Row::Done { .. } => DONE_ROW,
        Row::Foot => FOOT_ROW,
    }
}

/// The card's height, as its rows stand now.
#[must_use]
pub fn panel_height() -> f64 {
    let rows: f64 = rows().iter().map(row_height).sum();
    rows + 2.0 * cards::PAD + 2.0 * cards::SHADOW
}

/// What a click in the card does.
enum Hit {
    /// Stop a file that is coming in.
    Cancel(usize),
    /// Show a file that came in, in the Finder.
    Reveal(PathBuf),
    /// Open the folder the files land in.
    Folder,
}

/// The card's own state: how far its list is scrolled, and how far it has come in.
pub struct PanelIvars {
    state: cards::State,
}

define_class!(
    /// The card of what came down: the files coming in, with how far each has come,
    /// and the ones that came, newest first.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadsPanel"]
    #[ivars = PanelIvars]
    pub struct Panel;

    impl Panel {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// One frame of the card's coming in.
        #[unsafe(method(panelFrame:))]
        fn panel_frame(&self, _link: &CADisplayLink) {
            let dt = self.ivars().state.dt();
            if !self.ivars().state.tick(self, dt) {
                self.ivars().state.pause();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.press(event);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// Only the card takes clicks; its shadow's room lets them through to whatever
        /// is under it.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: a plain read of a live view's superview.
            let holder = unsafe { self.superview() };
            let local = self.convertPoint_fromView(point, holder.as_deref());
            let card = self.card();
            let inside = palette::inside(card, local);
            // SAFETY: the superclass's own hit test, for a point inside the card.
            inside.then(|| unsafe { msg_send![super(self), hitTest: point] }).flatten()
        }
    }
);

impl Panel {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PanelIvars {
            state: cards::State::default(),
        });
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// The card comes up under the door.
    pub fn show(&self) {
        self.ivars().state.show(self, sel!(panelFrame:));
    }

    /// The card goes away.
    pub fn hide(&self) {
        self.ivars().state.hide(self);
    }

    /// Whether a point in the window is on the card itself.
    pub fn holds(&self, point: NSPoint) -> bool {
        let rect = self.convertRect_toView(self.card(), None);
        palette::inside(rect, point)
    }

    /// The card's own ground, without the shadow's room around it.
    fn card(&self) -> NSRect {
        cards::State::card(self.bounds())
    }

    /// A click on the card: stop a file, show a file, or open the folder.
    fn press(&self, event: &NSEvent) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let Some(hit) = self.hit_at(point) else {
            return;
        };
        match hit {
            Hit::Cancel(index) => cancel(index),
            Hit::Reveal(path) => reveal(&path),
            Hit::Folder => open_folder(),
        }
    }

    /// What a point on the card would do.
    fn hit_at(&self, point: NSPoint) -> Option<Hit> {
        let card = self.card();
        let rows = rows();
        let mut y = card.origin.y + cards::PAD;
        for (index, row) in rows.iter().enumerate() {
            let height = row_height(row);
            let rect = NSRect::new(
                NSPoint::new(card.origin.x + cards::PAD, y),
                NSSize::new(card.size.width - 2.0 * cards::PAD, height),
            );
            y += height;
            if point.y < rect.origin.y || point.y >= rect.origin.y + rect.size.height {
                continue;
            }
            return match row {
                // The running files come first in the rows, so a row's number is the
                // running file's number.
                Row::Going { .. } => {
                    // The way to stop it: the cross at the row's end.
                    let cross = NSRect::new(
                        NSPoint::new(rect.origin.x + rect.size.width - 18.0, rect.origin.y + 2.0),
                        NSSize::new(16.0, 16.0),
                    );
                    palette::inside(cross, point).then_some(Hit::Cancel(index))
                }
                Row::Done { path, error, .. } => error.is_none().then(|| Hit::Reveal(path.clone())),
                Row::Foot => {
                    let label = "Open Folder";
                    let width = palette::text_width(label);
                    let open = NSRect::new(
                        NSPoint::new(rect.origin.x + rect.size.width - width, rect.origin.y),
                        NSSize::new(width, rect.size.height),
                    );
                    palette::inside(open, point).then_some(Hit::Folder)
                }
                Row::Heading(_) => None,
            };
        }
        None
    }

    fn draw(&self) {
        let dark = palette::is_dark(self);
        let rows = rows();
        let card = self.card();
        cards::ground(card, dark);
        // The rows are clipped to the card, so a row never draws over its rounded ends.
        let path = palette::card_path(card);
        path.addClip();
        let mut y = card.origin.y + cards::PAD;
        for row in &rows {
            let height = row_height(row);
            let rect = NSRect::new(
                NSPoint::new(card.origin.x + cards::PAD, y),
                NSSize::new(card.size.width - 2.0 * cards::PAD, height),
            );
            draw_row(row, rect, dark);
            y += height;
        }
    }
}

/// One row of the card: a file coming in, a heading, a file that came, or the foot.
fn draw_row(row: &Row, rect: NSRect, dark: bool) {
    match row {
        Row::Going { name, done, size } => {
            let line = NSRect::new(rect.origin, NSSize::new(rect.size.width, 20.0));
            if let Some(image) = symbol("arrow.down.circle") {
                palette::draw_image(
                    &image,
                    NSRect::new(
                        NSPoint::new(line.origin.x, line.origin.y + 2.0),
                        NSSize::new(16.0, 16.0),
                    ),
                );
            }
            // The way to stop it stands at the row's end, so the bytes stop short of it.
            let end = NSRect::new(
                line.origin,
                NSSize::new(line.size.width - 20.0, line.size.height),
            );
            let bytes = bytes_line(*done, *size);
            let width = palette::text_width(&bytes);
            palette::draw_text(
                name,
                palette::text_line(line, 22.0, width + 44.0),
                &palette::INK.resolve(dark),
            );
            palette::draw_text(
                &bytes,
                right_line(end, width),
                &palette::MUTED.resolve(dark),
            );
            // The way to stop it.
            if let Some(image) = symbol("xmark") {
                palette::draw_image(
                    &image,
                    NSRect::new(
                        NSPoint::new(rect.origin.x + rect.size.width - 16.0, line.origin.y + 2.0),
                        NSSize::new(14.0, 14.0),
                    ),
                );
            }
            bar(rect, fraction_of(*done, *size), dark);
        }
        Row::Heading(title) => {
            palette::draw_text(
                title,
                palette::text_line(rect, 0.0, 0.0),
                &palette::MUTED.resolve(dark),
            );
        }
        Row::Done {
            name, at, error, ..
        } => {
            let icon = if error.is_some() {
                "exclamationmark.circle"
            } else {
                "doc"
            };
            if let Some(image) = symbol(icon) {
                palette::draw_image(
                    &image,
                    NSRect::new(
                        NSPoint::new(rect.origin.x, rect.origin.y + 6.0),
                        NSSize::new(15.0, 15.0),
                    ),
                );
            }
            let detail = match error {
                Some(reason) => reason.clone(),
                None => ago(*at),
            };
            let width = palette::text_width(&detail);
            palette::draw_text(
                name,
                palette::text_line(rect, 22.0, width + 10.0),
                &palette::INK.resolve(dark),
            );
            palette::draw_text(
                &detail,
                right_line(rect, width),
                &palette::MUTED.resolve(dark),
            );
        }
        Row::Foot => {
            palette::draw_text(
                &format!("Files land in {}", folder_name()),
                palette::text_line(rect, 0.0, 90.0),
                &palette::MUTED.resolve(dark),
            );
            palette::draw_text(
                "Open Folder",
                right_line(rect, palette::text_width("Open Folder")),
                &palette::INK.resolve(dark),
            );
        }
    }
}

/// How far a running file has come, as a share of what is coming.
fn fraction_of(done: i64, size: i64) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)] // bytes
    (size > 0).then(|| done as f64 / size as f64)
}

/// The bar under a running file's name: how far it has come.
fn bar(rect: NSRect, fraction: Option<f64>, dark: bool) {
    let track = NSRect::new(
        NSPoint::new(rect.origin.x, rect.origin.y + rect.size.height - 8.0),
        NSSize::new(rect.size.width, 3.0),
    );
    palette::HAIRLINE.resolve(dark).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, 1.5, 1.5).fill();
    let Some(fraction) = fraction else {
        return;
    };
    let width = (track.size.width * fraction.clamp(0.0, 1.0)).max(2.0);
    palette::INK
        .resolve(dark)
        .colorWithAlphaComponent(0.7)
        .setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new(track.origin, NSSize::new(width, 3.0)),
        1.5,
        1.5,
    )
    .fill();
}

/// A line of text at the right end of a row.
fn right_line(rect: NSRect, width: f64) -> NSRect {
    let line = palette::text_line(rect, 0.0, 0.0);
    NSRect::new(
        NSPoint::new(line.origin.x + line.size.width - width, line.origin.y),
        NSSize::new(width, line.size.height),
    )
}

/// What a file's progress says: how much has come, and how much is coming.
fn bytes_line(done: i64, size: i64) -> String {
    let done = bytes(done);
    if size > 0 {
        format!("{done} of {}", bytes(size))
    } else {
        done
    }
}

/// A number of bytes, in English, as the Finder says it.
fn bytes(count: i64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    #[allow(clippy::cast_precision_loss)] // bytes
    let mut size = count as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} bytes")
    } else if size.fract() < 0.05 {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// When a file came in, as the Finder says it: "3 minutes ago" for today, "Yesterday"
/// for the day before, and the day itself for older.
fn ago(at: f64) -> String {
    let formatter = NSDateFormatter::new();
    formatter.setLocale(Some(&english()));
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
    formatter.setDoesRelativeDateFormatting(true);
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(at))
        .to_string()
}

/// The locale the chrome's own words are in.
fn english() -> Retained<NSLocale> {
    NSLocale::localeWithLocaleIdentifier(&NSString::from_str("en_US"))
}

/// The folder the files land in, by name.
fn folder_name() -> String {
    folder()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Stops a file that is coming in.
fn cancel(index: usize) {
    // The download is taken out of the list's borrow before it is stopped: WebKit
    // may answer at once, and the answer takes the list.
    let download = GOING.with_borrow(|going| going.get(index).map(|entry| entry.download.clone()));
    let Some(download) = download else {
        return;
    };
    // The file is let go of here: WebKit does not always say a download that was
    // stopped failed, and the door must not keep waiting for it.
    let _ = release(&download);
    let block = RcBlock::new(|_resume: *mut NSData| {});
    // SAFETY: a live download, and a block WebKit copies before it calls it back.
    unsafe { download.cancel(Some(&block)) };
    measure();
}

/// Shows a file that came in, in the Finder.
fn reveal(path: &Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.display().to_string()));
    let urls = NSArray::from_retained_slice(&[url]);
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
}

/// The card of what came down, made once and hung from the window.
pub fn panel(mtm: MainThreadMarker) -> Retained<Panel> {
    Panel::new(mtm)
}

// MARK: the file on its way to the door

/// The file's own state: where it is going from and to, and how far it has come.
pub struct FlightIvars {
    from: Cell<NSPoint>,
    to: Cell<NSPoint>,
    glide: RefCell<Glide>,
    frames: Frames,
}

define_class!(
    /// The file that flies from the pointer to the downloads door: it shrinks and
    /// fades on its way, and the door bounces when it lands.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapDownloadFlight"]
    #[ivars = FlightIvars]
    pub struct Flight;

    impl Flight {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// One frame of the file's flight.
        #[unsafe(method(flightFrame:))]
        fn flight_frame(&self, _link: &CADisplayLink) {
            let dt = self.ivars().frames.dt();
            let moving = self.ivars().glide.borrow_mut().step(dt);
            self.setNeedsDisplay(true);
            if !moving {
                self.ivars().frames.pause();
                self.setHidden(true);
                // The file has landed: the door takes it.
                crate::window::downloads_landed();
            }
        }

        /// The file is only a picture: it never takes a click.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

impl Flight {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FlightIvars {
            from: Cell::new(NSPoint::new(0.0, 0.0)),
            to: Cell::new(NSPoint::new(0.0, 0.0)),
            glide: RefCell::new(Glide::default()),
            frames: Frames::default(),
        });
        // SAFETY: NSView's designated initialiser.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        view.setHidden(true);
        view
    }

    fn draw(&self) {
        let dark = palette::is_dark(self);
        let progress = self.ivars().glide.borrow().progress().clamp(0.0, 1.0);
        let from = self.ivars().from.get();
        let to = self.ivars().to.get();
        let x = from.x + (to.x - from.x) * progress;
        let y = from.y + (to.y - from.y) * progress;
        let size = 26.0 * (1.0 - 0.45 * progress);
        let alpha = (1.0 - progress * progress).clamp(0.0, 1.0);
        // The file itself: a page with a folded corner.
        let width = size * 0.78;
        let fold = size * 0.3;
        let rect = NSRect::new(
            NSPoint::new(x - width / 2.0, y - size / 2.0),
            NSSize::new(width, size),
        );
        palette::INK
            .resolve(dark)
            .colorWithAlphaComponent(alpha)
            .setFill();
        let page = NSBezierPath::bezierPath();
        page.moveToPoint(rect.origin);
        page.lineToPoint(NSPoint::new(rect.origin.x + width - fold, rect.origin.y));
        page.lineToPoint(NSPoint::new(rect.origin.x + width, rect.origin.y + fold));
        page.lineToPoint(NSPoint::new(rect.origin.x + width, rect.origin.y + size));
        page.lineToPoint(NSPoint::new(rect.origin.x, rect.origin.y + size));
        page.closePath();
        page.fill();
        // The corner, folded over in the ground's own colour.
        palette::GROUND
            .resolve(dark)
            .colorWithAlphaComponent(alpha)
            .setFill();
        let corner = NSBezierPath::bezierPath();
        corner.moveToPoint(NSPoint::new(rect.origin.x + width - fold, rect.origin.y));
        corner.lineToPoint(NSPoint::new(rect.origin.x + width, rect.origin.y + fold));
        corner.lineToPoint(NSPoint::new(
            rect.origin.x + width - fold,
            rect.origin.y + fold,
        ));
        corner.closePath();
        corner.fill();
    }
}

/// The file flies from the pointer to the door, in the window's own coordinates.
pub fn fly_to(flight: &Flight, from: NSPoint, to: NSPoint) {
    let curve = Curve::glide_now();
    if curve == Curve::Instant {
        // Under Reduce Motion the file simply lands.
        crate::window::downloads_landed();
        return;
    }
    flight
        .ivars()
        .from
        .set(flight.convertPoint_fromView(from, None));
    flight
        .ivars()
        .to
        .set(flight.convertPoint_fromView(to, None));
    flight.ivars().glide.borrow_mut().restart(curve);
    flight.setAlphaValue(1.0);
    flight.setHidden(false);
    flight.setNeedsDisplay(true);
    flight.ivars().frames.run(flight, sel!(flightFrame:));
}

/// The file on its way, made once and laid over the window by it.
pub fn flight(mtm: MainThreadMarker) -> Retained<Flight> {
    Flight::new(mtm)
}

/// The symbols the card draws, made once each.
fn symbol(name: &'static str) -> Option<Retained<NSImage>> {
    thread_local! {
        static SYMBOLS: RefCell<std::collections::HashMap<&'static str, Option<Retained<NSImage>>>> =
            RefCell::new(std::collections::HashMap::new());
    }
    SYMBOLS.with_borrow_mut(|kept| {
        if let Some(image) = kept.get(name) {
            return image.clone();
        }
        let made = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        );
        kept.insert(name, made.clone());
        made
    })
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
