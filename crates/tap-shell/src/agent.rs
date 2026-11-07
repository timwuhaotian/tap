//! Driving a page for an agent, in every frame (docs/PLAN.md §8 "Agents use Tap"):
//! `snapshot`, `act` and `text` see into cross-origin iframes the main frame can't
//! see into — an Apple sign-in lives entirely in one (`#aid-auth-widget-iFrame`).
//!
//! Each frame says hello at its document's start (`assets/js/agent-hello.js`, Tap's
//! own world, every frame), and the relay keeps its frame handle; a verb then runs
//! the agent script in the main frame and in every frame that said hello, and merges
//! the answers. Nothing crosses the page's own windows, so a page can't sniff or
//! forge another origin's controls the way a `postMessage` relay would let it.
//!
//! Not `agents.rs`: that installs the `tap` command and the skill; this drives pages.

use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_foundation::NSString;
use objc2_web_kit::{
    WKFrameInfo, WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
};
use serde_json::{Value, json};
use tap_core::browser::TabId;

use crate::{bench, prefs, web};

/// What introduces every frame: at each document's start, every frame, Tap's world.
pub const HELLO: &str = include_str!("../../../assets/js/agent-hello.js");

/// The agents' page script (docs/PLAN.md §8 "Agents use Tap"), as one expression.
/// Frame-local: it only ever sees its own frame, and each frame keeps its own refs.
const AGENT: &str = concat!(
    "(async () => {\n",
    include_str!("../../../assets/js/agent.js"),
    "\n})()"
);

/// A frame's words, as one expression.
const TEXT: &str = "document.body ? document.body.innerText : ''";

/// The name the hello script posts to.
const HANDLER: &str = "tapAgent";
/// Frames kept per tab; a page with more still answers through its first ones.
const MOST_FRAMES: usize = 32;
/// Controls listed in one snapshot, across all frames, as the script caps one frame.
const LIMIT: usize = 400;
/// How long a snapshot or text waits for slow frames before answering without them.
const PATIENCE: f64 = 2.0;

/// One subframe that said hello: its handle, its address, and its document's mark.
#[derive(Clone)]
struct Frame {
    handle: Retained<WKFrameInfo>,
    url: String,
    token: String,
}

/// Where a snapshot's control lives: the main frame, or one that said hello.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Route {
    Main(String),
    Frame {
        frame: usize,
        token: String,
        local: String,
    },
}

thread_local! {
    /// The subframes that said hello, per tab, oldest first.
    static FRAMES: RefCell<HashMap<TabId, Vec<Frame>>> = RefCell::new(HashMap::new());
    /// Where the last snapshot's controls live, per tab, in ref order.
    static ROUTES: RefCell<HashMap<TabId, Vec<Route>>> = RefCell::new(HashMap::new());
}

define_class!(
    /// Hears the hello script: one for every page, as Search's relays are.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TapAgentRelay"]
    struct Relay;

    unsafe impl NSObjectProtocol for Relay {}

    unsafe impl WKScriptMessageHandler for Relay {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            received(message);
        }
    }
);

impl Relay {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Puts the relay on the controller every page shares, in Tap's world: the hello
/// script stands there, not in the page's. Once, as the configuration is made:
/// WebKit refuses a second handler of the same name.
pub fn install(mtm: MainThreadMarker, controller: &WKUserContentController) {
    let relay = Relay::new(mtm);
    // SAFETY: the controller keeps the relay for as long as it lives.
    unsafe {
        controller.addScriptMessageHandler_contentWorld_name(
            ProtocolObject::from_ref(&*relay),
            &web::world(mtm),
            &NSString::from_str(HANDLER),
        );
    }
}

fn received(message: &WKScriptMessage) {
    // SAFETY: plain property reads on a message WebKit hands over for the call's length.
    let (body, web) = unsafe { (message.body(), message.webView()) };
    let Some(web) = web else { return };
    let Some(tab) = web::tab_of(&web) else { return };
    let Some(body) = prefs::to_json(&body) else {
        return;
    };
    if body.get("kind").and_then(Value::as_str) != Some("hello") {
        return;
    }
    // SAFETY: WebKit hands the sender's frame for the call's length; retained below.
    let frame = unsafe { message.frameInfo() };
    // SAFETY: a plain property read on a live frame.
    let main = unsafe { frame.isMainFrame() };
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    if main {
        // A new document: its frames will say hello in turn, and the last
        // snapshot's routes point at the last document's frames.
        FRAMES.with_borrow_mut(|all| all.insert(tab, Vec::new()));
        ROUTES.with_borrow_mut(|all| all.remove(&tab));
    } else {
        FRAMES.with_borrow_mut(|all| {
            let frames = all.entry(tab).or_default();
            if frames.len() < MOST_FRAMES {
                frames.push(Frame {
                    handle: frame,
                    url: text("url"),
                    token: text("token"),
                });
            }
        });
    }
}

/// The tab's page went: its frames and routes go with it.
pub fn forget(tab: TabId) {
    FRAMES.with_borrow_mut(|all| all.remove(&tab));
    ROUTES.with_borrow_mut(|all| all.remove(&tab));
}

/// A snapshot coming in: the main answer, one slot per frame asked, and replies left.
struct Gathering {
    reply: Option<bench::Reply>,
    main: Option<Value>,
    subs: Vec<Option<Value>>,
    left: usize,
}

impl Gathering {
    fn new(frames: usize, reply: bench::Reply) -> Self {
        Self {
            reply: Some(reply),
            main: None,
            subs: vec![None; frames],
            left: frames + 1,
        }
    }
}

/// The page's controls, from every frame that answered: the main frame's first,
/// then each subframe's in the order it said hello, renumbered `e1` up.
pub fn snapshot(id: TabId, web: &WKWebView, reply: bench::Reply) {
    let frames = FRAMES.with_borrow(|all| all.get(&id).cloned().unwrap_or_default());
    let shared = Arc::new(Mutex::new(Gathering::new(frames.len(), reply)));
    {
        let shared = Arc::clone(&shared);
        web::call_in_frame(
            web,
            None,
            AGENT,
            &[("verb", "snapshot"), ("ref", ""), ("value", "")],
            move |answer| land(id, &Arc::clone(&shared), None, answer),
        );
    }
    for (index, frame) in frames.iter().enumerate() {
        let shared = Arc::clone(&shared);
        web::call_in_frame(
            web,
            Some(&frame.handle),
            AGENT,
            &[("verb", "snapshot"), ("ref", ""), ("value", "")],
            move |answer| land(id, &Arc::clone(&shared), Some(index), answer),
        );
    }
    bench::later(PATIENCE, move || finish(id, &shared));
}

/// One frame's snapshot answer landed; the last one answers.
fn land(id: TabId, shared: &Arc<Mutex<Gathering>>, slot: Option<usize>, answer: Option<Value>) {
    let last = {
        let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        if shared.reply.is_none() {
            return;
        }
        match slot {
            None => shared.main = answer,
            Some(index) => {
                if let Some(slot) = shared.subs.get_mut(index) {
                    *slot = answer;
                }
            }
        }
        shared.left = shared.left.saturating_sub(1);
        shared.left == 0
    };
    if last {
        finish(id, shared);
    }
}

/// Merges what came in: stale frames (gone, or wearing an older document's mark)
/// are dropped, and what is left is renumbered into one list with its routes.
fn finish(id: TabId, shared: &Arc<Mutex<Gathering>>) {
    let (Some(reply), main, subs) = ({
        let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        (
            shared.reply.take(),
            shared.main.take(),
            std::mem::take(&mut shared.subs),
        )
    }) else {
        return;
    };
    // Frames that never answered, or answered for another document (a frame that
    // said hello twice answers twice; only the latest mark counts), go: indices
    // below a dropped one move down, so the dropped go from the top down.
    let mut kept: Vec<(usize, String, &Value)> = Vec::new();
    FRAMES.with_borrow_mut(|all| {
        let frames = all.get_mut(&id);
        let mut dropped = Vec::new();
        for (index, sub) in subs.iter().enumerate() {
            let entry = frames.as_ref().and_then(|f| f.get(index));
            let token = sub
                .as_ref()
                .and_then(|v| v.get("token"))
                .and_then(Value::as_str);
            match (entry, token, sub.as_ref()) {
                (Some(entry), Some(token), Some(answer)) if token == entry.token => {
                    kept.push((index, entry.token.clone(), answer));
                }
                _ => dropped.push(index),
            }
        }
        if let Some(frames) = frames {
            for index in dropped.into_iter().rev() {
                if index < frames.len() {
                    frames.remove(index);
                }
            }
        }
    });
    // Routes point at where each kept frame stands after the dropped went.
    let mut at: HashMap<usize, usize> = HashMap::new();
    for (next, (index, _, _)) in kept.iter().enumerate() {
        at.insert(*index, next);
    }
    let subs: Vec<(usize, &str, &Value)> = kept
        .iter()
        .map(|(index, token, sub)| (*at.get(index).unwrap_or(&0), token.as_str(), *sub))
        .collect();
    let (answer, routes) = merge(main.as_ref(), &subs);
    ROUTES.with_borrow_mut(|all| all.insert(id, routes));
    reply.say(answer);
}

/// One snapshot's answers as the bench answers them, with where each control lives.
/// `subs` carries each kept subframe's index (after stale frames went), its document's
/// mark, and its answer.
fn merge(main: Option<&Value>, subs: &[(usize, &str, &Value)]) -> (Value, Vec<Route>) {
    let mut controls = Vec::new();
    let mut routes = Vec::new();
    let mut more = 0;
    let mut take = |answer: &Value, frame: Option<(usize, &str)>| {
        let list = answer
            .get("controls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // A frame's overflow count: small, and only added up for the card.
        #[allow(clippy::cast_possible_truncation)]
        let more_here = answer.get("more").and_then(Value::as_u64).unwrap_or(0) as usize;
        more += more_here;
        for mut control in list {
            if controls.len() >= LIMIT {
                more += 1;
                continue;
            }
            let Some(local) = control
                .get("ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            control["ref"] = json!(format!("e{}", controls.len() + 1));
            controls.push(control);
            routes.push(match frame {
                None => Route::Main(local),
                Some((frame, token)) => Route::Frame {
                    frame,
                    token: token.to_owned(),
                    local,
                },
            });
        }
    };
    if let Some(main) = main {
        take(main, None);
    }
    for (frame, token, sub) in subs {
        take(sub, Some((*frame, token)));
    }
    let text = |key: &str| {
        main.and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    (
        json!({
            "url": text("url"),
            "title": text("title"),
            "controls": controls,
            "more": more,
        }),
        routes,
    )
}

/// A `snapshot` ref as an index into its routes, as the script reads one.
fn ref_index(target: &str) -> Option<usize> {
    let number = target
        .strip_prefix('@')
        .unwrap_or(target)
        .strip_prefix('e')?;
    if number.is_empty() {
        return None;
    }
    number.parse::<usize>().ok()?.checked_sub(1)
}

fn stale(target: &str) -> Value {
    json!({ "error": format!("no control {target} on this page now: take a snapshot again") })
}

/// Clicks, fills or presses a control from the last snapshot, in whichever frame
/// holds it.
pub fn act(
    id: TabId,
    web: &WKWebView,
    action: &str,
    target: &str,
    value: &str,
    reply: bench::Reply,
) {
    // A key with no ref goes where the main frame's focus is, as a key press does.
    if target.is_empty() {
        return run(web, None, action, "", value, reply);
    }
    let route = ref_index(target).and_then(|index| {
        ROUTES.with_borrow(|all| all.get(&id).and_then(|routes| routes.get(index).cloned()))
    });
    let Some(route) = route else {
        return reply.say(stale(target));
    };
    match route {
        // The main frame's own numbering is the joined list's head: already right.
        Route::Main(local) => run(web, None, action, &local, value, reply),
        Route::Frame {
            frame,
            token,
            local,
        } => {
            let handle = FRAMES.with_borrow(|all| {
                all.get(&id)
                    .and_then(|frames| frames.get(frame))
                    .filter(|entry| entry.token == token)
                    .map(|entry| entry.handle.clone())
            });
            let Some(handle) = handle else {
                return reply.say(stale(target));
            };
            let target = target.to_owned();
            let renamed = local.clone();
            let reply = RefCell::new(Some(reply));
            web::call_in_frame(
                web,
                Some(&handle),
                AGENT,
                &[("verb", action), ("ref", &local), ("value", value)],
                move |answer| {
                    let Some(reply) = reply.borrow_mut().take() else {
                        return;
                    };
                    if let Some(mut answer) = answer {
                        // The frame answers with its own numbering; the agent asked
                        // with the joined list's.
                        for key in ["clicked", "filled", "on"] {
                            if let Some(control) = answer.get_mut(key) {
                                control["ref"] = json!(target);
                            }
                        }
                        if let Some(error) = answer
                            .get("error")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        {
                            if error.starts_with("no control") {
                                answer = stale(&target);
                            } else if let Some(rest) = error.strip_prefix(&renamed) {
                                answer["error"] = json!(format!("{target}{rest}"));
                            }
                        }
                        reply.say(answer);
                    } else {
                        // The frame went mid-act: it goes, and the ref with it.
                        FRAMES.with_borrow_mut(|all| {
                            if let Some(frames) = all.get_mut(&id) {
                                frames.retain(|entry| entry.token != token);
                            }
                        });
                        reply.say(stale(&target));
                    }
                },
            );
        }
    }
}

/// One verb in one frame: the main frame when `frame` is none.
fn run(
    web: &WKWebView,
    frame: Option<&WKFrameInfo>,
    verb: &str,
    target: &str,
    value: &str,
    reply: bench::Reply,
) {
    let reply = RefCell::new(Some(reply));
    web::call_in_frame(
        web,
        frame,
        AGENT,
        &[("verb", verb), ("ref", target), ("value", value)],
        move |answer| {
            if let Some(reply) = reply.borrow_mut().take() {
                reply.say(answer.unwrap_or_else(|| json!({ "error": "the page gave no answer" })));
            }
        },
    );
}

/// The page's words coming in, with each asked frame's address for the join.
struct Reading {
    reply: Option<bench::Reply>,
    main: Option<String>,
    subs: Vec<Option<String>>,
    urls: Vec<String>,
    left: usize,
}

/// The page's words: the main frame's, then each subframe's below its address.
/// A sign-in's "wrong password" lives in its frame, not in the main page.
pub fn text(id: TabId, web: &WKWebView, reply: bench::Reply) {
    let frames = FRAMES.with_borrow(|all| all.get(&id).cloned().unwrap_or_default());
    let shared = Arc::new(Mutex::new(Reading {
        reply: Some(reply),
        main: None,
        subs: vec![None; frames.len()],
        urls: frames.iter().map(|f| f.url.clone()).collect(),
        left: frames.len() + 1,
    }));
    {
        let shared = Arc::clone(&shared);
        web::call_in_frame(web, None, TEXT, &[], move |answer| {
            land_text(&Arc::clone(&shared), None, answer);
        });
    }
    for (index, frame) in frames.iter().enumerate() {
        let shared = Arc::clone(&shared);
        web::call_in_frame(web, Some(&frame.handle), TEXT, &[], move |answer| {
            land_text(&Arc::clone(&shared), Some(index), answer);
        });
    }
    bench::later(PATIENCE, move || finish_text(&shared));
}

/// One frame's words landed; the last one answers.
fn land_text(shared: &Arc<Mutex<Reading>>, slot: Option<usize>, answer: Option<Value>) {
    let last = {
        let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        if shared.reply.is_none() {
            return;
        }
        let words = answer
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|w| !w.trim().is_empty());
        match slot {
            None => shared.main = words,
            Some(index) => {
                if let Some(slot) = shared.subs.get_mut(index) {
                    *slot = words;
                }
            }
        }
        shared.left = shared.left.saturating_sub(1);
        shared.left == 0
    };
    if last {
        finish_text(shared);
    }
}

/// Joins what came in: the main frame's words, then each answering subframe's
/// below its address.
fn finish_text(shared: &Arc<Mutex<Reading>>) {
    let (Some(reply), main, subs, urls) = ({
        let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        (
            shared.reply.take(),
            shared.main.take(),
            std::mem::take(&mut shared.subs),
            std::mem::take(&mut shared.urls),
        )
    }) else {
        return;
    };
    let mut parts = vec![main.unwrap_or_default()];
    for (words, url) in subs.into_iter().zip(urls) {
        if let Some(words) = words {
            parts.push(format!("[frame {url}]\n{words}"));
        }
    }
    reply.say(json!({ "value": parts.join("\n\n") }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(reference: &str, name: &str) -> Value {
        json!({ "ref": reference, "role": "button", "name": name })
    }

    #[test]
    fn a_snapshot_merges_every_frame_s_controls_in_order() {
        let main = json!({
            "url": "https://shop.example/", "title": "Shop",
            "controls": [control("e1", "Top")], "more": 0,
        });
        let login = json!({
            "url": "https://id.example/", "title": "Sign in",
            "controls": [control("e1", "Email"), control("e2", "Go")], "more": 0,
        });
        let (answer, routes) = merge(Some(&main), &[(0, "tok", &login)]);
        assert_eq!(
            answer["controls"],
            json!([
                { "ref": "e1", "role": "button", "name": "Top" },
                { "ref": "e2", "role": "button", "name": "Email" },
                { "ref": "e3", "role": "button", "name": "Go" },
            ])
        );
        assert_eq!(answer["url"], json!("https://shop.example/"));
        assert_eq!(answer["more"], json!(0));
        assert_eq!(
            routes,
            vec![
                Route::Main("e1".to_owned()),
                Route::Frame {
                    frame: 0,
                    token: "tok".to_owned(),
                    local: "e1".to_owned()
                },
                Route::Frame {
                    frame: 0,
                    token: "tok".to_owned(),
                    local: "e2".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_snapshot_lists_four_hundred_controls_at_most() {
        let many: Vec<Value> = (1..=405)
            .map(|n| control(&format!("e{n}"), "Bulk"))
            .collect();
        let main = json!({ "url": "", "title": "", "controls": many, "more": 0 });
        let (answer, routes) = merge(Some(&main), &[]);
        assert_eq!(answer["controls"].as_array().map(Vec::len), Some(400));
        assert_eq!(answer["more"], json!(5));
        assert_eq!(routes.len(), 400);
        assert_eq!(
            answer["controls"][399]["ref"],
            json!("e400"),
            "renumbered across the join"
        );
    }

    #[test]
    fn refs_read_as_the_script_reads_them() {
        assert_eq!(ref_index("e1"), Some(0));
        assert_eq!(ref_index("@e3"), Some(2));
        assert_eq!(ref_index("e400"), Some(399));
        assert_eq!(ref_index(""), None);
        assert_eq!(ref_index("e"), None);
        assert_eq!(ref_index("e0"), None);
        assert_eq!(ref_index("x"), None);
        assert_eq!(ref_index("e12x"), None);
    }
}
