//! What a Chromium browser on this Mac holds (Search's `Chromium` in `Import.swift`):
//! Chrome, Arc, Brave, Edge, Dia and the rest keep their profiles the same way, one
//! folder each under `~/Library/Application Support`, and Tap reads from the profile used
//! last, one you pick, or all of them ([`Pick`]): the open tabs and tab groups (the
//! session file, [`crate::snss`], Search #841), the history (the `History` database), the
//! bookmarks (one JSON file) and the passwords (`Login Data`). What it reads comes out as
//! a [`profile::Incoming`], so it is merged exactly as a profile file is; the passwords
//! come out as [`passwords::Login`]s for the keychain.
//!
//! Every Chromium browser keeps its passwords the same way: a SQLite file, each password
//! encrypted with a key the browser keeps in the macOS keychain under "<Name> Safe
//! Storage". macOS asks you before handing that key to anyone else, which is the one
//! thing here you have to say yes to, and the shell's to ask. After that it is
//! arithmetic: the key is stretched the way Chromium stretches it, and each password is
//! unwrapped. Counting them needs no key.
//!
//! Nothing in the other browser changes. Its databases are copied before they are read:
//! it is usually running, and reading its live file underneath it is how you get a lock
//! error. A link inside its folder can't lead a read out of it.

use std::{
    collections::HashMap,
    io::Read as _,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::{Value, json};
use tap_core::{
    fnd,
    session::{Entry, Shape, TabGroup},
};

use crate::{
    passwords::{self, Login},
    profile::Incoming,
    snss,
};

/// A browser this reads, by its folder under `~/Library/Application Support`.
#[derive(Debug, PartialEq, Eq)]
pub struct Browser {
    pub name: &'static str,
    folder: &'static str,
    /// Where its passwords' key is in the keychain: the generic password's service and
    /// account, which macOS asks you about before handing it over. None for a browser
    /// whose passwords Search can't read; its other things still come.
    key: Option<(&'static str, &'static str)>,
}

/// Search's list, in Search's order. Chrome's other channels keep their own data, and
/// share Chrome's key.
pub const KNOWN: &[Browser] = &[
    Browser {
        name: "Dia",
        folder: "Dia/User Data",
        key: Some(("Dia Safe Storage", "Dia")),
    },
    Browser {
        name: "Chrome",
        folder: "Google/Chrome",
        key: Some(("Chrome Safe Storage", "Chrome")),
    },
    Browser {
        name: "ego lite",
        folder: "Citro Labs/ego lite",
        key: None,
    },
    Browser {
        name: "Arc",
        folder: "Arc/User Data",
        key: Some(("Arc Safe Storage", "Arc")),
    },
    Browser {
        name: "Brave",
        folder: "BraveSoftware/Brave-Browser",
        key: Some(("Brave Safe Storage", "Brave")),
    },
    Browser {
        name: "Edge",
        folder: "Microsoft Edge",
        key: Some(("Microsoft Edge Safe Storage", "Microsoft Edge")),
    },
    Browser {
        name: "Vivaldi",
        folder: "Vivaldi",
        key: Some(("Vivaldi Safe Storage", "Vivaldi")),
    },
    Browser {
        name: "Chromium",
        folder: "Chromium",
        key: Some(("Chromium Safe Storage", "Chromium")),
    },
    Browser {
        name: "Chrome Beta",
        folder: "Google/Chrome Beta",
        key: Some(("Chrome Safe Storage", "Chrome")),
    },
    Browser {
        name: "Chrome Dev",
        folder: "Google/Chrome Dev",
        key: Some(("Chrome Safe Storage", "Chrome")),
    },
    Browser {
        name: "Chrome Canary",
        folder: "Google/Chrome Canary",
        key: Some(("Chrome Safe Storage", "Chrome")),
    },
    // Opera keeps its one profile in its own folder, not in one inside it.
    Browser {
        name: "Opera",
        folder: "com.operasoftware.Opera",
        key: Some(("Opera Safe Storage", "Opera")),
    },
    Browser {
        name: "Opera GX",
        folder: "com.operasoftware.OperaGX",
        key: Some(("Opera Safe Storage", "Opera")),
    },
    Browser {
        name: "Helium",
        folder: "net.imput.helium",
        key: Some(("Helium Storage Key", "Helium")),
    },
    Browser {
        name: "Comet",
        folder: "Comet",
        key: Some(("Comet Safe Storage", "Comet")),
    },
];

/// The files that make a folder a profile with something to bring.
const HOLDS: &[&str] = &[
    "Bookmarks",
    "History",
    "Sessions",
    "Current Session",
    "Login Data",
];
/// A profile's passwords: its own, and, beside them, those Chrome saves for the Google
/// account it is signed in to without sync — same shape, same key. Read alone, the first
/// can be nearly empty.
const LOGINS: &[&str] = &["Login Data", "Login Data For Account"];
/// The most of the history brought: what it takes to finish an address on the first day.
const PLACES: i64 = 3000;
/// The most a JSON or session file read may be.
const MOST: u64 = 64 << 20;
/// Microseconds from 1601-01-01 (Chromium's clock) to 1970, and seconds from 1970 to
/// 2001-01-01 (the reference date history keeps).
const CHROMIUM_EPOCH: f64 = 11_644_473_600.0;
const APPLE_EPOCH: f64 = 978_307_200.0;

impl Browser {
    #[must_use]
    pub fn root(&self, base: &Path) -> PathBuf {
        base.join(self.folder)
    }

    /// The keychain item holding its passwords' key: (service, account). None for a
    /// browser whose passwords can't be read here.
    #[must_use]
    pub fn safe_storage(&self) -> Option<(&'static str, &'static str)> {
        self.key
    }
}

/// One of a browser's profiles, as a picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The folder's name ("Default", "Profile 1"), which is what the readers go by.
    pub folder: String,
    /// What the browser calls it ("Work", "Person 1"), or the folder's own name when it
    /// says nothing; the browser's own name for Opera's, which is the browser's folder.
    pub name: String,
}

/// Which of a browser's profiles to read (Search's `profile: String?`, with the usual one
/// made explicit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    /// The one used last ([`profile_list`]'s second half).
    Usual,
    /// The one in this folder.
    One(&'a str),
    /// Every profile, as Search's "All profiles".
    All,
}

/// What one browser's profile has to bring.
pub struct Found {
    pub incoming: Incoming,
    /// The profile it came from, named as the browser names it, when there is more than
    /// one to come from.
    pub profile: Option<String>,
}

/// Every profile under `root` with something to bring: the folders inside it holding a
/// browser file, or `root` itself (Opera). A browser's own Guest and System profiles
/// aren't anybody's.
#[must_use]
pub fn profiles(root: &Path) -> Vec<PathBuf> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut inside: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .filter(|p| {
                    !p.file_name()
                        .is_some_and(|n| n == "Guest Profile" || n == "System Profile")
                })
                .collect()
        })
        .unwrap_or_default();
    inside.sort();
    std::iter::once(root.clone())
        .chain(inside)
        .filter(|folder| {
            HOLDS
                .iter()
                .any(|name| file(&root, &folder.join(name)).is_some())
        })
        .collect()
}

/// `path`, if it is a file or folder inside `root` that no link leads out of.
fn file(root: &Path, path: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    let real = path.canonicalize().ok()?;
    real.starts_with(root).then_some(real)
}

/// A regular file's bytes, at most [`MOST`] of them.
fn read_capped(root: &Path, path: &Path) -> Option<Vec<u8>> {
    let path = file(root, path)?;
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut data = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MOST + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= MOST).then_some(data)
}

/// What the browser says of its profiles in `Local State`: the name each folder goes by
/// ("Work", "Person 1"), and the folders used last, newest first.
fn local_state(root: &Path) -> (HashMap<String, String>, Vec<String>) {
    let top: Value = read_capped(root, &root.join("Local State"))
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    let profile = &top["profile"];
    let names = profile["info_cache"]
        .as_object()
        .map(|cache| {
            cache
                .iter()
                .filter_map(|(folder, info)| {
                    let name = info["name"].as_str().filter(|n| !n.is_empty())?;
                    Some((folder.clone(), name.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    let last = profile["last_used"]
        .as_str()
        .into_iter()
        .chain(
            profile["last_active_profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .map(str::to_owned)
        .collect();
    (names, last)
}

/// The profile brought in (Search's `usual`): the one the browser says it used last, or
/// else the one whose files changed last.
fn usual(root: &Path, profiles: &[PathBuf], last: &[String]) -> Option<PathBuf> {
    let named = |name: &str| {
        profiles
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n == name))
    };
    if let Some(found) = last.iter().find_map(|name| named(name)) {
        return Some(found.clone());
    }
    let touched = |folder: &Path| -> SystemTime {
        HOLDS
            .iter()
            .filter_map(|name| file(root, &folder.join(name)))
            .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok())
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    profiles.iter().max_by_key(|p| touched(p)).cloned()
}

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What the browser calls the profile in `folder`.
fn profile_name(
    browser: &Browser,
    root: &Path,
    names: &HashMap<String, String>,
    folder: &Path,
) -> String {
    let id = folder_name(folder);
    names.get(&id).cloned().unwrap_or_else(|| {
        if folder == root {
            browser.name.to_owned()
        } else {
            id
        }
    })
}

/// Every profile `browser` has here, by the name it gives each, in the order a person
/// would count them ("Profile 2" before "Profile 10"), and the folder of the one used
/// last, which is the one brought unless another is picked.
#[must_use]
pub fn profile_list(browser: &Browser, base: &Path) -> (Vec<Profile>, Option<String>) {
    let Ok(root) = browser.root(base).canonicalize() else {
        return (Vec::new(), None);
    };
    let found = profiles(&root);
    let (names, last) = local_state(&root);
    let usual = usual(&root, &found, &last).map(|p| folder_name(&p));
    let mut out: Vec<Profile> = found
        .iter()
        .map(|folder| Profile {
            folder: folder_name(folder),
            name: profile_name(browser, &root, &names, folder),
        })
        .collect();
    out.sort_by(|a, b| natural(&a.folder, &b.folder));
    (out, usual)
}

/// Text in the order Finder sorts it: runs of digits by their number, the rest without
/// regard to case.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn runs(text: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in text.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((was, run)) if *was == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ra, rb) = (runs(a), runs(b));
    for ((da, ta), (db, tb)) in ra.iter().zip(&rb) {
        let order = if *da && *db {
            let (na, nb) = (ta.trim_start_matches('0'), tb.trim_start_matches('0'));
            na.len().cmp(&nb.len()).then_with(|| na.cmp(nb))
        } else {
            ta.to_lowercase().cmp(&tb.to_lowercase())
        };
        if order.is_ne() {
            return order;
        }
    }
    ra.len().cmp(&rb.len()).then_with(|| a.cmp(b))
}

/// The profile folders `pick` names under `root`, or nothing when it names none here.
fn chosen(root: &Path, found: &[PathBuf], last: &[String], pick: Pick) -> Option<Vec<PathBuf>> {
    let folders = match pick {
        Pick::Usual => vec![usual(root, found, last)?],
        Pick::One(name) => found
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == name))
            .cloned()
            .collect(),
        Pick::All => found.to_vec(),
    };
    (!folders.is_empty()).then_some(folders)
}

/// What `browser` has to bring from the profile `pick` names, or from all of them, or
/// nothing when it has no such profile here. `now` stands in for a visit Chromium kept no
/// time for. With every profile, their open tabs and history come together, and each
/// one's bookmarks in turn, all in the one folder of the browser's name (as Search's
/// "All profiles" does).
#[must_use]
pub fn read(browser: &Browser, base: &Path, now: f64, pick: Pick) -> Option<Found> {
    let root = browser.root(base).canonicalize().ok()?;
    let found = profiles(&root);
    let (names, last) = local_state(&root);
    let folders = chosen(&root, &found, &last, pick)?;
    let profile = match (pick, folders.as_slice()) {
        (Pick::All, _) | (_, []) => None,
        (_, [folder, ..]) => {
            (found.len() > 1).then(|| profile_name(browser, &root, &names, folder))
        }
    };
    let mut session = snss::Snapshot::default();
    for folder in &folders {
        let read = snss::read(&session_data(&root, folder).unwrap_or_default());
        session.windows.extend(read.windows);
        for (token, group) in read.groups {
            session.groups.entry(token).or_insert(group);
        }
    }
    let mut places: Vec<Value> = folders
        .iter()
        .flat_map(|folder| places(&root, folder, now))
        .collect();
    if folders.len() > 1 {
        let last = |v: &Value| v["last"].as_f64().unwrap_or_default();
        places.sort_by(|a, b| last(b).total_cmp(&last(a)));
        places.truncate(usize::try_from(PLACES).unwrap_or(usize::MAX));
    }
    let bookmarks = bookmarks(&root, &folders, browser.name);
    Some(Found {
        incoming: Incoming {
            sessions: (session.tabs() > 0)
                .then(|| shape(&session))
                .into_iter()
                .collect(),
            histories: (!places.is_empty())
                .then(|| serde_json::to_vec(&places).unwrap_or_default())
                .into_iter()
                .collect(),
            bookmarks: bookmarks.into_iter().collect(),
            ..Incoming::default()
        },
        profile,
    })
}

/// The newest session file's bytes: the newest `Session_*` in `Sessions/`, whatever the
/// names say, or the `Current Session` beside it in an older profile.
fn session_data(root: &Path, folder: &Path) -> Option<Vec<u8>> {
    let newest = file(root, &folder.join("Sessions"))
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("Session_"))
        .filter_map(|e| {
            let path = file(root, &e.path())?;
            let modified = path
                .metadata()
                .ok()
                .filter(std::fs::Metadata::is_file)?
                .modified()
                .ok()?;
            Some((modified, e.file_name(), path))
        })
        .max();
    newest
        .and_then(|(_, _, path)| read_capped(root, &path))
        .or_else(|| read_capped(root, &folder.join("Current Session")))
}

/// The open tabs of every normal window as one row, each address once, each group under
/// its name and fold. Pinned tabs come as tabs: Tap has no pins yet.
fn shape(session: &snss::Snapshot) -> Shape {
    let id = |t: &snss::Token| format!("{:016X}{:016X}", t.high, t.low);
    let mut groups: Vec<TabGroup> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut tabs = Vec::new();
    for tab in session.windows.iter().flatten() {
        if !seen.insert(tab.url.as_str()) {
            continue;
        }
        let group = tab.group.and_then(|token| {
            let meta = session.groups.get(&token)?;
            let gid = id(&token);
            if !groups.iter().any(|g| g.id == gid) {
                groups.push(TabGroup {
                    id: gid.clone(),
                    name: meta.title.clone(),
                    collapsed: meta.collapsed,
                });
            }
            Some(gid)
        });
        tabs.push(Entry {
            url: tab.url.clone(),
            title: tab.title.clone(),
            group_id: group,
            ..Entry::default()
        });
    }
    Shape {
        tabs,
        active: -1,
        groups: (!groups.is_empty()).then_some(groups),
        ..Shape::default()
    }
}

/// The history, newest first, as `history.json`'s rows (seconds since 2001).
fn places(root: &Path, folder: &Path, now: f64) -> Vec<Value> {
    let Some(history) = file(root, &folder.join("History")) else {
        return Vec::new();
    };
    let Some(copy) = Copy::of(&history) else {
        return Vec::new();
    };
    let rows = || -> rusqlite::Result<Vec<Value>> {
        let db = open(&copy.file)?;
        let mut statement = db.prepare(
            "SELECT url, title, visit_count, last_visit_time FROM urls \
             WHERE hidden = 0 AND visit_count > 0 ORDER BY last_visit_time DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([PLACES], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        Ok(rows
            .filter_map(Result::ok)
            .filter(|(url, ..)| snss::is_web(url))
            .map(|(url, title, count, stamp)| {
                #[allow(clippy::cast_precision_loss)] // microseconds: exact to well past 2200
                let last = if stamp > 0 {
                    stamp as f64 / 1_000_000.0 - CHROMIUM_EPOCH - APPLE_EPOCH
                } else {
                    now
                };
                json!({ "url": url, "title": title, "count": count.max(1), "last": last })
            })
            .collect())
    };
    rows().unwrap_or_default()
}

/// A copy of one of the browser's SQLite files to read from, with the two files SQLite
/// keeps beside it while the browser runs: what was written last is often still in
/// `-wal`, and a copy without it misses the newest history. Gone with the copy.
struct Copy {
    folder: PathBuf,
    file: PathBuf,
}

impl Copy {
    fn of(source: &Path) -> Option<Self> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!("tap-import-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&folder).ok()?;
        let copy = Self {
            file: folder.join(source.file_name()?),
            folder,
        };
        std::fs::copy(source, &copy.file).ok()?;
        for side in ["-wal", "-shm"] {
            let mut beside = source.as_os_str().to_owned();
            beside.push(side);
            let beside = PathBuf::from(beside);
            if std::fs::symlink_metadata(&beside).is_ok_and(|m| m.is_file()) {
                let mut to = copy.file.as_os_str().to_owned();
                to.push(side);
                std::fs::copy(&beside, PathBuf::from(to)).ok()?;
            }
        }
        Some(copy)
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The bookmarks, in a folder of the browser's name (as Search keeps another browser's
/// apart): each profile's bar first, then anything it filed elsewhere. The folder's
/// identity is the browser's, so bringing them again adds no second one.
fn bookmarks(root: &Path, folders: &[PathBuf], browser: &str) -> Option<Value> {
    let mut children = Vec::new();
    for folder in folders {
        let Some(top) = read_capped(root, &folder.join("Bookmarks"))
            .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
        else {
            continue;
        };
        let roots = &top["roots"];
        children.extend(nodes(&roots["bookmark_bar"]["children"]));
        for (key, name) in [("other", "Other"), ("synced", "Mobile")] {
            let kids = nodes(&roots[key]["children"]);
            if !kids.is_empty() {
                children.push(json!({ "id": fnd::uuid(), "title": name, "children": kids }));
            }
        }
    }
    (!children.is_empty())
        .then(|| json!({ "id": folder_id(browser), "title": browser, "children": children }))
}

/// Chromium's bookmark nodes in Search's `bookmarks.json` shape: a page's address and
/// title, a folder's title and children. Only web pages come.
fn nodes(raw: &Value) -> Vec<Value> {
    raw.as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let title = entry["name"].as_str().unwrap_or_default();
            let id = entry["guid"]
                .as_str()
                .filter(|g| fnd::is_uuid(g))
                .map_or_else(fnd::uuid, str::to_uppercase);
            match entry["type"].as_str()? {
                "folder" => {
                    Some(json!({ "id": id, "title": title, "children": nodes(&entry["children"]) }))
                }
                "url" => {
                    let url = entry["url"].as_str().filter(|u| snss::is_web(u))?;
                    Some(json!({ "id": id, "title": title, "url": url }))
                }
                _ => None,
            }
        })
        .collect()
}

/// What a browser's passwords files hold, once its key has unwrapped them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Passwords {
    pub logins: Vec<Login>,
    /// Sites the other browser was told never to offer to save for.
    pub never: Vec<String>,
    /// Passwords left behind for having no site to go with (an app's, say).
    pub skipped: usize,
    /// Passwords that were there but didn't unwrap with the key the keychain handed over:
    /// said, not passed over in silence, so a bring-in that brought nothing says why.
    pub undecrypted: usize,
}

/// The passwords files of the profiles `pick` names, inside the browser's folder.
fn login_files(browser: &Browser, base: &Path, pick: Pick) -> Vec<PathBuf> {
    if browser.key.is_none() {
        return Vec::new();
    }
    let Ok(root) = browser.root(base).canonicalize() else {
        return Vec::new();
    };
    let found = profiles(&root);
    let (_, last) = local_state(&root);
    chosen(&root, &found, &last, pick)
        .unwrap_or_default()
        .iter()
        .flat_map(|folder| LOGINS.iter().map(move |name| folder.join(name)))
        .filter_map(|path| file(&root, &path))
        .filter(|path| path.is_file())
        .collect()
}

/// How many passwords there are to bring, read from the files alone: rows with something
/// in them that aren't a site marked never. No key is asked for and nothing is unwrapped,
/// so it never makes macOS ask.
#[must_use]
pub fn password_count(browser: &Browser, base: &Path, pick: Pick) -> usize {
    login_files(browser, base, pick)
        .iter()
        .filter_map(|path| {
            let copy = Copy::of(path)?;
            let db = open(&copy.file).ok()?;
            db.query_row(
                "SELECT COUNT(*) FROM logins WHERE blacklisted_by_user = 0 \
                 AND length(password_value) > 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        })
        .map(|n| usize::try_from(n).unwrap_or_default())
        .sum()
}

/// The passwords of the profiles `pick` names, unwrapped with `passphrase`, the key the
/// keychain gave for [`Browser::safe_storage`]. Nothing when no passwords file could be
/// read at all. A site's account comes once, however many files hold it.
#[must_use]
pub fn passwords(
    browser: &Browser,
    base: &Path,
    passphrase: &str,
    pick: Pick,
) -> Option<Passwords> {
    let key = stretch(passphrase);
    let mut out = Passwords::default();
    let mut seen = std::collections::HashSet::new();
    let mut read_any = false;
    for path in login_files(browser, base, pick) {
        let Some(rows) = Copy::of(&path).and_then(|copy| logins(&copy.file).ok()) else {
            continue;
        };
        read_any = true;
        for row in rows {
            let host = passwords::host_of(&row.origin);
            if host.is_empty() {
                if !row.never && !row.blob.is_empty() {
                    out.skipped += 1;
                }
                continue;
            }
            if row.never {
                out.never.push(host);
                continue;
            }
            let Some(password) = key.as_ref().and_then(|key| unwrap(&row.blob, key)) else {
                if !row.blob.is_empty() {
                    out.undecrypted += 1;
                }
                continue;
            };
            if password.is_empty() {
                continue;
            }
            let login = Login {
                clear: row.origin.to_lowercase().starts_with("http://"),
                host,
                user: row.user,
                password,
                used: row.used,
            };
            if seen.insert(login.id()) {
                out.logins.push(login);
            }
        }
    }
    read_any.then_some(out)
}

/// Chromium's own recipe, unchanged for a decade: PBKDF2 over HMAC-SHA1, the salt
/// "saltysalt", 1003 rounds, sixteen bytes out. No passphrase, no key: nothing unwraps.
fn stretch(passphrase: &str) -> Option<[u8; 16]> {
    if passphrase.is_empty() {
        return None;
    }
    Some(pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(
        passphrase.as_bytes(),
        b"saltysalt",
        1003,
    ))
}

/// The sixteen spaces Chromium uses as every password's IV.
const IV: [u8; 16] = [b' '; 16];

/// "v10" and then AES-128-CBC. Encrypted, as every Chromium browser on the Mac has done
/// since 2014, or not taken at all: a profile's files can be written by anything running
/// as you, and a password lying there in the clear is one only such a program would have
/// put, to have Tap keep it.
fn unwrap(blob: &[u8], key: &[u8; 16]) -> Option<String> {
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
    let body = blob.strip_prefix(b"v10")?;
    let mut buf = body.to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new_from_slices(key, &IV)
        .ok()?
        .decrypt_padded::<Pkcs7>(&mut buf)
        .ok()?;
    if let Ok(text) = std::str::from_utf8(plain) {
        return Some(text.to_owned());
    }
    // Newer builds put a hash of the site before the password. Past it, the password is
    // the same as ever.
    let past = plain.get(32..).filter(|_| plain.len() > 32)?;
    std::str::from_utf8(past).ok().map(str::to_owned)
}

/// One row of a passwords file, still wrapped.
struct Row {
    origin: String,
    user: String,
    blob: Vec<u8>,
    never: bool,
    used: Option<f64>,
}

fn open(path: &Path) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Every row of a copy of a passwords file.
fn logins(path: &Path) -> rusqlite::Result<Vec<Row>> {
    let db = open(path)?;
    let mut statement = db.prepare(
        "SELECT origin_url, username_value, password_value, blacklisted_by_user, \
         date_last_used FROM logins",
    )?;
    let rows = statement.query_map([], |row| {
        let stamp = row.get::<_, Option<i64>>(4)?.unwrap_or_default();
        Ok(Row {
            origin: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            user: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            blob: row.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
            never: row.get::<_, Option<i64>>(3)?.unwrap_or_default() != 0,
            // Microseconds since 1601, Chromium's idea of a date.
            #[allow(clippy::cast_precision_loss)] // microseconds: exact to well past 2200
            used: (stamp > 0).then(|| stamp as f64 / 1_000_000.0 - CHROMIUM_EPOCH),
        })
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// A folder identity of the browser's own, the same every time: FNV-1a over its name,
/// written as a UUID.
fn folder_id(browser: &str) -> String {
    let hash = |seed: u64| {
        format!("tap.bookmarks.{browser}")
            .bytes()
            .fold(seed, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3)
            })
    };
    let (a, b) = (hash(0xCBF2_9CE4_8422_2325), hash(0x8422_2325_CBF2_9CE4));
    let hex = format!("{a:016X}{b:016X}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snss::tests::{frame, grouping, metadata, navigation, raw, session};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("tap-chromium-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap_or_default();
            Self(base)
        }

        fn write(&self, path: &str, data: &[u8]) {
            let path = self.0.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap_or_default();
            }
            std::fs::write(path, data).unwrap_or_default();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn chrome() -> &'static Browser {
        &KNOWN[1]
    }

    fn history(path: &Path, rows: &[(&str, &str, i64, i64)]) {
        let db = rusqlite::Connection::open(path).ok();
        let Some(db) = db else { return };
        let _ = db.execute_batch(
            "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, \
             typed_count INTEGER DEFAULT 0, last_visit_time INTEGER, hidden INTEGER DEFAULT 0)",
        );
        for (url, title, count, last) in rows {
            let _ = db.execute(
                "INSERT INTO urls (url, title, visit_count, last_visit_time) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![url, title, count, last],
            );
        }
    }

    #[test]
    fn a_browser_with_nothing_to_bring_is_not_found() {
        let here = Fixture::new("empty");
        // What this Mac has for Arc: a folder Chrome's helpers made, and no profile.
        std::fs::create_dir_all(here.0.join("Arc/User Data/NativeMessagingHosts"))
            .unwrap_or_default();
        assert_eq!(profiles(&KNOWN[3].root(&here.0)), Vec::<PathBuf>::new());
        assert!(read(&KNOWN[3], &here.0, 0.0, Pick::Usual).is_none());
    }

    #[test]
    fn the_profile_used_last_brings_its_tabs_groups_history_and_bookmarks() {
        let here = Fixture::new("chrome");
        let root = "Google/Chrome";
        here.write(
            &format!("{root}/Local State"),
            br#"{"profile":{"last_used":"Profile 1","info_cache":{"Default":{"name":"Home"},"Profile 1":{"name":"Work"}}}}"#,
        );
        here.write(&format!("{root}/Default/Bookmarks"), br#"{"roots":{}}"#);
        here.write(
            &format!("{root}/Guest Profile/Bookmarks"),
            br#"{"roots":{}}"#,
        );
        let work = format!("{root}/Profile 1");
        here.write(
            &format!("{work}/Sessions/Session_1"),
            &session(&[
                frame(0, &raw(&[1, 10])),
                navigation(10, 0, "https://a.example/", "A"),
                grouping(10, 4, 2, true),
                metadata(4, 2, "Reading", true),
                frame(0, &raw(&[1, 11])),
                navigation(11, 0, "https://b.example/", "B"),
                frame(0, &raw(&[1, 12])),
                navigation(12, 0, "https://a.example/", "A again"),
            ]),
        );
        history(
            &here.0.join(format!("{work}/History")),
            &[
                // 2024-01-01 in Chromium's microseconds since 1601.
                ("https://a.example/", "A", 7, 13_348_540_800_000_000),
                ("chrome://settings/", "Settings", 3, 13_348_540_800_000_000),
            ],
        );
        here.write(
            &format!("{work}/Bookmarks"),
            br#"{"roots":{"bookmark_bar":{"children":[
                {"type":"url","name":"R","url":"https://r.example/","guid":"0bc5d13f-2cba-5d74-951f-3f233fe6c908"},
                {"type":"folder","name":"F","children":[{"type":"url","name":"S","url":"https://s.example/"}]},
                {"type":"url","name":"JS","url":"javascript:void(0)"}]},
              "other":{"children":[{"type":"url","name":"O","url":"https://o.example/"}]},
              "synced":{"children":[]}}}"#,
        );

        let names: Vec<String> = profiles(&chrome().root(&here.0))
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(names, ["Default", "Profile 1"]);

        let found = read(chrome(), &here.0, 1.0, Pick::Usual).expect("Chrome is here");
        assert_eq!(found.profile.as_deref(), Some("Work"));
        let incoming = &found.incoming;
        assert_eq!(incoming.sessions.len(), 1);
        let shape = &incoming.sessions[0];
        let urls: Vec<&str> = shape.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://a.example/", "https://b.example/"]);
        let groups = shape.groups.as_ref().expect("a group");
        assert_eq!(
            (groups[0].name.as_str(), groups[0].collapsed),
            ("Reading", true)
        );
        assert_eq!(
            shape.tabs[0].group_id.as_deref(),
            Some(groups[0].id.as_str())
        );
        assert_eq!(shape.tabs[1].group_id, None);

        let places: Vec<Value> = serde_json::from_slice(&incoming.histories[0]).expect("rows");
        assert_eq!(places.len(), 1);
        assert_eq!(places[0]["count"], 7);
        // 2024-01-01 is 725,760,000 seconds after 2001-01-01.
        assert!((places[0]["last"].as_f64().unwrap_or_default() - 725_760_000.0).abs() < 1.0);

        let folder = &incoming.bookmarks[0];
        assert_eq!(folder["title"], "Chrome");
        assert_eq!(folder["id"], folder_id("Chrome"));
        let kids = folder["children"].as_array().expect("children");
        let titles: Vec<&str> = kids.iter().filter_map(|k| k["title"].as_str()).collect();
        assert_eq!(titles, ["R", "F", "Other"]);
        assert_eq!(kids[0]["id"], "0BC5D13F-2CBA-5D74-951F-3F233FE6C908");
        assert_eq!(kids[1]["children"][0]["url"], "https://s.example/");
    }

    #[test]
    fn without_a_word_from_local_state_the_profile_touched_last_is_read() {
        let here = Fixture::new("touched");
        here.write("Vivaldi/Default/Bookmarks", br#"{"roots":{}}"#);
        std::thread::sleep(std::time::Duration::from_millis(20));
        here.write(
            "Vivaldi/Profile 2/Current Session",
            &session(&[
                frame(0, &raw(&[1, 1])),
                navigation(1, 0, "https://v.example/", "V"),
            ]),
        );
        let found = read(&KNOWN[6], &here.0, 0.0, Pick::Usual).expect("Vivaldi is here");
        assert_eq!(found.profile.as_deref(), Some("Profile 2"));
        assert_eq!(found.incoming.sessions[0].tabs[0].url, "https://v.example/");
    }

    #[test]
    fn the_newest_session_file_wins_and_a_link_out_reads_nothing() {
        let here = Fixture::new("sessions");
        let one = session(&[
            frame(0, &raw(&[1, 1])),
            navigation(1, 0, "https://old.example/", "Old"),
        ]);
        let two = session(&[
            frame(0, &raw(&[1, 2])),
            navigation(2, 0, "https://new.example/", "New"),
        ]);
        here.write("Chromium/Default/Sessions/Session_200", &one);
        std::thread::sleep(std::time::Duration::from_millis(20));
        here.write("Chromium/Default/Sessions/Session_100", &two);
        let found = read(&KNOWN[7], &here.0, 0.0, Pick::Usual).expect("Chromium is here");
        assert_eq!(
            found.incoming.sessions[0].tabs[0].url,
            "https://new.example/"
        );

        // Opera's one profile is its own folder; a session linked from outside it isn't read.
        here.write("outside/Session_9", &one);
        here.write("com.operasoftware.Opera/Bookmarks", br#"{"roots":{}}"#);
        std::fs::create_dir_all(here.0.join("com.operasoftware.Opera/Sessions"))
            .unwrap_or_default();
        let _ = std::os::unix::fs::symlink(
            here.0.join("outside/Session_9"),
            here.0.join("com.operasoftware.Opera/Sessions/Session_9"),
        );
        let opera = read(&KNOWN[11], &here.0, 0.0, Pick::Usual).expect("Opera is here");
        assert_eq!(opera.incoming.sessions.len(), 0);
        assert_eq!(opera.profile, None);
    }

    /// A passwords file as Chromium writes one, with the columns read here.
    fn login_data(path: &Path, rows: &[(&str, &str, &[u8], bool, i64)]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_default();
        }
        let Ok(db) = rusqlite::Connection::open(path) else {
            return;
        };
        let _ = db.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, \
             username_value VARCHAR, password_value BLOB, blacklisted_by_user INTEGER NOT NULL, \
             date_last_used INTEGER NOT NULL DEFAULT 0)",
        );
        for (origin, user, blob, never, used) in rows {
            let _ = db.execute(
                "INSERT INTO logins (origin_url, username_value, password_value, \
                 blacklisted_by_user, date_last_used) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![origin, user, blob, i64::from(*never), used],
            );
        }
    }

    /// A password wrapped as Chromium wraps it, under `passphrase`.
    fn wrap(passphrase: &str, plain: &[u8]) -> Vec<u8> {
        use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
        let key = stretch(passphrase).expect("a key");
        let mut buf = plain.to_vec();
        buf.resize(plain.len() + 16, 0);
        let sealed = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &IV)
            .expect("sixteen bytes each")
            .encrypt_padded::<Pkcs7>(&mut buf, plain.len())
            .expect("room for the padding")
            .to_vec();
        [b"v10".as_slice(), &sealed].concat()
    }

    #[test]
    fn the_key_is_stretched_as_chromium_stretches_it() {
        // Python's hashlib.pbkdf2_hmac("sha1", b"peanuts", b"saltysalt", 1003, 16): the
        // same recipe from another implementation. ("peanuts" is Chromium's passphrase
        // on Linux, where it takes one round.)
        assert_eq!(
            stretch("peanuts"),
            Some([
                217, 160, 157, 73, 155, 78, 27, 116, 97, 242, 142, 103, 151, 44, 109, 189
            ])
        );
        assert_eq!(stretch(""), None);
    }

    #[test]
    fn passwords_unwrap_with_the_key_and_are_said_when_they_do_not() {
        let here = Fixture::new("passwords");
        let secret = "s3cret-passphrase";
        let root = "Google/Chrome";
        here.write(
            &format!("{root}/Local State"),
            br#"{"profile":{"last_used":"Default"}}"#,
        );
        // A hash of the site, as newer builds put before the password: not text.
        let hashed = [[0xE9u8; 32].as_slice(), b"after-the-hash"].concat();
        // 2024-01-01 in Chromium's microseconds since 1601.
        let used = 13_348_540_800_000_000;
        login_data(
            &here.0.join(format!("{root}/Default/Login Data")),
            &[
                (
                    "https://www.example.com/login",
                    "alice",
                    &wrap(secret, b"one"),
                    false,
                    used,
                ),
                (
                    "http://intranet.example/",
                    "bob",
                    &wrap(secret, &hashed),
                    false,
                    0,
                ),
                (
                    "https://example.com/other",
                    "alice",
                    &wrap(secret, b"again"),
                    false,
                    0,
                ),
                ("https://plain.example/", "carol", b"in the clear", false, 0),
                ("https://never.example/", "", b"", true, 0),
                (
                    "android://hash@com.vendor.app/",
                    "dan",
                    &wrap(secret, b"app"),
                    false,
                    0,
                ),
                ("https://empty.example/", "erin", b"", false, 0),
            ],
        );
        login_data(
            &here
                .0
                .join(format!("{root}/Default/Login Data For Account")),
            &[(
                "https://account.example/",
                "frank",
                &wrap(secret, b"synced"),
                false,
                0,
            )],
        );

        assert_eq!(password_count(chrome(), &here.0, Pick::Usual), 6);
        let found = passwords(chrome(), &here.0, secret, Pick::Usual).expect("readable");
        let got: Vec<(&str, &str, &str, bool)> = found
            .logins
            .iter()
            .map(|l| {
                (
                    l.host.as_str(),
                    l.user.as_str(),
                    l.password.as_str(),
                    l.clear,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("example.com", "alice", "one", false),
                ("intranet.example", "bob", "after-the-hash", true),
                ("account.example", "frank", "synced", false),
            ]
        );
        // 2024-01-01 is 1,704,067,200 seconds after 1970.
        assert!((found.logins[0].used.unwrap_or_default() - 1_704_067_200.0).abs() < 1.0);
        assert_eq!(found.logins[1].used, None);
        assert_eq!(found.never, ["never.example"]);
        assert_eq!((found.skipped, found.undecrypted), (1, 1));

        // The wrong key unwraps nothing, and says so for every one.
        let wrong = passwords(chrome(), &here.0, "not it", Pick::Usual).expect("readable");
        assert_eq!(wrong.logins, Vec::<Login>::new());
        assert_eq!((wrong.skipped, wrong.undecrypted), (1, 5));
        let none = passwords(chrome(), &here.0, "", Pick::Usual).expect("readable");
        assert_eq!((none.logins.len(), none.undecrypted), (0, 5));
    }

    #[test]
    fn no_passwords_file_or_no_key_to_read_it_with_brings_none() {
        let here = Fixture::new("no-passwords");
        let secret = "s3cret-passphrase";
        here.write("Arc/User Data/Default/Bookmarks", br#"{"roots":{}}"#);
        assert!(passwords(&KNOWN[3], &here.0, secret, Pick::Usual).is_none());
        here.write("Citro Labs/ego lite/Default/Bookmarks", br#"{"roots":{}}"#);
        login_data(
            &here.0.join("Citro Labs/ego lite/Default/Login Data"),
            &[("https://e.example/", "x", &wrap(secret, b"y"), false, 0)],
        );
        assert_eq!(KNOWN[2].safe_storage(), None);
        assert_eq!(
            chrome().safe_storage(),
            Some(("Chrome Safe Storage", "Chrome"))
        );
        assert_eq!(password_count(&KNOWN[2], &here.0, Pick::Usual), 0);
        assert!(passwords(&KNOWN[2], &here.0, secret, Pick::Usual).is_none());
    }

    #[test]
    fn a_profile_is_picked_by_its_folder_and_all_of_them_come_together() {
        let here = Fixture::new("picks");
        let root = "BraveSoftware/Brave-Browser";
        let brave = &KNOWN[4];
        here.write(
            &format!("{root}/Local State"),
            br#"{"profile":{"last_used":"Profile 2","info_cache":{"Default":{"name":"Home"},"Profile 2":{"name":"Work"}}}}"#,
        );
        for (folder, url) in [
            ("Default", "https://home.example/"),
            ("Profile 2", "https://work.example/"),
            ("Profile 10", "https://ten.example/"),
        ] {
            here.write(
                &format!("{root}/{folder}/Current Session"),
                &session(&[frame(0, &raw(&[1, 1])), navigation(1, 0, url, folder)]),
            );
            here.write(
                &format!("{root}/{folder}/Bookmarks"),
                format!(
                    r#"{{"roots":{{"bookmark_bar":{{"children":[{{"type":"url","name":"{folder}","url":"{url}"}}]}}}}}}"#
                )
                .as_bytes(),
            );
        }
        login_data(
            &here.0.join(format!("{root}/Profile 10/Login Data")),
            &[("https://ten.example/", "t", &wrap("k", b"p"), false, 0)],
        );

        let (list, usual) = profile_list(brave, &here.0);
        let got: Vec<(&str, &str)> = list
            .iter()
            .map(|p| (p.folder.as_str(), p.name.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Default", "Home"),
                ("Profile 2", "Work"),
                ("Profile 10", "Profile 10")
            ]
        );
        assert_eq!(usual.as_deref(), Some("Profile 2"));

        let tabs = |pick| -> Vec<String> {
            read(brave, &here.0, 0.0, pick)
                .map(|f| {
                    f.incoming.sessions[0]
                        .tabs
                        .iter()
                        .map(|t| t.url.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(tabs(Pick::Usual), ["https://work.example/"]);
        assert_eq!(tabs(Pick::One("Default")), ["https://home.example/"]);
        assert_eq!(
            tabs(Pick::All),
            [
                "https://home.example/",
                "https://ten.example/",
                "https://work.example/"
            ]
        );
        assert!(read(brave, &here.0, 0.0, Pick::One("Nobody")).is_none());
        let one = read(brave, &here.0, 0.0, Pick::One("Profile 10")).expect("there");
        assert_eq!(one.profile.as_deref(), Some("Profile 10"));
        let all = read(brave, &here.0, 0.0, Pick::All).expect("there");
        assert_eq!(all.profile, None);
        let marks = all.incoming.bookmarks[0]["children"]
            .as_array()
            .map(|kids| {
                kids.iter()
                    .filter_map(|k| k["title"].as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(marks, ["Default", "Profile 10", "Profile 2"]);

        assert_eq!(password_count(brave, &here.0, Pick::Usual), 0);
        assert_eq!(password_count(brave, &here.0, Pick::All), 1);
        assert_eq!(password_count(brave, &here.0, Pick::One("Profile 10")), 1);
        let all = passwords(brave, &here.0, "k", Pick::All).expect("readable");
        assert_eq!(all.logins[0].password, "p");
    }

    #[test]
    fn profiles_count_as_a_person_would() {
        let mut names = vec!["Profile 10", "Default", "profile 2", "Profile 1"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["Default", "Profile 1", "profile 2", "Profile 10"]);
    }
}
