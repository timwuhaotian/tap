//! The bench: a way for a script on this Mac to drive Tap, in Search's protocol
//! (Search's `Bench.swift`, docs/PLAN.md §9).
//!
//! A Unix socket in the store folder, readable by this user only, and the other end is
//! checked for the same uid before a word is read. One JSON object per line in, one per
//! line out, one request per connection, and always exactly one answer: a request that
//! nothing answers is answered with an error after its patience runs out.
//!
//! Listening only on a test run (`TAP_PROBE`) for now: such a world holds nothing of
//! yours. Search's Settings switch and keychain consent come with Settings.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    sync::mpsc,
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

use crate::{dialogs, rt, store, web, window};

/// The longest request line read: a line that never ends is not a request.
const LONGEST: usize = 4_000_000;

/// Starts listening, on a test run.
pub fn start() {
    if std::env::var_os(identity::PROBE_ENV).is_none() {
        return;
    }
    let path = store::file("bench.sock");
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!("bench: can't listen at {}", path.display());
        return;
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let _ = thread::Builder::new()
        .name("tap.bench".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = thread::Builder::new()
                    .name("tap.bench.client".into())
                    .spawn(move || serve(stream));
            }
        });
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
            out["fieldText"] = json!(window::field_text());
            out["firstResponder"] = json!(window::first_responder());
            out["asked"] = json!(dialogs::asked());
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
            let Some(editor) = window::field_editor().and_then(|e| e.downcast::<NSTextView>().ok())
            else {
                return reply.say(json!({ "error": "the field doesn't have the keyboard" }));
            };
            // SAFETY: a standard text command sent to the field editor.
            unsafe { editor.doCommandBySelector(command) };
            reply.say(json!({ "field": window::field_text() }));
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
        "history" => reply.say(json!({ "visits": rt::read(|b| b.history().len()) })),
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
                    NSApplication::sharedApplication(mtm).terminate(None);
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
