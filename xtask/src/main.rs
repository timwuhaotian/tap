//! Tap's development tooling, run as `cargo xtask <command>` (docs/PLAN.md §4).
//!
//! `icon` renders `packaging/icon.svg` into the PNGs the app and the README use:
//! `packaging/app-icon.png` (release builds), `packaging/app-icon-dev.png` (debug builds,
//! on a graphite plate so a local build is never mistaken for the installed one) and
//! `docs/icon.png`. `scripts/bundle.sh` turns the PNGs into `AppIcon.icns`.
//!
//! `search-profile <file.tapprofile>` packs Search's own profile on this Mac (its tabs and
//! groups, history, bookmarks and allowlisted settings) as a Tap profile file, for File ›
//! Import Profile… to bring in. Search's files and settings are only read.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use resvg::{tiny_skia, usvg};

/// The release plate's gradient, and the graphite one a debug build wears instead.
const PLATE: [&str; 2] = ["#FFB238", "#FF5A36"];
const DEV_PLATE: [&str; 2] = ["#7A7F87", "#2F3237"];
/// The mark's own shadow is tinted for the warm plate; on graphite it is plain black.
const LIFT: &str = "#7A1E00";
const DEV_LIFT: &str = "#000000";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("icon") => icon(),
        Some("search-profile") => search_profile(args.get(1)),
        _ => Err("usage: cargo xtask icon | search-profile <file.tapprofile>".to_owned()),
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

fn search_profile(path: Option<&String>) -> Result<(), String> {
    let path = PathBuf::from(path.ok_or("usage: cargo xtask search-profile <file.tapprofile>")?);
    let said = tap_shell::write_search_profile(&path)?;
    println!("{}: {said}", path.display());
    Ok(())
}

fn icon() -> Result<(), String> {
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
    write(&release, &root.join("packaging/app-icon.png"))?;
    write(&release, &root.join("docs/icon.png"))?;
    write(&render(&dev)?, &root.join("packaging/app-icon-dev.png"))?;
    Ok(())
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
