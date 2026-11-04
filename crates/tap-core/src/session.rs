//! What was open last time: `session.json`, in Search's format (Search's `Session.swift`),
//! so a Search session reads as it is (docs/PLAN.md §7).
//!
//! Read leniently, as Search does: a value that doesn't make sense in one entry is taken
//! as absent, never as a file that won't decode, which would lose every tab.

use serde::{Deserialize, Deserializer, Serialize};

/// One tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    pub title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub pin: Option<String>,
    /// The name you gave the tab, when you gave it one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub name: Option<String>,
    /// A pin's own page.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub home: Option<String>,
    #[serde(
        rename = "groupID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub group_id: Option<String>,
    #[serde(
        rename = "pinID",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub pin_id: Option<String>,
    /// Seconds since 2001-01-01, as Swift's `JSONEncoder` writes a `Date`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub touched: Option<f64>,
    #[serde(
        rename = "letGo",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub let_go: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub listed: Option<bool>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub muted: bool,
}

/// A named section of ordinary tabs (Search's `TabGroup.swift`). Its tabs stay ordinary
/// tabs; their own `groupID` says which group they are in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    /// Folded: only its heading shows, and the tab in front if it is one of its own. A
    /// group saved without it is an open one.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub collapsed: bool,
}

/// The row of tabs and which one was in front; `active` is -1 for none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub tabs: Vec<Entry>,
    pub active: i64,
    /// The row's tab groups, in order (Search's `TabGroup`). Read leniently: a value
    /// that doesn't make sense is taken for no groups at all, never for a file that won't
    /// decode, which would bring back not a single tab.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub groups: Option<Vec<TabGroup>>,
    /// Split View pairs are kept exactly as read, so a session written by Search loses
    /// nothing when Tap writes it back before Split View lands.
    #[serde(default, deserialize_with = "lenient_splits")]
    pub splits: serde_json::Value,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            tabs: Vec::new(),
            active: 0,
            groups: None,
            splits: serde_json::Value::Array(Vec::new()),
        }
    }
}

impl Shape {
    /// The file's contents, or `None` for a file that is there but won't decode, which
    /// the caller quarantines rather than overwrites.
    #[must_use]
    pub fn decode(data: &[u8]) -> Option<Shape> {
        serde_json::from_slice(data).ok()
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The row as Settings › Start with a fresh window leaves it at launch (Search's
    /// `Session.fresh`): what you said to keep, as tabs that close themselves count it —
    /// pins, named tabs, and the tabs in groups with their groups — and nothing else. A
    /// Split View pair stays only when both its pages do. No tab is in front, so the
    /// window opens on a new tab beside them.
    #[must_use]
    pub fn fresh(self) -> Shape {
        let groups = self.groups.unwrap_or_default();
        let mut places = std::collections::HashMap::new();
        let mut tabs = Vec::new();
        for (index, entry) in self.tabs.into_iter().enumerate() {
            let grouped = entry
                .group_id
                .as_deref()
                .is_some_and(|id| groups.iter().any(|g| g.id == id));
            if entry.pin.is_some() || entry.name.is_some() || grouped {
                places.insert(index, tabs.len());
                tabs.push(entry);
            }
        }
        let groups: Vec<TabGroup> = groups
            .into_iter()
            .filter(|g| tabs.iter().any(|t| t.group_id.as_deref() == Some(&g.id)))
            .collect();
        let moved = |value: &serde_json::Value| {
            value
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| places.get(&i))
                .map(|&i| serde_json::Value::from(i))
        };
        let splits = self
            .splits
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pair| {
                let pages = pair.get("tabs")?.as_array()?;
                let kept: Vec<serde_json::Value> = pages.iter().filter_map(moved).collect();
                if kept.len() != pages.len() {
                    return None;
                }
                let mut pair = pair.clone();
                pair["tabs"] = kept.into();
                if let Some(focused) = pair.get("focused").cloned() {
                    pair["focused"] = moved(&focused).unwrap_or_default();
                }
                Some(pair)
            })
            .collect();
        Shape {
            tabs,
            active: -1,
            groups: (!groups.is_empty()).then_some(groups),
            splits: serde_json::Value::Array(splits),
        }
    }
}

fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

fn lenient_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(lenient::<D, bool>(deserializer)?.unwrap_or(false))
}

fn lenient_splits<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<serde_json::Value, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(if value.is_array() {
        value
    } else {
        serde_json::Value::Array(Vec::new())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file in Search's own format: what Tap needs reads, what it doesn't know yet
    /// survives a round trip.
    #[test]
    fn a_search_session_reads_and_survives() {
        let written = br#"{"tabs":[
            {"url":"https://example.com","title":"Example","muted":true,"touched":815000000.5,
             "groupID":"8E2D4C1A-0000-4000-8000-000000000001"},
            {"url":"https://www.rust-lang.org/","title":"Rust","listed":"not a bool","pin":"R"}],
          "active":1,
          "groups":[{"id":"8E2D4C1A-0000-4000-8000-000000000001","name":"Work","folded":false}],
          "splits":[{"tabs":[0,1],"axis":"horizontal","sizes":[0.5,0.5]}]}"#;
        let shape = Shape::decode(written).expect("decodes");
        assert_eq!(shape.tabs.len(), 2);
        assert_eq!(shape.active, 1);
        assert!(shape.tabs[0].muted);
        assert_eq!(shape.tabs[0].touched, Some(815_000_000.5));
        assert_eq!(shape.tabs[1].listed, None, "a bad value is taken as absent");
        assert_eq!(shape.tabs[1].pin.as_deref(), Some("R"));

        let again = Shape::decode(&shape.encode()).expect("round trip");
        assert_eq!(again, shape);
        let groups = again.groups.expect("groups survive");
        assert_eq!(
            (groups[0].name.as_str(), groups[0].collapsed),
            ("Work", false)
        );
        assert_eq!(again.splits.as_array().map(Vec::len), Some(1));
    }

    /// Search's leniency for groups: nonsense there is no groups, not a lost session.
    #[test]
    fn groups_that_make_no_sense_are_no_groups() {
        let shape = Shape::decode(
            br#"{"tabs":[{"url":"https://a.com","title":"A"}],"active":0,
            "groups":[{"id":"X"}]}"#,
        )
        .expect("the tabs still come back");
        assert_eq!((shape.tabs.len(), shape.groups), (1, None));
        let folded = Shape::decode(
            br#"{"tabs":[],"active":0,
            "groups":[{"id":"X","name":"Later","collapsed":true}]}"#,
        )
        .expect("decodes");
        assert!(folded.groups.is_some_and(|g| g[0].collapsed));
    }

    #[test]
    fn a_file_that_wont_decode_is_not_an_empty_session() {
        assert!(Shape::decode(b"{\"tabs\": 3}").is_none());
        assert!(Shape::decode(b"not json").is_none());
    }

    #[test]
    fn a_fresh_start_keeps_pins_named_and_grouped_tabs_only() {
        let written = br#"{"tabs":[
            {"url":"https://loose.com","title":"Loose"},
            {"url":"https://pin.com","title":"Pin","pin":"P"},
            {"url":"https://g.com","title":"G","groupID":"G1"},
            {"url":"https://named.com","title":"N","name":"Mine"},
            {"url":"https://orphan.com","title":"O","groupID":"GONE"}],
          "active":0,
          "groups":[{"id":"G1","name":"Work"},{"id":"G2","name":"Empty"}],
          "splits":[{"tabs":[1,2],"focused":2},{"tabs":[0,3]}]}"#;
        let fresh = Shape::decode(written).expect("decodes").fresh();
        let urls: Vec<&str> = fresh.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://pin.com", "https://g.com", "https://named.com"]
        );
        assert_eq!(
            fresh.active, -1,
            "no tab in front: a new one opens beside them"
        );
        let groups = fresh.groups.expect("its group");
        assert_eq!(groups.len(), 1, "a group with no tab left goes");
        assert_eq!(
            fresh.splits,
            serde_json::json!([{"tabs":[0,1],"focused":1}]),
            "a pair stays only whole"
        );
    }
}
