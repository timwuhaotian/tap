//! What a Chromium browser on this Mac holds (Search's `Chromium` in `Import.swift`):
//! Chrome, Arc, Brave, Edge, Dia and the rest keep their profiles the same way, one
//! folder each under `~/Library/Application Support`, and Tap reads three things from the
//! profile used last: its open tabs and tab groups (the session file, [`crate::snss`],
//! Search #841), its history (the `History` database) and its bookmarks (one JSON file).
//! What it reads comes out as a [`profile::Incoming`], so it is merged exactly as a
//! profile file is.
//!
//! Nothing in the other browser changes. Its databases are copied before they are read:
//! it is usually running, and reading its live file underneath it is how you get a lock
//! error. A link inside its folder can't lead a read out of it. Passwords stay behind:
//! Tap fills none yet, and reading them means macOS asking for the browser's key.

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

use crate::{profile::Incoming, snss};

/// A browser this reads, by its folder under `~/Library/Application Support`.
#[derive(Debug, PartialEq, Eq)]
pub struct Browser {
    pub name: &'static str,
    folder: &'static str,
}

/// Search's list, in Search's order. Chrome's other channels keep their own data.
pub const KNOWN: &[Browser] = &[
    Browser {
        name: "Dia",
        folder: "Dia/User Data",
    },
    Browser {
        name: "Chrome",
        folder: "Google/Chrome",
    },
    Browser {
        name: "ego lite",
        folder: "Citro Labs/ego lite",
    },
    Browser {
        name: "Arc",
        folder: "Arc/User Data",
    },
    Browser {
        name: "Brave",
        folder: "BraveSoftware/Brave-Browser",
    },
    Browser {
        name: "Edge",
        folder: "Microsoft Edge",
    },
    Browser {
        name: "Vivaldi",
        folder: "Vivaldi",
    },
    Browser {
        name: "Chromium",
        folder: "Chromium",
    },
    Browser {
        name: "Chrome Beta",
        folder: "Google/Chrome Beta",
    },
    Browser {
        name: "Chrome Dev",
        folder: "Google/Chrome Dev",
    },
    Browser {
        name: "Chrome Canary",
        folder: "Google/Chrome Canary",
    },
    // Opera keeps its one profile in its own folder, not in one inside it.
    Browser {
        name: "Opera",
        folder: "com.operasoftware.Opera",
    },
    Browser {
        name: "Opera GX",
        folder: "com.operasoftware.OperaGX",
    },
    Browser {
        name: "Helium",
        folder: "net.imput.helium",
    },
    Browser {
        name: "Comet",
        folder: "Comet",
    },
];

/// The files that make a folder a profile with something to bring.
const HOLDS: &[&str] = &["Bookmarks", "History", "Sessions", "Current Session"];
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

/// What `browser` has to bring from its usual profile, or nothing when it has no profile
/// here. `now` stands in for a visit Chromium kept no time for.
#[must_use]
pub fn read(browser: &Browser, base: &Path, now: f64) -> Option<Found> {
    let root = browser.root(base).canonicalize().ok()?;
    let found = profiles(&root);
    let (names, last) = local_state(&root);
    let folder = usual(&root, &found, &last)?;
    let profile = (found.len() > 1).then(|| {
        let id = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        names.get(&id).cloned().unwrap_or(id)
    });
    let session = snss::read(&session_data(&root, &folder).unwrap_or_default());
    let places = places(&root, &folder, now);
    let bookmarks = bookmarks(&root, &folder, browser.name);
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
        let db = rusqlite::Connection::open_with_flags(
            &copy.file,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
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
/// apart): the bar first, then anything filed elsewhere. The folder's identity is the
/// browser's, so bringing them again adds no second one.
fn bookmarks(root: &Path, folder: &Path, browser: &str) -> Option<Value> {
    let top: Value = serde_json::from_slice(&read_capped(root, &folder.join("Bookmarks"))?).ok()?;
    let roots = &top["roots"];
    let mut children = nodes(&roots["bookmark_bar"]["children"]);
    for (key, name) in [("other", "Other"), ("synced", "Mobile")] {
        let kids = nodes(&roots[key]["children"]);
        if !kids.is_empty() {
            children.push(json!({ "id": fnd::uuid(), "title": name, "children": kids }));
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
        assert!(read(&KNOWN[3], &here.0, 0.0).is_none());
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

        let found = read(chrome(), &here.0, 1.0).expect("Chrome is here");
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
        let found = read(&KNOWN[6], &here.0, 0.0).expect("Vivaldi is here");
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
        let found = read(&KNOWN[7], &here.0, 0.0).expect("Chromium is here");
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
        let opera = read(&KNOWN[11], &here.0, 0.0).expect("Opera is here");
        assert_eq!(opera.incoming.sessions.len(), 0);
        assert_eq!(opera.profile, None);
    }
}
