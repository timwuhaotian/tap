# Contributing to Tap

Thanks for helping. Bug reports, fixes and ports are all welcome. For anything larger than
a fix, open an issue first so we can agree on the shape before you write the code.

## Setup

You need a Mac with Apple silicon, Xcode, and Rust 1.94 or later. The formatter needs a
nightly toolchain for its import options:

```sh
rustup toolchain install nightly --component rustfmt
cargo run -p tap-app             # debug build, straight from the tree
scripts/bundle.sh debug          # or a Tap.app in target/debug
```

To compare behaviour, keep a checkout of [Search](https://github.com/driceroland/Search)
next to this one. Tap is ported from it, and its tests are Tap's parity gate.

## Before you open a pull request

CI runs these on Apple silicon; run them first:

```sh
cargo +nightly fmt --all          # with Homebrew's cargo first on PATH: rustup run nightly cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## How we work

- **[`docs/PLAN.md`](docs/PLAN.md) is the plan and the spec.** It's the only plan
  document. A change in direction, a decision, a milestone or a parity status is
  recorded there, in the same pull request.
- **Search is the reference.** A port does what Search does at the plan's pinned
  version, and says which Swift file it came from. Where Tap deliberately differs (a
  fix, a dropped feature), the parity tracker in the plan marks it
  `fixed-deviation` or `dropped`, with the reason.
- **Every fix comes with a test** that fails without it, wherever the behavior can be
  tested: pure logic in `tap-core`, WebKit in `tap-web`'s main-thread tests, or the app
  through the bench socket. Things only a person can check, like Pinyin input or how an
  animation feels, go in the pull request's description.
- **Keep the layers.** `tap-core` and `tap-import` have no AppKit, no WebKit and no
  `unsafe`. Objective-C and C calls live in `tap-sys`, `tap-ui`, `tap-web` and
  `tap-shell`, each `unsafe` block with a `SAFETY` comment.
- **Never call AppKit or WebKit while holding the app state's borrow.** The model
  returns effects, and the shell runs them (plan §5.1).
- **Comments explain why**, not what. Match the density and style of the code around you.
- **Commit messages** start with a short summary of what now happens ("A page that keeps
  crashing stops reloading itself"), then explain the problem and the fix.
- **Licensing:** Tap is MIT, and so is Search. Keep Office Commun's notice in
  [LICENSE](LICENSE). Don't copy code under licenses that don't fit MIT (GPL, for
  example). Tap links the system's WebKit and doesn't include its source.

## Releases

Releases are built, signed and notarized locally by the maintainer, following plan §12.
Each one replaces `releases/Tap-arm64.dmg`, bumps the version in `Cargo.toml`, and dates
the `Unreleased` entries in [`CHANGELOG.md`](CHANGELOG.md).

By contributing, you agree that your contributions are licensed under the MIT license,
and to follow the [code of conduct](CODE_OF_CONDUCT.md).
