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

> **Status: early.** Tap browses well and can be your Mac's browser, but much of
> Search isn't in yet (below). It updates itself; if yours never offers an update,
> download it once more. [`docs/PLAN.md`](docs/PLAN.md) is the plan; its parity tracker
> shows where each part stands.

**Download:** [`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg)
· macOS 14 or later · Apple silicon · signed and notarized by Apple

## What it does now

- **One field** for addresses and searches. It finishes addresses from your own
  history as you type, with the rest drawn grey after the caret (Tab takes it), and
  leaves pinyin and kana alone while an input method is composing.
- **Tabs across the top**, with the traffic lights in the row: ⌘T, ⌘W, ⇧⌘T, ⌃Tab,
  ⌘1–⌘9, links opened behind the page with ⌘-click, each tab with its site's icon.
- **Tab groups**: a group's name heads its tabs, and a click folds it away or opens it.
- **Settings** (⌘,), laid out as Search's: every setting is there to find, and the ones
  Tap acts on already (search engine, sleeping tabs, light or dark, downloads folder,
  default browser, clearing data, updates) work from there.
- **Your profile on your other Mac.** File › Export Tap Profile… writes one offline
  file of your tabs and their groups, history, bookmarks and settings, and offers it to
  AirDrop; open it on the other Mac and it all comes in, groups folded or open as they
  were. Search's profile files
  ([#845](https://github.com/driceroland/Search/pull/845)) come in too. Sign-ins,
  passwords and cookies never go in the file.
- **It updates itself**: checked against Apple's notarization and the developer's
  signature, installed when Tap quits, or at once with Restart Now.
- **Tabs come back** where you left them, and cost nothing until you look at them.
- **Tabs you aren't using sleep** after half an hour and give their memory back; they
  wake on the same page, with Back and the scroll position. Never a tab playing sound,
  on a call, downloading or holding something you typed.
- **Find on page** (⌘F) with a live "3 of 17", Match Case and Whole Words.
- **Downloads** into Downloads, under their own names.
- **Pages can ask**: alerts, confirms, prompts, file uploads and site sign-ins, each
  titled with the site that asks.
- **It can be your browser.** Choose Tap under System Settings › Desktop & Dock ›
  Default web browser; links from other apps open in it, and so do HTML files.
- **A page that keeps crashing stops reloading itself** and says so
  ([#849](https://github.com/driceroland/Search/pull/849)).
- **Nothing phones home.** No account, no sync, no telemetry. The only requests Tap
  makes on its own are the pages you open, their icons, and a look at this
  repository's `releases/latest.toml` every six hours for an update (Settings › About
  turns that off).

## Still to come

Everything else Search does, including:

- Bookmarks, the History page, private windows, and tabs down the side, pinned tabs,
  making and naming groups, Spaces and profiles, two pages side by side.
- **A content blocker that runs before the page**, in WebKit's network layer.
- **Reading mode, picture-in-picture, hiding any element for good, dark pages.**
- **Passwords and passkeys** in the macOS keychain, and **Chrome extensions** on
  WebKit's own extension engine.
- **Import** from Chrome, Arc, Brave, Edge, Dia, Safari and Firefox, including their
  open tabs and tab groups ([#841](https://github.com/driceroland/Search/pull/841)).
  Search's own data comes over too, without exporting it first.

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
