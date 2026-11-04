//! A word typed before the search words that picks the site: "yt cats" goes straight to
//! YouTube's search. A port of Search's `Keyword.swift`.

use serde::{Deserialize, Serialize};

use crate::{engine, fnd};

const MARK: &str = "SEARCHWORDSGOHERE";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyword {
    pub id: String,
    pub keyword: String,
    pub template: String,
}

impl Keyword {
    /// The words to search with when `typed` starts with this keyword and a space.
    fn matches(&self, typed: &str) -> Option<String> {
        let word = self.keyword.trim();
        if word.is_empty() {
            return None;
        }
        let space = typed.find(' ')?;
        if typed[..space].to_lowercase() != word.to_lowercase() {
            return None;
        }
        let rest = typed[space + 1..].trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    }
}

/// The first keyword `typed` names and the words to search it with. One whose template
/// wouldn't be accepted today is passed over rather than followed.
#[must_use]
pub fn find<'a>(typed: &str, keywords: &'a [Keyword]) -> Option<(&'a Keyword, String)> {
    keywords
        .iter()
        .filter(|k| accepts(&k.template))
        .find_map(|k| k.matches(typed).map(|rest| (k, rest)))
}

/// A keyword's template: http or https, one site whatever is searched for, and a single
/// `%s` in the path, query or fragment, never in the host or before an `@`.
#[must_use]
pub fn accepts(template: &str) -> bool {
    let trimmed = template.trim();
    if !engine::accepts(trimmed) || trimmed.split("%s").count() != 2 {
        return false;
    }
    let Some(parts) = fnd::components_tail(&trimmed.replace("%s", MARK)) else {
        return false;
    };
    parts.iter().flatten().any(|part| part.contains(MARK))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yt() -> Keyword {
        Keyword {
            id: "1".into(),
            keyword: "yt".into(),
            template: "https://www.youtube.com/results?search_query=%s".into(),
        }
    }

    #[test]
    fn a_keyword_takes_the_words_after_it() {
        let list = [yt()];
        let (keyword, rest) = find("YT  lo-fi beats ", &list).expect("matches");
        assert_eq!(keyword.keyword, "yt");
        assert_eq!(rest, "lo-fi beats");
        assert!(find("yt", &list).is_none());
        assert!(find("yt   ", &list).is_none());
        assert!(find("youtube cats", &list).is_none());
    }

    #[test]
    fn templates_must_put_the_words_after_the_host() {
        assert!(accepts("https://example.com/s?q=%s"));
        assert!(accepts("https://example.com/s/%s"));
        assert!(!accepts("https://example.com/s?q=%s&r=%s"));
        assert!(!accepts("https://%s.example.com/"));
    }
}
