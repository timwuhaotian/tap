//! Tap updates itself: Kil's updater (`~/code/kil/crates/kil/src/update.rs`), ported
//! (docs/PLAN.md §12). It reads `releases/latest.toml` on `main`; when that names a newer
//! version, it downloads the DMG beside it, checks the checksum, the version, the
//! Developer ID team and Apple's notarization, and keeps the app from it until Tap quits.
//! Quitting then replaces the running bundle, so the next launch is the new version.
//! Restart to Update does the same at once and opens the new Tap.
//!
//! On its own five seconds after launch and every six hours after, while Settings ›
//! About › "Install updates on its own" (`update.install`) is on; Check for Updates…
//! any time. Once a newer Tap is staged, the update window opens on its own — asked
//! for or not — naming the version, showing that version's changelog entries (fetched
//! best-effort from `CHANGELOG.md` on `main`; a failure never blocks the update), and
//! offering Update & Restart or Later. Later does not ask again for that version in this
//! run: the staged copy still installs when Tap quits. A copy that isn't signed with
//! a Developer ID (a local build) never updates.
//! A test run may point it at another place with `TAP_UPDATE_BASE`.

use std::{
    cell::RefCell,
    ffi::OsStr,
    fs,
    io::Read as _,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use objc2::MainThreadMarker;
use objc2_app_kit::NSMenuItem;
use objc2_foundation::{NSBundle, NSString, NSTimer};
use tap_core::{identity, settings_index};

use crate::{prefs, settings, store, update_window};

/// Raw from `main`, where the release commit puts the DMG and this file together.
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/timwuhaotian/tap/main/releases/latest.toml";
const DMG_URL: &str = "https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg";
const CHANGELOG_URL: &str = "https://raw.githubusercontent.com/timwuhaotian/tap/main/CHANGELOG.md";
/// After launch, so the check never competes with reopening the session.
const FIRST_CHECK: f64 = 5.0;
/// One wakeup in hours keeps idle CPU at zero.
const CHECK_EVERY: f64 = 6.0 * 60.0 * 60.0;
/// How often a download's size is read, and the update window redraws its bar.
pub const POLL: Duration = Duration::from_millis(100);

/// What a check is doing, for the update window to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Checking,
    /// The DMG's bytes so far, and its size once the server has said it.
    Downloading {
        version: String,
        got: u64,
        total: Option<u64>,
    },
    Verifying {
        version: String,
    },
}

/// A check's current step: written by its thread, read by the update window.
#[derive(Debug, Default)]
pub struct Progress(Mutex<Step>);

impl Progress {
    pub fn step(&self) -> Step {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, step: Step) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = step;
    }
}

/// `releases/latest.toml`: the release's version and its DMG's SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub sha256: String,
}

impl Manifest {
    /// The two lines `scripts/release.sh` writes, `key = "value"`, and nothing else.
    ///
    /// # Errors
    /// If `text` isn't a manifest.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (mut version, mut sha256) = (None, None);
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("latest.toml: {line}"))?;
            let value = value
                .trim()
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .filter(|v| !v.contains('"'))
                .ok_or_else(|| format!("latest.toml: {line}"))?
                .to_owned();
            match key.trim() {
                "version" => version = Some(value),
                "sha256" => sha256 = Some(value),
                other => return Err(format!("latest.toml: unknown key {other}")),
            }
        }
        match (version, sha256) {
            (Some(version), Some(sha256)) => Ok(Manifest { version, sha256 }),
            _ => Err("latest.toml: version or sha256 is missing".to_owned()),
        }
    }
}

/// `major.minor.patch`, compared as numbers.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.trim().split('.').map(|part| part.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// Whether `candidate` is a later version than `current`; anything unreadable is not.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

/// The version this copy shows: its bundle's, which is what About and the release say.
pub fn current() -> String {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map_or_else(|| env!("CARGO_PKG_VERSION").to_owned(), |v| v.to_string())
}

/// The `.app` that `exe` runs in, if Tap may replace it: not a bare binary (`cargo run`),
/// not the copy inside the DMG, and not one Gatekeeper translocated to a read-only place.
fn replaceable_bundle(exe: &Path) -> Result<PathBuf, &'static str> {
    let bundle = exe
        .parent()
        .filter(|dir| dir.ends_with("Contents/MacOS"))
        .and_then(|dir| dir.parent()?.parent())
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Tap isn't running from Tap.app")?;
    let path = bundle.to_string_lossy();
    if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
        return Err("Tap is running from the disk image: drag it to Applications first");
    }
    Ok(bundle.to_owned())
}

/// Runs `program`; a failure is its stderr, else how it ended.
fn run(program: &str, args: &[&OsStr]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    Err(failure(program, output.status, &output.stderr))
}

fn failure(program: &str, status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let name = Path::new(program).file_name().unwrap_or_default().display();
    match stderr.trim() {
        "" => format!("{name}: {status}"),
        why => format!("{name}: {why}"),
    }
}

/// The body size the last response in a `curl -D` dump gives; redirects come before it.
fn content_length(headers: &str) -> Option<u64> {
    let last = headers
        .rsplit_once("\nHTTP/")
        .map_or(headers, |(_, last)| last);
    last.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .filter(|&size| size > 0)
}

/// Downloads `url` to `to`, telling `progress` how much has arrived, by the file's size
/// and the headers curl writes before the body.
fn download(url: &str, to: &Path, version: &str, progress: &Progress) -> Result<(), String> {
    const CURL: &str = "/usr/bin/curl";
    let headers = to.with_extension("headers");
    let mut curl = Command::new(CURL)
        .args(["-fsSL", "--max-time", "900", "-D"])
        .arg(&headers)
        .arg("-o")
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{CURL}: {err}"))?;
    let report = || {
        progress.set(Step::Downloading {
            version: version.to_owned(),
            got: fs::metadata(to).map_or(0, |file| file.len()),
            total: fs::read_to_string(&headers)
                .ok()
                .and_then(|headers| content_length(&headers)),
        });
    };
    let status = loop {
        report();
        match curl.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(POLL),
            Err(err) => return Err(format!("{CURL}: {err}")),
        }
    };
    report();
    let _ = fs::remove_file(&headers);
    if status.success() {
        return Ok(());
    }
    let mut stderr = Vec::new();
    if let Some(mut pipe) = curl.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    Err(failure(CURL, status, &stderr))
}

/// The Developer ID team that signed `app`; `None` for an ad-hoc signature (a local
/// build).
fn team_of(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|team| *team != "not set")
        .map(str::to_owned)
}

/// The version `app`'s Info.plist shows.
fn bundle_version(app: &Path) -> Result<String, String> {
    let plist = app.join("Contents/Info.plist");
    run(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print :CFBundleShortVersionString".as_ref(),
            plist.as_os_str(),
        ],
    )
}

/// Checks that `app` is Tap `version`, signed by `team` and notarized by Apple.
fn verify(app: &Path, version: &str, team: &str) -> Result<(), String> {
    let id = run(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print :CFBundleIdentifier".as_ref(),
            app.join("Contents/Info.plist").as_os_str(),
        ],
    )?;
    if id != identity::BUNDLE_ID {
        return Err(format!("the download is {id}, not Tap"));
    }
    let shown = bundle_version(app)?;
    if shown != version {
        return Err(format!("the download is Tap {shown}, not {version}"));
    }
    let requirement =
        format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            requirement.as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("the download isn't signed as Tap is ({err})"))?;
    run(
        "/usr/sbin/spctl",
        &[
            "--assess".as_ref(),
            "--type".as_ref(),
            "execute".as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("Apple hasn't notarized the download ({err})"))?;
    Ok(())
}

fn fetch_text(url: &str, max_time: &str) -> Result<String, String> {
    run(
        "/usr/bin/curl",
        &[
            "-fsSL".as_ref(),
            "--max-time".as_ref(),
            max_time.as_ref(),
            url.as_ref(),
        ],
    )
}

/// Reads the manifest at `url`.
fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    Manifest::parse(&fetch_text(url, "30")?)
}

/// The changelog's words for the versions between `current` and `latest`, newest
/// first, as plain text for the update window: Markdown's marks off, hard-wrapped
/// bullets joined back into one line. `Unreleased` and versions at or below
/// `current` are left out; an unreadable `current` falls back to `latest`'s own
/// section, so the window still says what is new.
pub fn notes_between(changelog: &str, current: &str, latest: &str) -> String {
    fn version_of(heading: &str) -> &str {
        heading.split_whitespace().next().unwrap_or_default()
    }
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.push((heading.trim(), Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    let mut picked: Vec<(&str, &Vec<&str>)> = sections
        .iter()
        .filter(|(heading, _)| {
            let version = version_of(heading);
            is_newer(version, current) && !is_newer(version, latest)
        })
        .map(|(heading, body)| (*heading, body))
        .collect();
    if picked.is_empty() {
        picked = sections
            .iter()
            .filter(|(heading, _)| version_of(heading) == latest)
            .map(|(heading, body)| (*heading, body))
            .collect();
    }
    let mut out: Vec<String> = Vec::new();
    for (heading, body) in picked {
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(format!("Tap {heading}"));
        let mut para: Option<String> = None;
        for raw in body {
            let line = raw.trim();
            if line.is_empty() {
                out.extend(para.take());
            } else if let Some(sub) = line.strip_prefix("### ") {
                out.extend(para.take());
                out.push(plain(sub.trim()));
            } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                out.extend(para.take());
                para = Some(format!("\u{2022} {}", plain(item.trim())));
            } else {
                match &mut para {
                    Some(para) => {
                        para.push(' ');
                        para.push_str(&plain(line));
                    }
                    None => para = Some(plain(line)),
                }
            }
        }
        out.extend(para);
    }
    out.join("\n")
}

/// Markdown as the words alone: links keep their text, bold and code marks go.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let Some(open) = rest.find('[') else {
            out.push_str(rest);
            break;
        };
        let linked = rest[open + 1..]
            .split_once("](")
            .and_then(|(words, after)| after.find(')').map(|close| (words, close)));
        if let Some((words, close)) = linked {
            out.push_str(&rest[..open]);
            out.push_str(words);
            rest = &rest[open + 1 + words.len() + 2 + close + 1..];
        } else {
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
        }
    }
    out.replace("**", "").replace('`', "")
}

/// The notes for a staged update: fetched best-effort, empty when unreachable, so
/// showing them never stands in the update's way.
fn fetch_notes(url: &str, current: &str, latest: &str) -> String {
    fetch_text(url, "10")
        .map(|text| notes_between(&text, current, latest))
        .unwrap_or_default()
}

/// Downloads the DMG at `dmg_url` into `dir`, checks it against `manifest` and `team`,
/// and returns the app copied out of it. An app staged earlier for the same version is
/// reused.
fn stage(
    manifest: &Manifest,
    dmg_url: &str,
    dir: &Path,
    team: &str,
    progress: &Progress,
) -> Result<PathBuf, String> {
    let version = &manifest.version;
    let verifying = || {
        progress.set(Step::Verifying {
            version: version.clone(),
        });
    };
    let app = dir.join("Tap.app");
    if app.exists() {
        verifying();
        if verify(&app, version, team).is_ok() {
            return Ok(app);
        }
    }
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let dmg = dir.join("Tap.dmg");
    download(dmg_url, &dmg, version, progress)?;
    verifying();
    let sum = run(
        "/usr/bin/shasum",
        &["-a".as_ref(), "256".as_ref(), dmg.as_os_str()],
    )?;
    if !sum
        .split_whitespace()
        .next()
        .is_some_and(|sum| sum.eq_ignore_ascii_case(&manifest.sha256))
    {
        return Err("the download doesn't match its checksum".to_owned());
    }
    let mount = dir.join("mnt");
    run(
        "/usr/bin/hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-quiet".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            dmg.as_os_str(),
        ],
    )?;
    let copied = run(
        "/usr/bin/ditto",
        &[mount.join("Tap.app").as_os_str(), app.as_os_str()],
    );
    let detach = |force: bool| {
        let mut args = vec!["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()];
        if force {
            args.push("-force".as_ref());
        }
        run("/usr/bin/hdiutil", &args)
    };
    if detach(false).is_err() {
        let _ = detach(true);
    }
    copied?;
    let _ = fs::remove_file(&dmg);
    verify(&app, version, team).inspect_err(|_| {
        let _ = fs::remove_dir_all(&app);
    })?;
    Ok(app)
}

/// Replaces `bundle` with `staged`: copied in beside it, then two renames, so `bundle` is
/// never missing or half-written, and a failure leaves the old app in place. The running
/// app keeps the code it loaded.
fn install(staged: &Path, bundle: &Path) -> Result<(), String> {
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return Err(format!("{}: not an app", bundle.display()));
    };
    let name = name.to_string_lossy();
    let new = parent.join(format!(".{name}.new"));
    let old = parent.join(format!(".{name}.old"));
    for leftover in [&new, &old] {
        let _ = fs::remove_dir_all(leftover);
    }
    run("/usr/bin/ditto", &[staged.as_os_str(), new.as_os_str()])?;
    if let Err(err) = fs::rename(bundle, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    if let Err(err) = fs::rename(&new, bundle) {
        let _ = fs::rename(&old, bundle);
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Whether Tap can create files in `dir`, which replacing the bundle needs.
fn writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".tap-update-{}", std::process::id()));
    fs::create_dir(&probe)
        .and_then(|()| fs::remove_dir(&probe))
        .map_err(|err| format!("Tap can't replace itself in {} ({err})", dir.display()))
}

fn testing() -> bool {
    std::env::var_os(identity::PROBE_ENV).is_some()
}

/// Where the manifest, the DMG and the changelog are: `main` on GitHub, or, on a
/// test run only, `TAP_UPDATE_BASE` (a folder holding `latest.toml` and
/// `Tap-arm64.dmg`, and `CHANGELOG.md` if its notes are wanted).
fn urls() -> (String, String, String) {
    match std::env::var("TAP_UPDATE_BASE").ok().filter(|_| testing()) {
        Some(base) => {
            let base = base.trim_end_matches('/');
            (
                format!("{base}/latest.toml"),
                format!("{base}/Tap-arm64.dmg"),
                format!("{base}/CHANGELOG.md"),
            )
        }
        None => (
            MANIFEST_URL.to_owned(),
            DMG_URL.to_owned(),
            CHANGELOG_URL.to_owned(),
        ),
    }
}

/// `~/Library/Caches/com.timwuhaotian.tap/update`, or a test world's own.
fn cache_dir() -> Result<PathBuf, String> {
    if testing() {
        return Ok(store::file("update"));
    }
    std::env::var_os("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join("Library/Caches")
                .join(identity::BUNDLE_ID)
                .join("update")
        })
        .ok_or_else(|| "no home directory".to_owned())
}

/// A newer Tap, checked and waiting for Tap to quit.
#[derive(Clone, Debug)]
struct Ready {
    version: String,
    /// The changelog's words for the versions this update crosses, possibly empty.
    notes: String,
    app: PathBuf,
    bundle: PathBuf,
}

enum Found {
    UpToDate,
    Ready(Ready),
}

/// One check, start to end; blocks, so it runs on a thread of its own.
fn find(current: &str, progress: &Progress) -> Result<Found, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let bundle = replaceable_bundle(&exe)?;
    let team = team_of(&bundle)
        .ok_or("this copy of Tap isn't signed with a Developer ID, so it doesn't update itself")?;
    let (manifest_url, dmg_url, changelog_url) = urls();
    let manifest = fetch_manifest(&manifest_url)?;
    if !is_newer(&manifest.version, current) {
        return Ok(Found::UpToDate);
    }
    writable(bundle.parent().unwrap_or(&bundle))?;
    let app = stage(&manifest, &dmg_url, &cache_dir()?, &team, progress)?;
    let notes = fetch_notes(&changelog_url, current, &manifest.version);
    Ok(Found::Ready(Ready {
        version: manifest.version,
        notes,
        app,
        bundle,
    }))
}

/// What the update window shows.
#[derive(Clone, Debug)]
pub enum Shown {
    Working(Arc<Progress>),
    Ready { version: String, notes: String },
    UpToDate,
    Failed(String),
}

#[derive(Default)]
struct Updater {
    /// The check under way.
    checking: Option<Arc<Progress>>,
    /// Someone chose Check for Updates, and is told the outcome.
    asked: bool,
    ready: Option<Ready>,
    /// Restart to Update: open the new app once this one has quit.
    reopen: bool,
    /// The Tap menu's item, retitled once an update is ready.
    item: Option<objc2::rc::Retained<NSMenuItem>>,
    timer: Option<objc2::rc::Retained<NSTimer>>,
    /// The last outcome, for Settings and the bench.
    said: Option<String>,
}

thread_local! {
    static UPDATER: RefCell<Updater> = RefCell::new(Updater::default());
}

/// Starts the checks: the first a few seconds after launch, then every six hours, each
/// only while Settings says to install updates on their own.
pub fn start(item: objc2::rc::Retained<NSMenuItem>) {
    UPDATER.with_borrow_mut(|updater| updater.item = Some(item));
    schedule(FIRST_CHECK);
}

fn schedule(after: f64) {
    let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        if after < CHECK_EVERY {
            schedule(CHECK_EVERY);
        }
        if settings_index::flag(&prefs::Kept::new(), "update.install") {
            check(false);
        }
    });
    // SAFETY: a timer on the main run loop, its block kept by the timer.
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(after, after >= CHECK_EVERY, &block)
    };
    timer.setTolerance(after / 10.0);
    UPDATER.with_borrow_mut(|updater| {
        if let Some(old) = updater.timer.replace(timer) {
            old.invalidate();
        }
    });
}

/// Check for Updates…: a check whose progress and outcome the update window shows.
pub fn check_now() {
    check(true);
}

/// The Tap menu's item: Check for Updates…, or Restart to Update once one is ready.
pub fn menu_title() -> String {
    UPDATER.with_borrow(|updater| match &updater.ready {
        Some(ready) => format!("Restart to Update to {}", ready.version),
        None => "Check for Updates…".to_owned(),
    })
}

/// The menu item chosen: a check, or the restart once an update is ready.
pub fn chosen() {
    if UPDATER.with_borrow(|updater| updater.ready.is_some()) {
        restart();
    } else {
        check_now();
    }
}

/// What Settings › About and the bench say about updates now.
pub fn status() -> serde_json::Value {
    UPDATER.with_borrow(|updater| {
        let step = updater.checking.as_ref().map(|p| p.step());
        serde_json::json!({
            "current": current(),
            "ready": updater.ready.as_ref().map(|r| r.version.clone()),
            "notes": updater.ready.as_ref().map(|r| r.notes.clone()),
            "checking": step.map(|s| format!("{s:?}")),
            "said": updater.said,
            "menu": updater.item.as_ref().map(|item| item.title().to_string()),
        })
    })
}

/// Restart to Update: quits without asking, installs, and opens the new Tap.
pub fn restart() {
    let ready = UPDATER.with_borrow_mut(|updater| {
        updater.reopen = updater.ready.is_some();
        updater.reopen
    });
    if ready && let Some(mtm) = MainThreadMarker::new() {
        crate::menu::quit_now(mtm);
    }
}

fn check(asked: bool) {
    // Already staged: this run has asked once (when it landed), and further silent
    // checks never ask again for the same version. An explicit check shows it.
    let ready = UPDATER.with_borrow(|updater| {
        updater
            .ready
            .as_ref()
            .map(|r| (r.version.clone(), r.notes.clone()))
    });
    if let Some((version, notes)) = ready {
        if asked {
            update_window::show(Shown::Ready { version, notes });
        }
        return;
    }
    let progress = UPDATER.with_borrow_mut(|updater| {
        updater.asked |= asked;
        updater.checking.clone()
    });
    let progress = progress.unwrap_or_else(begin);
    if asked {
        update_window::show(Shown::Working(progress));
    }
}

/// Starts a check on a thread of its own; [`checked`] gets its outcome on the main
/// thread.
fn begin() -> Arc<Progress> {
    let progress = Arc::new(Progress::default());
    UPDATER.with_borrow_mut(|updater| updater.checking = Some(progress.clone()));
    let current = current();
    let working = progress.clone();
    let _ = std::thread::Builder::new()
        .name("tap.update".into())
        .spawn(move || {
            let found = find(&current, &working);
            dispatch2::DispatchQueue::main().exec_async(move || checked(found));
        });
    progress
}

fn checked(found: Result<Found, String>) {
    let (asked, item, shown) = UPDATER.with_borrow_mut(|updater| {
        updater.checking = None;
        let asked = std::mem::take(&mut updater.asked);
        let shown = match found {
            Ok(Found::Ready(ready)) => {
                let (version, notes) = (ready.version.clone(), ready.notes.clone());
                updater.said = Some(format!("Tap {version} is ready"));
                updater.ready = Some(ready);
                Shown::Ready { version, notes }
            }
            Ok(Found::UpToDate) => {
                updater.said = Some("up to date".to_owned());
                Shown::UpToDate
            }
            Err(err) => {
                updater.said = Some(err.clone());
                Shown::Failed(err)
            }
        };
        (asked, updater.item.clone(), shown)
    });
    if let Some(item) = item {
        item.setTitle(&NSString::from_str(&menu_title()));
    }
    let summary = match &shown {
        Shown::Ready { version, .. } => format!("Tap {version} is ready"),
        Shown::UpToDate => "up to date".to_owned(),
        Shown::Failed(why) => why.clone(),
        Shown::Working(_) => "working".to_owned(),
    };
    crate::oslog::note(&format!("update check: {summary}"));
    // An open Settings › About page shows the same status as the menu item: it
    // is built lazily, so tell it the check landed.
    settings::changed();
    // A staged update asks on its own, whether or not anyone chose Check for
    // Updates…: that is the one outcome a silent check speaks up about.
    if asked || matches!(shown, Shown::Ready { .. }) {
        update_window::show(shown);
    } else {
        update_window::refresh(shown);
    }
}

/// When Tap quits: a staged update replaces the bundle, and Restart to Update opens it.
pub fn install_on_quit() {
    let (ready, reopen) = UPDATER.with_borrow(|updater| (updater.ready.clone(), updater.reopen));
    let Some(ready) = ready else {
        return;
    };
    // The bundle may have been replaced by hand since the update was staged (a
    // maintainer installing a release); a newer app there stays, and the stale download
    // goes.
    let on_disk = bundle_version(&ready.bundle).ok();
    let stale = on_disk
        .as_deref()
        .is_some_and(|on_disk| !is_newer(&ready.version, on_disk));
    if !stale && let Err(err) = install(&ready.app, &ready.bundle) {
        crate::oslog::note(&format!("installing Tap {}: {err}", ready.version));
        return;
    }
    if let Some(dir) = ready.app.parent() {
        let _ = fs::remove_dir_all(dir);
    }
    if reopen {
        open_after_exit(&ready.bundle);
    }
}

/// Opens `bundle` once this process has ended: a `/bin/sh` waits for the pid to go away,
/// then `open`s it. A test run's world goes with it.
fn open_after_exit(bundle: &Path) -> bool {
    let world = std::env::var(identity::PROBE_ENV).ok();
    let base = std::env::var("TAP_UPDATE_BASE").ok();
    let mut env = Vec::new();
    if let Some(world) = world {
        env.push(format!("--env={}={world}", identity::PROBE_ENV));
        if let Some(base) = base {
            env.push(format!("--env=TAP_UPDATE_BASE={base}"));
        }
    }
    let script = r#"pid="$1"; app="$2"; shift 2; while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$@" "$app""#;
    // Its own process group: nothing that ends Tap's takes the waiter with it.
    Command::new("/bin/sh")
        .args(["-c", script, "sh"])
        .arg(std::process::id().to_string())
        .arg(bundle)
        .args(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compares_numbers_not_text() {
        assert_eq!(
            [
                is_newer("0.0.10", "0.0.9"),
                is_newer("0.1.0", "0.0.99"),
                is_newer("0.0.5", "0.0.5"),
                is_newer("0.0.4", "0.0.5"),
                is_newer("0.0.x", "0.0.5"),
                is_newer("0.0.6.1", "0.0.5"),
            ],
            [true, true, false, false, false, false]
        );
    }

    #[test]
    fn the_manifest_is_two_lines_and_nothing_else() {
        assert_eq!(
            Manifest::parse("version = \"0.1.1\"\nsha256 = \"ab\"\n"),
            Ok(Manifest {
                version: "0.1.1".into(),
                sha256: "ab".into()
            })
        );
        assert!(Manifest::parse("version = \"0.1.1\"\nurl = \"x\"\nsha256 = \"ab\"").is_err());
        assert!(Manifest::parse("version = \"0.1.1\"").is_err());
        assert!(Manifest::parse("version = 0.1.1\nsha256 = \"ab\"").is_err());
    }

    #[test]
    fn notes_cover_every_version_in_between_and_no_others() {
        let log = "# Changelog\n\n## Unreleased\n\n- **Secret**\n\n## 0.3.0 \u{2014} 2026-11-08\n\n### Added\n\n- **New thing**: does [stuff](https://example.com)\n  over two lines.\n\n## 0.2.0 \u{2014} 2026-11-07\n\n- Middle `bit`\n\n## 0.1.0 \u{2014} 2026-11-06\n\n- Old\n";
        let notes = notes_between(log, "0.1.0", "0.3.0");
        assert!(notes.contains("Tap 0.3.0"), "{notes}");
        assert!(notes.contains("Added"), "{notes}");
        assert!(
            notes.contains("\u{2022} New thing: does stuff over two lines."),
            "{notes}"
        );
        assert!(notes.contains("Middle bit"), "{notes}");
        assert!(!notes.contains("Old"), "{notes}");
        assert!(!notes.contains("Secret"), "{notes}");
        // An unreadable current version still gets the latest version's own notes.
        let fallback = notes_between(log, "weird", "0.3.0");
        assert!(fallback.contains("New thing"), "{fallback}");
        assert!(!fallback.contains("Middle"), "{fallback}");
        assert_eq!(notes_between(log, "0.3.0", "0.4.0"), "");
    }

    #[test]
    fn only_an_installed_bundle_is_replaceable() {
        let bundle = |exe: &str| replaceable_bundle(Path::new(exe)).ok();
        assert_eq!(
            [
                bundle("/Applications/Tap.app/Contents/MacOS/tap"),
                bundle("/Users/me/code/tap/target/release/tap"),
                bundle("/Volumes/Tap/Tap.app/Contents/MacOS/tap"),
                bundle("/private/var/folders/x/AppTranslocation/1/d/Tap.app/Contents/MacOS/tap"),
            ],
            [
                Some(PathBuf::from("/Applications/Tap.app")),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn content_length_is_the_last_response_s() {
        let redirected = "HTTP/2 302 \r\ncontent-length: 0\r\n\r\nHTTP/2 200 \r\nContent-Length: 4570610\r\n\r\n";
        assert_eq!(
            [
                content_length(redirected),
                content_length("HTTP/2 302 \r\ncontent-length: 0\r\n\r\n"),
                content_length("Content-Length: 94\r\nAccept-ranges: bytes\r\n"),
            ],
            [Some(4_570_610), None, Some(94)]
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tap-update-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_default();
        dir
    }

    fn fake_app(at: &Path, version: &str) {
        let _ = fs::create_dir_all(at.join("Contents/MacOS"));
        let _ = fs::write(at.join("Contents/MacOS/tap"), version);
    }

    #[test]
    fn install_swaps_the_bundle_and_leaves_nothing_beside_it() {
        let dir = scratch("install");
        let (bundle, staged) = (dir.join("Apps/Tap.app"), dir.join("cache/Tap.app"));
        fake_app(&bundle, "old");
        fake_app(&staged, "new");
        assert!(install(&staged, &bundle).is_ok());
        let left: Vec<_> = fs::read_dir(dir.join("Apps"))
            .map(|d| d.filter_map(Result::ok).map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert_eq!(
            (
                fs::read_to_string(bundle.join("Contents/MacOS/tap")).ok(),
                left
            ),
            (
                Some("new".to_owned()),
                vec![std::ffi::OsString::from("Tap.app")]
            )
        );
        assert!(install(&dir.join("nowhere/Tap.app"), &bundle).is_err());
        assert_eq!(
            fs::read_to_string(bundle.join("Contents/MacOS/tap"))
                .ok()
                .as_deref(),
            Some("new"),
            "a failed install leaves the app as it was"
        );
    }

    /// The release committed on `main`, end to end, from the repository instead of
    /// GitHub: the manifest matches the DMG, and the app in it passes every check an
    /// update makes.
    #[test]
    fn the_committed_release_passes_the_checks_an_update_makes() {
        let releases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../releases");
        let Ok(text) = fs::read_to_string(releases.join("latest.toml")) else {
            return;
        };
        let manifest = Manifest::parse(&text).expect("the committed manifest parses");
        let dmg = fs::canonicalize(releases.join("Tap-arm64.dmg")).expect("the DMG is there");
        let dir = scratch("stage");
        let url = format!("file://{}", dmg.display());
        let progress = Progress::default();
        let staged = stage(&manifest, &url, &dir, "43VLF3KTFZ", &progress)
            .expect("the release passes every check");
        assert!(staged.join("Contents/MacOS/tap").is_file());
        assert!(verify(&staged, &manifest.version, "AAAAAAAAAA").is_err());
        assert!(verify(&staged, "9.9.9", "43VLF3KTFZ").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
