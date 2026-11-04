<p align="center">
  <img src="docs/icon.png" width="128" height="128" alt="Tap app icon">
</p>

<h1 align="center">Tap</h1>

[![CI](https://github.com/timwuhaotian/tap/actions/workflows/ci.yml/badge.svg)](https://github.com/timwuhaotian/tap/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![macOS 14+ · Apple silicon](https://img.shields.io/badge/macOS-14%2B%20%C2%B7%20Apple%20silicon-lightgrey)

A small, fast, quiet web browser for the Mac, written in Rust. Pages run on WebKit, the
engine already inside every Mac (Safari's), so there's no second browser engine to
download, update and keep in memory.

Tap is a port of [Search](https://github.com/driceroland/Search), an MIT-licensed Swift
browser by Office Commun. Search's behaviour is the specification. Tap rewrites all of
it in Rust (AppKit and WebKit through [`objc2`](https://github.com/madsmtm/objc2)),
with its own name and icon, and with fixes of its own.

> **Status: in development.** Nothing to download yet. The first release comes once
> Tap does everything Search does, page translation aside, and passes Search's own
> test suite. [`docs/PLAN.md`](docs/PLAN.md) is the plan, and its parity tracker shows
> where each part stands.

**Download:** [`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg)
(from the first release) · macOS 14 or later · Apple silicon

## What it will do

Everything Search does, including:

- **One field** for addresses and searches, completed from your own history.
- **Tabs** across the top or down the left, pinned tabs, groups, Spaces and profiles, and
  two pages side by side.
- **A content blocker that runs before the page**, in WebKit's network layer.
- **Reading mode, picture-in-picture, hiding any element for good, dark pages.**
- **Passwords and passkeys** in the macOS keychain, and **Chrome extensions** on
  WebKit's own extension engine.
- **Import** from Chrome, Arc, Brave, Edge, Dia, Safari and Firefox, including their
  open tabs and tab groups. Search's own data comes over too.
- **Nothing phones home.** No account, no sync, no telemetry. The only requests Tap
  makes on its own are the pages you open, their icons, and a check for a newer
  release.

Tap also carries three changes first proposed to Search:

- Open tabs and tab groups come over when importing from a Chromium browser
  ([#841](https://github.com/driceroland/Search/pull/841)).
- A whole profile can be exported and imported as one offline file
  ([#845](https://github.com/driceroland/Search/pull/845)).
- A page whose process keeps crashing stops reloading itself and says so
  ([#849](https://github.com/driceroland/Search/pull/849)).

## Building

You need a Mac with Apple silicon, Xcode, and Rust 1.94 or later:

```sh
cargo run -p tap-app          # debug build, straight from the tree
scripts/bundle.sh debug       # or a Tap.app in target/debug (graphite icon)
cargo xtask icon              # redraw the icon PNGs from packaging/icon.svg
```

A build you make yourself is signed ad hoc, so it runs only on your own Mac.

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Found a
security problem? Report it privately, as [SECURITY.md](SECURITY.md) explains.

## License

MIT; see [LICENSE](LICENSE). Tap includes work from Search, © 2026 Office Commun, under
the same license. "Search" and its icon belong to Office Commun; Tap uses neither.
