//! Search's own profile on this Mac as Tap's profile file (docs/PLAN.md §7, "Bring things
//! over from Search"): its store folder read as `SearchProfile.collectCurrentProfile`
//! reads it, then packed by [`profile::pack`], so the file is the one File › Export Tap
//! Profile… writes and File › Import Profile… reads.
//!
//! Every file goes in as Search wrote it, byte for byte. Personal's files and its first
//! space's session go in, as Tap's own export carries them (Tap has one profile and one
//! space so far); Search's other spaces and profiles stay behind.

use std::path::Path;

use serde_json::{Map, Value};

use crate::profile::{self, Outgoing};

/// Search's profile in `folder` (`~/Library/Application Support/Search`), with its
/// allowlisted settings already in a profile file's JSON shape.
///
/// # Errors
/// If the folder has no session, so is no Search store, or the zip can't be written.
pub fn pack(
    folder: &Path,
    prefs: Map<String, Value>,
    app_version: &str,
    now: f64,
) -> Result<Vec<u8>, String> {
    let read = |name: &str| std::fs::read(folder.join(name)).ok();
    let session =
        read("session.json").ok_or_else(|| format!("{} has no session.json", folder.display()))?;
    profile::pack(&Outgoing {
        session,
        history: read("history.json").unwrap_or_else(|| b"[]".to_vec()),
        read: &read,
        prefs,
        extensions: extension_ids(read("Extensions/installed.json").as_deref()),
        app_version: app_version.to_owned(),
        now,
    })
}

/// The store ids of what Search has installed, whatever else its list carries (Search's
/// `extensionIDs`): only the ids travel.
fn extension_ids(list: Option<&[u8]>) -> Vec<String> {
    let Some(Ok(Value::Array(list))) = list.map(serde_json::from_slice::<Value>) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::unpack;

    #[test]
    fn a_search_folder_packs_as_a_profile_file() {
        let folder = std::env::temp_dir().join(format!("tap-from-search-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("Extensions")).expect("a folder");
        let session = br#"{"tabs":[{"url":"https://a.com","title":"A","groupID":"11111111-1111-4111-8111-111111111111"},{"url":"https://b.com","title":"B"}],"active":1,"groups":[{"id":"11111111-1111-4111-8111-111111111111","name":"Work","collapsed":true}],"splits":[]}"#;
        for (name, data) in [
            ("session.json", &session[..]),
            (
                "history.json",
                br#"[{"url":"https://a.com","key":"a.com","title":"A","count":3,"last":1}]"#,
            ),
            (
                "bookmarks.json",
                br#"[{"id":"1","title":"R","url":"https://r.com"}]"#,
            ),
            (
                "Extensions/installed.json",
                br#"[{"id":"abcdefghijklmnopabcdefghijklmnop","path":"/x"}]"#,
            ),
        ] {
            std::fs::write(folder.join(name), data).expect("writes");
        }
        let mut prefs = Map::new();
        prefs.insert("search.engine".into(), "kagi".into());

        let bytes = pack(&folder, prefs, "0.1.1", 815_000_000.0).expect("packs");
        let _ = std::fs::remove_dir_all(&folder);
        let incoming = unpack(&bytes).expect("unpacks");
        assert_eq!(incoming.sessions.len(), 1);
        let shape = &incoming.sessions[0];
        assert_eq!(shape.tabs.len(), 2);
        let groups = shape.groups.as_ref().expect("groups");
        assert_eq!(
            (groups[0].name.as_str(), groups[0].collapsed),
            ("Work", true)
        );
        assert_eq!(incoming.histories.len(), 1);
        assert_eq!(incoming.bookmarks.len(), 1);
        assert_eq!(incoming.prefs["search.engine"], "kagi");
        assert_eq!(incoming.extensions, ["abcdefghijklmnopabcdefghijklmnop"]);
    }

    #[test]
    fn a_folder_without_a_session_is_not_a_search_store() {
        let folder = std::env::temp_dir().join("tap-from-search-nothing-here");
        assert!(pack(&folder, Map::new(), "0.1.1", 0.0).is_err());
    }
}
