//! The bench: a way for a script on this Mac to drive Tap, in Search's protocol
//! (Search's `Bench.swift`, docs/PLAN.md §9).
//!
//! A Unix socket in the store folder, readable by this user only, and the other end is
//! checked for the same uid before a word is read. One JSON object per line in, one per
//! line out, one request per connection, and always exactly one answer: a request that
//! nothing answers is answered with an error after its patience runs out.
//!
//! Listening on a test run (`TAP_PROBE`) always: such a world holds nothing of yours,
//! and the test worlds are set up by scripts that expect the socket there. Off a test
//! run, only while Settings' `bench` switch is on. Search also demands a keychain
//! consent mark before it honours that switch, so a defaults write alone can't open
//! it; Tap has no Security-framework link to keep such a mark with, so here the
//! switch alone gates it and the same-uid check below is the guard.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use block2::RcBlock;
use objc2::{
    MainThreadMarker,
    rc::Retained,
    runtime::{AnyObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSTextInputClient, NSTextView,
};
use objc2_foundation::{NSError, NSNotFound, NSPoint, NSRange, NSString};
use serde_json::{Value, json};
use tap_core::{
    address,
    browser::{Browser, Tab, TabId},
    identity,
};

use crate::{
    address_bar, bring, dialogs, downloads, links,
    menu::{self, Command},
    profile, rt, settings, sleep, store, update, web, window,
};

/// The longest request line read: a line that never ends is not a request.
const LONGEST: usize = 4_000_000;

/// The listening thread's stop flag, while there is one. Kept so the Settings switch
/// can close the socket again; a blocking `accept` couldn't be woken to notice.
static CONTROL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Whether a run should listen: a test world always (its scripts set the world up and
/// drive it through the socket, whatever its copied-in settings say), a real one only
/// while the switch is on.
fn should_listen(test_run: bool, switched_on: bool) -> bool {
    test_run || switched_on
}

fn test_run() -> bool {
    identity::world(std::env::var(identity::PROBE_ENV).ok().as_deref()).is_some()
}

/// At launch, and whenever the switch moves (Settings, or a `settings` request here):
/// listening follows the switch, and stops following it the moment it goes off.
pub fn refresh() {
    let on = should_listen(
        test_run(),
        tap_core::settings_index::flag(&crate::prefs::Kept::new(), "bench"),
    );
    if on {
        listen();
    } else {
        stop();
    }
}

/// Starts listening, if this run should. Called once at launch; the switch moving is
/// [`refresh`]'s business.
pub fn start() {
    refresh();
}

/// Closes the socket and takes its file away, so a script can see the bench is off
/// rather than knock on a door nobody answers.
pub fn stop() {
    let flag = CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(flag) = flag {
        flag.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_file(store::file("bench.sock"));
    }
}

/// Whether something is listening now, for the bench's own `settings` answer.
pub fn running() -> bool {
    CONTROL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|flag| !flag.load(Ordering::Relaxed))
}

fn listen() {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut control = CONTROL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if control
            .as_ref()
            .is_some_and(|flag| !flag.load(Ordering::Relaxed))
        {
            return;
        }
        *control = Some(stop.clone());
    }
    let path = store::file("bench.sock");
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!("bench: can't listen at {}", path.display());
        stop.store(true, Ordering::Relaxed);
        return;
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    if listener.set_nonblocking(true).is_err() {
        stop.store(true, Ordering::Relaxed);
        return;
    }
    let mine = stop.clone();
    let thread_stop = stop.clone();
    let spawned = thread::Builder::new()
        .name("tap.bench".into())
        .spawn(move || {
            // Polled, not blocking: `stop` has to be able to end this between knocks.
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = thread::Builder::new()
                            .name("tap.bench.client".into())
                            .spawn(move || serve(stream));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(25)),
                }
            }
            let mut control = CONTROL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if control
                .as_ref()
                .is_some_and(|flag| Arc::ptr_eq(flag, &mine))
            {
                *control = None;
            }
        });
    if spawned.is_err() {
        stop.store(true, Ordering::Relaxed);
    }
}

/// Whether the other end is this user.
fn same_user(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: a connected socket's descriptor and two out-parameters of the right type.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) } == 0;
    // SAFETY: `getuid` has no preconditions.
    ok && uid == unsafe { libc::getuid() }
}

fn serve(stream: UnixStream) {
    if !same_user(&stream) {
        return;
    }
    // The listener is nonblocking so `stop` can end it; an accepted socket inherits
    // that on macOS, and a nonblocking `write_all` then drops everything past the
    // socket buffer instead of waiting (a `settings` answer is bigger than it is).
    let _ = stream.set_nonblocking(false);
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(LONGEST as u64 + 1)
        .read_until(b'\n', &mut line);
    let answer = match read {
        Ok(_) if line.len() > LONGEST => json!({ "error": "request too long" }),
        Ok(_) => match serde_json::from_slice::<Value>(&line) {
            Ok(request @ Value::Object(_)) => ask_main(request),
            _ => json!({ "error": "not a JSON object" }),
        },
        Err(_) => return,
    };
    let mut out = serde_json::to_vec(&answer)
        .unwrap_or_else(|_| b"{\"error\":\"unwritable answer\"}".to_vec());
    out.push(b'\n');
    let mut stream = stream;
    let _ = stream.write_all(&out);
}

/// Hands the request to the main thread and waits for its one answer.
fn ask_main(request: Value) -> Value {
    let verb = request
        .get("do")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let patience = match verb.as_str() {
        "wait" => {
            request
                .get("seconds")
                .and_then(Value::as_f64)
                .unwrap_or(30.0)
                + 5.0
        }
        _ => 25.0,
    };
    let (send, receive) = mpsc::channel::<Value>();
    dispatch2::DispatchQueue::main().exec_async(move || handle(&request, Reply(Some(send))));
    receive
        .recv_timeout(Duration::from_secs_f64(patience))
        .unwrap_or_else(|_| json!({ "error": format!("no answer within {} s", patience.round()) }))
}

/// The one answer to a request. Dropped without one, it answers with an error, so a
/// script never waits on a verb that forgot to reply.
struct Reply(Option<mpsc::Sender<Value>>);

impl Reply {
    fn say(mut self, answer: Value) {
        if let Some(send) = self.0.take() {
            let _ = send.send(answer);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(send) = self.0.take() {
            let _ = send.send(json!({ "error": "the verb gave no answer" }));
        }
    }
}

fn describe(browser: &Browser, tab: &Tab) -> Value {
    json!({
        "id": tab.id.0.to_string(),
        "url": tab.url(),
        "title": tab.title(),
        "label": tab.label(),
        "loading": tab.loading(),
        "progress": tab.progress(),
        "awake": tab.awake(),
        "failure": tab.failure(),
        "active": browser.active() == Some(tab.id),
        "back": tab.can_back(),
        "forward": tab.can_forward(),
        "group": tab.group().and_then(|id| browser.groups().iter().find(|g| g.id == id)).map(|g| g.name.clone()),
        "page": tab.page().map(tap_core::internal::Page::title),
    })
}

/// The row as drawn: group headings among the tabs, a folded group's tabs left out.
fn row(browser: &Browser) -> Value {
    let items: Vec<Value> = browser
        .row()
        .iter()
        .map(|item| match item {
            tap_core::groups::Item::Heading(place) => {
                let group = &browser.groups()[*place];
                json!({ "heading": group.name, "folded": group.collapsed, "colour": group.colour })
            }
            tap_core::groups::Item::Tab(index) => {
                json!({ "tab": browser.tabs()[*index].label() })
            }
        })
        .collect();
    json!({
        "groups": browser.groups().iter().map(|g| json!({ "name": g.name, "folded": g.collapsed, "colour": g.colour })).collect::<Vec<_>>(),
        "row": items,
    })
}

fn describe_id(id: TabId) -> Value {
    rt::read(|browser| browser.tab(id).map(|tab| describe(browser, tab)))
        .flatten()
        .unwrap_or_else(|| json!({ "error": "no such tab" }))
}

fn tab_id(request: &Value) -> Option<TabId> {
    let id = request.get("id")?;
    let number = id
        .as_u64()
        .or_else(|| id.as_str().and_then(|s| s.parse().ok()))?;
    rt::read(|browser| browser.tab(TabId(number)).map(|t| t.id)).flatten()
}

#[allow(clippy::too_many_lines)] // the verbs, one arm each, as Search's switch has them
fn handle(request: &Value, reply: Reply) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let verb = request.get("do").and_then(Value::as_str).unwrap_or("");
    // A value given as a number (`key=9`) is still text to a verb that wants text.
    let text = |key: &str| {
        request.get(key).and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_number().map(ToString::to_string))
        })
    };
    match verb {
        "probe" => {
            let mut out = rt::read(|browser| {
                json!({
                    "field": browser.editing().is_some(),
                    "launching": browser.editing().is_some_and(|e| e.launching),
                    "typed": browser.editing().map(|e| e.typed.clone()),
                    "tabs": browser.tabs().len(),
                    "active": browser.active().map(|id| id.0.to_string()),
                    "refusals": browser.refusals(),
                    "offers": browser.editing().map(|e| e.offers.iter().map(|o| o.key.clone()).collect::<Vec<_>>()),
                    "ending": browser.editing().and_then(|e| e.ending.clone()),
                    "picked": browser.editing().and_then(|e| e.picked),
                })
            })
            .unwrap_or_else(|| json!({}));
            out["lights"] = json!(window::lights());
            out["addressBar"] = address_bar::describe();
            out["window"] = json!(window::number());
            out["selected"] = json!(rt::read(Browser::selection_count));
            out["fieldText"] = json!(window::field_text());
            out["firstResponder"] = json!(window::first_responder());
            out["asked"] = json!(dialogs::asked());
            out["find"] = rt::read(|b| {
                let find = b.find();
                json!({ "open": find.open(), "needle": find.needle(), "status": find.status(), "missed": find.missed() })
            })
            .unwrap_or_default();
            reply.say(out);
        }
        "tabs" | "state" => {
            let out = rt::read(|browser| {
                json!({
                    "tabs": browser.tabs().iter().map(|t| describe(browser, t)).collect::<Vec<_>>(),
                    "active": browser.active().map(|id| id.0.to_string()),
                })
            });
            reply.say(out.unwrap_or_else(|| json!({ "error": "no browser" })));
        }
        "row" => reply.say(rt::read(row).unwrap_or_default()),
        "fold" => {
            // A click on a group's heading, by its place among the groups.
            let Some(place) = request.get("group").and_then(Value::as_u64) else {
                return reply.say(json!({ "error": "fold needs a group number" }));
            };
            rt::dispatch(move |browser| {
                browser.toggle_group(usize::try_from(place).unwrap_or(usize::MAX));
            });
            reply.say(rt::read(row).unwrap_or_default());
        }
        "move" => {
            // A tab carried to a place among its block's (by its place among the tabs),
            // or a group past others (`group`, `to`), as the hand would in the strip.
            let to = request
                .get("to")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok());
            let Some(to) = to else {
                return reply.say(json!({ "error": "move needs to" }));
            };
            if let Some(group) = request
                .get("group")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
            {
                rt::dispatch(move |browser| browser.move_group(group, to));
            } else if let Some(id) = request
                .get("tab")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .and_then(|index| rt::read(|b| b.tabs().get(index).map(|t| t.id)).flatten())
            {
                rt::dispatch(move |browser| browser.move_tab(id, to));
            } else {
                return reply.say(json!({ "error": "move needs a tab or a group" }));
            }
            reply.say(rt::read(row).unwrap_or_default());
        }
        "group" => {
            // The group menus' commands: `new` (tab), `into` (tab, group), `out` (tab),
            // `rename` (group, name), `colour` (group, name), `ungroup` (group),
            // `close` (group), `reopen`.
            let op = text("op").unwrap_or_default();
            let number = |key: &str| {
                request
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|v| usize::try_from(v).ok())
            };
            let tab = number("tab")
                .and_then(|index| rt::read(|b| b.tabs().get(index).map(|t| t.id)).flatten());
            let group = number("group");
            let name = text("name").unwrap_or_default();
            match (op.as_str(), tab, group) {
                ("new", Some(id), _) => rt::dispatch(move |b| {
                    b.add_group(id);
                }),
                ("into", Some(id), Some(place)) => {
                    rt::dispatch(move |b| b.move_to_group(id, Some(place)));
                }
                ("out", Some(id), _) => rt::dispatch(move |b| b.move_to_group(id, None)),
                ("rename", _, Some(place)) => rt::dispatch(move |b| b.rename_group(place, &name)),
                ("ungroup", _, Some(place)) => rt::dispatch(move |b| b.ungroup(place)),
                ("colour", _, Some(place)) => {
                    rt::dispatch(move |b| b.set_group_colour(place, &name));
                }
                ("close", _, Some(place)) => rt::dispatch(move |b| b.close_group(place)),
                ("reopen", _, _) => rt::dispatch(Browser::reopen),
                _ => return reply.say(json!({ "error": "group needs op and its tab or group" })),
            }
            reply.say(rt::read(row).unwrap_or_default());
        }
        "export-profile" => {
            // File › Export Tap Profile… without its panel: straight to the path given.
            let Some(path) = text("path") else {
                return reply.say(json!({ "error": "export-profile needs a path" }));
            };
            match profile::write(std::path::Path::new(&path)) {
                Ok(written) => reply.say(json!({
                    "path": path, "tabs": written.tabs, "groups": written.groups, "settings": written.settings,
                })),
                Err(why) => reply.say(json!({ "error": why })),
            }
        }
        "import-profile" => {
            // File › Import Profile… without its panel, or a file opened from the Finder.
            let Some(path) = text("path") else {
                return reply.say(json!({ "error": "import-profile needs a path" }));
            };
            match profile::take(std::path::Path::new(&path)) {
                Ok(taken) => reply.say(json!({
                    "said": taken.said, "tabs": taken.tabs, "groups": taken.groups, "places": taken.places,
                    "bookmarks": taken.bookmarks, "settings": taken.settings,
                })),
                Err(why) => reply.say(json!({ "error": why })),
            }
        }
        "bring" => {
            // Bring things over: with `from`, that browser brought in at once, unasked
            // (`passwords`, `bookmarks`, `history`, `tabs`, `settings` false to leave one
            // out); with `sheet`, Search's sheet opened, or `sheet: "Chrome"` its Bring
            // them in pressed for that browser, or `sheet: false` closed; without either,
            // the browsers found and the sheet as it shows.
            if let Some(sheet) = request.get("sheet") {
                match sheet {
                    Value::Bool(true) => bring::open(),
                    Value::Bool(false) => bring::bench_close(),
                    Value::String(name) => bring::bench_bring(name),
                    _ => {}
                }
                return reply.say(json!({ "sheet": bring::snapshot() }));
            }
            let Some(from) = text("from") else {
                let found: Vec<&str> = bring::found()
                    .into_iter()
                    .map(tap_import::Source::name)
                    .collect();
                return reply.say(json!({ "found": found, "sheet": bring::snapshot() }));
            };
            let wanted = |key: &str| request.get(key).and_then(Value::as_bool) != Some(false);
            let wants = bring::Wants {
                passwords: wanted("passwords"),
                bookmarks: wanted("bookmarks"),
                history: wanted("history"),
                tabs: wanted("tabs"),
                settings: wanted("settings"),
            };
            match bring::take(&from, wants) {
                Ok(said) => reply.say(json!({
                    "said": said.iter().map(|(ok, text)| json!({"ok": ok, "text": text})).collect::<Vec<_>>(),
                })),
                Err(why) => reply.say(json!({ "error": why })),
            }
        }
        "signin" => {
            // Sign-ins: the accounts hanging under a box and the offer to keep one; with
            // `pick`, a row of the list clicked; with `answer` ("save", "never", "later"),
            // the offer's button pressed; with `kept`, what Tap's drawer (and Search's,
            // when on) holds for that host, without secrets; with `seed`, a made-up
            // Search item in the test world's own Search drawer.
            if crate::vault::search_label() == "Search" && request.get("seed").is_some() {
                return reply.say(json!({ "error": "seed only works on a test run" }));
            }
            if let Some(Value::Object(seed)) = request.get("seed") {
                let get = |k: &str| seed.get(k).and_then(Value::as_str).unwrap_or_default();
                let ok = crate::keychain::save(
                    crate::vault::search_drawer(&crate::vault::search_label()),
                    get("host"),
                    get("user"),
                    get("password"),
                    None,
                    seed.get("clear").and_then(Value::as_bool).unwrap_or(false),
                );
                return reply.say(json!({ "seeded": ok }));
            }
            if let Some(row) = request.get("pick").and_then(Value::as_u64) {
                crate::signin::bench_pick(usize::try_from(row).unwrap_or(usize::MAX));
            }
            if let Some(answer) = text("answer") {
                crate::signin::bench_answer(&answer);
            }
            if let Some(host) = text("kept") {
                let kept: Vec<Value> = crate::vault::kept_for(&host)
                    .into_iter()
                    .map(|k| json!({"host": k.host, "user": k.user, "search": k.search, "clear": k.clear}))
                    .collect();
                return reply.say(json!({ "kept": kept }));
            }
            if let Some(host) = text("forget") {
                let user = text("user").unwrap_or_default();
                return reply.say(json!({ "forgot": crate::vault::forget(&host, &user) }));
            }
            reply.say(crate::signin::snapshot());
        }
        "settings" => {
            // Settings as it shows them; with `set`, a setting changed first, as its
            // control would (`{"set": {"look": "dark"}}`; a merged control by its row's id).
            if let Some(Value::Object(set)) = request.get("set") {
                let mut kept = crate::prefs::Kept::new();
                for (key, value) in set {
                    let merged =
                        tap_core::settings_index::item(key).and_then(|item| match item.control {
                            tap_core::settings_index::Control::Merged(which) => Some(which),
                            _ => None,
                        });
                    match (merged, value.as_str()) {
                        (Some(which), Some(raw)) => {
                            tap_core::settings_index::set_merged(&mut kept, which, raw);
                        }
                        _ => tap_core::settings_index::Prefs::set(&mut kept, key, value.clone()),
                    }
                }
                crate::prefs::apply(mtm);
                // A `bench` setting among them opens or closes this very socket.
                refresh();
                settings::changed();
            }
            if let Some(page) = text("page")
                .as_deref()
                .and_then(tap_core::settings_index::Page::named)
            {
                rt::dispatch(|browser| browser.open_page(tap_core::internal::Page::Settings));
                settings::show_page(page);
            }
            if let Some(words) = text("find") {
                rt::dispatch(|browser| browser.open_page(tap_core::internal::Page::Settings));
                settings::find(&words);
            }
            let mut out = settings::describe();
            out["bench"] = json!({
                "on": tap_core::settings_index::flag(&crate::prefs::Kept::new(), "bench"),
                "listening": running(),
            });
            out["engine"] = json!(rt::read(|b| b.engine.raw()));
            out["keywords"] = json!(rt::read(|b| b
                .keywords
                .iter()
                .map(|k| k.keyword.clone())
                .collect::<Vec<_>>()));
            reply.say(out);
        }
        "update" => {
            // The updater: what it says, or a check as Check for Updates… starts one, or
            // Restart to Update.
            match text("action").as_deref() {
                Some("check") => update::check_now(),
                Some("restart") => {
                    // Answered before Tap goes: always one answer.
                    reply.say(update::status());
                    return later(0.3, update::restart);
                }
                _ => {}
            }
            reply.say(update::status());
        }
        "open" => {
            let Some(url) = text("url").as_deref().and_then(address::url) else {
                return reply.say(json!({ "error": "open needs a url" }));
            };
            let Some(id) = rt::update(|browser| browser.open(Some(&url), false, None)) else {
                return;
            };
            rt::settle();
            reply.say(describe_id(id));
        }
        "link" => {
            // A link as another app would hand it over (Search's `Links.arrived`).
            let Some(url) = text("url") else {
                return reply.say(json!({ "error": "link needs a url" }));
            };
            if !links::take(&url) {
                return reply.say(json!({ "error": "not a link Tap opens" }));
            }
            rt::settle();
            let active = rt::read(Browser::active).flatten();
            reply.say(active.map_or(Value::Null, describe_id));
        }
        "go" => {
            let (Some(id), Some(url)) = (
                tab_id(request),
                text("url").as_deref().and_then(address::url),
            ) else {
                return reply.say(json!({ "error": "go needs a tab id and a url" }));
            };
            rt::dispatch(move |browser| browser.go(id, &url));
            reply.say(describe_id(id));
        }
        "select" => {
            let Some(id) = tab_id(request) else {
                return reply.say(json!({ "error": "no such tab" }));
            };
            rt::dispatch(move |browser| browser.select(id));
            reply.say(describe_id(id));
        }
        "close" => {
            let Some(id) = tab_id(request) else {
                return reply.say(json!({ "error": "no such tab" }));
            };
            rt::dispatch(move |browser| browser.close(id));
            reply.say(json!({ "closed": 1 }));
        }
        "wait" => {
            let Some(id) = tab_id(request) else {
                return reply.say(json!({ "error": "no such tab" }));
            };
            let seconds = request
                .get("seconds")
                .and_then(Value::as_f64)
                .unwrap_or(20.0);
            wait(id, seconds, reply);
        }
        "eval" => {
            let (Some(id), Some(js)) = (tab_id(request), text("js")) else {
                return reply.say(json!({ "error": "eval needs a tab id and js" }));
            };
            // "world": "tap" (or Search's "search"): in Tap's own content world.
            if matches!(text("world").as_deref(), Some("tap" | "search")) {
                let Some(web) = web::existing(id) else {
                    return reply.say(json!({ "error": "the tab has no page" }));
                };
                let reply = std::cell::RefCell::new(Some(reply));
                web::evaluate_in_world(&web, &js, move |value| {
                    if let Some(reply) = reply.borrow_mut().take() {
                        reply.say(json!({ "value": value }));
                    }
                });
                return;
            }
            evaluate(id, &js, reply);
        }
        "text" => {
            let Some(id) = tab_id(request) else {
                return reply.say(json!({ "error": "no such tab" }));
            };
            evaluate(id, "document.body ? document.body.innerText : ''", reply);
        }
        "field" => field(request, reply),
        "fieldkey" => {
            // A key the field's editor turns into a command (↓ ↑ Tab Return Esc), sent
            // through the same path, so the field's delegate answers it.
            let command =
                match text("key").as_deref() {
                    Some("down") => sel!(moveDown:),
                    Some("up") => sel!(moveUp:),
                    Some("tab") => sel!(insertTab:),
                    Some("return") => sel!(insertNewline:),
                    Some("escape") => sel!(cancelOperation:),
                    _ => return reply.say(
                        json!({ "error": "fieldkey needs key: down, up, tab, return or escape" }),
                    ),
                };
            // Whichever field has the keyboard: the address field, or the find bar's.
            let Some(editor) = window::key_editor() else {
                return reply.say(json!({ "error": "no field has the keyboard" }));
            };
            // SAFETY: a standard text command sent to the field editor.
            unsafe { editor.doCommandBySelector(command) };
            reply.say(json!({ "field": window::field_text() }));
        }
        "sleep" => {
            // Now rather than after half an hour, but past every other check a tab has to
            // clear: the answer says which one kept it awake.
            let Some(id) = tab_id(request) else {
                return reply.say(json!({ "error": "no such tab" }));
            };
            sleep::try_sleep(id, move |said| {
                let asleep = rt::read(|b| b.tab(id).is_some_and(|t| !t.awake())).unwrap_or(false);
                reply.say(json!({ "said": said, "asleep": asleep }));
            });
        }
        "crash" => {
            // The tab's page process, killed as the system's memory killer kills it: the
            // way #849's loop is reproduced (WebKit's own `_killWebContentProcess` ends it
            // quietly and never tells the delegate).
            let Some(web) = tab_id(request).and_then(web::existing) else {
                return reply.say(json!({ "error": "the tab has no page" }));
            };
            let pid = web::process_id(&web);
            if pid <= 0 {
                return reply.say(json!({ "error": "no page process" }));
            }
            // SAFETY: a signal to a process id WebKit named as this page's.
            let killed = unsafe { libc::kill(pid, libc::SIGKILL) } == 0;
            reply.say(json!({ "killed": killed, "process": pid }));
        }
        "command" => {
            // A command as its menu item or the find bar's buttons send it.
            let tag = match text("name").as_deref() {
                Some("find") => Command::Find,
                Some("findNext") => Command::FindNext,
                Some("findPrevious") => Command::FindPrevious,
                Some("closeFind") => Command::CloseFind,
                Some("matchCase") => Command::MatchCase,
                Some("wholeWords") => Command::WholeWords,
                Some("settings") => Command::Settings,
                Some("importProfile") => Command::ImportProfile,
                Some("exportProfile") => Command::ExportProfile,
                Some("checkForUpdates") => Command::CheckForUpdates,
                _ => return reply.say(json!({ "error": "command needs a name" })),
            };
            menu::run(tag as isize);
            reply.say(json!({ "ran": text("name") }));
        }
        "type" => {
            // Text into whichever field has the keyboard, as typing puts it there.
            let Some(text) = text("text") else {
                return reply.say(json!({ "error": "type needs text" }));
            };
            let Some(editor) = window::key_editor() else {
                return reply.say(json!({ "error": "no field has the keyboard" }));
            };
            let selection = NSRange::new(usize::try_from(NSNotFound).unwrap_or(usize::MAX), 0);
            // SAFETY: the field editor takes a string to insert in place of the selection.
            unsafe { editor.insertText_replacementRange(&NSString::from_str(&text), selection) };
            reply.say(json!({ "typed": text }));
        }
        "unmark" => {
            // An input method committing what it was composing.
            let Some(editor) = window::field_editor().and_then(|e| e.downcast::<NSTextView>().ok())
            else {
                return reply.say(json!({ "error": "the field doesn't have the keyboard" }));
            };
            editor.unmarkText();
            reply.say(json!({ "field": window::field_text() }));
        }
        "downloads" => reply.say(json!({ "done": downloads::done(), "going": downloads::going() })),
        "history" => reply.say(json!({ "visits": rt::read(|b| b.history().len()) })),
        "drag" => {
            // A press, a carry and (unless `release` is false) a let-go, through the
            // window as the hand would make them: from `from` to `to`, each [x, y] in
            // the window's points from its top left, in `steps` moves.
            let point = |key: &str| {
                let pair = request.get(key)?.as_array()?;
                Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
            };
            let (Some(from), Some(to)) = (point("from"), point("to")) else {
                return reply.say(json!({ "error": "drag needs from and to" }));
            };
            let steps = request
                .get("steps")
                .and_then(Value::as_u64)
                .unwrap_or(12)
                .max(1);
            let release = request
                .get("release")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut sent = mouse(NSEventType::LeftMouseDown, from);
            for step in 1..=steps {
                #[allow(clippy::cast_precision_loss)] // a handful of steps
                let t = step as f64 / steps as f64;
                let at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                sent &= mouse(NSEventType::LeftMouseDragged, at);
            }
            if release {
                sent &= mouse(NSEventType::LeftMouseUp, to);
            }
            reply.say(json!({ "sent": sent, "row": rt::read(row).unwrap_or_default() }));
        }
        "rightclick" => {
            // A right-click at `x`, `y` (the window's points from its top left): the
            // menu it opens holds the main thread until it closes.
            let x = request.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = request.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let sent = mouse(NSEventType::RightMouseDown, (x, y));
            reply.say(json!({ "sent": sent, "row": rt::read(row).unwrap_or_default() }));
        }
        "click" => {
            // A click at `x`, `y` with `modifiers` held ("command", "shift").
            let x = request.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = request.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let mut flags = NSEventModifierFlags::empty();
            for held in request
                .get("modifiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                flags |= match held {
                    "command" => NSEventModifierFlags::Command,
                    "shift" => NSEventModifierFlags::Shift,
                    _ => NSEventModifierFlags::empty(),
                };
            }
            let sent = mouse_with(NSEventType::LeftMouseDown, (x, y), flags)
                && mouse_with(NSEventType::LeftMouseUp, (x, y), flags);
            reply.say(json!({ "sent": sent, "selected": rt::read(Browser::selection_count) }));
        }
        "mouseup" => {
            let x = request.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = request.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let sent = mouse(NSEventType::LeftMouseUp, (x, y));
            reply.say(json!({ "sent": sent, "row": rt::read(row).unwrap_or_default() }));
        }
        "keyeq" => {
            let Some(key) = text("key") else {
                return reply.say(json!({ "error": "keyeq needs a key" }));
            };
            let modifiers: Vec<String> = request
                .get("modifiers")
                .and_then(Value::as_array)
                .map_or_else(
                    || vec!["command".to_owned()],
                    |list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    },
                );
            let handled = key_equivalent(mtm, &key, &modifiers);
            reply.say(json!({ "handled": handled }));
        }
        "quit" => {
            reply.say(json!({ "quitting": true }));
            later(0.3, || {
                if let Some(mtm) = MainThreadMarker::new() {
                    // Past any sheet left open, which would keep AppKit from quitting.
                    menu::quit_now(mtm);
                }
            });
        }
        _ => reply.say(json!({ "error": format!("unknown verb: {verb}") })),
    }
}

/// Until the tab has finished loading, failed, or `seconds` have passed.
fn wait(id: TabId, seconds: f64, reply: Reply) {
    let done = rt::read(|browser| {
        browser.tab(id).is_none_or(|t| {
            t.failure().is_some() || (!t.loading() && !t.url().is_empty() && t.progress() >= 1.0)
        })
    })
    .unwrap_or(true);
    if done || seconds <= 0.0 {
        return reply.say(describe_id(id));
    }
    let reply = std::sync::Mutex::new(Some(reply));
    later(0.1, move || {
        if let Some(reply) = reply.lock().ok().and_then(|mut r| r.take()) {
            wait(id, seconds - 0.1, reply);
        }
    });
}

/// Runs `work` on the main thread after `seconds`.
fn later(seconds: f64, work: impl FnOnce() + Send + 'static) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // short delays
    let nanos = (seconds * 1e9) as u64;
    let when = dispatch2::DispatchTime::NOW.time(i64::try_from(nanos).unwrap_or(i64::MAX));
    let _ = dispatch2::DispatchQueue::main().after(when, work);
}

/// Evaluates an expression in the tab's page and answers with its value, as JSON.
fn evaluate(id: TabId, js: &str, reply: Reply) {
    let Some(web) = web::existing(id) else {
        return reply.say(json!({ "error": "the tab has no page" }));
    };
    let script = format!("JSON.stringify((() => ({js}))())");
    let reply = std::cell::RefCell::new(Some(reply));
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(reply) = reply.borrow_mut().take() else {
            return;
        };
        // SAFETY: WebKit hands either a valid object or null for each, for the call.
        let (value, error) = unsafe { (value.as_ref(), error.as_ref()) };
        if let Some(error) = error {
            return reply.say(json!({ "error": error.localizedDescription().to_string() }));
        }
        let text = value
            .and_then(|v| v.downcast_ref::<NSString>())
            .map(ToString::to_string);
        let value = text
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        reply.say(json!({ "value": value }));
    });
    // SAFETY: a live web view, a valid script, and a block that outlives the call.
    unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&script), Some(&block)) };
}

/// Puts text into the address field through its own editor, as a paste or typing does,
/// then presses Return through the field's delegate if asked (`go`).
fn field(request: &Value, reply: Reply) {
    let Some(text) = request
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return reply.say(json!({ "error": "field needs some text" }));
    };
    let go = request.get("go").and_then(Value::as_bool).unwrap_or(false);
    let launching = request
        .get("launching")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // As an input method puts text in while still composing: marked, not committed.
    let marked = request
        .get("marked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    rt::dispatch(move |browser| {
        if browser.editing().is_none() {
            browser.begin_editing(launching);
        }
    });
    later(0.3, move || {
        let Some(editor) = window::field_editor() else {
            return reply.say(json!({ "error": "the address field has no editor" }));
        };
        let Some(editor) = editor.downcast_ref::<NSTextView>() else {
            return reply.say(json!({ "error": "the field's editor is not a text view" }));
        };
        // As an input method inserts text: in place of the selection (NSNotFound).
        let selection = NSRange::new(usize::try_from(NSNotFound).unwrap_or(usize::MAX), 0);
        let string = NSString::from_str(&text);
        if marked {
            let caret = NSRange::new(text.encode_utf16().count(), 0);
            // SAFETY: the field editor takes marked text in place of the selection.
            unsafe {
                editor.setMarkedText_selectedRange_replacementRange(&string, caret, selection);
            }
        } else {
            // SAFETY: the field editor takes a string to insert in place of the selection.
            unsafe { editor.insertText_replacementRange(&string, selection) };
        }
        // What the field holds and offers, read before Return puts it away.
        let state = rt::read(|b| {
            b.editing().map(|e| {
                json!({
                    "typed": e.typed,
                    "offers": e.offers.iter().map(|o| o.key.clone()).collect::<Vec<_>>(),
                    "ending": e.ending,
                })
            })
        })
        .flatten()
        .unwrap_or_else(|| json!({}));
        if go {
            let enter: Sel = sel!(insertNewline:);
            // SAFETY: the standard command the field's delegate answers for Return.
            unsafe { editor.doCommandBySelector(enter) };
        }
        let active = rt::read(Browser::active).flatten();
        let mut out = state;
        out["field"] = json!(window::field_text());
        if let Some(id) = active {
            out["tab"] = describe_id(id);
        }
        reply.say(out);
    });
}

/// A mouse event through the window, at a point in its points from its top left.
fn mouse(kind: NSEventType, at: (f64, f64)) -> bool {
    mouse_with(kind, at, NSEventModifierFlags::empty())
}

/// As `mouse`, with keys held.
fn mouse_with(kind: NSEventType, (x, y): (f64, f64), flags: NSEventModifierFlags) -> bool {
    let Some(window) = window::main_window() else {
        return false;
    };
    let height = window.frame().size.height;
    let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        NSPoint::new(x, height - y),
        flags,
        objc2_foundation::NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    ) else {
        return false;
    };
    window.sendEvent(&event);
    true
}

/// A key equivalent, sent through the menu bar as a key press would be.
fn key_equivalent(mtm: MainThreadMarker, key: &str, modifiers: &[String]) -> bool {
    let mut flags = NSEventModifierFlags::empty();
    for modifier in modifiers {
        flags |= match modifier.as_str() {
            "command" => NSEventModifierFlags::Command,
            "shift" => NSEventModifierFlags::Shift,
            "control" => NSEventModifierFlags::Control,
            "option" => NSEventModifierFlags::Option,
            _ => NSEventModifierFlags::empty(),
        };
    }
    let key = if key == "tab" { "\t" } else { key };
    let window_number = window::number().unwrap_or(0);
    let Some(event) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::new(0.0, 0.0),
        flags,
        0.0,
        window_number,
        None,
        &NSString::from_str(key),
        &NSString::from_str(key),
        false,
        0,
    ) else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .mainMenu()
        .is_some_and(|menu: Retained<_>| menu.performKeyEquivalent(&event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_world_always_listens_and_a_real_one_only_when_switched_on() {
        assert_eq!(
            [
                should_listen(true, true),
                should_listen(true, false),
                should_listen(false, true),
                should_listen(false, false),
            ],
            [true, true, true, false]
        );
    }
}
