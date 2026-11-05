//! A whole profile as one offline file, out and back in: Search's #845 (`SearchProfile.swift`
//! and `ImportFile.swift` on `export-search-profile`), docs/PLAN.md §7.
//!
//! The file is a plain zip of flat files, the same layout Search writes: the profile's
//! own files byte for byte (`session.json`, `history.json`, `bookmarks.json`...), the
//! shared ones beside them, `prefs.json` cut to the keys Settings shows, `installed.json`
//! with the extensions' store ids, and `manifest.json` (`format` 1, `scope` "profile").
//! Tap writes it as `.tapprofile` and reads that or a `.zip` that is one.
//! Passwords, cookies and anything signed stay where they are: the keychain and
//! WebKit's stores never enter the file.
//!
//! Reading never unpacks to disk: each file is read by its name, flat, into memory, with
//! a cap on its size, so neither a `../` name nor a zip bomb can reach anything.

use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tap_core::session::Shape;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

/// What a reader refuses: anything written by a newer Tap or Search.
pub const FORMAT: i64 = 1;
/// Tap's profile extension.
pub const EXTENSION: &str = "tapprofile";
/// The first space's identity (Search's `Space.firstID`): its session is `session.json`.
pub const FIRST_SPACE: &str = "00000000-0000-0000-0000-000000000001";

/// The space list of Personal standing alone, as Swift encodes `Space.first()`.
const ONE_SPACE: &str =
    r#"[{"id":"00000000-0000-0000-0000-000000000001","name":"Personal","colour":0}]"#;

/// The most one file in a profile may unpack to, and the whole file together: far more
/// than a real profile holds (a big history is a couple of megabytes).
const MOST_PER_FILE: u64 = 64 << 20;
const MOST_IN_ALL: u64 = 256 << 20;

/// `manifest.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: i64,
    #[serde(rename = "appVersion")]
    pub app_version: String,
    pub scope: String,
    /// Seconds since 2001-01-01, as Swift's `JSONEncoder` writes a `Date`.
    #[serde(rename = "exportedAt")]
    pub exported_at: f64,
    pub counts: BTreeMap<String, i64>,
}

/// What goes out: the files the store folder has for this profile, read by the caller,
/// and what is live in memory.
pub struct Outgoing<'a> {
    /// The session as the window holds it now, already encoded.
    pub session: Vec<u8>,
    /// `history.json` as the window holds it now.
    pub history: Vec<u8>,
    /// A store file by its name, if there is one (bookmarks, pins and the rest).
    pub read: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    /// The allowlisted settings, already in Search's JSON shape (`$data`, `$date`).
    pub prefs: Map<String, Value>,
    /// The installed extensions' store ids.
    pub extensions: Vec<String>,
    pub app_version: String,
    pub now: f64,
}

/// The profile as one file's bytes (Search's `collectCurrentProfile` then `zip`). Tap
/// has one profile and one space so far: Personal's files, the first space's session.
///
/// # Errors
/// If the zip can't be written, which only a bug could make happen.
pub fn pack(out: &Outgoing<'_>) -> Result<Vec<u8>, String> {
    let mut files: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut counts = BTreeMap::new();
    // No file while Personal stands alone: still one profile.
    let profiles = (out.read)("profiles.json");
    counts.insert(
        "profiles".to_owned(),
        profiles
            .as_deref()
            .and_then(|data| serde_json::from_slice::<Vec<Value>>(data).ok())
            .map_or(1, |list| i64::try_from(list.len()).unwrap_or(1).max(1)),
    );
    if let Some(data) = profiles {
        files.push(("profiles.json", data));
    }
    // Search keeps no list while Personal has its one space, and #845's import restores
    // only the sessions of spaces it is given: the list `Spaces.read` makes on the spot
    // goes in instead.
    files.push((
        "spaces.json",
        (out.read)("spaces.json").unwrap_or_else(|| ONE_SPACE.as_bytes().to_vec()),
    ));
    counts.insert("spaces".to_owned(), 1);
    files.push(("session.json", out.session.clone()));
    counts.insert("sessions".to_owned(), 1);
    files.push(("history.json", out.history.clone()));
    counts.insert("history".to_owned(), 1);
    let bookmarks = (out.read)("bookmarks.json");
    counts.insert("bookmarks".to_owned(), i64::from(bookmarks.is_some()));
    if let Some(data) = bookmarks {
        files.push(("bookmarks.json", data));
    }
    // Shared by every profile: whole, for the reader to remap.
    for name in ["pins.json", "hidden.json", "search-sites.json"] {
        if let Some(data) = (out.read)(name) {
            files.push((name, data));
        }
    }
    let mut ids = out.extensions.clone();
    ids.sort();
    counts.insert(
        "extensions".to_owned(),
        i64::try_from(ids.len()).unwrap_or(0),
    );
    files.push(("installed.json", to_json(&ids)?));
    counts.insert(
        "prefs".to_owned(),
        i64::try_from(out.prefs.len()).unwrap_or(0),
    );
    files.push(("prefs.json", to_json(&out.prefs)?));
    let manifest = Manifest {
        format: FORMAT,
        app_version: out.app_version.clone(),
        scope: "profile".to_owned(),
        exported_at: out.now,
        counts,
    };
    files.push(("manifest.json", to_json(&manifest)?));

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in files {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&data).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

fn to_json(value: &impl Serialize) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}

/// Why a file couldn't be taken in, as it is said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Not a zip, or no manifest of a profile in it.
    Unreadable,
    /// Written by a newer (or much older) version: its format number.
    Format(i64),
}

impl Problem {
    #[must_use]
    pub fn said(&self) -> String {
        match self {
            Problem::Unreadable => "That file couldn't be read".to_owned(),
            Problem::Format(format) => format!(
                "That profile needs a {} Tap (format {format})",
                if *format > FORMAT { "newer" } else { "older" }
            ),
        }
    }
}

/// Everything one file holds, keyed as the flat folder holds it (Search's
/// `IncomingProfile`).
#[derive(Clone, Debug, Default)]
pub struct Incoming {
    pub manifest: Option<Manifest>,
    /// The profiles' names, Personal's included when the file has the list.
    pub profiles: Vec<String>,
    /// Each space's session, the first space's first, then in the order `spaces.json`
    /// gives them, then any it doesn't name.
    pub sessions: Vec<Shape>,
    /// Each profile's `history.json`, as written.
    pub histories: Vec<Vec<u8>>,
    /// Each profile's bookmarks, top level.
    pub bookmarks: Vec<Value>,
    pub prefs: Map<String, Value>,
    pub extensions: Vec<String>,
}

impl Incoming {
    /// How many tabs its sessions hold.
    #[must_use]
    pub fn tabs(&self) -> usize {
        self.sessions.iter().map(|s| s.tabs.len()).sum()
    }

    /// How many tab groups its sessions hold.
    #[must_use]
    pub fn groups(&self) -> usize {
        self.sessions
            .iter()
            .map(|s| s.groups.as_ref().map_or(0, Vec::len))
            .sum()
    }

    /// How many places its histories hold.
    #[must_use]
    pub fn places(&self) -> usize {
        self.histories
            .iter()
            .filter_map(|data| serde_json::from_slice::<Vec<Value>>(data).ok())
            .map(|rows| rows.len())
            .sum()
    }

    /// How many bookmarks, folders aside.
    #[must_use]
    pub fn bookmark_count(&self) -> usize {
        let mut urls = Vec::new();
        for node in &self.bookmarks {
            walk(node, &mut urls, &mut Vec::new());
        }
        urls.len()
    }

    /// Leaves out the tabs whose address is already open in `open`, and the groups that
    /// leaves empty: a browser brought over again adds only what is new.
    pub fn leave_out(&mut self, open: &std::collections::HashSet<String>) {
        for shape in &mut self.sessions {
            shape.tabs.retain(|t| !open.contains(&t.url));
            if let Some(groups) = shape.groups.as_mut() {
                let used: Vec<&String> = shape
                    .tabs
                    .iter()
                    .filter_map(|t| t.group_id.as_ref())
                    .collect();
                groups.retain(|g| used.contains(&&g.id));
            }
            if shape.groups.as_ref().is_some_and(Vec::is_empty) {
                shape.groups = None;
            }
        }
        self.sessions.retain(|s| !s.tabs.is_empty());
    }
}

/// Reads one file's bytes. Section by section: one unreadable section never hides the
/// rest. Only the manifest has to be there, and say it is a profile in a format this
/// version reads.
///
/// # Errors
/// If it isn't a zip with a profile's manifest, or the manifest's format is another.
pub fn unpack(bytes: &[u8]) -> Result<Incoming, Problem> {
    let files = flat_files(bytes).ok_or(Problem::Unreadable)?;
    let decode = |name: &str| -> Option<Value> {
        files
            .get(name)
            .and_then(|data| serde_json::from_slice(data).ok())
    };
    let manifest: Manifest = files
        .get("manifest.json")
        .and_then(|data| serde_json::from_slice(data).ok())
        .ok_or(Problem::Unreadable)?;
    if manifest.scope != "profile" {
        return Err(Problem::Unreadable);
    }
    if manifest.format != FORMAT {
        return Err(Problem::Format(manifest.format));
    }
    let mut incoming = Incoming {
        manifest: Some(manifest),
        ..Incoming::default()
    };
    if let Some(Value::Array(list)) = decode("profiles.json") {
        incoming.profiles = list
            .iter()
            .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect();
    }
    if let Some(Value::Object(prefs)) = decode("prefs.json") {
        incoming.prefs = prefs;
    }
    if let Some(Value::Array(ids)) = decode("installed.json") {
        incoming.extensions = ids
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();
    }
    // The spaces' order, by identity, upper case as Swift writes a UUID.
    let order: Vec<String> = match decode("spaces.json") {
        Some(Value::Array(spaces)) => spaces
            .iter()
            .filter_map(|s| s.get("id").and_then(Value::as_str).map(str::to_uppercase))
            .collect(),
        _ => Vec::new(),
    };
    let mut sessions: Vec<(usize, String, Shape)> = Vec::new();
    for (name, data) in &files {
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if let Some(space) = session_space(stem) {
            if let Some(shape) = Shape::decode(data) {
                let place = if space == FIRST_SPACE {
                    0
                } else {
                    order
                        .iter()
                        .position(|id| *id == space)
                        .map_or(usize::MAX, |i| i + 1)
                };
                sessions.push((place, space, shape));
            }
        } else if stem == "history" || per_profile(stem, "history-") {
            incoming.histories.push(data.clone());
        } else if (stem == "bookmarks" || per_profile(stem, "bookmarks-"))
            && let Ok(Value::Array(nodes)) = serde_json::from_slice::<Value>(data)
        {
            incoming.bookmarks.extend(nodes);
        }
    }
    sessions.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    incoming.sessions = sessions.into_iter().map(|(_, _, shape)| shape).collect();
    Ok(incoming)
}

/// Whether a zip is a profile rather than, say, Safari's export: a manifest with the
/// profile's scope. Decides which reader takes a `.zip`.
#[must_use]
pub fn is_profile(bytes: &[u8]) -> bool {
    !matches!(unpack(bytes), Err(Problem::Unreadable))
}

/// The space a session file is for: `session` is the first space's, `session-<UUID>`
/// another's.
fn session_space(stem: &str) -> Option<String> {
    if stem == "session" {
        return Some(FIRST_SPACE.to_owned());
    }
    let id = stem.strip_prefix("session-")?;
    tap_core::fnd::is_uuid(id).then(|| id.to_uppercase())
}

fn per_profile(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(tap_core::fnd::is_uuid)
}

/// The zip's files at its top level, by name, each read whole up to its cap. A folder,
/// a name with a path in it, or macOS's `__MACOSX` resource forks are passed over.
fn flat_files(bytes: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut files = BTreeMap::new();
    let mut total: u64 = 0;
    for index in 0..zip.len() {
        let Ok(mut file) = zip.by_index(index) else {
            continue;
        };
        let Ok(name) = file.name().map(std::borrow::Cow::into_owned) else {
            continue;
        };
        if file.is_dir() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
            continue;
        }
        let mut data = Vec::new();
        // Read past the cap by one byte, so a file that lies about its size is caught.
        let read = (&mut file)
            .take(MOST_PER_FILE + 1)
            .read_to_end(&mut data)
            .ok()?;
        total += read as u64;
        if read as u64 > MOST_PER_FILE || total > MOST_IN_ALL {
            return None;
        }
        files.insert(name, data);
    }
    Some(files)
}

/// The profile's bookmarks added to the ones kept here (a top-level list in Search's
/// `bookmarks.json` shape), without doubles: a bookmark whose address is already
/// anywhere here stays out; folders with the same identity are merged. Answers the file
/// to keep, with how many bookmarks came in and how many were here already.
#[must_use]
pub fn merge_bookmarks(kept: Option<&[u8]>, incoming: Vec<Value>) -> (Vec<u8>, usize, usize) {
    let mut list: Vec<Value> = kept
        .and_then(|data| serde_json::from_slice(data).ok())
        .unwrap_or_default();
    let mut urls = Vec::new();
    let mut ids = Vec::new();
    for node in &list {
        walk(node, &mut urls, &mut ids);
    }
    let (mut added, mut already) = (0, 0);
    for node in incoming {
        merge_node(
            &mut list,
            &[],
            node,
            &mut urls,
            &mut ids,
            &mut added,
            &mut already,
        );
    }
    (
        serde_json::to_vec(&list).unwrap_or_default(),
        added,
        already,
    )
}

fn folder_path(list: &[Value], id: &str) -> Option<Vec<usize>> {
    for (index, node) in list.iter().enumerate() {
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            if node.get("id").and_then(Value::as_str) == Some(id) {
                return Some(vec![index]);
            }
            if let Some(mut path) = folder_path(children, id) {
                path.insert(0, index);
                return Some(path);
            }
        }
    }
    None
}

fn children_at<'a>(mut list: &'a mut Vec<Value>, path: &[usize]) -> Option<&'a mut Vec<Value>> {
    for &index in path {
        list = list.get_mut(index)?.get_mut("children")?.as_array_mut()?;
    }
    Some(list)
}

fn merge_node(
    list: &mut Vec<Value>,
    parent: &[usize],
    mut node: Value,
    urls: &mut Vec<String>,
    ids: &mut Vec<String>,
    added: &mut usize,
    already: &mut usize,
) {
    let url = node.get("url").and_then(Value::as_str).map(str::to_owned);
    let id = node.get("id").and_then(Value::as_str).map(str::to_owned);
    if let Some(url) = url {
        if urls.contains(&url) {
            *already += 1;
        } else {
            urls.push(url);
            if let Some(id) = id {
                ids.push(id);
            }
            *added += 1;
            if let Some(target) = children_at(list, parent) {
                target.push(node);
            }
        }
        return;
    }
    let existing = id.as_deref().and_then(|id| folder_path(list, id));
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        let children = std::mem::take(children);
        let is_existing = existing.is_some();
        let path = if let Some(path) = existing {
            path
        } else if id.as_ref().is_some_and(|id| ids.contains(id)) {
            let mut found = Vec::new();
            for child in &children {
                walk(child, &mut found, &mut Vec::new());
            }
            *already += found.len().max(1);
            return;
        } else {
            if let Some(id) = id {
                ids.push(id);
            }
            let Some(target) = children_at(list, parent) else {
                return;
            };
            let mut path = parent.to_vec();
            path.push(target.len());
            target.push(node);
            path
        };
        let before = (*added, *already);
        for child in children {
            merge_node(list, &path, child, urls, ids, added, already);
        }
        if is_existing && before == (*added, *already) {
            *already += 1;
        }
    } else if id.as_ref().is_some_and(|id| ids.contains(id)) {
        *already += 1;
    } else {
        if let Some(id) = id {
            ids.push(id);
        }
        if let Some(target) = children_at(list, parent) {
            target.push(node);
        }
    }
}

fn walk(node: &Value, urls: &mut Vec<String>, ids: &mut Vec<String>) {
    if let Some(id) = node.get("id").and_then(Value::as_str) {
        ids.push(id.to_owned());
    }
    if let Some(url) = node.get("url").and_then(Value::as_str) {
        urls.push(url.to_owned());
    }
    if let Some(Value::Array(children)) = node.get("children") {
        for child in children {
            walk(child, urls, ids);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_open_here_already_stay_out_with_the_groups_they_leave_empty() {
        let tab = |url: &str, group: &str| tap_core::session::Entry {
            url: url.to_owned(),
            title: url.to_owned(),
            group_id: Some(group.to_owned()),
            ..Default::default()
        };
        let group = |id: &str| tap_core::session::TabGroup {
            id: id.to_owned(),
            name: id.to_owned(),
            collapsed: false,
            colour: None,
        };
        let mut incoming = Incoming {
            sessions: vec![
                Shape {
                    tabs: vec![
                        tab("https://a/", "G"),
                        tab("https://b/", "G"),
                        tab("https://c/", "H"),
                    ],
                    active: -1,
                    groups: Some(vec![group("G"), group("H")]),
                    ..Shape::default()
                },
                Shape {
                    tabs: vec![tab("https://a/", "G")],
                    active: -1,
                    ..Shape::default()
                },
            ],
            ..Incoming::default()
        };
        let open = ["https://a/", "https://c/"].map(str::to_owned).into();
        incoming.leave_out(&open);
        assert_eq!(incoming.sessions.len(), 1);
        assert_eq!(incoming.tabs(), 1);
        assert_eq!(incoming.sessions[0].tabs[0].url, "https://b/");
        let groups = incoming.sessions[0].groups.as_ref().expect("G stays");
        assert_eq!(
            groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
            ["G"]
        );
    }

    fn session(urls: &[&str]) -> Vec<u8> {
        Shape {
            tabs: urls
                .iter()
                .map(|u| tap_core::session::Entry {
                    url: (*u).to_owned(),
                    title: (*u).to_owned(),
                    ..Default::default()
                })
                .collect(),
            active: 0,
            ..Shape::default()
        }
        .encode()
    }

    fn outgoing(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Outgoing<'_> {
        let mut prefs = Map::new();
        prefs.insert("search.engine".into(), "kagi".into());
        Outgoing {
            session: session(&["https://a.com", "https://b.com"]),
            history: br#"[{"url":"https://a.com","key":"a.com","title":"A","count":2,"last":1}]"#
                .to_vec(),
            read,
            prefs,
            extensions: vec![],
            app_version: "0.1.1".into(),
            now: 815_000_000.0,
        }
    }

    #[test]
    fn a_profile_goes_out_and_comes_back_whole() {
        let read = |name: &str| {
            (name == "bookmarks.json")
                .then(|| br#"[{"id":"1","title":"R","url":"https://r.com"}]"#.to_vec())
        };
        let bytes = pack(&outgoing(&read)).expect("packs");
        let incoming = unpack(&bytes).expect("unpacks");
        let manifest = incoming.manifest.expect("a manifest");
        assert_eq!((manifest.format, manifest.scope.as_str()), (1, "profile"));
        assert_eq!(manifest.counts["prefs"], 1);
        assert_eq!(manifest.counts["bookmarks"], 1);
        assert_eq!(incoming.sessions.len(), 1);
        assert_eq!(incoming.sessions[0].tabs[1].url, "https://b.com");
        assert_eq!(incoming.histories.len(), 1);
        assert_eq!(incoming.bookmarks.len(), 1);
        assert_eq!(incoming.prefs["search.engine"], "kagi");
        assert!(is_profile(&bytes));
    }

    /// Search's #845 import brings in only the sessions of spaces the file lists.
    #[test]
    fn personal_alone_still_lists_its_space() {
        let bytes = pack(&outgoing(&|_| None)).expect("packs");
        let files = flat_files(&bytes).expect("a zip");
        let spaces: Vec<Value> =
            serde_json::from_slice(&files["spaces.json"]).expect("a list of spaces");
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0]["id"], FIRST_SPACE);
        assert_eq!(spaces[0]["name"], "Personal");

        let kept = br#"[{"id":"X"}]"#;
        let read = |name: &str| (name == "spaces.json").then(|| kept.to_vec());
        let bytes = pack(&outgoing(&read)).expect("packs");
        assert_eq!(flat_files(&bytes).expect("a zip")["spaces.json"], kept);
    }

    #[test]
    fn a_file_from_a_newer_version_or_no_profile_at_all_is_refused() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .expect("starts");
        zip.write_all(
            br#"{"format":2,"appVersion":"9","scope":"profile","exportedAt":0,"counts":{}}"#,
        )
        .expect("writes");
        let newer = zip.finish().expect("finishes").into_inner();
        assert_eq!(unpack(&newer).err(), Some(Problem::Format(2)));
        assert!(
            is_profile(&newer),
            "a profile still, just not one this version reads"
        );
        assert_eq!(unpack(b"not a zip").err(), Some(Problem::Unreadable));
        assert!(!is_profile(b"not a zip"));
    }

    /// A name with a path in it is never read, let alone written anywhere.
    #[test]
    fn only_flat_files_are_read() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, data) in [
            (
                "manifest.json",
                &br#"{"format":1,"appVersion":"1","scope":"profile","exportedAt":0,"counts":{}}"#[..],
            ),
            ("../session.json", &session(&["https://evil.com"])[..]),
            ("deep/session.json", &session(&["https://evil.com"])[..]),
            ("session-NOTAUUID.json", &session(&["https://evil.com"])[..]),
        ] {
            zip.start_file(name, options).expect("starts");
            zip.write_all(data).expect("writes");
        }
        let bytes = zip.finish().expect("finishes").into_inner();
        assert_eq!(unpack(&bytes).expect("unpacks").sessions.len(), 0);
    }

    #[test]
    fn spaces_come_in_their_order_the_first_first() {
        let other = "AAAAAAAA-1111-4000-8000-000000000002";
        let last = "BBBBBBBB-1111-4000-8000-000000000003";
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let spaces = format!(r#"[{{"id":"{FIRST_SPACE}"}},{{"id":"{last}"}},{{"id":"{other}"}}]"#);
        for (name, data) in [
            (
                "manifest.json".to_owned(),
                br#"{"format":1,"appVersion":"1","scope":"profile","exportedAt":0,"counts":{}}"#
                    .to_vec(),
            ),
            ("spaces.json".to_owned(), spaces.into_bytes()),
            (
                format!("session-{other}.json"),
                session(&["https://other.com"]),
            ),
            (
                format!("session-{last}.json"),
                session(&["https://last.com"]),
            ),
            ("session.json".to_owned(), session(&["https://first.com"])),
        ] {
            zip.start_file(name, options).expect("starts");
            zip.write_all(&data).expect("writes");
        }
        let bytes = zip.finish().expect("finishes").into_inner();
        let firsts: Vec<String> = unpack(&bytes)
            .expect("unpacks")
            .sessions
            .iter()
            .map(|s| s.tabs[0].url.clone())
            .collect();
        assert_eq!(
            firsts,
            ["https://first.com", "https://last.com", "https://other.com"]
        );
    }

    #[test]
    fn safari_reimport_merges_nested_folders_and_deduplicates_urls() {
        let read = |html: &str| {
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file("bookmarks.html", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(html.as_bytes()).unwrap();
            crate::safari::read(&zip.finish().unwrap().into_inner())
                .unwrap()
                .incoming
                .bookmarks
        };
        let first = "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><H3>Folder</H3><DL><H3>Nested</H3><DL><A HREF='https://a.com/'>A</A></DL></DL></DL>";
        let second = "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><H3>Folder</H3><DL><H3>Nested</H3><DL><A HREF='https://a.com/'>Changed</A><A HREF='https://b.com/'>B</A></DL><H3>New</H3><DL><A HREF='https://b.com/'>Duplicate</A><A HREF='https://c.com/'>C</A></DL></DL></DL>";
        let (kept, added, already) = merge_bookmarks(None, read(first));
        assert_eq!((added, already), (1, 0));
        let (merged, added, already) = merge_bookmarks(Some(&kept), read(second));
        assert_eq!((added, already), (2, 2));
        let nodes: Vec<Value> = serde_json::from_slice(&merged).unwrap();
        assert_eq!(nodes[0]["children"][0]["children"][0]["title"], "A");
        assert_eq!(
            nodes[0]["children"][0]["children"][1]["url"],
            "https://b.com/"
        );
        assert_eq!(
            nodes[0]["children"][1]["children"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let (again, added, already) = merge_bookmarks(Some(&merged), read(second));
        assert_eq!((added, already), (0, 4));
        assert_eq!(again, merged);
    }

    #[test]
    fn bookmarks_merge_without_doubles() {
        let kept = br#"[{"id":"1","title":"A","url":"https://a.com"}]"#;
        let incoming: Vec<Value> = serde_json::from_str(
            r#"[{"id":"9","title":"A again","url":"https://a.com"},
                {"id":"2","title":"B","url":"https://b.com"},
                {"id":"F","title":"Folder","children":[{"id":"3","title":"C","url":"https://c.com"}]}]"#,
        )
        .expect("json");
        let (data, added, already) = merge_bookmarks(Some(kept), incoming.clone());
        assert_eq!((added, already), (2, 1));
        let (_, added, already) = merge_bookmarks(Some(&data), incoming);
        assert_eq!((added, already), (0, 3), "the same file again adds nothing");
    }
}
