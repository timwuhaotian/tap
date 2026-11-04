//! Where words typed in the field are searched. A port of Search's `Engine.swift`.

use std::fmt::Write as _;

use crate::fnd::{self, Url};

/// The search engines Settings offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Google,
    DuckDuckGo,
    Bing,
    Ecosia,
    Startpage,
    Kagi,
    Brave,
    Qwant,
    Custom,
}

/// What stands in for the words while a template is parsed, so `%s` itself never meets
/// a URL parser.
const MARK: &str = "SEARCHWORDSGOHERE";

impl Engine {
    pub const ALL: [Engine; 9] = [
        Engine::Google,
        Engine::DuckDuckGo,
        Engine::Bing,
        Engine::Ecosia,
        Engine::Startpage,
        Engine::Kagi,
        Engine::Brave,
        Engine::Qwant,
        Engine::Custom,
    ];

    /// The value Search stores under `search.engine`.
    #[must_use]
    pub fn raw(self) -> &'static str {
        match self {
            Engine::Google => "google",
            Engine::DuckDuckGo => "duckduckgo",
            Engine::Bing => "bing",
            Engine::Ecosia => "ecosia",
            Engine::Startpage => "startpage",
            Engine::Kagi => "kagi",
            Engine::Brave => "brave",
            Engine::Qwant => "qwant",
            Engine::Custom => "custom",
        }
    }

    #[must_use]
    pub fn from_raw(raw: &str) -> Option<Engine> {
        Engine::ALL.into_iter().find(|e| e.raw() == raw)
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Engine::Google => "Google",
            Engine::DuckDuckGo => "DuckDuckGo",
            Engine::Bing => "Bing",
            Engine::Ecosia => "Ecosia",
            Engine::Startpage => "Startpage",
            Engine::Kagi => "Kagi",
            Engine::Brave => "Brave Search",
            Engine::Qwant => "Qwant",
            Engine::Custom => "Custom",
        }
    }

    /// The address with `%s` where the words go. A custom template that isn't usable
    /// falls back to the standard engine's.
    #[must_use]
    pub fn template(self, custom: &str) -> String {
        match self {
            Engine::Google => "https://www.google.com/search?q=%s".to_owned(),
            Engine::DuckDuckGo => "https://duckduckgo.com/?q=%s".to_owned(),
            Engine::Bing => "https://www.bing.com/search?q=%s".to_owned(),
            Engine::Ecosia => "https://www.ecosia.org/search?q=%s".to_owned(),
            Engine::Startpage => "https://www.startpage.com/sp/search?query=%s".to_owned(),
            Engine::Kagi => "https://kagi.com/search?q=%s".to_owned(),
            Engine::Brave => "https://search.brave.com/search?q=%s".to_owned(),
            Engine::Qwant => "https://www.qwant.com/?q=%s".to_owned(),
            Engine::Custom => {
                let trimmed = custom.trim();
                if accepts(trimmed) {
                    trimmed.to_owned()
                } else {
                    Engine::default().template("")
                }
            }
        }
    }

    /// What the field calls this engine: a custom one by its site.
    #[must_use]
    pub fn name(self, custom: &str) -> String {
        if self != Engine::Custom {
            return self.title().to_owned();
        }
        bare_host(custom).unwrap_or_else(|| Engine::default().title().to_owned())
    }
}

/// A template is usable when it has `%s` and the words can't change which site it is.
#[must_use]
pub fn accepts(template: &str) -> bool {
    host(template).is_some()
}

/// A template's host without a leading `www.`: what a person would call the site.
#[must_use]
pub fn bare_host(template: &str) -> Option<String> {
    let host = host(template)?;
    Some(
        host.strip_prefix("www.")
            .map_or(host.clone(), str::to_owned),
    )
}

/// The address that searches `text` with `template`, or `None` for nothing to search.
#[must_use]
pub fn url_for(text: &str, template: &str) -> Option<Url> {
    let words = text.trim();
    if words.is_empty() {
        return None;
    }
    let escaped = escape(words);
    let base = Url::parse(&template.replace("%s", MARK))?;
    Url::parse_strict(&base.as_str().replace(MARK, &escaped))
}

/// `addingPercentEncoding(withAllowedCharacters:)` with only the unreserved characters
/// allowed: everything else, spaces and `+` and `&` included, is encoded byte by byte.
fn escape(words: &str) -> String {
    let mut out = String::with_capacity(words.len() * 3);
    for byte in words.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn host(template: &str) -> Option<String> {
    let trimmed = template.trim();
    if !trimmed.contains("%s") {
        return None;
    }
    let one = trimmed.replace("%s", "a");
    let other = trimmed.replace("%s", "b");
    let scheme = fnd::components_scheme(&one)?.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = fnd::components_host(&one)?;
    if host.is_empty() || fnd::components_host(&other).as_deref() != Some(host.as_str()) {
        return None;
    }
    Some(host.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_escaped_into_the_template() {
        let url = url_for("cats & dogs", &Engine::Google.template("")).expect("an address");
        assert_eq!(
            url.as_str(),
            "https://www.google.com/search?q=cats%20%26%20dogs"
        );
        let url = url_for("  ü+1 ", &Engine::DuckDuckGo.template("")).expect("an address");
        assert_eq!(url.as_str(), "https://duckduckgo.com/?q=%C3%BC%2B1");
        assert_eq!(url_for("   ", &Engine::Google.template("")), None);
    }

    #[test]
    fn a_custom_template_must_keep_one_site() {
        assert!(accepts("https://example.com/search?q=%s"));
        assert!(!accepts("https://%s.example.com/"));
        assert!(!accepts("ftp://example.com/?q=%s"));
        assert!(!accepts("https://example.com/"));
        assert_eq!(
            Engine::Custom.template("https://%s.evil.com/"),
            "https://www.google.com/search?q=%s"
        );
        assert_eq!(
            Engine::Custom.name("https://www.example.com/?q=%s"),
            "example.com"
        );
    }
}
