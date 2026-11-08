//! Where you have been, so the field can finish the address for you. A port of Search's
//! `History.swift`: the same file (`history.json`, a list of visits), the same identity
//! for a page and the same completion. Visited URL paths and queries also match.
//!
//! Not yet: Search's move of pre-1.0.4 "front door" credits (`rehomeCredits`), which only
//! files written by Search before 1.0.4 need, and the history list (⌘Y).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    address,
    fnd::{Parts, Url},
};

/// Visits counted at most: far more than anyone makes, and room to add.
pub const MOST_VISITS: i64 = 1_000_000_000;
/// The file keeps this many places, those visited least and longest ago going first.
const KEPT: usize = 2_000;
/// Seconds from 1970 to 2001, where Swift's `Date` counts from.
const APPLE_EPOCH: f64 = 978_307_200.0;

/// Now, as Swift's `JSONEncoder` writes a `Date`: seconds since 2001-01-01.
#[must_use]
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
        - APPLE_EPOCH
}

/// One place, as `history.json` keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Visit {
    url: String,
    key: String,
    title: String,
    count: i64,
    /// Seconds since 2001-01-01.
    last: f64,
}

/// Worked out once when a visit enters memory: suggestions run on every key.
#[derive(Clone, Debug, PartialEq)]
struct PageText {
    /// The address as the field shows it.
    address: String,
    /// The same, lower case, without the scheme and a leading www.
    searchable: String,
    /// The host without www.
    host: String,
    homepage: bool,
}

/// What the field offers for what is typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// What you could type to get here, kept reversible for completion.
    pub key: String,
    pub title: String,
    pub url: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Somewhere you have actually been.
    Visited,
    /// One of the well-known addresses the field knows from the start.
    Known,
    /// Not a place at all: words, and an engine to ask.
    Search,
}

/// A page's identity: the scheme and host in lower case, no name or password, no
/// fragment, and an absent path read as `/`. The path and query keep their spelling.
#[must_use]
pub fn identity(url: &str) -> String {
    let Some(parts) = Parts::of(url) else {
        return url.to_owned();
    };
    parts.set_scheme(parts.scheme().map(|s| s.to_lowercase()).as_deref());
    parts.set_host(parts.host().map(|h| h.to_lowercase()).as_deref());
    parts.clear_credentials();
    parts.clear_fragment();
    if parts.path().is_empty() {
        parts.set_path("/");
    }
    parts.string().unwrap_or_else(|| url.to_owned())
}

/// A page as it is kept: without a name and password in its address. They were never
/// part of which page it is, and the history file is no place for a password.
fn kept(url: &str) -> Option<String> {
    let parts = Parts::of(url)?;
    if !parts.has_credentials() {
        return Some(url.to_owned());
    }
    parts.clear_credentials();
    parts.url().map(|u| u.as_str().to_owned())
}

/// The site's front door: the same scheme, host and port, at `/`.
fn homepage(url: &str) -> Option<String> {
    let parts = Parts::of(url)?;
    parts.clear_credentials();
    parts.set_path("/");
    parts.clear_query();
    parts.clear_fragment();
    parts.url().map(|u| u.as_str().to_owned())
}

fn is_web(url: &str) -> bool {
    Parts::of(url)
        .and_then(|p| p.scheme())
        .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
}

/// What a typed address and a kept one are compared as.
#[must_use]
pub fn stripped(address: &str) -> String {
    let mut text = address.trim().to_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = text.strip_prefix(scheme) {
            text = rest.to_owned();
        }
    }
    if let Some(rest) = text.strip_prefix("www.") {
        text = rest.to_owned();
    }
    text
}

fn page_text(url: &str) -> PageText {
    let shown = Parts::of(url)
        .and_then(|p| {
            p.clear_fragment();
            p.url()
        })
        .or_else(|| Url::parse(url));
    let Some(shown) = shown else {
        return PageText {
            address: url.to_owned(),
            searchable: stripped(url),
            host: String::new(),
            homepage: false,
        };
    };
    let address = address::editable(&shown);
    let host = shown.host().map(|h| h.to_lowercase()).unwrap_or_default();
    let path = shown.path();
    PageText {
        searchable: stripped(&address),
        address,
        host: host
            .strip_prefix("www.")
            .map_or_else(|| host.clone(), str::to_owned),
        homepage: path.is_empty() || path == "/",
    }
}

/// Often, and lately: a month-old visit counts for about a third of a fresh one.
#[allow(clippy::cast_precision_loss)] // counts are held far below 2^52
fn score(visit: &Visit, now: f64) -> f64 {
    let days = ((now - visit.last) / 86_400.0).max(0.0);
    visit.count as f64 * (-days / 30.0).exp()
}

/// Where the match falls decides most of the ordering: the start of the host is what
/// people mean, the middle of a path almost never is.
fn rank(key: &str, needle: &str, needle_chars: usize) -> Option<f64> {
    if key.starts_with(needle) {
        return Some(6.0);
    }
    let host = key.split(['/', '?', '#']).next().unwrap_or(key);
    // "google" finding mail.google.com without the subdomain.
    if let Some((_, after)) = host.split_once('.')
        && after.starts_with(needle)
    {
        return Some(3.0);
    }
    // Only from two letters up: one letter matching anywhere inside a name turns "x"
    // into example.com and netflix.com, which is not what anybody meant by it.
    if needle_chars >= 2 && host.contains(needle) {
        return Some(2.0);
    }
    // Path and query matches are useful, but less specific than a host match.
    (needle_chars >= 2 && key.contains(needle)).then_some(1.0)
}

/// Somewhere to start on the first day. Ranked below anything visited, and dropped the
/// moment you have been there yourself.
const KNOWN: [(&str, &str); 47] = [
    ("google.com", "Google"),
    ("mail.google.com", "Gmail"),
    ("drive.google.com", "Google Drive"),
    ("calendar.google.com", "Google Calendar"),
    ("maps.google.com", "Google Maps"),
    ("youtube.com", "YouTube"),
    ("github.com", "GitHub"),
    ("figma.com", "Figma"),
    ("vercel.com", "Vercel"),
    ("notion.so", "Notion"),
    ("linear.app", "Linear"),
    ("slack.com", "Slack"),
    ("discord.com", "Discord"),
    ("x.com", "X"),
    ("linkedin.com", "LinkedIn"),
    ("instagram.com", "Instagram"),
    ("reddit.com", "Reddit"),
    ("news.ycombinator.com", "Hacker News"),
    ("stackoverflow.com", "Stack Overflow"),
    ("claude.ai", "Claude"),
    ("chatgpt.com", "ChatGPT"),
    ("dribbble.com", "Dribbble"),
    ("behance.net", "Behance"),
    ("awwwards.com", "Awwwards"),
    ("mobbin.com", "Mobbin"),
    ("siteinspire.com", "SiteInspire"),
    ("are.na", "Are.na"),
    ("pinterest.com", "Pinterest"),
    ("framer.com", "Framer"),
    ("webflow.com", "Webflow"),
    ("developer.apple.com", "Apple Developer"),
    ("swift.org", "Swift"),
    ("npmjs.com", "npm"),
    ("supabase.com", "Supabase"),
    ("stripe.com", "Stripe"),
    ("shopify.com", "Shopify"),
    ("cloudflare.com", "Cloudflare"),
    ("netlify.com", "Netlify"),
    ("apple.com", "Apple"),
    ("spotify.com", "Spotify"),
    ("netflix.com", "Netflix"),
    ("wikipedia.org", "Wikipedia"),
    ("deepl.com", "DeepL"),
    ("loom.com", "Loom"),
    ("amazon.fr", "Amazon"),
    ("leboncoin.fr", "leboncoin"),
    ("lemonde.fr", "Le Monde"),
];

/// The places you have been. Behind an `Arc` so the copy handed to the writer thread
/// on every finished page is a refcount, not a few thousand string clones; mutating
/// goes through `Arc::make_mut`, which copies only while a snapshot is still held.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    inner: Arc<Inner>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Inner {
    visits: HashMap<String, Visit>,
    page_text: HashMap<String, PageText>,
    visited_hosts: HashSet<String>,
}

impl Inner {
    fn remember(&mut self, key: &str) {
        let Some(visit) = self.visits.get(key) else {
            return;
        };
        let text = page_text(&visit.url);
        self.keep_text(key, text);
    }

    fn keep_text(&mut self, key: &str, text: PageText) {
        if !text.host.is_empty() {
            self.visited_hosts.insert(text.host.clone());
        }
        self.page_text.insert(key.to_owned(), text);
    }
}

impl History {
    /// The file's contents, or `None` for a file that won't decode (quarantined by the
    /// caller, never overwritten). Keys are rebuilt from the addresses, duplicates under
    /// the rule merged, counts held to a sane range, and passwords taken off. The second
    /// value says whether anything was cleaned and the file should be written again now.
    #[must_use]
    pub fn load(data: &[u8]) -> Option<(History, bool)> {
        let list: Vec<Visit> = serde_json::from_slice(data).ok()?;
        let mut inner = Inner::default();
        let mut cleaned = false;
        for mut visit in list {
            visit.count = visit.count.clamp(0, MOST_VISITS);
            if let Some(clean) = kept(&visit.url) {
                if clean != visit.url {
                    visit.url = clean;
                    cleaned = true;
                }
                visit.key = identity(&visit.url);
            }
            let merged = match inner.visits.remove(&visit.key) {
                Some(other) => merged(other, visit),
                None => visit,
            };
            inner.visits.insert(merged.key.clone(), merged);
        }
        let keys: Vec<String> = inner.visits.keys().cloned().collect();
        for key in keys {
            inner.remember(&key);
        }
        Some((
            History {
                inner: Arc::new(inner),
            },
            cleaned,
        ))
    }

    /// The file as Search writes it: the most-visited-lately first, capped.
    #[must_use]
    pub fn encode(&self, now: f64) -> Vec<u8> {
        // Scores worked out once each, not inside the comparator (`score` is an `exp`).
        let mut list: Vec<(f64, &Visit)> = self
            .inner
            .visits
            .values()
            .map(|visit| (score(visit, now), visit))
            .collect();
        list.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
        list.truncate(KEPT);
        let list: Vec<&Visit> = list.into_iter().map(|(_, visit)| visit).collect();
        serde_json::to_vec(&list).unwrap_or_default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.visits.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.visits.is_empty()
    }

    /// A page you looked at. A deep page is also, in the way that matters here, a visit
    /// to its site: typing three letters should offer the front door, not the article
    /// you happened to open last week. Answers whether anything was recorded.
    pub fn record(&mut self, url: &str, title: &str, now: f64) -> bool {
        if !is_web(url) {
            return false;
        }
        let Some(url) = kept(url) else { return false };
        let key = identity(&url);
        if key.is_empty() {
            return false;
        }
        let text = page_text(&url);
        let inner = Arc::make_mut(&mut self.inner);
        if !text.homepage
            && let Some(home) = homepage(&url)
        {
            let root = identity(&home);
            let mut fresh = false;
            let door = inner.visits.entry(root.clone()).or_insert_with(|| {
                fresh = true;
                Visit {
                    url: home,
                    key: root.clone(),
                    title: String::new(),
                    count: 0,
                    last: now,
                }
            });
            door.count = (door.count + 1).min(MOST_VISITS);
            door.last = now;
            // A front door already standing has its text worked out; only a new one
            // needs it (every visit's text is noted as the visit lands).
            if fresh {
                inner.remember(&root);
            }
        }
        let mut fresh = false;
        let visit = inner.visits.entry(key.clone()).or_insert_with(|| {
            fresh = true;
            Visit {
                url: url.clone(),
                key: key.clone(),
                title: String::new(),
                count: 0,
                last: now,
            }
        });
        visit.count = (visit.count + 1).min(MOST_VISITS);
        visit.last = now;
        if visit.url != url {
            visit.url = url;
            fresh = true;
        }
        if !title.is_empty() {
            title.clone_into(&mut visit.title);
        }
        if fresh {
            inner.keep_text(&key, text);
        }
        true
    }

    /// A place brought in from elsewhere: another browser's history, or a profile file
    /// (Search's `History.take`). The larger of the two counts, not their sum, so the
    /// same file brought in twice doesn't count every visit twice. Answers whether it was
    /// a place Tap keeps.
    pub fn take(&mut self, url: &str, title: &str, count: i64, last: f64) -> bool {
        if !is_web(url) {
            return false;
        }
        let Some(url) = kept(url) else { return false };
        let key = identity(&url);
        if key.is_empty() {
            return false;
        }
        let inner = Arc::make_mut(&mut self.inner);
        if let Some(seen) = inner.visits.get_mut(&key) {
            seen.count = seen.count.max(count).min(MOST_VISITS);
            if last > seen.last {
                seen.last = last;
            }
            if seen.title.is_empty() {
                title.clone_into(&mut seen.title);
            }
            return true;
        }
        inner.visits.insert(
            key.clone(),
            Visit {
                url,
                key: key.clone(),
                title: title.to_owned(),
                count: count.clamp(0, MOST_VISITS),
                last,
            },
        );
        inner.remember(&key);
        true
    }

    /// Every place a `history.json` holds, as [`History::take`] takes them. Read
    /// leniently, as Search's `History.Entry` is: only the address has to be there. Answers
    /// how many were taken.
    pub fn take_file(&mut self, data: &[u8], now: f64) -> usize {
        #[derive(Deserialize)]
        struct Row {
            url: String,
            #[serde(default)]
            title: String,
            #[serde(default = "one")]
            count: i64,
            #[serde(default)]
            last: Option<f64>,
        }
        fn one() -> i64 {
            1
        }
        let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(data) else {
            return 0;
        };
        rows.into_iter()
            .filter_map(|row| serde_json::from_value::<Row>(row).ok())
            .filter(|row| self.take(&row.url, &row.title, row.count, row.last.unwrap_or(now)))
            .count()
    }

    /// The page's title arrived, or changed. Answers whether anything changed.
    pub fn retitle(&mut self, url: &str, title: &str) -> bool {
        let key = identity(url);
        let changes = self
            .inner
            .visits
            .get(&key)
            .is_some_and(|visit| !title.is_empty() && visit.title != title);
        if !changes {
            return false;
        }
        if let Some(visit) = Arc::make_mut(&mut self.inner).visits.get_mut(&key) {
            title.clone_into(&mut visit.title);
        }
        true
    }

    /// The best few places for what was typed: a place you have been always beats one
    /// the app merely knows, and among places you have been, one you go to often and
    /// lately beats one you saw once in March. An empty field offers nothing.
    #[must_use]
    pub fn suggestions(&self, typed: &str, limit: usize, now: f64) -> Vec<Suggestion> {
        // A candidate before it is built: matches are borrowed and ranked first, and
        // only the few that survive pay for their strings. A short needle can match
        // nearly everything kept, and this runs on every key.
        enum Source<'a> {
            Visited(&'a Visit, &'a PageText),
            Known(&'a str, &'a str),
        }
        let needle = stripped(typed);
        if needle.is_empty() {
            return Vec::new();
        }
        let needle_chars = needle.chars().count();
        let mut found: Vec<(f64, usize, &str, Source<'_>)> = Vec::new();
        for visit in self.inner.visits.values() {
            let Some(text) = self.inner.page_text.get(&visit.key) else {
                continue;
            };
            let Some(rank) = rank(&text.searchable, &needle, needle_chars) else {
                continue;
            };
            // The front door before the room inside it.
            let score = rank + 4.0 + score(visit, now) + if text.homepage { 1.5 } else { 0.0 };
            found.push((
                score,
                text.address.chars().count(),
                &text.address,
                Source::Visited(visit, text),
            ));
        }
        for (host, title) in KNOWN {
            if self.inner.visited_hosts.contains(host) {
                continue;
            }
            if let Some(rank) = rank(host, &needle, needle_chars) {
                found.push((rank, host.chars().count(), host, Source::Known(host, title)));
            }
        }
        // Higher first, then the shorter key, then the key itself, so the order is the
        // same every time.
        found.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(b.2))
        });
        found
            .into_iter()
            .take(limit)
            .map(|(_, _, _, source)| match source {
                Source::Visited(visit, text) => Suggestion {
                    key: text.address.clone(),
                    title: visit.title.clone(),
                    url: visit.url.clone(),
                    kind: Kind::Visited,
                },
                Source::Known(host, title) => Suggestion {
                    key: host.to_owned(),
                    title: title.to_owned(),
                    url: format!("https://{host}"),
                    kind: Kind::Known,
                },
            })
            .collect()
    }

    /// What the field draws greyed out after the caret: the rest of the best match, or
    /// nothing if it doesn't carry on from what was typed. One letter finishes only the
    /// top row, and only when it is the front door of a site you have been to: "x"
    /// becomes x.com. A completion never changes the case of a path.
    #[must_use]
    pub fn completion(typed: &str, among: &[Suggestion]) -> Option<String> {
        let typed_len = typed.chars().count();
        if typed_len == 0 {
            return None;
        }
        let single = typed_len == 1;
        if single
            && among
                .first()
                .is_some_and(|first| first.kind != Kind::Visited)
        {
            return None;
        }
        let options = if single {
            &among[..among.len().min(1)]
        } else {
            among
        };
        let typed_lower = typed.to_lowercase();
        for option in options {
            for (key, target) in endings(option) {
                if key.chars().count() < typed_len {
                    continue;
                }
                if single && key.contains(['/', '?', '#']) {
                    continue;
                }
                let split = key
                    .char_indices()
                    .nth(typed_len)
                    .map_or(key.len(), |(i, _)| i);
                let (prefix, rest) = key.split_at(split);
                if prefix.to_lowercase() != typed_lower {
                    continue;
                }
                let Some(completed) = address::url(&format!("{typed}{rest}")) else {
                    continue;
                };
                if identity(completed.as_str()) != target {
                    continue;
                }
                return (!rest.is_empty()).then(|| rest.to_owned());
            }
        }
        None
    }
}

/// What a row can be finished as, and the page that has to come of it: its address as
/// shown, and for a www site the name without the www ("app" still becomes apple.com).
/// Only the www is let go of; the scheme, port and the path's case still count.
fn endings(option: &Suggestion) -> Vec<(String, String)> {
    let mut found = vec![(option.key.clone(), identity(&option.url))];
    if option.key.to_lowercase().starts_with("www.")
        && let Some(parts) = Parts::of(&option.url)
        && let Some(host) = parts
            .host()
            .filter(|h| h.to_lowercase().starts_with("www."))
    {
        parts.set_host(Some(&host[4..]));
        if let Some(bare) = parts.url() {
            found.push((option.key[4..].to_owned(), identity(bare.as_str())));
        }
    }
    found
}

/// The same page twice: its visits added up, the latest date, and a title if either
/// had one.
fn merged(a: Visit, b: Visit) -> Visit {
    let (newer, older) = if a.last >= b.last { (a, b) } else { (b, a) };
    let mut kept = newer;
    kept.count = kept.count.saturating_add(older.count).min(MOST_VISITS);
    if kept.title.is_empty() {
        kept.title = older.title;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 815_000_000.0;

    fn pages(history: &History, host: &str) -> Vec<Visit> {
        let mut list: Vec<Visit> = history
            .inner
            .visits
            .values()
            .filter(|v| {
                Url::parse(&v.url)
                    .and_then(|u| u.host())
                    .is_some_and(|h| h.eq_ignore_ascii_case(host))
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| a.key.cmp(&b.key));
        list
    }

    fn path(url: &str) -> String {
        Url::parse(url).map(|u| u.path()).unwrap_or_default()
    }

    #[test]
    fn the_file_orders_the_often_and_late_first_and_always_the_same_way() {
        let mut history = History::default();
        for _ in 0..5 {
            history.record("https://often.com/", "Often", NOW - 10.0 * 86_400.0);
        }
        history.record("https://stale.com/", "Stale", NOW - 90.0 * 86_400.0);
        history.record("https://fresh.com/", "Fresh", NOW);
        let order = |file: &[u8]| {
            serde_json::from_slice::<Vec<Visit>>(file)
                .unwrap()
                .iter()
                .map(|visit| visit.url.clone())
                .collect::<Vec<_>>()
        };
        // Five visits ten days ago outweigh one fresh one; one fresh one outweighs a
        // single visit three months back.
        assert_eq!(
            order(&history.encode(NOW)),
            [
                "https://often.com/",
                "https://fresh.com/",
                "https://stale.com/"
            ]
        );
        assert_eq!(
            order(&history.encode(NOW)),
            order(&history.encode(NOW)),
            "encoding twice writes the same file"
        );
    }

    /// Search's `testRecordAndTakeKeepCanonicalIdentityComponents` (record half).
    #[test]
    fn identity_keeps_scheme_port_and_case_but_not_the_fragment() {
        let mut history = History::default();
        for address in [
            "http://localhost:3000/a",
            "http://localhost:4000/a",
            "http://localhost:3000/A",
            "http://localhost:4000/A",
            "http://localhost:3000/A#later",
        ] {
            history.record(address, address, NOW);
        }
        let deep: Vec<Visit> = pages(&history, "localhost")
            .into_iter()
            .filter(|v| path(&v.url) != "/")
            .collect();
        assert_eq!(deep.len(), 4, "port and /A against /a are four pages");
        assert!(deep.iter().all(|v| !v.key.contains('#')));
        assert_eq!(
            deep.iter()
                .find(|v| v.key == "http://localhost:3000/A")
                .map(|v| v.count),
            Some(2)
        );
    }

    /// Search's `testRetitleAndForgetUseIndependentIdentities` (retitle half).
    #[test]
    fn retitle_changes_only_the_matching_scheme() {
        let mut history = History::default();
        history.record(
            "http://titles.example.test:8080/A?Q=One",
            "Insecure title",
            NOW,
        );
        history.record(
            "https://titles.example.test:8080/A?Q=One",
            "Secure title",
            NOW,
        );
        assert!(history.retitle("http://titles.example.test:8080/A?Q=One", "Retitled HTTP"));
        let deep: Vec<Visit> = pages(&history, "titles.example.test")
            .into_iter()
            .filter(|v| path(&v.url) == "/A")
            .collect();
        assert_eq!(deep.len(), 2);
        assert_eq!(
            deep.iter()
                .find(|v| v.url.starts_with("http:"))
                .map(|v| v.title.as_str()),
            Some("Retitled HTTP")
        );
        assert_eq!(
            deep.iter()
                .find(|v| v.url.starts_with("https:"))
                .map(|v| v.title.as_str()),
            Some("Secure title")
        );
    }

    /// Search's `testLegacyLoadRebuildsIdentityFromStoredURL`.
    /// Search's import rule: a place brought in twice keeps the larger count, never the
    /// sum, and a row needs only its address.
    #[test]
    fn places_brought_in_twice_are_not_counted_twice() {
        let mut history = History::default();
        let file =
            br#"[{"url":"https://example.com/a","key":"x","title":"A","count":5,"last":815000000},
                        {"url":"https://example.com/b"},
                        {"url":"ftp://example.com/c","count":3},
                        {"title":"no address"}]"#;
        assert_eq!(history.take_file(file, NOW), 2);
        assert_eq!(history.take_file(file, NOW), 2);
        let a = &pages(&history, "example.com")[0];
        assert_eq!((a.count, a.title.as_str(), a.last), (5, "A", 815_000_000.0));
        assert_eq!(
            history.len(),
            2,
            "the bad scheme and the row without an address stay out"
        );
    }

    #[test]
    fn loading_rebuilds_keys_from_addresses_and_merges_what_meets() {
        let file = br#"[
            {"url":"https://legacy.example.test:8080/A?Q=One#first","key":"stale-collision","title":"Upper","count":2,"last":815000000},
            {"url":"https://legacy.example.test:8080/a?Q=One","key":"stale-collision","title":"Lower","count":3,"last":815000001},
            {"url":"https://legacy.example.test:8080/A?Q=One#again","key":"old-third-key","title":"Again","count":7,"last":815000002}]"#;
        let (history, _) = History::load(file).expect("decodes");
        let list = pages(&history, "legacy.example.test");
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.iter().find(|v| path(&v.url) == "/A").map(|v| v.count),
            Some(9)
        );
        assert_eq!(
            list.iter().find(|v| path(&v.url) == "/a").map(|v| v.count),
            Some(3)
        );
        assert!(
            list.iter()
                .all(|v| !v.key.starts_with("stale-") && v.key != "old-third-key")
        );
    }

    /// Search's `testRootCreditsKeepOriginAndDirectRootCountsOnce`.
    #[test]
    fn front_door_credits_keep_their_origin_and_count_once() {
        let mut history = History::default();
        for address in [
            "http://origin.example.test:8080/page",
            "https://origin.example.test:8080/page",
            "https://origin.example.test:9090/page",
        ] {
            history.record(address, "Page", NOW);
        }
        let mut roots: Vec<String> = history
            .suggestions("ORIGIN.EXAMPLE.TEST", 50, NOW)
            .into_iter()
            .filter(|s| path(&s.url) == "/")
            .map(|s| s.url)
            .collect();
        roots.sort();
        assert_eq!(
            roots,
            [
                "http://origin.example.test:8080/",
                "https://origin.example.test:8080/",
                "https://origin.example.test:9090/"
            ]
        );

        let mut direct = History::default();
        direct.record("https://home.example.test/", "Home", NOW);
        direct.record("https://home.example.test/deep", "Deep page", NOW);
        let home: Vec<Visit> = pages(&direct, "home.example.test")
            .into_iter()
            .filter(|v| path(&v.url) == "/")
            .collect();
        assert_eq!(home.len(), 1);
        assert_eq!((home[0].count, home[0].title.as_str()), (2, "Home"));

        let mut canonical = History::default();
        canonical.record("HTTPS://Normalize.example.test", "First form", NOW);
        canonical.record("https://normalize.example.test/", "Slash form", NOW);
        let one = pages(&canonical, "normalize.example.test");
        assert_eq!(one.len(), 1);
        assert_eq!(
            (one[0].count, one[0].key.as_str()),
            (2, "https://normalize.example.test/")
        );
    }

    /// Search's `testSuggestionIdsAndSafeCaseInsensitiveCompletion`.
    #[test]
    fn completion_matches_the_host_in_any_case_but_never_changes_a_path() {
        let mut history = History::default();
        history.record("https://suggest.example.test/A?Q=One", "Upper page", NOW);
        history.record("https://suggest.example.test/a?Q=One", "Lower page", NOW);
        let options = history.suggestions("SUGGEST.EXAMPLE.TEST", 50, NOW);
        let upper = options
            .iter()
            .find(|s| s.url.ends_with("/A?Q=One"))
            .expect("offered")
            .clone();
        assert_eq!(
            History::completion("SUGGEST.EXAMPLE.TEST", std::slice::from_ref(&upper)).as_deref(),
            Some("/A?Q=One")
        );
        assert_eq!(
            History::completion("suggest.example.test/a?Q=", std::slice::from_ref(&upper)),
            None
        );
        assert_eq!(
            History::completion("suggest/A", std::slice::from_ref(&upper)),
            None
        );

        let mut known = History::default();
        known.record("https://github.com/", "GitHub home", NOW);
        assert!(
            known
                .suggestions("github", 50, NOW)
                .iter()
                .all(|s| s.kind != Kind::Known)
        );
    }

    /// Search's `testCredentialsNeverReachTheFile` and the hand-edited counts.
    #[test]
    fn passwords_never_reach_the_file_and_counts_stay_sane() {
        let mut history = History::default();
        history.record(
            "https://user:secret@cred.example.test/page",
            "Signed in",
            NOW,
        );
        assert!(history.retitle("https://user:secret@cred.example.test/page", "Retitled"));
        let list = pages(&history, "cred.example.test");
        assert_eq!(
            list.iter()
                .find(|v| path(&v.url) == "/page")
                .map(|v| v.title.as_str()),
            Some("Retitled")
        );
        assert!(!String::from_utf8_lossy(&history.encode(NOW)).contains("secret"));

        let file = br#"[{"url":"https://user:secret@oldcred.example.test/page","key":"x","title":"Old","count":4,"last":815000000},
                        {"url":"https://big.example.test/","key":"b","title":"Big","count":9223372036854775807,"last":815000000},
                        {"url":"https://neg.example.test/","key":"n","title":"Neg","count":-5,"last":815000000}]"#;
        let (mut loaded, cleaned) = History::load(file).expect("decodes");
        assert!(
            cleaned,
            "a password kept by an older version is written away at once"
        );
        assert!(!String::from_utf8_lossy(&loaded.encode(NOW)).contains("secret"));
        loaded.record("https://big.example.test/", "Big", NOW);
        assert_eq!(pages(&loaded, "big.example.test")[0].count, MOST_VISITS);
        assert_eq!(pages(&loaded, "neg.example.test")[0].count, 0);
    }

    /// Search's `testCompletionFinishesWWWSitesWithoutTheirWWW`.
    #[test]
    fn www_sites_complete_without_their_www_and_keep_path_case() {
        let mut history = History::default();
        history.record("https://www.apple-test.example/iPhone", "iPhone", NOW);
        let options = history.suggestions("apple-t", 3, NOW);
        assert_eq!(
            History::completion("apple-t", &options).as_deref(),
            Some("est.example")
        );
        assert_eq!(
            History::completion("www.apple-t", &options).as_deref(),
            Some("est.example")
        );
        let page: Vec<Suggestion> = options
            .into_iter()
            .filter(|s| path(&s.url) == "/iPhone")
            .collect();
        assert_eq!(
            History::completion("apple-test.example/iP", &page).as_deref(),
            Some("hone")
        );
        assert_eq!(History::completion("apple-test.example/ip", &page), None);
    }

    #[test]
    fn one_letter_finishes_only_a_visited_front_door_and_an_empty_field_offers_nothing() {
        let mut history = History::default();
        history.record("https://x.com/home", "X", NOW);
        let options = history.suggestions("x", 3, NOW);
        assert_eq!(History::completion("x", &options).as_deref(), Some(".com"));
        assert_eq!(history.suggestions("  ", 3, NOW), Vec::new());
        let fresh = History::default();
        let known = fresh.suggestions("g", 3, NOW);
        assert_eq!(known.first().map(|s| s.kind), Some(Kind::Known));
        assert_eq!(
            History::completion("g", &known),
            None,
            "a name the app merely knows waits for a second letter"
        );
    }

    #[test]
    fn url_fragments_find_saved_pages_after_reload_without_inline_completion() {
        let mut history = History::default();
        history.record(
            "https://example.test/articles/the-pair?view=full",
            "Pair",
            NOW,
        );
        let (history, _) = History::load(&history.encode(NOW)).expect("decodes");

        for typed in ["the-pair", "PAIR?VIEW", "view=full"] {
            let options = history.suggestions(typed, 3, NOW);
            assert_eq!(options.len(), 1, "{typed}");
            assert_eq!(
                options[0].url,
                "https://example.test/articles/the-pair?view=full"
            );
            assert_eq!(options[0].kind, Kind::Visited);
            assert_eq!(History::completion(typed, &options), None);
        }
        assert!(
            history
                .suggestions("p", 50, NOW)
                .iter()
                .all(|s| s.kind != Kind::Visited)
        );
    }

    #[test]
    fn url_fragment_matches_order_by_visits_and_recency() {
        let mut history = History::default();
        history.record(
            "https://old.example.test/the-pair",
            "Old",
            NOW - 90.0 * 86_400.0,
        );
        history.record("https://fresh.example.test/the-pair", "Fresh", NOW);
        for _ in 0..3 {
            history.record("https://often.example.test/the-pair", "Often", NOW);
        }
        let (history, _) = History::load(&history.encode(NOW)).expect("decodes");
        let titles: Vec<_> = history
            .suggestions("the-pair", 3, NOW)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(titles, ["Often", "Fresh", "Old"]);
    }

    #[test]
    fn host_matches_rank_above_url_fragment_matches_at_equal_frecency() {
        let mut history = History::default();
        history.record("https://the-pair.example.test/", "Host", NOW);
        history.record("https://other.example.test/the-pair", "Path", NOW);
        let titles: Vec<_> = history
            .suggestions("the-pair", 3, NOW)
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(titles, ["Host", "Path"]);
    }

    #[test]
    fn often_and_lately_outranks_once_long_ago() {
        let mut history = History::default();
        history.record("https://docs.example.test/", "Old", NOW - 400.0 * 86_400.0);
        for _ in 0..5 {
            history.record("https://dashboard.example.test/", "Dashboard", NOW);
        }
        let top = history.suggestions("d", 3, NOW);
        assert_eq!(top.first().map(|s| s.title.as_str()), Some("Dashboard"));
    }
}
