//! Settings as they are kept: `NSUserDefaults`, under Search's key names and with Search's
//! values (docs/PLAN.md §7), so a Search profile's settings read as they are. A test run
//! keeps its own suite, `com.timwuhaotian.tap.test[.<world>]` (plan §6), so it never
//! touches yours.
//!
//! Values cross as JSON in the shape Search's profile file gives them: a `Data` is
//! `{"$data": base64}`, a `Date` `{"$date": seconds since 1970}`.

use objc2::{AnyThread, MainThreadMarker, Message, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication};
use objc2_foundation::{
    NSArray, NSData, NSDataBase64DecodingOptions, NSDataBase64EncodingOptions, NSDate,
    NSDictionary, NSNumber, NSString, NSUserDefaults,
};
use serde_json::{Map, Number, Value};
use tap_core::{
    browser::{Behaviour, NewPlace},
    engine::Engine,
    identity,
    keyword::Keyword,
    settings_index::{self, Prefs},
};

use crate::rt;

/// Where settings are kept: the app's own domain, or a test world's suite.
pub fn defaults() -> Retained<NSUserDefaults> {
    let Some(world) = identity::world(std::env::var(identity::PROBE_ENV).ok().as_deref()) else {
        return NSUserDefaults::standardUserDefaults();
    };
    let suite = format!("{}.test.{world}", identity::BUNDLE_ID);
    NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&NSString::from_str(&suite)))
        .unwrap_or_else(NSUserDefaults::standardUserDefaults)
}

/// The settings Settings reads and writes.
pub struct Kept(Retained<NSUserDefaults>);

impl Kept {
    pub fn new() -> Kept {
        Kept(defaults())
    }

    /// Another app's settings, read only: Search's, for its profile packed on this Mac.
    fn of(domain: &str) -> Option<Kept> {
        NSUserDefaults::initWithSuiteName(
            NSUserDefaults::alloc(),
            Some(&NSString::from_str(domain)),
        )
        .map(Kept)
    }
}

impl Prefs for Kept {
    fn get(&self, key: &str) -> Option<Value> {
        let object = self.0.objectForKey(&NSString::from_str(key))?;
        to_json(&object)
    }

    fn set(&mut self, key: &str, value: Value) {
        let key = NSString::from_str(key);
        match from_json(&value) {
            // SAFETY: a property-list object, as NSUserDefaults takes.
            Some(object) => unsafe { self.0.setObject_forKey(Some(&object), &key) },
            None => self.0.removeObjectForKey(&key),
        }
    }
}

/// Every allowlisted setting that is set, for a profile file (Search's `collectPrefs`):
/// what was never set isn't written, so another Mac keeps its own defaults for it.
pub fn for_profile() -> Map<String, Value> {
    allowlisted(&Kept::new())
}

/// The same, from another app's settings under the same keys: Search's, by its domain.
pub fn for_profile_of(domain: &str) -> Map<String, Value> {
    Kept::of(domain)
        .map(|kept| allowlisted(&kept))
        .unwrap_or_default()
}

fn allowlisted(kept: &Kept) -> Map<String, Value> {
    settings_index::profile_keys()
        .into_iter()
        .filter_map(|key| kept.get(key).map(|value| (key.to_owned(), value)))
        .collect()
}

/// A profile's settings taken in, allowlisted again on the way in: a file never sets
/// what it has no business setting. Answers how many were set.
pub fn from_profile(prefs: &Map<String, Value>) -> usize {
    let allowed = settings_index::profile_keys();
    let mut kept = Kept::new();
    let mut count = 0;
    for (key, value) in prefs {
        if value.is_null() || !allowed.contains(&key.as_str()) {
            continue;
        }
        kept.set(key, value.clone());
        count += 1;
    }
    count
}

/// The settings Tap acts on, put where they act: the engine and the site shortcuts in
/// the field, the look on the app. At launch, and whenever one changes.
pub fn apply(mtm: MainThreadMarker) {
    let kept = Kept::new();
    let engine =
        Engine::from_raw(&settings_index::text(&kept, "search.engine")).unwrap_or_default();
    let custom = settings_index::text(&kept, "search.custom");
    let keywords = keywords(&kept);
    let behaviour = behaviour(&kept);
    rt::dispatch(move |browser| {
        browser.engine = engine;
        browser.custom_engine = custom;
        browser.keywords = keywords;
        browser.behaviour = behaviour;
    });
    crate::web::apply_prefs(&kept);
    let look = settings_index::text(&kept, "look");
    // SAFETY: AppKit's own appearance names.
    let name = unsafe {
        match look.as_str() {
            "light" => Some(NSAppearanceNameAqua),
            "dark" => Some(NSAppearanceNameDarkAqua),
            _ => None,
        }
    };
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

/// How tabs open and close, from Settings › Tabs and › Address bar commands.
fn behaviour(kept: &Kept) -> Behaviour {
    let hours = settings_index::value(kept, "tabs.close.after")
        .as_f64()
        .unwrap_or(0.0);
    // `close.after` in seconds stands in for the chosen time on a test run, as Search's.
    let seconds = kept
        .get("close.after")
        .and_then(|v| v.as_f64())
        .filter(|s| *s > 0.0);
    Behaviour {
        new_place: NewPlace::from_raw(&settings_index::text(kept, "tabs.newPlace")),
        new_after_last: settings_index::flag(kept, "tabs.newAfterLast"),
        close_goes_back: settings_index::flag(kept, "tabs.closeGoesBack"),
        lazy: settings_index::flag(kept, "tabs.lazy"),
        commands: settings_index::flag(kept, "commandbar"),
        close_after: (hours > 0.0).then(|| seconds.unwrap_or(hours * 3600.0)),
    }
}

/// Settings › Tabs › Start with a fresh window, read once at launch.
pub fn starts_fresh() -> bool {
    settings_index::flag(&Kept::new(), "start.fresh")
}

/// The site shortcuts: Search keeps them as JSON in a `Data`.
pub fn keywords(prefs: &dyn Prefs) -> Vec<Keyword> {
    match prefs.get("search.keywords") {
        Some(Value::Object(data)) => data
            .get("$data")
            .and_then(Value::as_str)
            .and_then(decode_base64)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let data = NSData::initWithBase64EncodedString_options(
        NSData::alloc(),
        &NSString::from_str(text),
        NSDataBase64DecodingOptions::empty(),
    )?;
    Some(data.to_vec())
}

/// A property-list object as JSON, or `None` for one JSON can't carry. A script
/// message's `null` arrives as `NSNull`, which no property list holds: it is JSON's null.
pub(crate) fn to_json(object: &AnyObject) -> Option<Value> {
    if object.downcast_ref::<objc2_foundation::NSNull>().is_some() {
        return Some(Value::Null);
    }
    if let Some(number) = object.downcast_ref::<NSNumber>() {
        // SAFETY: NSNumber always answers with a valid C string.
        let kind = unsafe { std::ffi::CStr::from_ptr(number.objCType().as_ptr()) }.to_bytes();
        return Some(match kind {
            // A BOOL is a `char` to NSNumber; a Swift Bool arrives as one too.
            b"c" | b"B" => Value::Bool(number.boolValue()),
            b"f" | b"d" => {
                Number::from_f64(number.doubleValue()).map_or(Value::Null, Value::Number)
            }
            _ => Value::Number(number.longLongValue().into()),
        });
    }
    if let Some(text) = object.downcast_ref::<NSString>() {
        return Some(Value::String(text.to_string()));
    }
    if let Some(data) = object.downcast_ref::<NSData>() {
        let encoded = data.base64EncodedStringWithOptions(NSDataBase64EncodingOptions::empty());
        let mut marked = Map::new();
        marked.insert("$data".into(), Value::String(encoded.to_string()));
        return Some(Value::Object(marked));
    }
    if let Some(date) = object.downcast_ref::<NSDate>() {
        let mut marked = Map::new();
        marked.insert(
            "$date".into(),
            Number::from_f64(date.timeIntervalSince1970()).map_or(Value::Null, Value::Number),
        );
        return Some(Value::Object(marked));
    }
    if let Some(list) = object.downcast_ref::<NSArray>() {
        return list
            .iter()
            .map(|item| to_json(&item))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array);
    }
    if let Some(table) = object.downcast_ref::<NSDictionary>() {
        let mut out = Map::new();
        for key in table.allKeys() {
            let name = key.downcast_ref::<NSString>()?.to_string();
            let value = table.objectForKey(&key)?;
            out.insert(name, to_json(&value)?);
        }
        return Some(Value::Object(out));
    }
    None
}

fn any<T: Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an `AnyObject`; nothing more is claimed.
    unsafe { Retained::cast_unchecked(object) }
}

/// JSON as a property-list object; `None` for null, which means no value.
fn from_json(value: &Value) -> Option<Retained<AnyObject>> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(on) => any(NSNumber::new_bool(*on)),
        Value::Number(n) => any(match n.as_i64() {
            Some(whole) => NSNumber::new_i64(whole),
            None => NSNumber::new_f64(n.as_f64().unwrap_or(0.0)),
        }),
        Value::String(text) => any(NSString::from_str(text)),
        Value::Array(list) => {
            let items: Vec<Retained<AnyObject>> = list.iter().filter_map(from_json).collect();
            any(NSArray::from_retained_slice(&items))
        }
        Value::Object(table) => {
            if table.len() == 1 {
                if let Some(encoded) = table.get("$data").and_then(Value::as_str) {
                    let bytes = decode_base64(encoded)?;
                    return Some(any(NSData::with_bytes(&bytes)));
                }
                if let Some(stamp) = table.get("$date").and_then(Value::as_f64) {
                    return Some(any(NSDate::dateWithTimeIntervalSince1970(stamp)));
                }
            }
            let keys: Vec<Retained<NSString>> =
                table.keys().map(|k| NSString::from_str(k)).collect();
            let values: Vec<Retained<AnyObject>> = table.values().filter_map(from_json).collect();
            if keys.len() != values.len() {
                return None;
            }
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            any(NSDictionary::from_retained_objects(&key_refs, &values))
        }
    })
}
