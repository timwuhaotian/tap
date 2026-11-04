//! A site's own icon, for the tabs (Search's `Icons.swift`, its rules).
//!
//! WebKit doesn't hand icons over, so the page is asked what it declares; the best of
//! those is fetched once and kept as a small PNG beside the history, named after its own
//! address, and found again by site and page. A tab brought back from yesterday has its
//! icon before it has a page. `icons/pages.json` is Search's own index, so a Search
//! profile's icons read as they are.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fnd::Url;

/// More pages than this, and the ones seen longest ago are forgotten.
const MOST: usize = 4000;
/// Declared icons tried for one page, best first; favicon.ico makes six.
const TRIED: usize = 5;

/// What a page declares: one `<link rel="…icon…">`, as the page reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Declared {
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub sizes: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub media: String,
}

/// Which look an icon is meant for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Any,
    Light,
    Dark,
}

fn look(media: &str) -> Look {
    let media = media.replace(' ', "");
    if media.contains("prefers-color-scheme:dark") {
        Look::Dark
    } else if media.contains("prefers-color-scheme:light") {
        Look::Light
    } else {
        Look::Any
    }
}

/// Which site an address is, for its icon: its host, and its port when it names one
/// (localhost:3000 and localhost:4321 are two projects). The scheme's own port doesn't
/// count.
#[must_use]
pub fn site(url: &Url) -> Option<String> {
    let host = url.host()?.to_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = crate::fnd::Parts::of(url.as_str()).and_then(|p| p.port());
    let scheme = url.scheme().unwrap_or_default().to_lowercase();
    let own = match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    match port {
        Some(port) if Some(port) != own => Some(format!("{host}:{port}")),
        _ => Some(host),
    }
}

/// Which page of its site an address is: its path. The query and the fragment are left
/// off: neither changes what a page declares, and a query can carry a token.
#[must_use]
pub fn page(url: &Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".to_owned()
    } else {
        path
    }
}

/// The name an icon is kept under: its own address, so pages pointing at one file
/// share one copy.
#[must_use]
pub fn ident(icon: &str) -> String {
    use std::fmt::Write;
    Sha256::digest(icon.as_bytes())[..16]
        .iter()
        .fold(String::with_capacity(32), |mut name, b| {
            let _ = write!(name, "{b:02x}");
            name
        })
}

/// The icons to try for a page, best first (Search's `rank`): a crisp one around 32–64
/// pixels, then the touch icon, then the file at the root every site has had since 1999.
/// An icon for the other look is skipped; one for this look comes first.
///
/// Not yet: SVG and inline icons, which Search draws in a web view of their own rather
/// than in the app.
#[must_use]
pub fn rank(declared: &[Declared], page: &Url, dark: bool) -> Vec<String> {
    let mut scored: Vec<(String, i32)> = Vec::new();
    for entry in declared {
        let href = entry.href.trim();
        let lower = href.to_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            continue;
        }
        let svg = entry.kind.contains("svg")
            || entry.sizes == "any"
            || lower.split(['?', '#']).next().is_some_and(|p| {
                std::path::Path::new(p)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
            });
        if svg {
            continue;
        }
        let meant = look(&entry.media);
        if meant == if dark { Look::Light } else { Look::Dark } {
            continue;
        }
        let mut score = if entry.rel.contains("apple-touch") {
            40
        } else {
            25
        };
        let largest = entry
            .sizes
            .split(' ')
            .filter_map(|size| size.split('x').next().and_then(|w| w.parse::<u32>().ok()))
            .max();
        if let Some(px) = largest {
            score = match px {
                0..24 => 10,
                24..48 => 45,
                48..128 => 50,
                128..260 => 42,
                _ => 20,
            };
        }
        if meant != Look::Any {
            score += 40;
        }
        scored.push((href.to_owned(), score));
    }
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut list: Vec<String> = scored.into_iter().take(TRIED).map(|(url, _)| url).collect();
    if let Some(root) = crate::fnd::Parts::of(page.as_str()).and_then(|parts| {
        parts.clear_credentials();
        parts.set_path("/favicon.ico");
        parts.clear_query();
        parts.clear_fragment();
        parts.url()
    }) && page.host().is_some()
    {
        list.push(root.as_str().to_owned());
    }
    let mut seen = std::collections::HashSet::new();
    list.retain(|url| seen.insert(url.clone()));
    list
}

/// What a page was last seen wearing, in each look, by the icon's name; when (seconds
/// since 2001), 0 for one that stands for the whole site.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<String>,
    #[serde(default)]
    pub seen: f64,
}

impl Page {
    fn wears(&self, dark: bool) -> Option<&str> {
        if dark {
            self.dark.as_deref().or(self.light.as_deref())
        } else {
            self.light.as_deref().or(self.dark.as_deref())
        }
    }
}

/// Site, then page, then icon: kept the way WebKit, Firefox and Chromium keep theirs.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pages: HashMap<String, HashMap<String, Page>>,
}

impl Index {
    /// `pages.json`, or `None` for a file that won't decode.
    #[must_use]
    pub fn load(data: &[u8]) -> Option<Index> {
        serde_json::from_slice(data)
            .ok()
            .map(|pages| Index { pages })
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(&self.pages).unwrap_or_default()
    }

    /// The page wears `icon` now. The pages seen longest ago go once there are too many.
    pub fn record(&mut self, url: &Url, icon: &str, dark: bool, now: f64) {
        let Some(site) = site(url) else { return };
        let entry = self
            .pages
            .entry(site)
            .or_default()
            .entry(page(url))
            .or_default();
        if dark {
            entry.dark = Some(icon.to_owned());
        } else {
            entry.light = Some(icon.to_owned());
        }
        entry.seen = now;
        let count: usize = self.pages.values().map(HashMap::len).sum();
        if count > MOST {
            let mut all: Vec<(f64, String, String)> = self
                .pages
                .iter()
                .flat_map(|(site, pages)| {
                    pages
                        .iter()
                        .map(move |(page, p)| (p.seen, site.clone(), page.clone()))
                })
                .collect();
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, site, page) in all.into_iter().take(count - MOST * 3 / 4) {
                if let Some(pages) = self.pages.get_mut(&site) {
                    pages.remove(&page);
                }
            }
            self.pages.retain(|_, pages| !pages.is_empty());
        }
    }

    /// The icon a page wears: its own, or the page of its site that shares the most of
    /// its path; the same site with or without www. A host whose pages don't agree on one
    /// icon is several apps, and another one's page is no guide.
    #[must_use]
    pub fn lookup(&self, url: &Url, dark: bool) -> Option<String> {
        let site = site(url)?;
        let other = site
            .strip_prefix("www.")
            .map_or_else(|| format!("www.{site}"), str::to_owned);
        let page = page(url);
        self.nearest(&site, &page, dark)
            .or_else(|| self.nearest(&other, &page, dark))
    }

    fn nearest(&self, site: &str, page: &str, dark: bool) -> Option<String> {
        let kept = self.pages.get(site)?;
        if let Some(icon) = kept.get(page).and_then(|p| p.wears(dark)) {
            return Some(icon.to_owned());
        }
        let mine: Vec<&str> = page.split('/').filter(|s| !s.is_empty()).collect();
        let shared = |other: &str| {
            mine.iter()
                .zip(other.split('/').filter(|s| !s.is_empty()))
                .take_while(|(a, b)| *a == b)
                .count()
        };
        let agreed = kept
            .values()
            .filter_map(|p| p.wears(dark))
            .collect::<std::collections::HashSet<_>>()
            .len()
            <= 1;
        let mut scored: Vec<(usize, bool, &Page)> = kept
            .iter()
            .map(|(key, p)| (shared(key), key == "/", p))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.2.seen.total_cmp(&a.2.seen)));
        scored
            .into_iter()
            .filter(|(shared, root, _)| *shared > 0 || *root || agreed)
            .find_map(|(_, _, p)| p.wears(dark).map(str::to_owned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("parses")
    }

    fn declared(href: &str, rel: &str, sizes: &str) -> Declared {
        Declared {
            href: href.into(),
            rel: rel.into(),
            sizes: sizes.into(),
            ..Declared::default()
        }
    }

    #[test]
    fn sites_keep_their_port_but_not_the_schemes_own() {
        assert_eq!(
            site(&url("http://localhost:3000/a")).as_deref(),
            Some("localhost:3000")
        );
        assert_eq!(
            site(&url("https://Example.com:443/a")).as_deref(),
            Some("example.com")
        );
        assert_eq!(page(&url("https://example.com")), "/");
        assert_eq!(page(&url("https://example.com/a/b?token=1#x")), "/a/b");
    }

    #[test]
    fn a_crisp_icon_first_then_the_touch_icon_then_the_root_file() {
        let page = url("https://example.com/docs");
        let list = rank(
            &[
                declared("https://example.com/16.png", "icon", "16x16"),
                declared("https://example.com/touch.png", "apple-touch-icon", ""),
                declared("https://example.com/32.png", "icon", "32x32"),
                declared("https://example.com/i.svg", "icon", "any"),
                declared("data:image/png;base64,AAAA", "icon", ""),
            ],
            &page,
            false,
        );
        assert_eq!(
            list,
            [
                "https://example.com/32.png",
                "https://example.com/touch.png",
                "https://example.com/16.png",
                "https://example.com/favicon.ico"
            ]
        );
        let dark = Declared {
            media: "(prefers-color-scheme: dark)".into(),
            ..declared("https://example.com/dark.png", "icon", "")
        };
        assert_eq!(
            rank(std::slice::from_ref(&dark), &page, false),
            ["https://example.com/favicon.ico"]
        );
        assert_eq!(
            rank(&[dark], &page, true)[0],
            "https://example.com/dark.png"
        );
    }

    #[test]
    fn names_are_the_icon_address_hashed_as_search_names_them() {
        assert_eq!(ident("https://example.com/favicon.ico").len(), 32);
        assert_ne!(
            ident("https://a.example/f.ico"),
            ident("https://b.example/f.ico")
        );
    }

    #[test]
    fn a_page_wears_its_own_icon_or_the_nearest_of_its_site() {
        let mut index = Index::default();
        index.record(&url("https://mac-mini/sonarr/queue"), "sonarr", false, 10.0);
        index.record(
            &url("https://mac-mini/radarr/movies"),
            "radarr",
            false,
            20.0,
        );
        assert_eq!(
            index
                .lookup(&url("https://mac-mini/sonarr/calendar"), false)
                .as_deref(),
            Some("sonarr")
        );
        assert_eq!(
            index.lookup(&url("https://mac-mini/other"), false),
            None,
            "two apps on one host: no guess"
        );
        index.record(&url("https://www.example.com/"), "example", false, 1.0);
        assert_eq!(
            index
                .lookup(&url("https://example.com/about"), false)
                .as_deref(),
            Some("example")
        );
        let again = Index::load(&index.encode()).expect("round trip");
        assert_eq!(
            again
                .lookup(&url("https://mac-mini/radarr/x"), false)
                .as_deref(),
            Some("radarr")
        );
    }
}
