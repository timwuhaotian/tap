## What and why

<!-- What this changes, and the problem it solves. Link the issue if there is one. -->

## Parity

<!-- The Search file(s) this ports or changes, and whether Tap now matches, deliberately
     differs (and why), or drops it. Update the parity tracker in docs/PLAN.md. -->

## How it was checked

<!-- Tests added or run, and anything checked by hand in the app (IME, animations, restore...). -->

- [ ] `cargo +nightly fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `docs/PLAN.md` / `README.md` updated if behavior or plans changed
