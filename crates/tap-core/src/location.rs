//! Where the page in front is, as the address bar says it: what its connection is, and
//! its address split so the host can stand out from the rest. Tap's own (Search has no
//! address bar: its live tab says where it is, `AtRest.swift`); the loopback and
//! address rules are the ones that tab line uses.

/// What the address bar's badge says about the page's connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// https, and everything on the page came that way.
    Secure,
    /// https, but an image or a script on it came over plain http.
    Mixed,
    /// Plain http to somewhere other than this Mac.
    NotSecure,
    /// A file on this Mac.
    File,
    /// A server on this Mac (`localhost`, `127.x`, `::1`), over whatever it speaks.
    ThisMac,
    /// One of Tap's own pages.
    Tap,
    /// Nothing to say: a new tab, `about:`, `data:`, a page of no website.
    Nothing,
}

/// Four numbers, or an IPv6 address: a machine, not a name.
#[must_use]
pub fn is_address(host: &str) -> bool {
    if host.contains(':') {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() == 4 && labels.iter().all(|label| label.parse::<u8>().is_ok())
}

/// This Mac: plain http here is the rule rather than a risk.
#[must_use]
pub fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    // 127.x as an address only: 127.0.0.1.example.com is a website like any other.
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || (is_address(host) && host.starts_with("127."))
}

/// An address's scheme, lower case, and its host, when it has one.
fn scheme_and_host(address: &str) -> Option<(String, &str)> {
    let (scheme, _) = address.split_once(':')?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), split(address).1))
}

/// The page's connection, from its address and whether WebKit says everything on it
/// came over a secure one (`hasOnlySecureContent`).
#[must_use]
pub fn security(address: &str, only_secure: bool) -> Security {
    let Some((scheme, host)) = scheme_and_host(address) else {
        return Security::Nothing;
    };
    match scheme.as_str() {
        "https" | "http" if is_loopback(host) => Security::ThisMac,
        "https" if only_secure => Security::Secure,
        "https" => Security::Mixed,
        "http" => Security::NotSecure,
        "file" => Security::File,
        "tap" => Security::Tap,
        _ => Security::Nothing,
    }
}

/// An address in three runs for the bar: what comes before the host (the scheme), the
/// host, and what follows (port, path, query). An address with no host is all lead.
#[must_use]
pub fn split(address: &str) -> (&str, &str, &str) {
    let Some(start) = address.find("://").map(|i| i + 3) else {
        return (address, "", "");
    };
    let after = &address[start..];
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..end];
    // Credentials before the host are the lead's, never the host's.
    let skip = authority.rfind('@').map_or(0, |at| at + 1);
    let host_end = if authority[skip..].starts_with('[') {
        authority[skip..].find(']').map_or(end, |b| skip + b + 1)
    } else {
        authority[skip..].find(':').map_or(end, |c| skip + c)
    };
    (
        &address[..start + skip],
        &address[start + skip..start + host_end],
        &address[start + host_end..],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_follows_the_scheme_and_what_the_page_pulled_in() {
        assert_eq!(security("https://example.com/", true), Security::Secure);
        assert_eq!(security("https://example.com/", false), Security::Mixed);
        assert_eq!(security("http://example.com/", true), Security::NotSecure);
        assert_eq!(security("HTTP://Example.com/", false), Security::NotSecure);
        assert_eq!(security("http://localhost:3000/", false), Security::ThisMac);
        assert_eq!(
            security("http://127.0.0.1:8080/x", false),
            Security::ThisMac
        );
        assert_eq!(security("http://[::1]:8080/", false), Security::ThisMac);
        assert_eq!(
            security("http://127.0.0.1.example.com/", false),
            Security::NotSecure,
            "a website that only starts like this Mac"
        );
        assert_eq!(security("file:///tmp/a.pdf", false), Security::File);
        assert_eq!(security("tap://settings", false), Security::Tap);
        assert_eq!(security("about:blank", false), Security::Nothing);
        assert_eq!(security("", false), Security::Nothing);
    }

    #[test]
    fn an_address_splits_around_its_host() {
        assert_eq!(
            split("https://www.example.com/a?b=1#c"),
            ("https://", "www.example.com", "/a?b=1#c")
        );
        assert_eq!(
            split("http://example.com:8080/x"),
            ("http://", "example.com", ":8080/x")
        );
        assert_eq!(
            split("https://user:pw@example.com/"),
            ("https://user:pw@", "example.com", "/")
        );
        assert_eq!(split("http://[::1]:3000/"), ("http://", "[::1]", ":3000/"));
        assert_eq!(
            split("https://example.com"),
            ("https://", "example.com", "")
        );
        assert_eq!(split("about:blank"), ("about:blank", "", ""));
    }

    #[test]
    fn loopback_is_this_mac_only() {
        assert!(is_loopback("localhost") && is_loopback("app.localhost"));
        assert!(is_loopback("127.0.0.1") && is_loopback("::1") && is_loopback("[::1]"));
        assert!(!is_loopback("127.0.0.1.example.com") && !is_loopback("example.com"));
        assert!(is_address("10.0.0.1") && is_address("::1") && !is_address("10.io"));
    }
}
