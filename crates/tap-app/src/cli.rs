//! `tap`, the command agents drive Tap with (docs/PLAN.md §8 "Agents use Tap"): the
//! app's own binary, given arguments. Each command is one or two bench requests
//! (`tap-shell/src/bench.rs`) over the socket, answered in text an agent reads easily,
//! or as the bench's JSON with `--json`. No AppKit here: only the socket and `open`.

use std::{
    fmt::Write as _,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Command as Process, ExitCode},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tap_core::identity;

const HELP: &str = "\
tap — drive Tap, this Mac's browser, from a shell or an agent

  tap tabs                      every tab: [id] title — address (* is the one in front)
  tap open <url>                a new tab behind yours; waits for the page, prints its id
  tap go <id> <url>             the tab to another address, and waits
  tap snapshot <id>             the page's controls, each with a ref: [e3] button \"Sign in\"
  tap click <id> <ref>          clicks a control from the last snapshot
  tap fill <id> <ref> <text>    types text into a field (or picks a list's option)
  tap press <id> <key> [<ref>]  a key (Enter, Tab, Escape, …) on a control, or the focus
  tap text <id>                 the page's text
  tap info <id>                 the tab's details: state, place, the page's own facts
  tap eval <id> <js>            a JavaScript expression's value, as JSON
  tap console <id>              the tab's console: what its scripts threw
  tap wait <id> [<seconds>]     until the tab has loaded
  tap select <id>               brings the tab to the front, to show it to the person
  tap close <id>                closes the tab

  --json   the bench's answers as they come, one JSON object per line

Tap starts in the background if it isn't running. Refs last until the page changes:
take a new snapshot after a click that loads something.";

/// A command, read from the arguments.
#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Tabs,
    Open(String),
    Go(String, String),
    Snapshot(String),
    Act {
        id: String,
        action: &'static str,
        target: String,
        value: String,
    },
    Text(String),
    Info(String),
    Eval(String, String),
    Console(String),
    Wait(String, f64),
    Select(String),
    Close(String),
}

/// Whether these arguments are for the CLI rather than the app: AppKit and Xcode hand an
/// app `-Flag value` pairs of their own, and none of them is a command.
#[must_use]
pub fn wanted(args: &[String]) -> bool {
    args.first().is_some_and(|first| {
        !first.starts_with("-psn") && !first.starts_with("-N") && !first.starts_with("-Apple")
    })
}

fn parse(args: &[String]) -> Result<Command, String> {
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
        .collect();
    let need = |count: usize, usage: &str| {
        if words.len() > count {
            Ok(())
        } else {
            Err(format!("usage: tap {usage}"))
        }
    };
    let word = |at: usize| words.get(at).map(|w| (*w).to_owned()).unwrap_or_default();
    let rest = |from: usize| words.get(from..).map(|w| w.join(" ")).unwrap_or_default();
    Ok(match words.first().copied().unwrap_or("help") {
        "help" | "-h" | "--help" => Command::Help,
        "tabs" => Command::Tabs,
        "open" => {
            need(1, "open <url>")?;
            Command::Open(word(1))
        }
        "go" => {
            need(2, "go <id> <url>")?;
            Command::Go(word(1), word(2))
        }
        "snapshot" => {
            need(1, "snapshot <id>")?;
            Command::Snapshot(word(1))
        }
        "click" => {
            need(2, "click <id> <ref>")?;
            Command::Act {
                id: word(1),
                action: "click",
                target: word(2),
                value: String::new(),
            }
        }
        "fill" => {
            need(3, "fill <id> <ref> <text>")?;
            Command::Act {
                id: word(1),
                action: "fill",
                target: word(2),
                value: rest(3),
            }
        }
        "press" => {
            need(2, "press <id> <key> [<ref>]")?;
            Command::Act {
                id: word(1),
                action: "press",
                target: word(3),
                value: word(2),
            }
        }
        "text" => {
            need(1, "text <id>")?;
            Command::Text(word(1))
        }
        "info" => {
            need(1, "info <id>")?;
            Command::Info(word(1))
        }
        "eval" => {
            need(2, "eval <id> <js>")?;
            Command::Eval(word(1), rest(2))
        }
        "console" => {
            need(1, "console <id>")?;
            Command::Console(word(1))
        }
        "wait" => {
            need(1, "wait <id> [<seconds>]")?;
            Command::Wait(
                word(1),
                words.get(2).and_then(|s| s.parse().ok()).unwrap_or(20.0),
            )
        }
        "select" => {
            need(1, "select <id>")?;
            Command::Select(word(1))
        }
        "close" => {
            need(1, "close <id>")?;
            Command::Close(word(1))
        }
        other => return Err(format!("tap: no command \"{other}\" (tap help lists them)")),
    })
}

/// The bench request a command starts with.
fn request(command: &Command) -> Value {
    match command {
        Command::Help => Value::Null,
        Command::Tabs => json!({ "do": "tabs" }),
        Command::Open(url) => json!({ "do": "open", "url": url }),
        Command::Go(id, url) => json!({ "do": "go", "id": id, "url": url }),
        Command::Snapshot(id) => json!({ "do": "snapshot", "id": id }),
        Command::Act {
            id,
            action,
            target,
            value,
        } => {
            json!({ "do": "act", "id": id, "action": action, "ref": target, "value": value })
        }
        Command::Text(id) => json!({ "do": "text", "id": id }),
        Command::Info(id) => json!({ "do": "info", "id": id }),
        Command::Eval(id, js) => json!({ "do": "eval", "id": id, "js": js }),
        Command::Console(id) => json!({ "do": "console", "id": id }),
        Command::Wait(id, seconds) => json!({ "do": "wait", "id": id, "seconds": seconds }),
        Command::Select(id) => json!({ "do": "select", "id": id }),
        Command::Close(id) => json!({ "do": "close", "id": id }),
    }
}

/// The tab to wait on after the first answer, if the command may have started a load.
fn loads(command: &Command, answer: &Value) -> Option<String> {
    match command {
        Command::Open(_) => answer.get("id").and_then(Value::as_str).map(str::to_owned),
        Command::Go(id, _) => Some(id.clone()),
        Command::Act { id, action, .. } if *action != "fill" => Some(id.clone()),
        _ => None,
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn tab_line(tab: &Value) -> String {
    let mark = if tab.get("active").and_then(Value::as_bool) == Some(true) {
        "*"
    } else {
        " "
    };
    let mut line = format!(
        "[{}]{mark} {} — {}",
        text(tab, "id"),
        text(tab, "label"),
        text(tab, "url")
    );
    if tab.get("loading").and_then(Value::as_bool) == Some(true) {
        line.push_str(" (loading)");
    }
    // A blank tab has never had a page to be awake with.
    if tab.get("awake").and_then(Value::as_bool) == Some(false) && !text(tab, "url").is_empty() {
        line.push_str(" (asleep)");
    }
    if let Some(failure) = tab.get("failure").filter(|f| !f.is_null()) {
        let _ = write!(
            line,
            " (failed: {})",
            failure
                .as_str()
                .map_or_else(|| failure.to_string(), str::to_owned)
        );
    }
    line
}

fn control_line(control: &Value) -> String {
    let mut line = format!(
        "[{}] {} \"{}\"",
        text(control, "ref"),
        text(control, "role"),
        text(control, "name")
    );
    if let Some(value) = control.get("value").and_then(Value::as_str) {
        let _ = write!(line, " = \"{value}\"");
    }
    match control.get("checked").and_then(Value::as_bool) {
        Some(true) => line.push_str(" (checked)"),
        Some(false) => line.push_str(" (unchecked)"),
        None => {}
    }
    if control.get("disabled").and_then(Value::as_bool) == Some(true) {
        line.push_str(" (disabled)");
    }
    if let Some(href) = control.get("href").and_then(Value::as_str) {
        let _ = write!(line, " → {href}");
    }
    line
}

/// The answer as an agent reads it.
fn render(command: &Command, answer: &Value) -> String {
    match command {
        Command::Tabs => answer
            .get("tabs")
            .and_then(Value::as_array)
            .map(|tabs| tabs.iter().map(tab_line).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default(),
        Command::Snapshot(_) => {
            let mut lines = vec![format!(
                "{} — {}",
                text(answer, "title"),
                text(answer, "url")
            )];
            let controls = answer.get("controls").and_then(Value::as_array);
            lines.extend(controls.into_iter().flatten().map(control_line));
            if controls.is_none_or(Vec::is_empty) {
                lines.push("(no controls on this page; tap text reads it)".to_owned());
            }
            match answer.get("more").and_then(Value::as_u64) {
                Some(more) if more > 0 => lines.push(format!("({more} more not listed)")),
                _ => {}
            }
            lines.join("\n")
        }
        Command::Text(_) | Command::Info(_) | Command::Console(_) => answer
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Command::Eval(..) => answer
            .get("value")
            .map(ToString::to_string)
            .unwrap_or_default(),
        Command::Close(_) => "closed".to_owned(),
        Command::Act { action, .. } => {
            let control = answer
                .get("clicked")
                .or_else(|| answer.get("filled"))
                .or_else(|| answer.get("on"));
            let done = match *action {
                "click" => "clicked",
                "fill" => "filled",
                _ => "pressed",
            };
            let mut line = control.map_or_else(
                || done.to_owned(),
                |c| format!("{done} {}", control_line(c)),
            );
            if let Some(tab) = answer.get("tab") {
                let _ = write!(line, "\nnow {}", tab_line(tab));
            }
            line
        }
        Command::Help
        | Command::Open(_)
        | Command::Go(..)
        | Command::Wait(..)
        | Command::Select(_) => tab_line(answer.get("tab").unwrap_or(answer)),
    }
}

fn socket() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let world = identity::world(std::env::var(identity::PROBE_ENV).ok().as_deref());
    home.join("Library/Application Support")
        .join(identity::folder_name(world.as_deref()))
        .join("bench.sock")
}

/// One request, one answer, as the bench speaks.
fn ask(request: &Value) -> Result<Value, std::io::Error> {
    let mut stream = UnixStream::connect(socket())?;
    stream.set_read_timeout(Some(Duration::from_secs(90)))?;
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    Ok(serde_json::from_str(&answer)
        .unwrap_or_else(|_| json!({ "error": "Tap's answer wasn't JSON" })))
}

/// Starts Tap behind the app in front if nothing listens, and waits for its socket. A
/// test world is never started here: its scripts start it.
fn reach() -> Result<(), String> {
    if UnixStream::connect(socket()).is_ok() {
        return Ok(());
    }
    if std::env::var_os(identity::PROBE_ENV).is_none() {
        let _ = Process::new("/usr/bin/open")
            .args(["-g", "-b", identity::BUNDLE_ID])
            .status();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            if UnixStream::connect(socket()).is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    Err(format!(
        "Tap isn't listening at {}. Is Tap installed, and Settings › Let agents use Tap on? If both are true, restart Tap (or switch that setting off and on) and try again.",
        socket().display()
    ))
}

/// The answer after the follow-up wait: a failed wait replaces it, so its
/// error decides the exit code even for `act`, whose tab nests below.
fn after_wait(command: &Command, mut answer: Value, waited: Value) -> Value {
    if waited.get("error").is_some() {
        waited
    } else if matches!(command, Command::Act { .. }) {
        answer["tab"] = waited;
        answer
    } else {
        waited
    }
}

/// Runs a command and says how it went.
pub fn run(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let command = match parse(args) {
        Ok(Command::Help) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let Err(message) = reach() {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }
    let mut answer = match ask(&request(&command)) {
        Ok(answer) => answer,
        Err(error) => {
            eprintln!("tap: {error}");
            return ExitCode::FAILURE;
        }
    };
    if answer.get("error").is_none()
        && let Some(id) = loads(&command, &answer)
    {
        if json {
            println!("{answer}");
        }
        // A click or a key may start a load a moment later, or never.
        if matches!(command, Command::Act { .. }) {
            thread::sleep(Duration::from_millis(400));
        }
        let waited = ask(&json!({ "do": "wait", "id": id, "seconds": 20 })).unwrap_or(Value::Null);
        if json {
            println!("{waited}");
            if waited.get("error").is_some() {
                return ExitCode::FAILURE;
            }
            return ExitCode::SUCCESS;
        }
        answer = after_wait(&command, answer, waited);
    }
    if let Some(error) = answer.get("error") {
        if json {
            println!("{answer}");
        } else {
            eprintln!(
                "tap: {}",
                error
                    .as_str()
                    .map_or_else(|| error.to_string(), str::to_owned)
            );
        }
        return ExitCode::FAILURE;
    }
    println!(
        "{}",
        if json {
            answer.to_string()
        } else {
            render(&command, &answer)
        }
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn the_app_s_own_launch_arguments_are_not_commands() {
        assert!(!wanted(&[]));
        assert!(!wanted(&args("-psn_0_12345")));
        assert!(!wanted(&args("-NSDocumentRevisionsDebugMode YES")));
        assert!(!wanted(&args("-ApplePersistenceIgnoreState YES")));
        assert!(wanted(&args("tabs")));
        assert!(wanted(&args("--help")));
    }

    #[test]
    fn commands_become_the_bench_s_requests() {
        let asked = |line: &str| parse(&args(line)).map(|c| request(&c));
        assert_eq!(asked("tabs"), Ok(json!({ "do": "tabs" })));
        assert_eq!(
            asked("open example.com --json"),
            Ok(json!({ "do": "open", "url": "example.com" }))
        );
        assert_eq!(
            asked("fill 3 e2 hello there"),
            Ok(
                json!({ "do": "act", "id": "3", "action": "fill", "ref": "e2", "value": "hello there" })
            )
        );
        assert_eq!(
            asked("press 3 Enter"),
            Ok(json!({ "do": "act", "id": "3", "action": "press", "ref": "", "value": "Enter" }))
        );
        assert_eq!(
            asked("wait 3"),
            Ok(json!({ "do": "wait", "id": "3", "seconds": 20.0 }))
        );
        assert_eq!(
            asked("eval 3 document.title"),
            Ok(json!({ "do": "eval", "id": "3", "js": "document.title" }))
        );
        assert_eq!(
            asked("console 3"),
            Ok(json!({ "do": "console", "id": "3" }))
        );
        assert_eq!(
            parse(&args("click 3")),
            Err("usage: tap click <id> <ref>".to_owned())
        );
        assert!(parse(&args("frobnicate")).is_err());
        assert_eq!(parse(&args("help")), Ok(Command::Help));
    }

    #[test]
    fn only_what_may_load_a_page_is_waited_on() {
        let open = Command::Open("x".to_owned());
        assert_eq!(loads(&open, &json!({ "id": "4" })), Some("4".to_owned()));
        let fill = parse(&args("fill 4 e1 a")).ok();
        assert_eq!(fill.and_then(|c| loads(&c, &json!({}))), None);
        let click = parse(&args("click 4 e1")).ok();
        assert_eq!(
            click.and_then(|c| loads(&c, &json!({}))),
            Some("4".to_owned())
        );
        assert_eq!(loads(&Command::Snapshot("4".to_owned()), &json!({})), None);
    }

    #[test]
    fn a_snapshot_reads_as_one_control_a_line() {
        let answer = json!({
            "title": "Sign in", "url": "https://example.com/",
            "controls": [
                { "ref": "e1", "role": "textbox", "name": "Email", "value": "a@b.c" },
                { "ref": "e2", "role": "checkbox", "name": "Keep me", "checked": false },
                { "ref": "e3", "role": "button", "name": "Go", "disabled": true },
                { "ref": "e4", "role": "link", "name": "Help", "href": "https://example.com/help" },
            ],
            "more": 2,
        });
        assert_eq!(
            render(&Command::Snapshot("1".to_owned()), &answer),
            "Sign in — https://example.com/\n\
             [e1] textbox \"Email\" = \"a@b.c\"\n\
             [e2] checkbox \"Keep me\" (unchecked)\n\
             [e3] button \"Go\" (disabled)\n\
             [e4] link \"Help\" → https://example.com/help\n\
             (2 more not listed)"
        );
    }

    #[test]
    fn a_failed_wait_replaces_the_answer_so_it_fails() {
        let click = parse(&args("click 4 e1")).unwrap();
        let answer = json!({ "clicked": {} });
        let waited = json!({ "error": "no such tab" });
        let settled = after_wait(&click, answer, waited.clone());
        assert_eq!(settled, waited);
        assert!(settled.get("error").is_some());
    }

    #[test]
    fn a_finished_wait_nests_below_an_act() {
        let click = parse(&args("click 4 e1")).unwrap();
        let answer = json!({ "clicked": {} });
        let waited = json!({ "tab": {} });
        assert_eq!(
            after_wait(&click, answer.clone(), waited.clone()),
            json!({ "clicked": {}, "tab": { "tab": {} } })
        );
        let go = Command::Go("4".to_owned(), "https://example.com/".to_owned());
        assert_eq!(after_wait(&go, answer, waited.clone()), waited);
    }

    #[test]
    fn tabs_read_with_the_one_in_front_marked() {
        let answer = json!({ "tabs": [
            { "id": "1", "label": "Mail", "url": "https://mail.example/", "active": true, "awake": true },
            { "id": "2", "label": "Docs", "url": "https://docs.example/", "active": false, "awake": false },
        ]});
        assert_eq!(
            render(&Command::Tabs, &answer),
            "[1]* Mail — https://mail.example/\n[2]  Docs — https://docs.example/ (asleep)"
        );
    }
}
