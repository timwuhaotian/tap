//! What Tap brings in from elsewhere and sends out (docs/PLAN.md §4, §7): a whole profile
//! as one offline file (Search #845), and what the other browsers on this Mac hold:
//! Search's own profile, and a Chromium browser's open tabs (Search #841), history,
//! bookmarks and passwords, and a passwords CSV another browser exported. No AppKit, no
//! WebKit, no keychain: everything here runs under plain `cargo test`.

#![deny(unsafe_code)]

pub mod chromium;
pub mod from_search;
pub mod passwords;
pub mod profile;
pub mod snss;

use std::path::Path;

/// A browser on this Mac with something to bring over (Search's `ImportSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Search itself: its store folder and its settings.
    Search,
    Chromium(&'static chromium::Browser),
}

impl Source {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Chromium(browser) => browser.name,
        }
    }
}

/// Every browser with something to bring, under `base` (`~/Library/Application
/// Support`): Search first, the one Tap comes from, then the Chromium browsers in
/// Search's order. Folders a browser's helpers left without a profile don't count.
#[must_use]
pub fn found(base: &Path) -> Vec<Source> {
    let search = from_search::present(&base.join(from_search::FOLDER)).then_some(Source::Search);
    search
        .into_iter()
        .chain(
            chromium::KNOWN
                .iter()
                .filter(|b| !chromium::profiles(&b.root(base)).is_empty())
                .map(Source::Chromium),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_and_chromium_browsers_with_profiles_are_found() {
        let base = std::env::temp_dir().join(format!("tap-found-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let write = |path: &str, data: &[u8]| {
            let path = base.join(path);
            std::fs::create_dir_all(path.parent().expect("a folder")).expect("made");
            std::fs::write(path, data).expect("written");
        };
        assert_eq!(found(&base), []);
        write("Arc/User Data/NativeMessagingHosts/x.json", b"{}");
        write("Search/history.json", b"[]");
        assert!(
            found(&base).is_empty(),
            "no profile and no session: nothing to bring"
        );
        write("Search/session.json", br#"{"tabs":[],"active":-1}"#);
        write("BraveSoftware/Brave-Browser/Default/History", b"");
        write("Google/Chrome/Profile 3/Bookmarks", b"{}");
        let names: Vec<&str> = found(&base).into_iter().map(Source::name).collect();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(names, ["Search", "Chrome", "Brave"]);
    }
}
