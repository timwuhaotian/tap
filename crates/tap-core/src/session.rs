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

/// The row of tabs and which one was in front; `active` is -1 for none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub tabs: Vec<Entry>,
    pub active: i64,
    /// Tab groups and Split View pairs are kept exactly as read, so a session written
    /// by Search loses nothing when Tap writes it back before those features land.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub groups: Option<serde_json::Value>,
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
        assert!(again.groups.is_some());
        assert_eq!(again.splits.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_file_that_wont_decode_is_not_an_empty_session() {
        assert!(Shape::decode(b"{\"tabs\": 3}").is_none());
        assert!(Shape::decode(b"not json").is_none());
    }
}
