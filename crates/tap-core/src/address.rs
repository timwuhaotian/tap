//! What was typed has to be a place. A port of Search's `Address.swift`: this hands back
//! an address or nothing, never a search. The browser decides what to do with nothing.

use std::path::Path;

use crate::{fnd::Url, identity};

/// Schemes the window shows itself. Anything else typed with a scheme (`mailto:`, an
/// app's link) is somebody else's job and refused here rather than opening a blank tab.
const OURS: [&str; 6] = ["http", "https", "file", "about", "data", identity::SCHEME];

/// A local file, for committed navigation only: suggestions and dropped text never look
/// at the disk. A regular file, not a folder, pipe, device or link.
#[must_use]
pub fn file(typed: &str) -> Option<Url> {
    let text = typed.trim();
    let absolute = text.starts_with('/') && !text.starts_with("//");
    if !absolute && !text.starts_with("~/") {
        return None;
    }
    let path = match text.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").ok()?),
        None => text.to_owned(),
    };
    let metadata = std::fs::symlink_metadata(Path::new(&path)).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    Url::file(&path)
}

/// The address `typed` names, or `None` when it isn't one.
#[must_use]
#[allow(clippy::case_sensitive_file_extension_comparisons)] // host names, not file names, as Search compares them
pub fn url(typed: &str) -> Option<Url> {
    let text = typed.trim();
    if text.is_empty() || text.contains(' ') {
        return None;
    }

    // Written with a scheme, it is taken at its word.
    if let Some(split) = text.find("://") {
        let scheme = text[..split].to_lowercase();
        if !OURS.contains(&scheme.as_str()) {
            return None;
        }
        return Url::parse(text);
    }
    let lower = text.to_lowercase();
    if lower.starts_with("about:") || lower.starts_with("data:") {
        return Url::parse(text);
    }

    // Everything else has to look like a host before it gets a scheme in front of it.
    // "hello world" is not a website, and neither is "todo".
    let head: &str = text.split(['/', '?', '#']).next().unwrap_or(text);
    if head.contains('@') {
        return None; // an email address
    }
    // This Mac by its IPv6 address, [::1]:3000, is a place too.
    if head.starts_with("[::1]") {
        return Url::parse(&format!("http://{text}"));
    }
    let host = head
        .split(':')
        .find(|part| !part.is_empty())
        .unwrap_or(head);
    if !looks_like_host(host) {
        return None;
    }

    // A local server almost never has a certificate, so https there is a connection
    // failure rather than a page. The same goes for printer.local and the like.
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "127.0.0.1"
        || host == "0.0.0.0"
        || private_range(host);
    let scheme = if local { "http://" } else { "https://" };
    Url::parse(&format!("{scheme}{text}"))
}

/// Four numbers, and in one of the private ranges where Docker and offices put their
/// machines. `10.io` is a website, not 10.0.0.0/8.
fn private_range(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    let octets: Vec<u8> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 4 || octets.len() != 4 {
        return false;
    }
    octets[0] == 10
        || (octets[0] == 192 && octets[1] == 168)
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
}

fn looks_like_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // Four numbers is an address on the local network as often as not.
    if labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok()) {
        return true;
    }
    if labels.len() < 2 {
        return false;
    }
    let well_formed = labels.iter().all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_alphabetic() || c.is_numeric() || c == '-')
    });
    if !well_formed {
        return false;
    }
    // The last label carries the weight: a dotted thing ending in letters is a domain,
    // a dotted thing ending in digits is a version number.
    let tld = labels[labels.len() - 1];
    tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic)
}

/// Dev servers say they listen on 0.0.0.0 and print that as the address to open.
/// WebKit shows nothing there, so it is opened as localhost, the same server.
#[must_use]
pub fn reachable(url: &Url) -> Option<Url> {
    let scheme = url.scheme()?.to_lowercase();
    if url.host().as_deref() != Some("0.0.0.0") || !(scheme == "http" || scheme == "https") {
        return None;
    }
    Url::parse(&url.as_str().replacen("0.0.0.0", "localhost", 1))
}

/// A page's address as the field shows it for editing: something that, sent as it is,
/// goes back to the same page. The scheme is left off only when the field would put that
/// very scheme back, and a bare "/" only when nothing follows it.
#[must_use]
pub fn editable(page: &Url) -> String {
    let full = page.as_str();
    let Some(scheme) = page.scheme() else {
        return full.to_owned();
    };
    let prefix = format!("{}://", scheme.to_lowercase());
    if !full.to_lowercase().starts_with(&prefix) {
        return full.to_owned();
    }
    let short = &full[prefix.len()..];
    let mut candidates = vec![short.to_owned()];
    if page.path() == "/"
        && page.query().is_none()
        && page.fragment().is_none()
        && short.ends_with('/')
    {
        candidates.insert(0, short[..short.len() - 1].to_owned());
    }
    for candidate in candidates {
        let Some(back) = url(&candidate) else {
            continue;
        };
        if back.as_str() == full || format!("{}/", back.as_str()) == full {
            return candidate;
        }
    }
    full.to_owned()
}

/// An address as a row shows it: without a leading www.
#[must_use]
pub fn without_www(address: &str) -> &str {
    if address.len() >= 4 && address[..4].eq_ignore_ascii_case("www.") {
        &address[4..]
    } else {
        address
    }
}

/// What a tab says before the page has told it a title: the address, with the parts
/// nobody reads taken off.
#[must_use]
pub fn pretty(url: &Url) -> String {
    let Some(host) = url.host() else {
        return url.as_str().to_owned();
    };
    let bare = host.strip_prefix("www.").unwrap_or(&host);
    let path = url.path();
    if path.is_empty() || path == "/" {
        bare.to_owned()
    } else {
        format!("{bare}{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Search's `Tests/AddressRegression/main.swift`, case for case.
    #[test]
    fn address_regressions() {
        let cases: [(&str, Option<&str>); 19] = [
            // A server on this Mac or on the local network: plain http.
            ("localhost:3000", Some("http://localhost:3000")),
            ("printer.local", Some("http://printer.local")),
            ("127.0.0.1:8080", Some("http://127.0.0.1:8080")),
            ("0.0.0.0:8080", Some("http://0.0.0.0:8080")),
            ("10.0.0.1", Some("http://10.0.0.1")),
            ("10.255.255.254:9000", Some("http://10.255.255.254:9000")),
            ("192.168.1.1", Some("http://192.168.1.1")),
            ("172.16.0.1", Some("http://172.16.0.1")),
            ("172.31.255.254", Some("http://172.31.255.254")),
            // A website, whatever its name looks like: https.
            ("example.com", Some("https://example.com")),
            ("172.15.0.1", Some("https://172.15.0.1")),
            ("172.32.0.1", Some("https://172.32.0.1")),
            ("10.io", Some("https://10.io")),
            ("10.org/about", Some("https://10.org/about")),
            ("10.0.0.1.example.com", Some("https://10.0.0.1.example.com")),
            (
                "192.168.1.1.example.com",
                Some("https://192.168.1.1.example.com"),
            ),
            // Not a place at all.
            ("hello world", None),
            ("todo", None),
            ("[::1]:3000", Some("http://[::1]:3000")),
        ];
        for (typed, wanted) in cases {
            assert_eq!(url(typed).as_ref().map(Url::as_str), wanted, "{typed}");
        }
    }

    /// Search's `AddressTests.testAddressesAndSearchPhrasesKeepTheirMeaning`.
    #[test]
    fn addresses_and_search_phrases_keep_their_meaning() {
        assert_eq!(
            url("example.com").map(|u| u.to_string()).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            url("localhost:3000/test").map(|u| u.to_string()).as_deref(),
            Some("http://localhost:3000/test")
        );
        assert_eq!(
            url("https://example.com/a?q=b#c")
                .map(|u| u.to_string())
                .as_deref(),
            Some("https://example.com/a?q=b#c")
        );
        for text in [
            "",
            "two words",
            "./report.pdf",
            "person@example.com",
            "javascript:alert(1)",
        ] {
            assert_eq!(url(text), None, "{text}");
            assert_eq!(file(text), None, "{text}");
        }
    }

    /// Search's `AddressTests.testOnlyRegularFilesAreAccepted`, with the same cases.
    #[test]
    fn only_regular_files_are_accepted() {
        let folder = std::env::temp_dir().join(format!("tap-address-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        let regular = folder.join("regular.txt");
        std::fs::write(&regular, "text").expect("file");
        let link = folder.join("link");
        std::os::unix::fs::symlink(&regular, &link).expect("link");
        let path = |p: &Path| p.to_string_lossy().into_owned();

        let url = file(&format!(" {}\n", path(&regular))).expect("a regular file");
        assert!(url.is_file());
        assert_eq!(
            super::url(&path(&regular)),
            None,
            "suggestions must not resolve paths"
        );
        for not_regular in [
            path(&folder),
            path(&link),
            "/dev/zero".into(),
            format!("/{}", path(&regular)),
        ] {
            assert_eq!(file(&not_regular), None, "{not_regular}");
        }
        std::fs::remove_dir_all(&folder).expect("cleanup");
    }

    #[test]
    fn editable_addresses_go_back_to_the_same_page() {
        let page = |s: &str| Url::parse(s).expect("parses");
        assert_eq!(editable(&page("https://example.com/")), "example.com");
        assert_eq!(
            editable(&page("https://example.com/a?b=1")),
            "example.com/a?b=1"
        );
        assert_eq!(
            editable(&page("http://example.com/")),
            "http://example.com/"
        );
        assert_eq!(editable(&page("http://localhost:3000/")), "localhost:3000");
    }

    #[test]
    fn pretty_drops_www_and_a_bare_slash() {
        let page = |s: &str| Url::parse(s).expect("parses");
        assert_eq!(pretty(&page("https://www.example.com/")), "example.com");
        assert_eq!(
            pretty(&page("https://example.com/docs/a")),
            "example.com/docs/a"
        );
        assert_eq!(without_www("WWW.Example.com"), "Example.com");
    }
}
