//! Tap's development tooling, run as `cargo xtask <command>` (docs/PLAN.md §4).
//!
//! `icon` renders `packaging/icon.svg` into the PNGs the app and the README use:
//! `packaging/app-icon.png` (release builds), `packaging/app-icon-dev.png` (debug builds,
//! on a graphite plate so a local build is never mistaken for the installed one) and
//! `docs/icon.png`. `scripts/bundle.sh` turns the PNGs into `AppIcon.icns`.
//!
//! `check` is the release gate's own step (plan §12, "Releasing"). It checks that the
//! committed icon PNGs are what `packaging/icon.svg` draws, without rewriting them, and
//! that the parity tracker (plan Appendix A) lists every Swift file of Search at the pin
//! exactly once, with its line count. Search is read from `$SEARCH_DIR`, by default
//! `~/code/Search`, and is never changed.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use resvg::{tiny_skia, usvg};

/// The release plate's gradient, and the graphite one a debug build wears instead.
const PLATE: [&str; 2] = ["#FFB238", "#FF5A36"];
const DEV_PLATE: [&str; 2] = ["#7A7F87", "#2F3237"];
/// The mark's own shadow is tinted for the warm plate; on graphite it is plain black.
const LIFT: &str = "#7A1E00";
const DEV_LIFT: &str = "#000000";

/// Search's `main` at parity-pin, before the user's PRs: the tracker's line counts for
/// Search's own files are read here (plan §1).
const SEARCH_PIN: &str = "d1639aa";
/// The reference build, which carries the PRs: a tracker row marked `(#nnn branch)` is a
/// file only a PR adds, and is read here (plan §3, item 7).
const SEARCH_REFERENCE: &str = "6d6effd";
const SEARCH_SOURCES: &str = "Sources/Search";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("icon") => icon(),
        Some("check") => check(),
        _ => Err("usage: cargo xtask icon | check".to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace root: xtask's own manifest is one level below it.
fn root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map_or(manifest.clone(), Path::to_path_buf)
}

fn icon() -> Result<(), String> {
    for (pixmap, path) in icons()? {
        write(&pixmap, &path)?;
    }
    Ok(())
}

/// Each PNG `icon` writes, with what it should hold.
fn icons() -> Result<Vec<(tiny_skia::Pixmap, PathBuf)>, String> {
    let root = root();
    let source = root.join("packaging/icon.svg");
    let svg = fs::read_to_string(&source).map_err(|err| format!("{}: {err}", source.display()))?;
    for colour in PLATE.iter().chain([&LIFT]) {
        if !svg.contains(colour) {
            return Err(format!(
                "{} no longer uses {colour}; update xtask's colours",
                source.display()
            ));
        }
    }
    let dev = svg
        .replace(PLATE[0], DEV_PLATE[0])
        .replace(PLATE[1], DEV_PLATE[1])
        .replace(LIFT, DEV_LIFT);

    let release = render(&svg)?;
    Ok(vec![
        (release.clone(), root.join("packaging/app-icon.png")),
        (release, root.join("docs/icon.png")),
        (render(&dev)?, root.join("packaging/app-icon-dev.png")),
    ])
}

fn check() -> Result<(), String> {
    let mut problems = check_icons()?;
    problems.extend(check_tracker()?);
    if problems.is_empty() {
        println!("xtask check: icons and parity tracker are up to date");
        return Ok(());
    }
    for problem in &problems {
        eprintln!("  {problem}");
    }
    Err(format!("check found {} problem(s)", problems.len()))
}

fn check_icons() -> Result<Vec<String>, String> {
    let root = root();
    let mut problems = Vec::new();
    for (pixmap, path) in icons()? {
        let want = pixmap
            .encode_png()
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let shown = path.strip_prefix(&root).unwrap_or(&path).display();
        if fs::read(&path).ok().as_deref() != Some(want.as_slice()) {
            problems.push(format!(
                "{shown} differs from packaging/icon.svg: run `cargo xtask icon`"
            ));
        }
    }
    Ok(problems)
}

/// A tracker row: the Swift file, whether it is a PR's file, and the line count it gives.
struct Row {
    pr_branch: bool,
    lines: u64,
}

fn check_tracker() -> Result<Vec<String>, String> {
    let plan_path = root().join("docs/PLAN.md");
    let plan =
        fs::read_to_string(&plan_path).map_err(|err| format!("{}: {err}", plan_path.display()))?;
    let (rows, total, mut problems) = tracker_rows(&plan)?;

    let search = env::var_os("SEARCH_DIR").map_or_else(
        || PathBuf::from(env::var_os("HOME").unwrap_or_default()).join("code/Search"),
        PathBuf::from,
    );
    let pinned = swift_files(&search, SEARCH_PIN)?;
    let reference = swift_files(&search, SEARCH_REFERENCE)?;

    for name in pinned.keys() {
        if !rows.contains_key(name) {
            problems.push(format!(
                "{name} is in Search at {SEARCH_PIN} but not in the tracker"
            ));
        }
    }
    let mut sum = 0;
    for (name, row) in &rows {
        sum += row.lines;
        let (files, rev) = if row.pr_branch {
            (&reference, SEARCH_REFERENCE)
        } else {
            (&pinned, SEARCH_PIN)
        };
        match files.get(name) {
            None => problems.push(format!(
                "{name} is in the tracker but not in Search at {rev}"
            )),
            Some(&lines) if lines != row.lines => problems.push(format!(
                "{name}: the tracker says {} lines, Search at {rev} has {lines}",
                row.lines
            )),
            Some(_) => {}
        }
    }
    match total {
        None => problems.push("the tracker has no `**n files** | **lines**` total row".to_owned()),
        Some((files, lines)) => {
            if files != rows.len() as u64 || lines != sum {
                problems.push(format!(
                    "the tracker's total says {files} files and {lines} lines; its rows add up to {} and {sum}",
                    rows.len()
                ));
            }
        }
    }
    Ok(problems)
}

/// Reads Appendix A's table: its file rows, its total row and any duplicates.
type Tracker = (BTreeMap<String, Row>, Option<(u64, u64)>, Vec<String>);

fn tracker_rows(plan: &str) -> Result<Tracker, String> {
    let appendix = plan
        .split_once("\n## Appendix A")
        .ok_or("docs/PLAN.md has no Appendix A")?
        .1;
    let appendix = appendix.split_once("\n## ").map_or(appendix, |(it, _)| it);
    let mut rows = BTreeMap::new();
    let mut total = None;
    let mut problems = Vec::new();
    for line in appendix.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let [_, file, lines, ..] = cells.as_slice() else {
            continue;
        };
        let number = |cell: &str| {
            cell.trim_matches('*')
                .split(' ')
                .next()?
                .parse::<u64>()
                .ok()
        };
        if let (Some(files), Some(lines)) = (number(file), number(lines))
            && file.ends_with("files**")
        {
            total = Some((files, lines));
            continue;
        }
        let Some((name, rest)) = file.split_once(".swift") else {
            continue;
        };
        let name = format!("{name}.swift");
        let Ok(count) = lines.parse() else {
            problems.push(format!(
                "{name}: the tracker's line count `{lines}` isn't a number"
            ));
            continue;
        };
        let row = Row {
            pr_branch: rest.contains("branch)"),
            lines: count,
        };
        if rows.insert(name.clone(), row).is_some() {
            problems.push(format!("{name} is in the tracker more than once"));
        }
    }
    Ok((rows, total, problems))
}

/// Each Swift file directly in Search's sources at `rev`, with its line count.
fn swift_files(search: &Path, rev: &str) -> Result<BTreeMap<String, u64>, String> {
    let git = |args: &[&str]| -> Result<Vec<u8>, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(search)
            .args(args)
            .output()
            .map_err(|err| format!("git: {err}"))?;
        if !out.status.success() {
            return Err(format!(
                "git -C {} {}: {} (set SEARCH_DIR to a Search checkout that has {rev})",
                search.display(),
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(out.stdout)
    };
    let listing = git(&["ls-tree", "--name-only", rev, &format!("{SEARCH_SOURCES}/")])?;
    let mut files = BTreeMap::new();
    for path in String::from_utf8_lossy(&listing).lines() {
        let Some(name) = path
            .rsplit('/')
            .next()
            .filter(|it| Path::new(it).extension().is_some_and(|ext| ext == "swift"))
        else {
            continue;
        };
        let text = git(&["show", &format!("{rev}:{path}")])?;
        // Counted as `wc -l` counts them.
        let lines = String::from_utf8_lossy(&text).matches('\n').count() as u64;
        files.insert(name.to_owned(), lines);
    }
    Ok(files)
}

/// Renders an icon SVG at its own size (1024 × 1024).
fn render(svg: &str) -> Result<tiny_skia::Pixmap, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|err| format!("icon.svg: {err}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap =
        tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("icon.svg has no size")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    Ok(pixmap)
}

fn write(pixmap: &tiny_skia::Pixmap, path: &Path) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|err| format!("{}: {err}", folder.display()))?;
    }
    pixmap
        .save_png(path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    println!("{}", path.display());
    Ok(())
}
