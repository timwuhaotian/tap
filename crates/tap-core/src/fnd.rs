//! Foundation's URL semantics, which are Search's (docs/PLAN.md §3.4).
//!
//! Search compares `absoluteString`s exactly: `example.com` must become
//! `https://example.com`, not the WHATWG `https://example.com/`. So a [`Url`] is made by
//! `NSURL` and keeps the exact string Foundation produced; its parts are read through
//! `NSURLComponents`, the percent-encoded way Swift's `url.host()`, `url.path()`,
//! `url.query()` and `url.fragment()` read them.

use objc2::rc::Retained;
use objc2_foundation::{NSString, NSURL, NSURLComponents};

/// An absolute address, exactly as Foundation writes it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Url(String);

impl std::fmt::Debug for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Url({})", self.0)
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Url {
    /// `URL(string:)`: invalid characters are percent-encoded, as Foundation does for
    /// apps linked against macOS 14 or later.
    #[must_use]
    pub fn parse(text: &str) -> Option<Url> {
        let url = NSURL::URLWithString(&NSString::from_str(text))?;
        Some(Url(url.absoluteString()?.to_string()))
    }

    /// `URL(string:encodingInvalidCharacters: false)`: nothing is encoded on the way
    /// in, and a string that isn't already a valid URL is refused.
    #[must_use]
    pub fn parse_strict(text: &str) -> Option<Url> {
        let url = NSURL::URLWithString_encodingInvalidCharacters(&NSString::from_str(text), false)?;
        Some(Url(url.absoluteString()?.to_string()))
    }

    /// `URL(fileURLWithPath:)`.
    #[must_use]
    pub fn file(path: &str) -> Option<Url> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        Some(Url(url.absoluteString()?.to_string()))
    }

    /// The whole address: Swift's `absoluteString`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn components(&self) -> Option<Retained<NSURLComponents>> {
        NSURLComponents::componentsWithString(&NSString::from_str(&self.0))
    }

    /// The scheme, as written.
    #[must_use]
    pub fn scheme(&self) -> Option<String> {
        self.components()?.scheme().map(|s| s.to_string())
    }

    /// Swift's `url.host()`: encoded (percent-encoded, and IDNA for a name in another
    /// script), as `NSURLComponents.encodedHost` gives it.
    #[must_use]
    pub fn host(&self) -> Option<String> {
        self.components()?.encodedHost().map(|s| s.to_string())
    }

    /// Swift's `url.path()`: percent-encoded, trailing slash kept.
    #[must_use]
    pub fn path(&self) -> String {
        self.components()
            .and_then(|c| c.percentEncodedPath())
            .map(|p| p.to_string())
            .unwrap_or_default()
    }

    /// Swift's `url.query()`: percent-encoded; `None` when there is no `?`.
    #[must_use]
    pub fn query(&self) -> Option<String> {
        self.components()?
            .percentEncodedQuery()
            .map(|s| s.to_string())
    }

    /// Swift's `url.fragment()`: percent-encoded; `None` when there is no `#`.
    #[must_use]
    pub fn fragment(&self) -> Option<String> {
        self.components()?
            .percentEncodedFragment()
            .map(|s| s.to_string())
    }

    /// Swift's `url.isFileURL`.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"))
    }
}

/// An address taken apart to change some of it, as Swift's `URLComponents` does: the
/// parts not touched keep their spelling and percent-encoding.
pub struct Parts(Retained<NSURLComponents>);

impl Parts {
    #[must_use]
    pub fn of(url: &str) -> Option<Parts> {
        NSURLComponents::componentsWithString(&NSString::from_str(url)).map(Parts)
    }

    #[must_use]
    pub fn scheme(&self) -> Option<String> {
        self.0.scheme().map(|s| s.to_string())
    }
    pub fn set_scheme(&self, scheme: Option<&str>) {
        self.0.setScheme(scheme.map(NSString::from_str).as_deref());
    }
    /// Decoded, as `URLComponents.host` reads it.
    #[must_use]
    pub fn host(&self) -> Option<String> {
        self.0.host().map(|s| s.to_string())
    }
    pub fn set_host(&self, host: Option<&str>) {
        self.0.setHost(host.map(NSString::from_str).as_deref());
    }
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        self.0.user().is_some() || self.0.password().is_some()
    }
    /// Takes off a name and password written into the address.
    pub fn clear_credentials(&self) {
        self.0.setUser(None);
        self.0.setPassword(None);
    }
    pub fn clear_fragment(&self) {
        self.0.setFragment(None);
    }
    pub fn clear_query(&self) {
        self.0.setPercentEncodedQuery(None);
    }
    #[must_use]
    pub fn path(&self) -> String {
        self.0
            .percentEncodedPath()
            .map(|p| p.to_string())
            .unwrap_or_default()
    }
    pub fn set_path(&self, path: &str) {
        self.0
            .setPercentEncodedPath(Some(&NSString::from_str(path)));
    }
    /// `URLComponents.string`.
    #[must_use]
    pub fn string(&self) -> Option<String> {
        self.0.string().map(|s| s.to_string())
    }
    /// `URLComponents.url`, as an address.
    #[must_use]
    pub fn url(&self) -> Option<Url> {
        Some(Url(self.0.URL()?.absoluteString()?.to_string()))
    }
}

/// `URLComponents(string:)`'s host, decoded, as Search's `Engine.host(of:)` reads it.
#[must_use]
pub fn components_host(text: &str) -> Option<String> {
    let parts = NSURLComponents::componentsWithString(&NSString::from_str(text))?;
    parts.host().map(|h| h.to_string())
}

/// `URLComponents(string:)`'s scheme.
#[must_use]
pub fn components_scheme(text: &str) -> Option<String> {
    let parts = NSURLComponents::componentsWithString(&NSString::from_str(text))?;
    parts.scheme().map(|s| s.to_string())
}

/// `URLComponents(string:)`'s percent-encoded path, query and fragment, for
/// `Keyword.accepts`.
#[must_use]
pub fn components_tail(text: &str) -> Option<[Option<String>; 3]> {
    let parts = NSURLComponents::componentsWithString(&NSString::from_str(text))?;
    Some([
        parts.percentEncodedPath().map(|s| s.to_string()),
        parts.percentEncodedQuery().map(|s| s.to_string()),
        parts.percentEncodedFragment().map(|s| s.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_keeps_a_bare_host_bare() {
        let url = Url::parse("https://example.com").expect("parses");
        assert_eq!(url.as_str(), "https://example.com");
        assert_eq!(url.path(), "");
        assert_eq!(url.host().as_deref(), Some("example.com"));
        assert_eq!(url.query(), None);
    }

    #[test]
    fn parts_read_percent_encoded_with_trailing_slash() {
        let url = Url::parse("https://example.com/a%20b/?q=1#top").expect("parses");
        assert_eq!(url.path(), "/a%20b/");
        assert_eq!(url.query().as_deref(), Some("q=1"));
        assert_eq!(url.fragment().as_deref(), Some("top"));
    }

    #[test]
    fn strict_parsing_refuses_what_lenient_parsing_mends() {
        assert!(Url::parse("https://example.com/a b").is_some());
        assert!(Url::parse_strict("https://example.com/a b").is_none());
    }
}
