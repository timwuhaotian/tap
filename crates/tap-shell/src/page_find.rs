//! The page's side of find (Search's `PageFind.swift`): the script in
//! `assets/js/page-find.js`, run in Tap's own content world with the word as an argument,
//! never pasted into the source. A PDF has no text the script can read; WebKit's own find
//! looks in it instead, and says only found or not.

use block2::RcBlock;
use objc2::{MainThreadMarker, MainThreadOnly, Message, runtime::AnyObject};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString};
use objc2_web_kit::{WKFindConfiguration, WKFindResult, WKWebView};
use serde_json::Value;
use tap_core::{
    browser::TabId,
    find::{Answer, Ask},
};

use crate::{rt, web};

/// The script, verbatim from Search but for its names.
const SCRIPT: &str = include_str!("../../../assets/js/page-find.js");
/// Matches measured at a time; Safari stops counting at the same number.
const LIMIT: i64 = 1000;

/// Asks the tab's page; its answer goes back to the model.
pub fn ask(ask: &Ask) {
    let Some(web) = web::existing(ask.tab) else {
        return;
    };
    let question = ask.clone();
    run(
        &web,
        &ask.query,
        ask.match_case,
        ask.whole_words,
        ask.steps,
        ask.generation,
        move |value, web| {
            let question = question.clone();
            let answer = match value
                .as_ref()
                .and_then(|v| v.get("status"))
                .and_then(Value::as_str)
            {
                Some("ok") => {
                    let number = |key: &str| {
                        value
                            .as_ref()
                            .and_then(|v| v.get(key))
                            .and_then(Value::as_u64)
                    };
                    let count = number("count").and_then(|n| u32::try_from(n).ok());
                    let index = number("index")
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|i| *i > 0);
                    Answer {
                        count,
                        index,
                        more: value
                            .as_ref()
                            .and_then(|v| v.get("more"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        found: count.unwrap_or(0) > 0,
                        native: false,
                        available: true,
                    }
                }
                // A PDF: WebKit's own find, unless whole words were asked for, which it can't.
                Some("pdf") if !question.whole_words => {
                    native(web, &question);
                    return;
                }
                Some("stale") => return,
                _ => Answer::default(),
            };
            rt::dispatch(move |browser| browser.find_answered(&question, answer));
        },
    );
}

/// Takes the highlights off the tab's page and gives back its own selection.
pub fn clear(tab: TabId, generation: u64) {
    let Some(web) = web::existing(tab) else {
        return;
    };
    run(&web, "", false, false, 0, generation, |_, _| {});
}

fn run(
    web: &WKWebView,
    query: &str,
    match_case: bool,
    whole_words: bool,
    steps: i32,
    generation: u64,
    then: impl Fn(Option<Value>, &WKWebView) + 'static,
) {
    // The script's own answer is an object; turned into JSON on the way out, it is read
    // here without walking Objective-C dictionaries.
    let body =
        format!("const run = async () => {{\n{SCRIPT}\n}};\nreturn JSON.stringify(await run());");
    let keys = [
        "query",
        "matchCase",
        "wholeWords",
        "steps",
        "limit",
        "generation",
    ]
    .map(NSString::from_str);
    let text = NSString::from_str(query);
    let numbers = [
        NSNumber::new_bool(match_case),
        NSNumber::new_bool(whole_words),
        NSNumber::new_i32(steps),
        NSNumber::new_i64(LIMIT),
        NSNumber::new_u64(generation),
    ];
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let mut value_refs: Vec<&AnyObject> = vec![text.as_ref()];
    value_refs.extend(numbers.iter().map(|n| -> &AnyObject { n.as_ref() }));
    let arguments = NSDictionary::from_slices(&key_refs, &value_refs);
    let held = web.retain();
    let block = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
        // SAFETY: WebKit hands a valid object or null, for the call's length.
        let value = unsafe { value.as_ref() };
        let text = value
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(ToString::to_string);
        then(text.and_then(|t| serde_json::from_str(&t).ok()), &held);
    });
    // SAFETY: a live web view, a script body, arguments of the types it reads, and a
    // block that outlives the call.
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&body),
            Some(&arguments),
            None,
            &web::world(MainThreadMarker::from(web)),
            Some(&block),
        );
    }
}

/// WebKit's own find, for a PDF: found or not, round from the end, no count.
fn native(web: &WKWebView, ask: &Ask) {
    let mtm = MainThreadMarker::from(web);
    // SAFETY: a fresh configuration with plain setters.
    let configuration = unsafe {
        let configuration = WKFindConfiguration::init(WKFindConfiguration::alloc(mtm));
        configuration.setBackwards(ask.steps < 0);
        configuration.setCaseSensitive(ask.match_case);
        configuration.setWraps(true);
        configuration
    };
    let question = ask.clone();
    let block = RcBlock::new(move |result: std::ptr::NonNull<WKFindResult>| {
        // SAFETY: WebKit hands a valid result for the call's length.
        let found = unsafe { result.as_ref().matchFound() };
        let answer = Answer {
            found,
            native: true,
            available: true,
            ..Answer::default()
        };
        let question = question.clone();
        rt::dispatch(move |browser| browser.find_answered(&question, answer));
    });
    // SAFETY: a live web view, the word, a configuration and a block outliving the call.
    unsafe {
        web.findString_withConfiguration_completionHandler(
            &NSString::from_str(&ask.query),
            Some(&configuration),
            &block,
        );
    }
}
