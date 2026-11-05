//! Safari's exported browsing-data ZIP. Nothing is extracted to disk.
//!
//! Apple documents the archive and its formats at
//! <https://developer.apple.com/documentation/safariservices/importing-data-exported-from-safari>.

use std::io::{Cursor, Read};

use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tap_core::fnd;
use zip::ZipArchive;

use crate::{passwords, profile::Incoming, snss};

const FILE_LIMIT: u64 = 64 << 20;
const TOTAL_LIMIT: u64 = 256 << 20;
const ENTRY_LIMIT: usize = 4096;
const APPLE_EPOCH: f64 = 978_307_200.0;

#[derive(Clone, Debug, Default)]
pub struct SafariImport {
    pub incoming: Incoming,
    pub passwords: Vec<passwords::Login>,
    pub password_skipped: usize,
}

/// Reads an export using its contents rather than localized filenames.
///
/// # Errors
/// If the ZIP is invalid, exceeds the limits, or has no recognizable supported data.
pub fn read(bytes: &[u8]) -> Result<SafariImport, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if zip.len() > ENTRY_LIMIT {
        return Err("Too many ZIP entries".into());
    }
    let mut out = SafariImport::default();
    let mut total = 0_u64;
    let mut recognized = false;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().map_err(|e| e.to_string())?;
        if name.starts_with("__MACOSX/")
            || name.rsplit('/').next().is_some_and(|n| n.starts_with("._"))
        {
            continue;
        }
        let remaining = TOTAL_LIMIT - total;
        let cap = FILE_LIMIT.min(remaining);
        if file.size() > cap {
            return Err("Safari export exceeds decompression limit".into());
        }
        let mut data = Vec::new();
        (&mut file)
            .take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        let size = u64::try_from(data.len()).map_err(|e| e.to_string())?;
        if size > cap {
            return Err("Safari export exceeds decompression limit".into());
        }
        total += size;
        let Ok(text) = std::str::from_utf8(&data) else {
            continue;
        };
        let text = text.trim_start_matches('\u{feff}');
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            let meta = &value["metadata"];
            let browser = meta["browser_name"].as_str();
            if matches!(browser, Some("Safari" | "Safari Technology Preview"))
                && meta["data_type"] == "history"
            {
                let rows = value["history"]
                    .as_array()
                    .ok_or("Invalid Safari history")?;
                let mut history = Vec::new();
                for row in rows {
                    let Some(url) = row["url"].as_str().filter(|url| snss::is_web(url)) else {
                        continue;
                    };
                    let Some(time) = row["time_usec"].as_i64() else {
                        continue;
                    };
                    let Some(count) = row["visits_count"].as_i64().filter(|n| *n > 0) else {
                        continue;
                    };
                    #[allow(clippy::cast_precision_loss)]
                    let last = time as f64 / 1_000_000.0 - APPLE_EPOCH;
                    history.push(json!({"url": url, "title": row["title"].as_str().unwrap_or_default(), "count": count, "last": last}));
                }
                out.incoming
                    .histories
                    .push(serde_json::to_vec(&history).map_err(|e| e.to_string())?);
                recognized = true;
            }
        } else if text
            .to_ascii_lowercase()
            .contains("<!doctype netscape-bookmark-file-1")
        {
            out.incoming.bookmarks.extend(bookmarks(text)?);
            recognized = true;
        } else if is_password_csv(text) {
            let parsed = passwords::csv(text);
            out.password_skipped += parsed.skipped;
            out.passwords.extend(parsed.logins);
            recognized = true;
        }
    }
    if !recognized {
        return Err("No supported Safari browsing data found".into());
    }
    Ok(out)
}

fn is_password_csv(text: &str) -> bool {
    let Some(header) = text.lines().next() else {
        return false;
    };
    let columns: Vec<String> = header
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_ascii_lowercase())
        .collect();
    ["title", "url", "username", "password", "notes", "otpauth"]
        .iter()
        .all(|name| columns.iter().any(|col| col == name))
}

struct Folder {
    id: String,
    title: String,
    children: Vec<Value>,
}

fn folder_id(parent: &str, title: &str, occurrence: usize) -> String {
    let mut hash = Sha1::new();
    hash.update(b"tap-safari-bookmark-folder\0");
    hash.update(parent.as_bytes());
    hash.update((title.len() as u64).to_be_bytes());
    hash.update(title.as_bytes());
    hash.update((occurrence as u64).to_be_bytes());
    let mut bytes = hash.finalize();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

// Netscape's DL/H3/A structure, preserving folders and the Reading List.
fn bookmarks(text: &str) -> Result<Vec<Value>, String> {
    let mut stack = vec![Folder {
        id: String::new(),
        title: String::new(),
        children: Vec::new(),
    }];
    let mut pending: Option<String> = None;
    let mut pos = 0;
    while let Some(start) = text[pos..].find('<').map(|n| n + pos) {
        let Some(end) = text[start..].find('>').map(|n| n + start) else {
            break;
        };
        let tag = &text[start + 1..end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("h3")
            && lower
                .as_bytes()
                .get(2)
                .is_some_and(|c| c.is_ascii_whitespace() || *c == b'>')
            || lower == "h3"
        {
            if let Some(close) = find_ascii_case(&text[end + 1..], "</h3>") {
                pending = Some(entities(&text[end + 1..end + 1 + close]));
                pos = end + 1 + close + 5;
                continue;
            }
        } else if lower == "dl" || lower.starts_with("dl ") {
            if let Some(title) = pending.take() {
                if stack.len() >= 128 {
                    return Err("Safari bookmark nesting exceeds limit".into());
                }
                let parent = stack.last().ok_or("Invalid Safari bookmarks")?;
                let occurrence = parent
                    .children
                    .iter()
                    .filter(|node| node.get("children").is_some() && node["title"] == title)
                    .count();
                let id = folder_id(&parent.id, &title, occurrence);
                stack.push(Folder {
                    id,
                    title,
                    children: Vec::new(),
                });
            }
        } else if lower.starts_with("/dl") && stack.len() > 1 {
            if let Some(folder) = stack.pop()
                && let Some(parent) = stack.last_mut()
            {
                parent.children.push(
                    json!({"id": folder.id, "title": folder.title, "children": folder.children}),
                );
            }
        } else if lower.starts_with("a ")
            && let Some(close) = find_ascii_case(&text[end + 1..], "</a>")
        {
            if let Some(url) = attribute(tag, "href")
                .map(|s| entities(&s))
                .filter(|u| snss::is_web(u))
                && let Some(parts) = fnd::Parts::of(&url)
                && let Some(parent) = stack.last_mut()
            {
                parts.clear_credentials();
                if let Some(url) = parts.string().filter(|u| snss::is_web(u)) {
                    let title = entities(&text[end + 1..end + 1 + close]);
                    parent
                        .children
                        .push(json!({"id": fnd::uuid(), "title": title, "url": url}));
                }
            }
            pos = end + 1 + close + 4;
            continue;
        }
        pos = end + 1;
    }
    // Malformed, unclosed folders are not silently promoted to the root.
    Ok(stack.remove(0).children)
}

fn find_ascii_case(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
}

fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut i = tag.find(char::is_whitespace)?;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let name = &tag[start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let quote = bytes[i];
        let quoted = quote == b'"' || quote == b'\'';
        if quoted {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && if quoted {
                bytes[i] != quote
            } else {
                !bytes[i].is_ascii_whitespace()
            }
        {
            i += 1;
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(tag[start..i].to_owned());
        }
        if quoted && i < bytes.len() {
            i += 1;
        }
    }
    None
}

fn entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|n| *n <= 16) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(c) = decoded {
            out.push(c);
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            zip.start_file(
                *name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_apple_formats_with_localized_names() {
        let bytes = archive(&[
            (
                "favoris.html",
                "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><DT><H3>Folder &amp; more</H3><DL><DT><A HREF=\"https://example.com/?a=1&amp;b=2\">Example</A></DL></DL>",
            ),
            (
                "history-profile.json",
                r#"{"metadata":{"browser_name":"Safari","data_type":"history"},"history":[{"url":"https://example.com/","time_usec":978307201000000,"visits_count":2}]}"#,
            ),
            (
                "passwords.csv",
                "Title,URL,Username,Password,Notes,OTPAuth\nExample,https://example.com,alice,secret,,\n",
            ),
        ]);
        let result = read(&bytes).unwrap();
        assert_eq!(result.incoming.bookmark_count(), 1);
        assert_eq!(result.incoming.bookmarks[0]["title"], "Folder & more");
        assert_eq!(
            result.incoming.bookmarks[0]["children"][0]["url"],
            "https://example.com/?a=1&b=2"
        );
        let history: Value = serde_json::from_slice(&result.incoming.histories[0]).unwrap();
        assert_eq!(history[0]["last"], 1.0);
        assert_eq!(result.passwords[0].user, "alice");
    }

    #[test]
    fn bookmark_credentials_are_removed_without_changing_the_rest() {
        let nodes = bookmarks("<DL><A HREF='https://user:p%40ss@example.com:8443/a%20b?q=1&amp;x=2#part'>Site</A><A HREF='https://user@example.org/'>Other</A></DL>").unwrap();
        assert_eq!(
            nodes[0]["url"],
            "https://example.com:8443/a%20b?q=1&x=2#part"
        );
        assert_eq!(nodes[1]["url"], "https://example.org/");
    }

    #[test]
    fn password_skips_accumulate_across_files() {
        let header = "Title,URL,Username,Password,Notes,OTPAuth\n";
        let first = format!(
            "{header}Valid,https://example.com,alice,secret,,\nMissing,https://example.com,bob,,,\n"
        );
        let second = format!("{header}Invalid,android://app,bob,secret,,\n");
        let result = read(&archive(&[("one.csv", &first), ("two.csv", &second)])).unwrap();
        assert_eq!(result.passwords.len(), 1);
        assert_eq!(result.password_skipped, 2);
    }

    #[test]
    fn folders_have_stable_distinct_ids_and_reimport_deduplicates() {
        let html = "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><H3>Same</H3><DL><H3>Nested</H3><DL><A HREF='https://example.com/'>A</A></DL></DL><H3>Same</H3><DL><H3>Nested</H3><DL><A HREF='https://example.org/'>B</A></DL></DL></DL>";
        let bytes = archive(&[("bookmarks.html", html)]);
        let first = read(&bytes).unwrap().incoming.bookmarks;
        let second = read(&bytes).unwrap().incoming.bookmarks;
        assert_eq!(first[0]["id"], second[0]["id"]);
        assert_eq!(
            first[0]["children"][0]["id"],
            second[0]["children"][0]["id"]
        );
        assert_ne!(first[0]["id"], first[1]["id"]);
        assert_ne!(first[0]["children"][0]["id"], first[1]["children"][0]["id"]);
        assert!(fnd::is_uuid(first[0]["id"].as_str().unwrap()));
        let (kept, added, _) = crate::profile::merge_bookmarks(None, first);
        let (merged, added_again, already) = crate::profile::merge_bookmarks(Some(&kept), second);
        assert_eq!((added, added_again, already), (2, 0, 2));
        assert_eq!(kept, merged);
    }

    #[test]
    fn rejects_unknown_invalid_and_oversized_data() {
        assert!(read(b"not zip").is_err());
        assert!(read(&archive(&[("history.json", "{}")])).is_err());
        let huge = "a".repeat(usize::try_from(FILE_LIMIT + 1).unwrap());
        assert!(read(&archive(&[("huge", &huge)])).is_err());
    }
}
