//! Copy Clean Link (Search's `CleanLink.swift`, #594): an address without the
//! parameters that only say where a click came from. A short list, kept on purpose: a
//! parameter that might do something for the page stays, unknown ones stay, and so do
//! the others' order, their encoding and the part after #. An address that carries a
//! signature is copied as it is, since taking anything out of it would break it.
//! Nothing is looked up: it all happens here, on the text.

use crate::fnd;

/// Names that track and do nothing else, on any site. `utm_` covers every one of
/// Google Analytics' campaign tags.
const TRACKING: [&str; 15] = [
    "fbclid", "gclid", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "ttclid",
    "mc_eid", "igshid", "igsh", "_hsenc", "_hsmi", "mkt_tok",
];

/// Names that track on some sites and may mean something on others: a YouTube or
/// Spotify share's `si` is who shared it.
const TRACKING_ON: [(&str, &[&str]); 1] = [("si", &["youtube.com", "youtu.be", "spotify.com"])];

/// A signed address: S3, Google Cloud, `CloudFront` and the like.
const SIGNATURES: [&str; 4] = ["x-amz-signature", "x-goog-signature", "signature", "sig"];

/// The address without its tracking parameters, or the same address when there is
/// nothing to take out or it can't be touched safely.
#[must_use]
pub fn clean(url: &str) -> String {
    cleaned(url).unwrap_or_else(|| url.to_owned())
}

fn cleaned(url: &str) -> Option<String> {
    let parts = fnd::Parts::of(url)?;
    let scheme = parts.scheme()?.to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let query = parts.query().filter(|q| !q.is_empty())?;
    let host = parts.host().unwrap_or_default().to_lowercase();
    let pieces: Vec<&str> = query.split('&').collect();
    let names: Vec<String> = pieces
        .iter()
        .map(|piece| {
            let raw = piece.split('=').next().unwrap_or_default();
            let spaced = raw.replace('+', " ");
            fnd::decode(&spaced)
                .unwrap_or_else(|| raw.to_owned())
                .to_lowercase()
        })
        .collect();
    if names.iter().any(|name| SIGNATURES.contains(&name.as_str())) {
        return None;
    }
    let kept: Vec<&str> = pieces
        .iter()
        .zip(&names)
        .filter(|(_, name)| !tracks(name, &host))
        .map(|(piece, _)| *piece)
        .collect();
    if kept.len() == pieces.len() {
        return None;
    }
    let joined = kept.join("&");
    parts.set_query((!kept.is_empty()).then_some(joined.as_str()));
    parts.url().map(|u| u.as_str().to_owned())
}

/// Whether `name` (decoded, lower case) only says where a click came from, on `host`.
#[must_use]
pub fn tracks(name: &str, host: &str) -> bool {
    if name.starts_with("utm_") || TRACKING.contains(&name) {
        return true;
    }
    TRACKING_ON
        .iter()
        .find(|(n, _)| *n == name)
        .is_some_and(|(_, sites)| {
            sites
                .iter()
                .any(|site| host == *site || host.ends_with(&format!(".{site}")))
        })
}

/// Whether Copy Clean Link would copy something other than the address itself.
#[must_use]
pub fn has_tracking(url: &str) -> bool {
    cleaned(url).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Search's CleanLinkTests, but for the pasteboard (the shell's).

    #[test]
    fn tracking_goes_the_rest_stays_in_order() {
        assert_eq!(
            clean("https://example.com/article?id=42&utm_source=newsletter#details"),
            "https://example.com/article?id=42#details"
        );
        assert_eq!(
            clean("https://example.com/?b=2&utm_medium=x&a=1&fbclid=abc&gclid=1&z=%20y"),
            "https://example.com/?b=2&a=1&z=%20y"
        );
        assert_eq!(
            clean("https://example.com/p?UTM_Campaign=Fall&mc_eid=9&igshid=q&msclkid=m"),
            "https://example.com/p"
        );
        assert_eq!(
            clean("https://example.com/p?utm_source=a"),
            "https://example.com/p"
        );
    }

    #[test]
    fn nothing_to_take_out_leaves_it_alone() {
        for text in [
            "https://example.com/",
            "https://example.com/?q=a+b&x=%2F#top",
            "https://example.com/?si=keep",
            "file:///Users/me/a.html?utm_source=x",
            "mailto:a@example.com?subject=utm_source",
        ] {
            assert_eq!(clean(text), text);
            assert!(!has_tracking(text), "{text}");
        }
    }

    #[test]
    fn share_id_only_where_it_tracks() {
        assert_eq!(
            clean("https://youtu.be/abc?si=XyZ&t=30"),
            "https://youtu.be/abc?t=30"
        );
        assert_eq!(
            clean("https://www.youtube.com/watch?v=abc&si=XyZ"),
            "https://www.youtube.com/watch?v=abc"
        );
        assert_eq!(
            clean("https://open.spotify.com/track/1?si=abc"),
            "https://open.spotify.com/track/1"
        );
        assert_eq!(
            clean("https://notyoutube.com/watch?si=1"),
            "https://notyoutube.com/watch?si=1"
        );
    }

    #[test]
    fn signed_addresses_are_copied_as_they_are() {
        let signed = "https://bucket.s3.amazonaws.com/f.pdf?utm_source=x&X-Amz-Signature=abc";
        assert_eq!(clean(signed), signed);
        let goog = "https://storage.googleapis.com/b/o?X-Goog-Signature=1&utm_medium=y";
        assert_eq!(clean(goog), goog);
    }
}
