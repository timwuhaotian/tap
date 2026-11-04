# Blank new-tab background — implementation plan

Scope: decorative-only animated backdrop for blank tabs (empty url, no page,
no failure). Approved design: native `BlankView`, quiet isometric signal-bench,
~50 parts, 12 s calm loop, still under Reduce Motion. No model changes.

Base: `origin/main`. Worktree: `.worktrees/feature-blank-new-tab-background`.

## Tasks (each: failing check → minimal code → pass → commit)

1. `crates/tap-shell/src/blank.rs` (new): pure geometry — `needle_angle(t)`,
   `drift(t)`, `fits(size)` helpers + unit tests. Fails: no file. Pass:
   `cargo test -p tap-shell blank`.
2. Same file: `BlankView` NSView (`define_class!`, flipped), `drawRect:`
   draws GROUND + instrument in MUTED/HAIRLINE/INK + one lit dot; dark via
   `palette::is_dark`. Manual: debug build shows it.
3. Same file: motion — `set_time(t)` + `setNeedsDisplay`; still frame when
   `motion::reduced()`; tolerance-friendly 30 fps NSTimer owned by `window.rs`,
   only while a blank tab is shown (start/stop in `show_page`). Test: timer
   absent when tab has a url.
4. `window.rs`: own a `BlankView` in `Views`/stage; `show_page` shows it iff
   `view.blank && view.page.is_none() && view.failure.is_none()`; resize with
   stage. Existing tests untouched.
5. Verify: `rustup run nightly cargo fmt --all` (per CONTRIBUTING),
   `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo test --workspace --locked` — all green in worktree.
6. Eyeball: `cargo run -p tap-app`, ⌘T → animation behind field; light + dark;
   Reduce Motion → still. Note results in PR body (animation feel can't be
   unit-tested, per CONTRIBUTING).

## Commands (in worktree)

- `cargo test -p tap-shell blank`
- `rustup run nightly cargo fmt --all`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`

## Notes for reviewer

- Search parity: Search leaves blank tabs empty ground; this is a deliberate
  `fixed-deviation` (ambience, no behaviour change). PLAN.md Appendix A row
  to be added on `main` if wanted — kept out of this branch to avoid conflicts.
- This `docs/plans/` file exists because feature-make requires it; drop at
  merge if the single-plan rule should hold.
