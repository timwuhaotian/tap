//! The ad and tracker blocker (Search's `Shield.swift` and `YouTubeAds.swift`): one
//! content rule list compiled once by `WKContentRuleListStore`, enforced inside
//! WebKit's networking before a request is made, plus two small YouTube scripts
//! because YouTube announces its ads in the player's own data rather than fetching
//! them from an ad server a rule list can name.
//!
//! A rule list is added per web view's content controller, so switching Shield off,
//! or pausing it for one site (`shield.paused`, hosts without `www.`), takes effect
//! on the pages already open from their next request.

use std::{cell::RefCell, rc::Rc};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_foundation::{NSError, NSString};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore, WKUserContentController};

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Default)]
struct State {
    /// The compiled list, once WebKit has made it.
    list: Option<Retained<WKContentRuleList>>,
    /// Compiling was asked for and hasn't answered yet.
    compiling: bool,
    /// Settings › Privacy › Block ads and trackers.
    on: bool,
    /// Sites Shield is off for, as Settings keeps them (no `www.`).
    paused: Vec<String>,
    /// Told once the list lands, so pages opened meanwhile get it.
    ready: Option<Rc<dyn Fn()>>,
}

/// What Settings says, kept here so a page tuned at navigation time and a script
/// rebuilt at settings time agree.
pub fn set(on: bool, paused: Vec<String>) {
    STATE.with_borrow_mut(|state| {
        state.on = on;
        state.paused = paused;
    });
}

/// Whether Shield is working on `host` (a page's host, with or without `www.`).
#[must_use]
pub fn on_for(host: Option<&str>) -> bool {
    STATE.with_borrow(|state| state.on && !is_paused(state, host))
}

fn is_paused(state: &State, host: Option<&str>) -> bool {
    let Some(host) = host.map(host_key) else {
        return false;
    };
    state.paused.iter().any(|site| host_key(site) == host)
}

/// A host as sites are kept: lower case, no `www.`.
#[must_use]
pub fn host_key(host: &str) -> String {
    let host = host.to_lowercase();
    host.strip_prefix("www.")
        .map_or_else(|| host.clone(), str::to_owned)
}

/// Compiles the list once; `ready` runs when it lands (or at once if it already has),
/// so the caller can put it on the pages that are open.
pub fn compile(mtm: MainThreadMarker, ready: impl Fn() + 'static) {
    let (already, compiling) = STATE.with_borrow(|state| (state.list.is_some(), state.compiling));
    if already {
        ready();
        return;
    }
    if compiling {
        STATE.with_borrow_mut(|state| state.ready = Some(Rc::new(ready)));
        return;
    }
    // SAFETY: WebKit's own store on the main thread; the block runs on the main queue
    // and only keeps what it is handed for the call.
    let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else {
        return;
    };
    STATE.with_borrow_mut(|state| {
        state.compiling = true;
        state.ready = Some(Rc::new(ready));
    });
    let done = RcBlock::new(move |list: *mut WKContentRuleList, _error: *mut NSError| {
        // SAFETY: WebKit hands the compiled list it made, retained for the receiver.
        let list = unsafe { Retained::retain(list) };
        let ready = STATE.with_borrow_mut(|state| {
            state.compiling = false;
            state.list = list;
            state.ready.take()
        });
        if let Some(ready) = ready {
            ready();
        }
    });
    let identifier = NSString::from_str("tap-shield");
    let rules = NSString::from_str(&rules_json());
    // SAFETY: a JSON string in WebKit's rule format (built and tested below) and a
    // block that outlives the call.
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&identifier),
            Some(&rules),
            Some(&done),
        );
    }
}

/// Puts the list on one page's controller, or takes it off, for the site that page
/// is on: from the next request, as Search's `tune` does at each navigation.
pub fn tune(controller: &WKUserContentController, host: Option<&str>) {
    let list = STATE.with_borrow(|state| state.list.clone());
    let Some(list) = list else { return };
    // SAFETY: plain calls on a live page's controller with our own list.
    unsafe {
        controller.removeContentRuleList(&list);
        if on_for(host) {
            controller.addContentRuleList(&list);
        }
    }
}

/// Third parties whose only job is to watch or to sell (Search's `Shield.unwanted`).
/// First-party requests are untouched: a site's own scripts are the site.
const UNWANTED: [&str; 41] = [
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "googletagservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "adservice.google.com",
    "amazon-adsystem.com",
    "adnxs.com",
    "adsrvr.org",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "casalemedia.com",
    "smartadserver.com",
    "sharethrough.com",
    "indexww.com",
    "bidswitch.net",
    "33across.com",
    "teads.tv",
    "moatads.com",
    "adroll.com",
    "scorecardresearch.com",
    "quantserve.com",
    "chartbeat.com",
    "hotjar.com",
    "mouseflow.com",
    "fullstory.com",
    "clarity.ms",
    "mixpanel.com",
    "amplitude.com",
    "segment.com",
    "segment.io",
    "branch.io",
    "appsflyer.com",
    "adjust.com",
    "analytics.tiktok.com",
];

/// Slots that are reliably an advertisement and nothing else (Search's `Shield.slots`),
/// hidden whole-word by WebKit's cosmetic rule.
const SLOTS: &str = ".adsbygoogle, ins.adsbygoogle, [id^=\"google_ads_\"], [id^=\"div-gpt-ad\"], \
    [id^=\"taboola-\"], #taboola-below-article, iframe[src*=\"doubleclick.net\"], \
    iframe[src*=\"googlesyndication\"], iframe[src*=\"amazon-adsystem\"], .ad-slot, \
    .ad-slot-container, .top-banner-ad-container, .ad-leaderboard, .ad-billboard, .ad-giga, \
    .ad-mpu, .ad-mrec, .ad-unit, .adunit, .adslot, .dfp-ad, .gpt-ad, .w_ad";

/// YouTube paths that carry ads and nothing a viewer asked for (Search's
/// `YouTubeAds.paths`), anchored on YouTube's own hosts.
const YOUTUBE_PATHS: [&str; 6] = [
    "/api/stats/ads",
    "/pagead/",
    "/ptracking",
    "/get_midroll_info",
    "/youtubei/v1/player/ad_break",
    "/pcs/activeview",
];

fn network_filter(domain: &str) -> String {
    // WebKit's canonical HTTP URLs end the host with a port or a slash.
    format!("^https?://([^/]+\\.)?{}[:/]", domain.replace('.', "\\."))
}

/// The rule list as JSON, in WebKit's content-rule format.
#[must_use]
pub fn rules_json() -> String {
    let mut rules: Vec<serde_json::Value> = UNWANTED
        .iter()
        .map(|domain| {
            serde_json::json!({
                "trigger": { "url-filter": network_filter(domain), "load-type": ["third-party"] },
                "action": { "type": "block" },
            })
        })
        .collect();
    rules.push(serde_json::json!({
        "trigger": { "url-filter": ".*" },
        "action": { "type": "css-display-none", "selector": SLOTS },
    }));
    for path in YOUTUBE_PATHS {
        rules.push(serde_json::json!({
            "trigger": { "url-filter": format!("^https?://([^/]+\\.)?youtube\\.com{}", path.replace('.', "\\.")) },
            "action": { "type": "block" },
        }));
    }
    rules.push(serde_json::json!({
        "trigger": { "url-filter": "^https?://([^/]+\\.)?youtube\\.com/api/stats/qoe\\?.*adformat=" },
        "action": { "type": "block" },
    }));
    for filter in [
        "^https?://([^/]+\\.)?google\\.com/pagead/",
        "^https?://imasdk\\.googleapis\\.com/",
    ] {
        rules.push(serde_json::json!({
            "trigger": { "url-filter": filter, "if-domain": ["*youtube.com", "*youtube-nocookie.com"] },
            "action": { "type": "block" },
        }));
    }
    serde_json::to_string(&rules).unwrap_or_else(|_| "[]".to_owned())
}

/// In the page's own world on YouTube's hosts, before any of its scripts (Search's
/// `YouTubeAds.page`, kept to the same four keys): the ad schedule is taken out of
/// the player's own data before the player reads it. Only YouTube-shaped answers
/// are touched; anything else is handed on as it came, so a change at YouTube
/// leaves the video playing, with its ads, rather than broken. Paused sites are
/// named at rebuild time, as Settings keeps them.
#[must_use]
pub fn youtube_page_script(paused: &[String]) -> String {
    let paused = serde_json::to_string(&paused.iter().map(|h| host_key(h)).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".to_owned());
    format!(
        r"(function () {{
  'use strict';
  var hosts = ['youtube.com', 'm.youtube.com', 'music.youtube.com', 'youtube-nocookie.com'];
  var host = location.hostname.toLowerCase().replace(/^www\./, '');
  if (hosts.indexOf(host) < 0 || {paused}.indexOf(host) >= 0) return;
  var ads = ['adPlacements', 'adSlots', 'playerAds', 'adBreakHeartbeatParams'];
  var marks = ['responseContext', 'playabilityStatus', 'streamingData', 'videoDetails'];
  function plain(o) {{ return o !== null && typeof o === 'object' && !Array.isArray(o); }}
  function strip(o) {{
    if (!plain(o)) return;
    var shaped = false;
    for (var i = 0; i < marks.length; i++) if (Object.prototype.hasOwnProperty.call(o, marks[i])) shaped = true;
    if (!shaped) return;
    for (var j = 0; j < ads.length; j++) delete o[ads[j]];
  }}
  function prune(value) {{
    try {{
      if (Array.isArray(value)) {{ for (var i = 0; i < value.length && i < 8; i++) {{ strip(value[i]); if (value[i]) strip(value[i].playerResponse); }} }}
      else {{ strip(value); if (value) strip(value.playerResponse); }}
    }} catch (e) {{}}
    return value;
  }}
  try {{
    var parse = JSON.parse;
    JSON.parse = function (text, reviver) {{ return prune(parse.call(JSON, text, reviver)); }};
  }} catch (e) {{}}
  try {{
    var json = Response.prototype.json, then = Promise.prototype.then;
    Response.prototype.json = function () {{ return then.call(json.call(this), prune); }};
  }} catch (e) {{}}
  ['ytInitialPlayerResponse', 'ytInitialData'].forEach(function (name) {{
    try {{
      if (Object.prototype.hasOwnProperty.call(window, name)) return;
      var held;
      Object.defineProperty(window, name, {{
        get: function () {{ return held; }},
        set: function (value) {{ held = prune(value); }},
        enumerable: true, configurable: true
      }});
    }} catch (e) {{}}
  }});
}})();"
    )
}

/// In Tap's own world (Search's `YouTubeAds.skipper`): the fallback when an ad
/// plays all the same. While the player says `ad-showing` the video is muted and
/// sped up, and put back the moment the ad is over. It never seeks.
pub const YOUTUBE_SKIPPER: &str = r"(function () {
  'use strict';
  var hosts = ['youtube.com', 'www.youtube.com', 'm.youtube.com', 'music.youtube.com', 'youtube-nocookie.com', 'www.youtube-nocookie.com'];
  if (hosts.indexOf(location.hostname) < 0) return;
  var held = new WeakMap();
  function look(event) {
    var video = event.target;
    if (!(video instanceof HTMLVideoElement)) return;
    var player = video.closest('.html5-video-player');
    var ad = !!player && player.classList.contains('ad-showing');
    var was = held.get(video);
    try {
      if (ad) {
        if (!was) { was = { muted: video.muted, rate: video.playbackRate }; held.set(video, was); video.muted = true; }
        if (video.playbackRate !== 16) video.playbackRate = 16;
      } else if (was) {
        held.delete(video);
        video.playbackRate = was.rate;
        video.muted = was.muted;
      }
    } catch (e) {}
  }
  ['playing', 'timeupdate', 'durationchange', 'emptied'].forEach(function (type) {
    document.addEventListener(type, look, true);
  });
})();";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keys_drop_www_and_case() {
        assert_eq!(host_key("WWW.YouTube.com"), "youtube.com");
        assert_eq!(host_key("example.com"), "example.com");
    }

    #[test]
    fn rules_are_valid_json_blocking_third_parties() {
        let rules: Vec<serde_json::Value> = serde_json::from_str(&rules_json()).unwrap_or_default();
        assert!(rules.len() > UNWANTED.len());
        let first = &rules[0];
        assert_eq!(first["action"]["type"], "block");
        assert_eq!(first["trigger"]["load-type"][0], "third-party");
        assert!(
            first["trigger"]["url-filter"]
                .as_str()
                .is_some_and(|f| f.contains("doubleclick\\.net"))
        );
        // A cosmetic rule and a YouTube rule ride along.
        assert!(
            rules
                .iter()
                .any(|r| r["action"]["type"] == "css-display-none")
        );
        assert!(rules.iter().any(|r| {
            r["trigger"]["url-filter"]
                .as_str()
                .is_some_and(|f| f.contains("youtube") && f.contains("/pagead/"))
        }));
    }

    #[test]
    fn youtube_script_names_its_paused_sites() {
        let script = youtube_page_script(&["www.youtube.com".to_owned()]);
        assert!(script.contains("\"youtube.com\""));
        assert!(script.contains("adPlacements"));
    }
}
