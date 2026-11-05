//! What Tap brings in from elsewhere and sends out (docs/PLAN.md §4, §7): a whole profile
//! as one offline file, Chrome and Safari browser data, and a passwords CSV another browser
//! exported. No AppKit, no WebKit, no keychain: everything here runs under plain `cargo test`.

#![deny(unsafe_code)]

pub mod chromium;
pub mod passwords;
pub mod profile;
pub mod safari;
pub mod snss;

use std::path::Path;

/// A browser on this Mac with something to bring over (Search's `ImportSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Safari,
    Chromium(&'static chromium::Browser),
}

impl Source {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Safari => "Safari",
            Self::Chromium(browser) => browser.name,
        }
    }
}

/// Chrome with a profile under `base` (`~/Library/Application Support`), and Safari,
/// whose file-based import is always available.
#[must_use]
pub fn found(base: &Path) -> Vec<Source> {
    chromium::KNOWN
        .iter()
        .filter(|b| b.name == "Chrome" && !chromium::profiles(&b.root(base)).is_empty())
        .map(Source::Chromium)
        .chain(std::iter::once(Source::Safari))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_chrome_with_a_profile_and_safari_are_found() {
        let base = std::env::temp_dir().join(format!(
            "tap-found-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir(&base).expect("created fixture");
        let write = |path: &str, data: &[u8]| {
            let path = base.join(path);
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("made");
            std::fs::write(path, data).expect("written");
        };
        assert_eq!(found(&base), [Source::Safari]);
        write("Google/Chrome/NativeMessagingHosts/x.json", b"{}");
        write("Search/session.json", br#"{"tabs":[],"active":-1}"#);
        write("BraveSoftware/Brave-Browser/Default/History", b"");
        write("Google/Chrome Beta/Default/History", b"");
        assert_eq!(found(&base), [Source::Safari]);
        write("Google/Chrome/Profile 3/Bookmarks", b"{}");
        let names: Vec<&str> = found(&base).into_iter().map(Source::name).collect();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(names, ["Chrome", "Safari"]);
    }
}
