//! What lets every coding agent on this Mac know Tap is its browser (docs/PLAN.md §8
//! "Agents use Tap"): the `tap` command on the PATH and a skill that says when and how
//! to use it, kept in place while Settings › Let agents use Tap is on.
//!
//! - `~/.local/bin/tap` → this app's binary, which is the command when given one.
//! - `~/.agents/skills/tap/SKILL.md`, where Codex, Gemini and opencode look for skills.
//! - `~/.claude/skills/tap` → that folder, for Claude Code, if Claude Code is here.
//!
//! Only the copy of Tap in an Applications folder does this, so a build run from the
//! tree never points the command at itself, and never a test run. Nothing that isn't
//! Tap's is replaced or removed: a file of the same name that someone else put there
//! stays, and Tap's own are known by their target or by the mark in the skill.

use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use crate::{bench, prefs};

const SKILL: &str = include_str!("../../../assets/agent/SKILL.md");
/// What marks the skill as Tap's to rewrite or remove.
const MARK: &str = "Installed by Tap.app";

struct Places {
    command: PathBuf,
    skill: PathBuf,
    claude: PathBuf,
    claude_home: PathBuf,
}

impl Places {
    fn under(home: &Path) -> Self {
        Self {
            command: home.join(".local/bin/tap"),
            skill: home.join(".agents/skills/tap"),
            claude: home.join(".claude/skills/tap"),
            claude_home: home.join(".claude"),
        }
    }
}

/// At launch, and whenever the switch moves: in place while it is on, gone when off.
pub fn refresh() {
    if bench::test_run() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) else {
        return;
    };
    let applications = [PathBuf::from("/Applications"), home.join("Applications")];
    if !applications.iter().any(|folder| exe.starts_with(folder)) {
        return;
    }
    let places = Places::under(&home);
    if tap_core::settings_index::flag(&prefs::Kept::new(), "bench") {
        install(&places, &exe);
    } else {
        remove(&places);
    }
}

/// Whether the command at `path` is Tap's: a link to some Tap.app's binary.
fn command_is_tap_s(path: &Path) -> bool {
    fs::read_link(path).is_ok_and(|target| {
        target.ends_with("Contents/MacOS/tap")
            && target.ancestors().any(|a| {
                a.extension().is_some_and(|e| e == "app")
                    && a.file_name().is_some_and(|name| name == "Tap.app")
            })
    })
}

/// Whether the skill folder is Tap's to write: not there yet, or holding Tap's skill.
fn skill_is_tap_s(folder: &Path) -> bool {
    match fs::read_to_string(folder.join("SKILL.md")) {
        Ok(text) => text.contains(MARK),
        Err(_) => fs::symlink_metadata(folder).is_err(),
    }
}

fn install(places: &Places, exe: &Path) {
    let report = |what: &str, result: std::io::Result<()>| {
        if let Err(error) = result {
            eprintln!("agents: can't put {what} in place: {error}");
        }
    };
    let here = fs::symlink_metadata(&places.command).is_ok();
    if !here
        || (command_is_tap_s(&places.command)
            && fs::read_link(&places.command).ok().as_deref() != Some(exe))
    {
        report(
            "the tap command",
            (|| {
                if here {
                    fs::remove_file(&places.command)?;
                }
                fs::create_dir_all(places.command.parent().unwrap_or(Path::new("/")))?;
                symlink(exe, &places.command)
            })(),
        );
    }
    let file = places.skill.join("SKILL.md");
    if skill_is_tap_s(&places.skill) && fs::read_to_string(&file).ok().as_deref() != Some(SKILL) {
        report(
            "the skill",
            fs::create_dir_all(&places.skill).and_then(|()| fs::write(&file, SKILL)),
        );
    }
    if places.claude_home.is_dir() && fs::symlink_metadata(&places.claude).is_err() {
        report(
            "Claude Code's skill",
            (|| {
                fs::create_dir_all(places.claude.parent().unwrap_or(Path::new("/")))?;
                symlink(&places.skill, &places.claude)
            })(),
        );
    }
}

fn remove(places: &Places) {
    if command_is_tap_s(&places.command) {
        let _ = fs::remove_file(&places.command);
    }
    if fs::read_link(&places.claude).is_ok_and(|target| target == places.skill) {
        let _ = fs::remove_file(&places.claude);
    }
    let file = places.skill.join("SKILL.md");
    if fs::read_to_string(&file).is_ok_and(|text| text.contains(MARK)) {
        let _ = fs::remove_file(&file);
        // Only if nothing else was put beside it.
        let _ = fs::remove_dir(&places.skill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home of its own, empty, with a Claude Code folder if asked for.
    fn home(name: &str, claude: bool) -> PathBuf {
        let home = std::env::temp_dir().join(format!("tap-agents-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap_or_default();
        if claude {
            fs::create_dir_all(home.join(".claude")).unwrap_or_default();
        }
        home
    }

    fn app(home: &Path, folder: &str) -> PathBuf {
        let exe = home.join(folder).join("Tap.app/Contents/MacOS/tap");
        fs::create_dir_all(exe.parent().unwrap_or(home)).unwrap_or_default();
        fs::write(&exe, "").unwrap_or_default();
        exe
    }

    #[test]
    fn an_empty_home_gets_the_command_and_the_skill_for_every_agent() {
        let home = home("empty", true);
        let exe = app(&home, "Applications");
        let places = Places::under(&home);
        install(&places, &exe);
        assert_eq!(fs::read_link(&places.command).ok(), Some(exe));
        assert_eq!(
            fs::read_to_string(places.skill.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some(SKILL)
        );
        assert_eq!(fs::read_link(&places.claude).ok(), Some(places.skill));
        assert!(
            SKILL.starts_with("---\nname: tap\n"),
            "a skill's front matter"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn no_claude_code_no_claude_skill() {
        let home = home("noclaude", false);
        let places = Places::under(&home);
        install(&places, &app(&home, "Applications"));
        assert!(places.skill.join("SKILL.md").exists());
        assert!(fs::symlink_metadata(&places.claude).is_err());
        assert!(
            !places.claude_home.exists(),
            "Tap doesn't make Claude Code's folder"
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn a_moved_app_and_a_new_skill_replace_tap_s_own() {
        let home = home("moved", true);
        let places = Places::under(&home);
        install(&places, &app(&home, "old"));
        fs::write(
            places.skill.join("SKILL.md"),
            format!("an older skill\n{MARK}"),
        )
        .unwrap_or_default();
        let exe = app(&home, "Applications");
        install(&places, &exe);
        assert_eq!(fs::read_link(&places.command).ok(), Some(exe));
        assert_eq!(
            fs::read_to_string(places.skill.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some(SKILL)
        );
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn what_isn_t_tap_s_stays_whether_on_or_off() {
        let home = home("foreign", true);
        let places = Places::under(&home);
        fs::create_dir_all(places.command.parent().unwrap_or(&home)).unwrap_or_default();
        fs::write(&places.command, "#!/bin/sh\n").unwrap_or_default();
        fs::create_dir_all(&places.skill).unwrap_or_default();
        fs::write(places.skill.join("SKILL.md"), "someone else's tap skill").unwrap_or_default();
        fs::create_dir_all(places.claude.parent().unwrap_or(&home)).unwrap_or_default();
        symlink(home.join("elsewhere"), &places.claude).unwrap_or_default();

        for _ in 0..2 {
            install(&places, &app(&home, "Applications"));
            remove(&places);
            assert_eq!(
                fs::read_to_string(&places.command).ok().as_deref(),
                Some("#!/bin/sh\n")
            );
            assert_eq!(
                fs::read_to_string(places.skill.join("SKILL.md"))
                    .ok()
                    .as_deref(),
                Some("someone else's tap skill")
            );
            assert_eq!(
                fs::read_link(&places.claude).ok(),
                Some(home.join("elsewhere"))
            );
        }
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn a_same_named_binary_in_another_app_is_not_tap_s() {
        let home = home("otherapp", true);
        let places = Places::under(&home);
        let foreign = home.join("Other.app/Contents/MacOS/tap");
        fs::create_dir_all(foreign.parent().unwrap_or(&home)).unwrap_or_default();
        fs::write(&foreign, "").unwrap_or_default();
        fs::create_dir_all(places.command.parent().unwrap_or(&home)).unwrap_or_default();
        symlink(&foreign, &places.command).unwrap_or_default();

        install(&places, &app(&home, "Applications"));
        assert_eq!(fs::read_link(&places.command).ok(), Some(foreign.clone()));
        remove(&places);
        assert_eq!(fs::read_link(&places.command).ok(), Some(foreign));
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn off_takes_away_only_tap_s_own() {
        let home = home("off", true);
        let places = Places::under(&home);
        install(&places, &app(&home, "Applications"));
        fs::write(home.join(".local/bin/other"), "").unwrap_or_default();
        remove(&places);
        assert!(fs::symlink_metadata(&places.command).is_err());
        assert!(fs::symlink_metadata(&places.skill).is_err());
        assert!(fs::symlink_metadata(&places.claude).is_err());
        assert!(home.join(".local/bin/other").exists());
        assert!(home.join(".agents/skills").is_dir());
        let _ = fs::remove_dir_all(home);
    }
}
