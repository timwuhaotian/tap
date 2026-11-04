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
- **Settings** (⌘,), laid out as Search's, with only the settings that work — 53 of
  its 81 rows: where new tabs go, what closing one lands on, background links that wait,
  closing tabs you leave alone, a fresh window at launch, sleeping tabs, the search
  engine (with suggestions and site shortcuts), "settings" or "new tab" typed in the
  field, asking where to save, videos that wait for a click, page zoom, light or dark
  (and pages that stay light, darkened), downloads, default browser, clearing data,
  updates. Blocking ads and trackers, reading mode (⇧⌘R), the floating video,
  120 Hz pages, middle-button scrolling and the tab bar's icons, reading progress
  and ⌘ numbers are all in there too.
- **Bring things over** from the other browsers on this Mac, found on their own:
  Settings › General lists Search and the Chromium browsers (Chrome, Arc, Brave, Edge,
  Dia, Vivaldi, Opera and more), and one pick brings in its open tabs and their groups
  ([#841](https://github.com/driceroland/Search/pull/841)), history and bookmarks.
  Search's settings come too, with no export first. Nothing in the other browser
  changes, and tabs already open aren't opened twice.
- **Your profile on your other Mac.** File › Export Tap Profile… writes one offline
  file of your tabs and their groups, history, bookmarks and settings, and offers it to
  AirDrop; open it on the other Mac and it all comes in, groups folded or open as they
  were. Search's profile files
  ([#845](https://github.com/driceroland/Search/pull/845)) come in too, and
  `cargo xtask search-profile` packs the Search on this Mac as one. Sign-ins,
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
  making and naming groups, Spaces and profiles, two pages side by side, peeking at
  a link and the small window for links from other apps.
- **Hiding any element for good**, and Search's fuller dark-pages engine (0.1.3
  darkens a light page's colours; it doesn't sample the page as Search does).
- **Saving and filling passwords, and passkeys**, in the macOS keychain (Settings ›
  Passwords opens the Passwords app meanwhile), and **Chrome extensions** on
  WebKit's own extension engine.
- **More to bring over**: Safari and Firefox, choosing which profile of a browser
  comes (it's the one used last for now), its passwords and extensions, and Arc's
  spaces and pins.

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
