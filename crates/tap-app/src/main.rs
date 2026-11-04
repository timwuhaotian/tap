//! Tap: a small, fast macOS browser on WebKit, written in Rust (docs/PLAN.md). All of
//! it lives in `tap-shell` (the Mac side) and `tap-core` (the model). Given a command,
//! the same binary is `tap`, the CLI agents drive it with (`cli`).

mod cli;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli::wanted(&args) {
        return cli::run(&args);
    }
    tap_shell::run();
    std::process::ExitCode::SUCCESS
}
