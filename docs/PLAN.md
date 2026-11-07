# Tap — execution plan: rewrite Search in Rust

> The single planning document for Tap. Every planning change, decision, parity
> status and perf result goes **here**; there are no other plan docs.
> Plan v1, 2026-11-04. The reference is Search at **parity-pin**: `main`
> `d1639aa`, plus #845, plus #849 (§9). The release flow and credentials are copied
> from Kil (`~/code/kil`, §12).

## 1. Context

**Tap** is a new, fast macOS browser built on WebKit. It copies the behaviour of
[Search](/Users/wuhaotian/code/Search) (Office Commun, MIT, v1.0.8; about 75k lines
of Swift in 139 files under `Sources/Search/`). It is rewritten entirely in Rust,
with its own name, identity and icon.

The user has three open upstream PRs on driceroland/Search. Their behaviour is part
of Tap from the start:

| PR | What it does | Where it is in Search | Tap milestone |
|---|---|---|---|
| **#841** | Brings a Chromium browser's open tabs and tab groups over, through an SNSS session parser | local `main` commit `d1639aa`, `ImportSession.swift` | parser M1, wiring M6 |
| **#845** | Exports and imports a whole profile as one offline zip | branch `export-search-profile` (built on #841): `ImportFile.swift`, `SearchProfile.swift` | M6 |
| **#849** | A page whose WebContent process keeps crashing stops reloading itself and says so: `CrashCount`, at most 2 automatic reloads a minute | branch `feature/fix-self-reload-blank-page`; merges cleanly with #845 | M2 (side panel M7) |

## 2. Decisions

**2026-11-05 import decision:** Browser imports offer only Chrome and Safari. Safari
uses an export ZIP chosen by the user; Tap profile backups and password CSV imports
remain. Search discovery, profile-file registration, and lazy Search password access
are removed. Earlier descriptions of those imports below are historical, not current.

### Made by the user (not to be revisited)

| Topic | Decision |
|---|---|
| Language | **Pure Rust, no Swift at all.** Features that need Swift-only Apple APIs are dropped. Today that means page translation (`Translate.swift` uses `TranslationSession`). |
| Engine | **WebKit**, the system `WKWebView`, driven from Rust through `objc2`. The UI is AppKit. |
| Scope | **Full parity with Search before 1.0** (minus translation). Changed 2026-11-04: 0.1.0 ships once M2 is done (§15). |
| Signing | **Developer ID Application: HAOTIAN WU (43VLF3KTFZ)**, the identity in the login keychain that Kil signs with. Signing, notarization, the passkeys entitlement and the updater are set up from the start. |
| Release flow | **Copied from Kil** (`~/code/kil/RELEASING.md`, `scripts/release.sh`, `crates/kil/src/update.rs`). Releases are built, signed and notarized locally; CI never builds them. Each release overwrites one file in git, `releases/Tap-arm64.dmg`. The updater reads `releases/latest.toml` on `main`. Versions are for maintainers, not users. As with Kil, only **arm64** is released (§12). |
| Icon | **"Bolt T"**: a heavy T whose stem breaks into a lightning bolt, on a warm gradient plate. Drawn in code (§13). |
| Location | `/Users/wuhaotian/code/tap` |

### Why WebKit and AppKit (alternatives considered)

- **Servo** (`servo` 0.7.0, MPL-2.0) as the engine. Search's core features are WebKit
  features, and Servo has no equivalent for them:
  - Chrome extensions (`WKWebExtension`)
  - passkeys (browser passkey entitlement plus AuthenticationServices)
  - the ad blocker (`WKContentRuleList`, enforced in WebKit's network process)
  - picture-in-picture
  - Apple Passwords

  Losing these breaks the full-parity decision. Real-site compatibility is behind
  WebKit's, the JS engine is C++ (SpiderMonkey) and video goes through GStreamer, the
  app would ship the whole engine (Search is about 9 MB because WebKit comes with
  macOS), and Tap would own engine security patches.
- **egui / iced** for the chrome. Search draws overlays above the page (omnibox, ⌘K,
  find bar, site card), which is awkward over a native WKWebView. Their input-method
  support (pinyin in the address field) and VoiceOver support are weaker than
  AppKit's.
- **GPUI**, as Kil uses. It's a good fit for Kil, which draws everything itself. A
  browser's main content is a native `WKWebView`, though, and GPUI draws into its own
  Metal layer. GPUI chrome can't sit above the page, and it brings the same input-method
  and VoiceOver gaps. Kil's *release flow* is reused, not its UI stack.
- **Leptos / Dioxus.** They build web apps that run inside a browser, not a browser.
  Dioxus desktop on macOS draws through WKWebView anyway.
- **Kept open:** `tap-core` stays free of engine dependencies, so a Servo experiment
  after 1.0 could reuse the models, importers and rules. No engine abstraction is
  built before then.

### Defaults chosen in this plan (change here if wrong)

- Bundle id `com.timwuhaotian.tap`. Every derived identifier is
  `com.timwuhaotian.tap.*` (§6).
- A fresh git repo with no Swift history. Search stays at `~/code/Search` as the
  reference implementation.
- `LICENSE` keeps Office Commun's MIT notice, because the port is a derivative work,
  and adds Tap's. The name "Search" and its icon are not used anywhere.
- No localization, as in Search: English only.

## 3. What reading Search changed

1. **Internal pages are native views, not web content.** Search has no
   `WKURLSchemeHandler`. Settings, History, Downloads, Bookmarks, Passwords, Import and
   Notes are SwiftUI views drawn where the page would be (`InternalPages.swift:4-13`).
   `search://` is caught before navigation (`Browser.swift:6106`). Tap does the same
   with `tap://`: native pages, no scheme handler, so saved passwords never enter a
   WebContent process.
2. **The model is also the WebKit delegate.** `extension Browser: WKNavigationDelegate,
   WKUIDelegate` is at `Browser.swift:6217`, and `WKDownloadDelegate` at `:7226`. Tap
   splits model and delegate apart (§5.1–5.2).
3. **On-disk format.** Dates are a Double of seconds since 2001-01-01 (Swift's
   `JSONEncoder` default), and UUIDs are uppercase. Tap's serde layer matches this
   (§7).
4. **URL semantics are a hidden parity risk.** There are 116 uses of `URL(string:)` and
   `URLComponents`, and the tests compare `absoluteString` exactly:
   `Tests/AddressRegression` expects `https://example.com` with no trailing slash. The
   WHATWG `url` crate writes `https://example.com/`. **Tap wraps NSURL and
   NSURLComponents (`tap_core::fnd::Url`) and does not use the `url` crate.**
5. **Two real IME bugs in Search's address field**, which Tap fixes as documented
   deviations:
   - `controlTextDidChange` rewrites `stringValue` without checking `hasMarkedText`.
   - `select(from: typed.count)` mixes grapheme counts with UTF-16 `NSRange`
     (`Omnibox.swift:536-566`).

   Both break pinyin input.
6. **Toolchain on this Mac.** Homebrew `rustc`/`cargo` 1.94 come first on PATH; Kil
   builds with them. Tap does the same:
   - `rust-version = "1.94"` and edition 2024, with no `rust-toolchain.toml`.
   - Formatting through `rustup run nightly cargo fmt`, because `+nightly` doesn't work
     with Homebrew's cargo.
   - Releases are arm64 only, so the missing x86_64 std doesn't matter.

   Plain `xcrun --show-sdk-version` fails; `--sdk macosx` gives 26.5.
7. **The reference build is Search's `main` on the user's fork** (`timwuhaotian/Search`:
   upstream `3279d28` + #841 + #845 with its `spaces.json` fix + #849, pushed
   2026-11-04 as `6d6effd`; it replaces the local `parity-pin` branch), built to
   `build/Search.app`. The installed `/Applications/Search.app` (1.0.8,
   8.95 MB arm64) does not have the PRs.
8. **Python tests leak Search's JS names and literals.** Examples:
   - `"world": "search"` in four tests
   - `window.__officeDusk` in `dark_pages.py`
   - `SEARCH_MEASURE` in `call_background.py:98` and `memory_close.py:82`
   - the `"/Search ("` probe in `split_view.py:127`

   The harness patch handles these (§9).

## 4. Workspace layout

```
tap/
  Cargo.toml              workspace ([workspace.package] version = the release version), profiles (§10)
  .cargo/config.toml      alias xtask; MACOSX_DEPLOYMENT_TARGET=14.0; arm64 rustflags
  CHANGELOG.md, README.md as in Kil: "## Unreleased" on top; README links the DMG exactly (§12)
  CLAUDE.md               agent notes, as in Kil: work on main; "release" means follow §12 step by step
  .github/workflows/ci.yml  fmt, clippy, tests on macos-15 (as Kil's); never builds releases
  scripts/bundle.sh       [debug|release] → target/<profile>/Tap.app, ad-hoc sealed (port of Kil's)
  scripts/release.sh      sign, notarize, staple, DMG, latest.toml (port of Kil's, §12)
  packaging/              Info.plist (__VERSION__ template), tap.entitlements, tap.passkeys.entitlements,
                          Tap.sdef, icon.svg, app-icon.png, app-icon-dev.png
                          (Tap.provisionprofile is git-ignored)
  releases/               Tap-arm64.dmg, Tap-arm64.dmg.sha256, latest.toml (tracked in git)
  assets/js/              JS copied verbatim from Search (§5.4) and manifest.toml
  crates/
    tap-core/    model, logic, file formats, policies, bench framing  (#![forbid(unsafe_code)])
    tap-import/  every importer, #841 SNSS, #845 profile, "from Search"  (no AppKit or WebKit)
    tap-assets/  include_str! of assets/js, template filling
    tap-sys/     thin unsafe FFI: Security, LAContext, Carbon TIS, IOKit, os_log, CFPreferences, dlsym SPI, proc_pid_rusage
    tap-ui/      AppKit kit: TapView, layout, springs and transitions, menus, tables, text field, AX
    tap-web/     WebKit glue: config, PageView, delegates, relays, KVO, SPI catalogue, stores, shield, downloads, extensions, passkeys
    tap-shell/   NSApplication subclass, windows, menus, scripting, updater, about 60 UI components, run loop, bench server
    tap-app/     main.rs (under 50 lines)
  xtask/                  dev tooling: icon, JS extraction, parity, perf, check (table below)
  tests/parity/           Search's Tests/*.py vendored at parity-pin, plus harness.patch
  perf/results/           perf run data (JSON); the summary goes in §10 of this doc
  docs/PLAN.md            this document
```

Dependencies: `tap-core` ← `tap-import`; `tap-core` ← `tap-sys` ← `tap-web` ←
`tap-shell` ← `tap-app`; `tap-ui` ← `tap-shell`. Splitting the glue across tap-web
and tap-shell keeps incremental builds fast.

**tap-core and Foundation.** tap-core may use Foundation value types, but only through
`tap_core::fnd`. It means no AppKit, no WebKit and no main-thread requirement; it
doesn't mean no Foundation. `fnd` wraps:
- `NSURL` and `NSURLComponents` (finding 3.4)
- `DateFormatter`, `RelativeDateTimeFormatter` and `ByteCountFormatter`
- `localizedCaseInsensitiveContains`, `localizedStandardCompare` and `folding`

`cargo test` runs all of it on macOS with no app.

**Building and releasing** use Kil's two shell scripts, ported:

- **`scripts/bundle.sh [debug|release]`** builds with `cargo` and assembles
  `target/<profile>/Tap.app`:
  - copies the binary and `Tap.sdef`
  - fills `packaging/Info.plist`'s `__VERSION__` from `cargo pkgid`
  - makes `AppIcon.icns` from `packaging/app-icon.png` with `sips` and `iconutil`
    (`app-icon-dev.png` for debug)
  - seals the app ad hoc

  Two additions over Kil's script:
  - It checks that `otool -l` reports minos 14.0 and the current SDK. If the linker
    recorded an older SDK, it applies `vtool -set-build-version macos 14.0 <sdk>`, as
    Search's `build.sh:121-129` does; otherwise AppKit draws old looks.
  - Release builds run `dsymutil` and then `strip -x`.
- **`scripts/release.sh`** signs, notarizes, staples, makes the DMG and writes
  `latest.toml` (§12).

**xtask** handles development tooling only. It always sets
`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` and uses
`xcrun --sdk macosx`.

| Command | What it does |
|---|---|
| `icon` | Renders `packaging/icon.svg` (the source of truth) with `resvg` into `packaging/app-icon.png` (1024²), `app-icon-dev.png` (graphite plate) and `docs/icon.png` (README). The files are committed, as Kil's are, and CI fails if they drift from the SVG |
| `check` | The release gate's own step (§12): the committed icon PNGs match `packaging/icon.svg` (nothing is rewritten), and the parity tracker (Appendix A) lists every Swift file in Search's `Sources/Search` exactly once with its line count — read at `d1639aa`, or at the reference build `6d6effd` for a row marked `(#nnn branch)`. Search comes from `$SEARCH_DIR`, by default `~/code/Search` |
| `fresh` | Port of Search's `fresh.sh`: a first launch in a wiped test world |
| `extract-js [--check]`, `sync-tests` | JS assets (§5.4) and the vendored Python suite (§9) |
| `parity`, `diff-run`, `ax-dump`, `perf` | Parity and perf tooling (§9, §10) |
| `check` | fmt, clippy `-D warnings`, all tests, `extract-js --check`, and completeness of the parity tracker (Appendix A) |

## 5. Architecture

### 5.1 What replaces SwiftUI and Combine

Today, Search's state is big `ObservableObject`s: `Browser` has 66 `@Published`
properties, `Preferences` 86, `Tab` 31. Logic sits in `didSet`, and about 40 Combine
and NotificationCenter relays connect it. SwiftUI coalesces all of it into one update
per run-loop turn.

**Tap: a functional core and an imperative shell, with field-level dirty bits.**

1. **One owner, main thread only.**
   - `tap_core::App` holds `Shared` (prefs, profiles, spaces, pins, history and
     bookmarks per profile, curtain, shield, extension registry),
     `windows: IndexMap<WinId, Browser>` and `tabs: HashMap<TabId, Tab>`.
   - Tabs live in one map because they move between windows and spaces.
   - The shell keeps `thread_local! APP: RefCell<App>` and reaches it only through
     `rt::dispatch(mtm, |app| …)`.
   - **Rule: never call AppKit or WebKit while holding the borrow.** A re-entrant
     dispatch is deferred until after the outer one (with `debug_assert!`).
     Synchronous queries such as `validateMenuItem:` use `try_borrow`.
2. **Observable structs.** `macro_rules! observable!` generates, for each struct:
   - a bitflags `XxxField` type (u128 for Browser and Prefs)
   - getters
   - `set_x()`, which marks the bit only on a real change

   Swift's `didSet` blocks become explicit model methods. For example,
   `Browser::set_active` holds `Browser.swift:60-105`.
3. **Effects, not calls.** The model never touches a view or a web view. It pushes
   values such as `Effect::Page { tab, op: Load | Eval{js, world, reply} | Reload |
   Snapshot | SetMuted … }`, `Effect::Window`, `Effect::Save(File)` and `Effect::Ext`.
   - The shell runs them after the borrow is released.
   - Async replies are `Box<dyn FnOnce(&mut App, Result<Json>)>`.
   - Background work (disk, imports, icons) runs on std threads and returns through
     `dispatch2`.
   - Disk writes go through one serial writer that drops writes a newer one has
     overtaken (a port of `Disk`, `Store.swift:205-235`).
4. **One flush per run-loop turn.**
   - A `CFRunLoopObserver` (`BeforeWaiting | Exit`, **common modes**, so it also fires
     during menus, drags and live resize) runs at **order 1_999_000**, just before
     Core Animation's commit.
   - It gathers the dirty bits and calls each subscribed component at most once, then
     clears the bits. This is the same coalescing SwiftUI does.
5. **Components.** Each Swift view file becomes one Rust module in
   `tap-shell/src/ui/`, holding retained NSViews:

   ```rust
   pub trait Component {
       fn interest(&self) -> Interest; // e.g. window(w, TABS|ACTIVE).any_tab(TITLE|ICON).prefs(SIDEBAR)
       fn update(&mut self, app: &App, dirty: &Dirty, ui: &mut UiCx); // plays the role of `body`
   }
   ```

   Lists (tab strip, column, bookmarks bar, switcher) use
   `tap_ui::KeyedList<TabId, Row>`: reuse, insert, remove and move by key, with
   transitions. It is the one diffing primitive needed, standing in for `ForEach`.
6. **Animation is part of the change.**
   - `app.animate(Motion::Glide, |app| …)` tags the fields changed inside the closure,
     and the component animates with `NSAnimationContext` and
     `CASpringAnimation(perceptualDuration:bounce:)`.
   - SwiftUI `spring(response r, dampingFraction d)` maps to
     `(perceptualDuration r, bounce 1−d)`: `glide` (0.34, 0.82) is (0.34, 0.18), and
     `settle` (0.30, 0.86) is (0.30, 0.14). `quick` is easeOut 0.14. Reduced-motion
     rules follow `Design.swift:236-289`.
   - `tap_ui::Transition` covers `.opacity`, `.move(edge)` and their combination
     (75 uses).
   - The 7 `matchedGeometryEffect("live")` uses become one shared highlight layer that
     springs to the selected tab.
7. **View tree and overlays.** `TapWindow` is an NSWindow subclass with full-size
   content and a hidden title; the traffic lights are placed per `Lights.swift`. Its
   content view, `WindowRoot`, holds children in this z-order:
   - ground
   - `StageView` (Search's rule: show `wanted`, put it right on every layout,
     `Stage.swift:260-345`)
   - page overlays as siblings above the web view: link bubble, find bar, assistant,
     account list, trouble view, reader look
   - sidebar or tab bar and the bookmarks bar
   - window overlays: omnibox and its dropdown, ⌘K switcher, Peek, Welcome, PinDrop,
     full-screen escape

   `hitTest:` returns nil outside a component's content, so clicks fall through to the
   page. Extension popups, Float, Little and an oversized site card are `NSPanel`s or
   `NSPopover`s.
8. **Keeping about 60 view files manageable.**
   - **One generic `define_class!` family:** `TapView`, `TapControl`, `TapTextField`,
     `TapScrollView`, `TapPanel`, `TapWindow`, `TapApplication`, each holding a
     `Box<dyn Behavior>` for layout, draw, mouse, hover, drag, menu and AX.
   - **Hand-written frame layout** with `hstack`/`vstack`/`zstack` helpers. Search's
     metrics are fixed (`Design.swift:148-211`), so Auto Layout and taffy aren't
     needed.
   - **Real AppKit controls where they exist:** NSTextField, NSSwitch,
     NSSegmentedControl, view-based NSTableView and NSOutlineView.
   - SF Symbols through `NSImage::imageWithSystemSymbolName`. The palette is dynamic
     light/dark `NSColor` pairs (`Design.swift:45-61`).
   - **Settings pages are generated from `settings_index.rs`**, which already lists
     every row's control kind and keys. `Plate.swift` becomes the shared pieces for
     internal pages.
   - **One event router** in `TapApplication::sendEvent:` replaces Search's 17 local
     event monitors, in an explicit order.
   - **Menus** are a declarative `MenuSpec` table ported from `App.swift:49-390`.
     Enabled state comes from `validateMenuItem:`, changing titles from
     `menuNeedsUpdate:`, and key equivalents from `ShortcutStore`.
9. **Raw objc2, not wrappers.** Use objc2 0.6 with the objc2-* 0.3.2 framework
   crates, `block2` and `dispatch2`. Rejected:
   - `cacao` (old `objc` crate)
   - `cocoa` and `core-foundation` (deprecated)
   - `winit` and `tao` (they take over the event loop, title bar and responder chain)
   - `muda` (no full NSMenu validation)

   Any missing ObjC API is a local `extern_class!`/`extern_methods!`.

### 5.2 WebKit delegates and protocols

| Tap class (`define_class!`, MainThreadOnly) | Implements | Ported from |
|---|---|---|
| `TapWebDelegate`, one per window | WKNavigationDelegate and WKUIDelegate: navigation and response policy, `createWebViewWithConfiguration…` (synchronous: make the Tab, run effects, return the view), `didBecomeDownload`, media capture, panels, close, commit/finish/fail, plus SPI | `Browser.swift:6217-7224` |
| `TapDownloadDelegate` | Destination, finish, fail with resumeData; progress via KVO on `NSProgress` | `Browser.swift:7226-7384`, `Fetching.swift` |
| `TapRelay` (one class, `RelayKind` ivar) | WKScriptMessageHandler and …WithReply. Replaces 14 Swift relays (Scroll, Veil, Form, Image, MediaKeys, Store, Passkey, Middle, Icon, PopupGesture, Dusk, HoveredLink, worker recovery, capture). Bodies convert NSObject ↔ `serde_json::Value` | `Tab.swift:30-38` and each `*Relay` |
| `TapKvo` | `observeValueForKeyPath:…` with a closure per context; safe add and remove | Swift `observe(\.title)` and similar |
| `TapPageView` (WKWebView subclass) | `willOpenMenu`, `didCloseMenu`, `mouseDown`, `scrollWheel` (Search's own swipe), `keyDown`, `smartMagnify`, tracking areas | `Tab.swift:2048-2910` |
| `TapExtensionsDelegate` | WKWebExtensionControllerDelegate (open windows and tabs, options page, the three permission prompts, `didUpdateAction`, `presentPopupForAction:`, native messaging, message ports) plus SPI `_webExtensionController:didCreateBackgroundWebView:…` | `Extensions.swift:1625-1858` |
| `TapExtTab`, `TapExtWindow` | WKWebExtensionTab and WKWebExtensionWindow, only the methods Search implements. **One stable adapter per tab** in `HashMap<TabId, Retained<TapExtTab>>` | `Extensions.swift:1867-2030` |
| Small delegates | Site panel (with #849's `CrashCount`), extension popup, offscreen page | `SidePanel.swift:400-447` and others |
| Scripting | Cocoa scripting via KVC; `Tap.sdef` names `cocoa class="TapWindow"` | `Scripting.swift` |

**Private delegate selectors (#849).** WebKit asks `respondsToSelector:`, so declaring
the method is enough:

```rust
impl TapWebDelegate {
    // WebKit calls this instead of the public callback when it is implemented (#849).
    #[unsafe(method(_webView:webContentProcessDidTerminateWithReason:))]
    fn ended_with(&self, web: &WKWebView, reason: NSInteger) {
        self.page_process_ended(web, PageEnd::from_webkit(reason))
    }
}
```

The public `webViewWebContentProcessDidTerminate:` stays as the fallback.

**Calling private SPI.**
- `tap_web::spi!` checks `respondsToSelector:` and then sends, as Search does with
  `NSSelectorFromString` (`Tab.swift:160-165`).
- Typed functions for SPI that returns a value live in `spi.rs`.
- The full catalogue is 39 selectors, 15 private delegate selectors and 2 `dlsym`
  symbols (WebKit geolocation and `_CFHostIsDomainTopLevel`). They live in
  `tap-web/src/spi.rs`, and a bench `spi` verb reports which are present.
- objc2's debug message verification catches signature drift between macOS releases.

**Completion handlers run exactly once.** `tap_web::Reply<F>` wraps each `RcBlock`
handler. If it is dropped unanswered, it answers with a default or an `NSError`. This
is Search's "one answer, always one" rule (`Bench.swift:291-299`) applied to every
`decisionHandler`, `completionHandler` and `replyHandler`.

### 5.3 Replacements for Apple and Swift-only APIs

| Search uses | Tap |
|---|---|
| CryptoKit SHA256, HMAC, SymmetricKey | `sha2`, `hmac`, `getrandom` |
| AES.GCM (Apple Passwords, Passkeys) | `aes-gcm` |
| P256 key agreement and HKDF (passkey PRF) | `p256` (ecdh), `hkdf` |
| SecKeyVerifySignature RSA PKCS#1 v1.5 (CRX3 proof, `Crx.swift:228`) | `rsa` and `spki` |
| SRP-6a 3072-bit (`ApplePasswordsProtocol.swift:22-340`) | `crypto-bigint`, checked against `ApplePasswordsVectors` |
| CommonCrypto PBKDF2, HMAC-SHA1, AES-CBC, 3DES (Chrome and Firefox decryption in `Import.swift`) | `pbkdf2`, `hmac`, `sha1`, `aes` and `cbc`, `des` |
| SQLite3 (read-only copies of other browsers' databases) | `rusqlite` against the **system** libsqlite3, not `bundled` (saves about 1 MB) |
| Keychain: internet and generic passwords, data-protection keychain, Safe Storage | `security-framework`, with Search's SecItem dictionaries ported one-to-one |
| SecStaticCode and SecRequirement (updater) | `security_framework::os::macos::code_signing` |
| LocalAuthentication, UserNotifications, CoreLocation, AuthenticationServices, SecurityInterface, NaturalLanguage, ImageIO, UTType | the matching `objc2-*` crates. Missing pieces use `extern_class!`. Every AuthenticationServices type (including PRF inputs) is checked in the M1 spike |
| os Logger with `privacy: .public/.private` | `tap_sys::oslog`: `os_log_create` plus `_os_log_impl` with a buffer encoded in Rust, so `%{public}`/`%{private}` behave the same; subsystem `com.timwuhaotian.tap` |
| IOKit power assertions and power source; Carbon TIS and `UCKeyTranslate` | `tap-sys` extern blocks |
| URLSession (suggestions, CRX, updater, AI) | `NSURLSession` through objc2-foundation: system proxies and trust, and no TLS crate in the binary |
| NSRegularExpression (19 uses) | `regex-lite`; ICU-specific patterns stay on NSRegularExpression through `fnd` |
| Translation (Swift-only) | **Dropped.** No relay, no Settings rows; the bench verb `translate` answers `{"error":"translation is not in Tap"}` |
| AI engine helper (llama.cpp, C++) | Search ships AI switched off (`AI.shipped = false`, `AI.swift:19`). Tap ports the AI code behind the same off switch and **doesn't build the C++ helper**. Revisit after 1.0 |

### 5.4 JS assets

About 8.3k lines of JS live in Swift string literals: ExtensionShims (3.5k),
DuskColours, PageFind, Dusk, Forms, Reader, Passkeys, Float, Curtain and smaller
ones. They move **verbatim** to `assets/js/*.js` and are embedded with `include_str!`,
so there is no disk read at launch and nothing extra to sign.

- **`cargo xtask extract-js`** pulls each literal out of Search at parity-pin and
  applies Swift's literal rules:
  - Strip the indentation of the closing `"""`.
  - Process escapes in `"""` literals, but not in `#"""` raw ones.
  - Turn `\(expr)` into `{{name}}` placeholders.
  - Apply the identity rename (`__search*`/`__office*` → `__tap*`, world names).
- `assets/manifest.toml` records each asset's source file, symbol and placeholders.
- `extract-js --check` fails on drift from the pin.
- A tap-web test runs `JSCheckScriptSyntax` over every asset.
- The SQL in `Import.swift` stays as Rust constants.

## 6. Identity map: what Search hard-codes and what Tap uses

Every value lives in `tap-core/src/identity.rs`.

| Search (source) | Tap |
|---|---|
| `CFBundleIdentifier com.officecommun.search` (`build.sh:186`) and the `ownContainer` check (`Store.swift:101`) | `com.timwuhaotian.tap`. The check compares against `identity::BUNDLE_ID`. |
| Application Support `Search` / `Search (<world>)` (`Store.swift:126`) | `Tap` / `Tap (<world>)` |
| Test suites `com.officecommun.search.test[.<world>]` (`Store.swift:162`) | `com.timwuhaotian.tap.test[.<world>]` |
| `SEARCH_PROBE` and 23 more `SEARCH_*` variables | `TAP_PROBE`, `TAP_*` |
| Keychain label `Search` (`Vault.swift:48`); services `…search.ai`, `…search.bench` | `Tap`; `com.timwuhaotian.tap.ai`, `…tap.bench` |
| Internal scheme `search://` (`InternalPages.swift:32`) | `tap://` (native pages, no scheme handler) |
| Content world `"Search"` (`Tab.swift:24`); shim app `"search"`, `search-shim.js`, `__search*` globals, `search-*` message names (`ExtensionShims.swift:29-43`, `Passkeys.swift:851`) | `"Tap"`, `"tap"`, `tap-shim.js`, `__tap*`, `tap-*` |
| WebKit store UUIDs `5E4C…` (FNV-1a of the world, `Store.swift:105-117`) | The same derivation with prefix `7A90…`, so the stores never coincide with Search's |
| Updater team `7BYKA895MC` (`Updater.swift:357`); signed appcast on officecommun.com | Kil's updater: the running app's own team (43VLF3KTFZ) and `releases/latest.toml` on `main` (§12) |
| AI engine `com.officecommun.search.ai-engine` | `com.timwuhaotian.tap.ai-engine` (reserved; the helper isn't built) |
| AppleScript `Search.sdef`, suite `Srch` | `Tap.sdef`, suite `TapS` |
| Logger subsystem `com.officecommun.search` | `com.timwuhaotian.tap`, with the same categories |
| `hello@officecommun.com`; "by Office Commun"; "© Office Commun · Search" | The user's contact (open item §14); "© 2026 Tap contributors". `LICENSE` keeps the MIT notice. |
| `Logomark` path (`Design.swift:301`, `Icon/icon.swift:22`) | The Bolt-T path in `packaging/icon.svg`. `tap-core/src/mark.rs` carries the same path for the in-app logo, with a test that keeps the two equal |

**Must stay "search":** anything that means the verb or feature. That includes:

- "Search Tabs…", "Search bookmarks", and "Search with …" strings
- prefs keys such as `search.engine`
- `chrome.search` and the `"search"` methods of `bookmarks`, `history` and `downloads`
- `role="search"`
- search-engine URLs

The port is written by hand, never by find-and-replace.

## 7. Data compatibility and migration

**Files** in the store folder have the same names and shapes as Search's:
- `session.json` and `session-<UUID>.json`
- `windows.json`, `spaces.json`, `profiles.json`, `pins.json`, `hidden.json`,
  `search-sites.json`, `downloads.json`
- `history[-<UUID>].json`, `bookmarks[-<UUID>].json`
- `icons/pages.json`, `Extensions/installed.json`, `crash.log`

**Settings** live in NSUserDefaults with **Search's key names** (`search.engine`,
`tabs.*`, `shield.paused` and so on). #845's allowlist depends on them.

**serde rules** (`tap_core::serde_compat`):
- `AppleDate(f64)`: seconds since 2001-01-01.
- UUIDs are written uppercase and read in any case. `Data` is base64.
- `Option` is skipped when `None` (Swift's `encodeIfPresent`).
- **Lenient fields**: a bad value becomes `None` or the default, as Search's
  hand-written decoders do (`Session.swift:136-195`). A file that fails as a whole goes
  to `Store::quarantine` (`Store.swift:139-146`).
- Search's old Split shape is accepted.
- Checked structurally, not byte for byte. Fixtures are synthetic, never user data.

**`.searchprofile` (#845).** `tap_import::profile` ports the manifest, the allowlist,
collect and import; `tap_shell::prefs` carries `AnyCodable`'s `$data`/`$date` shape
across NSUserDefaults (done in 0.1.1, §8).
- Uses the `zip` crate (deflate only) instead of `ditto`, and never unpacks to disk:
  files are read by flat name into memory, so a `../` name reaches nothing.
- Tap **reads** `.searchprofile` and Safari zips, and **writes** `.tapprofile` with the
  identical layout and `format: 1`.

**"Bring things over from Search"** (`tap_import::from_search`, new; its file half is
in: `from_search::pack` and `cargo xtask search-profile`, §8 "0.1.2"; in the app,
without a file, since 0.1.4: `from_search::read`, §8 "0.1.4"). If
`~/Library/Application Support/Search` exists, Search is offered as an import source.
- It builds the same `IncomingProfile` that #845's import reads, from the live folder
  plus `CFPreferencesCopyMultiple("com.officecommun.search")`, so both paths share all
  of the merge code.
- What comes over: profiles, spaces, sessions (open tabs, groups, splits), pins,
  bookmarks, history, hidden elements, search sites, allowlisted settings, and
  extension IDs (reinstalled).
- **Can't come over:**
  - cookies and sign-ins: WebKit keeps them per bundle id.
  - passwords all at once: each is the Search release's own keychain item, and macOS
    hands one to another app only after asking, per item (569 on this Mac). Since
    0.1.5 they come **on first use** instead (decided 2026-11-04, §8 "0.1.5"): names
    are readable without asking, so Search's accounts hang under sign-in boxes at
    once, and the first pick of one asks macOS for that one and copies it into Tap's
    drawer. A bulk path (one prompt per item) is not built.

## 8. Milestones (in dependency order)

Each milestone ends only when its **Done when** is true and its rows in Appendix A
are updated.

### M0 — Hello Tap
Set up, copying Kil's layout:
- [x] workspace (`rust-version = "1.94"`, edition 2024, Kil's lint set) and
  `.github/workflows/ci.yml` (§9)
- [x] `scripts/bundle.sh` and `scripts/release.sh` ported from Kil, with the `vtool`
  check and the passkeys variant (§12)
- [x] `packaging/`: the Info.plist template and both entitlement files. `Tap.sdef`
  arrives with scripting in M8.
- [x] `cargo xtask icon` (Bolt T) and the committed PNGs
- [x] Open-source metadata, as in Kil: `LICENSE` (with Office Commun's notice),
  `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`,
  `CLAUDE.md`, issue and PR templates, `CODEOWNERS`, Dependabot. On GitHub: topics,
  wiki and projects off, private vulnerability reporting on.
- [x] A first window: `tap-app` opens one `NSWindow` with one `WKWebView` showing a
  "being built" page (2026-11-04)
- [x] a panic hook that writes `crash.log` (`tap-shell/src/crash.rs`)

Then build `TapApplication`: one `TapWindow` with one `TapPageView` (shared process
pool, `Tab.swift:68-82`) and an NSTextField address field that loads what you type.
**The user asks Apple for the browser passkey entitlement this week.**

**Done when:**
- [x] `scripts/release.sh` completes, and Apple accepts the app and the DMG.
  First run 2026-11-04 with `TAP_ALLOW_NO_PASSKEYS=1`, notarized through the `asc`
  key; both were accepted and stapled.
- [x] `spctl -a -vv` on the app in the DMG says **Notarized Developer ID**, origin
  `Developer ID Application: HAOTIAN WU (43VLF3KTFZ)`. The DMG is not committed (§12).
- [x] `vtool -show-build` shows minos 14.0 and sdk 26.5.
- [x] Typing `example.com` loads `https://example.com`: in the model
  (`browser::tests::return_goes_to_an_address_or_searches`). In the app, the field and
  Return are in, and restored pages load; checking it by hand through the UI waits for
  the bench (M1): synthetic keystrokes can't reach Tap reliably.
- [x] The binary-size baseline is recorded in §10.

### M1 — Foundations and risk spikes
tap-core basics:
- `identity`, `store` (worlds, `TAP_PROBE`, `7A90` probe stores, quarantine, the disk
  writer), `fnd::Url`
- prefs over NSUserDefaults and every serde model
- the `observable!` macro, the run-loop flush, effects, oslog

Bench server with `probe tabs open go wait eval close select state quit alive`. Then
`harness.patch`, `extract-js`, and a local `parity-pin` branch in Search, built to
`build/Search.app`.

**Pure-code track in parallel:** the SNSS parser (#841), Markdown, CRX3, SRP, CSV,
BookmarksFile, Registrable.

**Spikes** that must succeed before M2 (§11):
1. WKWebExtension tab and window adapters from Rust: uBO Lite unpacked, `tabs.query`,
   `scripting`.
2. A tab-drag prototype.
3. Search's custom swipe.
4. Pinyin input in the address field.
5. Launch on a macOS 14 VM, including weak-linked symbols.
6. A binary-size check.

**Done when:**
- `cargo test -p tap-core -p tap-import` is green, including the serde fixtures, the
  ported `SessionImportTests` and the Apple Passwords vectors.
- One vendored Python smoke test passes against Tap through the patched harness.
- Each of the six spikes has a written result in §11.

### M2 — Browsing in one window

**In (2026-11-04), ahead of M1 at the user's request ("go straight build"):**
- [x] `tap-core`: `fnd::Url` (Foundation URL semantics), `address`, `engine`, `keyword`,
  `crash` (`CrashCount`, `PageEnd`), `session` (Search's format, lenient), `browser`
  (tabs, select, close/reopen with Search's rules, link runs, the field with ⌘T
  launching, `destination`/`submit`, the #849 crash flow, asleep restore). 25 tests,
  including Search's address regressions case for case.
- [x] `tap-shell`: the runtime (`rt`: one owner, effects, flush, queued re-entry), the
  window with the lights placed by hand (deferred past AppKit's own layout pass), the
  strip (pill, cross, plus, progress line), back/forward/reload, the field on the live
  tab and floating for ⌘T, the stage with the failure view and Try Again, web views
  (shared configuration, Safari user agent, one process pool, KVO), the navigation and
  UI delegates (⌘-click behind, other apps' links, new windows as tabs, #849's private
  termination reason), the menus with Search's keys, the session written on change and
  on quit, `crash.log`.
- [x] The bench, early (M1's, brought forward to check M2 by): Search's protocol (JSON per
  line, one request per connection, always one answer, 4 MB limit, 0600 socket with a
  uid check), on test runs only. Verbs so far: `probe` (field, launching, typed,
  lights, first responder), `tabs`/`state`, `open`, `go`, `select`, `close`, `wait`,
  `eval`, `text`, `field` (through the field editor's own `NSTextInputClient` path, and
  Return through the delegate), `keyeq` (a real key equivalent through the menu bar),
  `quit`. Client: `scripts/bench`.
- [x] Checked through the bench: ⌘L opens the field on the live tab with the editable
  address (`rust-lang.org`), typing and Return load the page; ⌘T floats the field and
  opens a tab only on Return; ⌘W lands on the right-hand neighbour; ⇧⌘T brings the tab
  back in its place; ⌃Tab and ⌃⇧Tab walk round the row; ⌘1, ⌘2 and ⌘9 (last); the
  page has the keyboard at launch; the lights' centres are 26 pt from the top.
- [x] History and the field's offers (2026-11-04): `tap_core::history` ports
  `History.swift` (identity, front-door credits, frecency, ranking, the known sites,
  completion, `history.json` in Search's format, capped at 2,000, passwords never
  kept), with Search's `HistoryRegression` cases as tests. The field offers three
  places and a search, with the grey ending after the caret; ↓/↑ pick a row, Tab (or → from
  the end of the text) takes the ending, Return goes to the picked row or the words as
  typed, and a click on a row goes there. URL path and query fragments also find visited
  pages (`fixed-deviation` from Search's host-only ranking). Nothing is completed while
  an input method composes (fixes Search's own gap, §3.5). Checked through the bench, including marked text (`field marked=true`,
  `unmark`). Not yet: bookmark and open-tab rows, address commands, engine suggestions,
  site search, the History page (⌘Y), per-profile files, and moving pre-1.0.4 Search
  front-door credits (`rehomeCredits`).
- [x] Page questions (2026-11-04), from `Dialogs.swift`: `alert`, `confirm`, `prompt`,
  file inputs and HTTP Basic/Digest/NTLM sign-in, each the system's sheet on the window
  and titled with the asking frame's own site; a question from a tab behind waits for
  that tab; every one answered exactly once (dismissed if its tab closes or its process
  goes); test runs note them for the bench (`probe` → `asked`) and answer as cancelled.
  A tab behind whose process went away is made again when shown, not straight away
  (Search's `pageProcessEnded`). Checked through the bench and, once, by eye outside a
  test run. Not yet: the bad-certificate sheet ("Continue Anyway") and trusting this
  Mac's own servers; a question over one page of a pair (no Split View yet).
- [x] Downloads (2026-11-04): `<a download>`, an `attachment` response or anything
  WebKit can't show becomes a download, into Downloads (a test world's own on a test
  run) under its own name, numbered when taken (`tap_core::fetching`); a tab opened
  for a download alone goes and its opener comes back (`drop_empty`); a finished file
  bounces the Dock's Downloads stack. WebKit marks each file with
  `com.apple.quarantine`. Checked through the bench (`downloads`). The door says what
  is happening (2026-11-07, Search's `FetchDoor` without the panel behind it): a ring
  filling with every running file's bytes together, turning while none says its size;
  the arrow down a moment once the last one is in, a warning with the reason a moment
  when one failed, each for Search's 5-second linger; every file carries its own bar in
  the Finder while it comes in. Saying a failure on the door is an interim deviation:
  Search says it in its panel, and the door goes back to quiet once the list lands.
  Not yet: the list (⇧⌘J) and `downloads.json`, pause, cancel and resume, asking where
  to save, the PDF bar's save button, local text files shown as text.
- [x] Find on page (2026-11-04): Search's page script, verbatim but for its names, in
  `assets/js/page-find.js` (copied from `PageFind.swift` at d1639aa by hand; the
  `extract-js` drift check comes with M1's tooling), run in Tap's own content world
  with the word as an argument; WebKit's own find for a PDF. `tap_core::find` ports the
  window's side (one question to the page at a time, generations, "3 of 17",
  "1000+", "No matches" in red, asked again on another tab or a new page). The bar:
  the field, the count, previous and next, Match Case and Whole Words, Done; ⌘F, ⌘G,
  ⇧⌘G, Return, ⇧Return, Esc. Checked through the bench on a page with known counts
  (7, 2 with Match Case, 6 with Whole Words), stepping both ways, and by eye.
- [x] #849 checked end to end in the app (2026-11-04): the bench's `crash` kills the
  tab's WebContent process (pid from `_webProcessIdentifier`), as the system's memory
  killer would. Two kills reload the page, the third leaves "This page kept crashing."
  with nothing reloading behind it, ⌘R brings the page back. Each ending is in the
  system log as Search writes it (`oslog::page_ended`, `_os_log_impl` from Rust, the site
  private), with WebKit's private reason ("crash"):
  `/usr/bin/log show --predicate 'subsystem == "com.timwuhaotian.tap"'` (plain `log`
  is a zsh builtin).
- [x] Sleeping tabs (2026-11-04), from `Sleep.swift`: a tab left alone half an hour
  (Search's `tabs.sleep.pace`, `sleep.after` in seconds for the bench) gives its page
  back and keeps its `interactionState`, so it wakes on the same page with Back and its
  scroll; memory pressure shrinks the wait to five minutes, or none when critical.
  Never the tab on screen, one loading, the opener of the page on screen (model), or
  one playing sound, on a call, downloading, with a question waiting, or holding
  something typed (shell). `touched` is kept in the session. Checked through the
  bench (`sleep` answers why a tab stays awake): asleep with no process left, woken at
  scroll 2400 with Back. Not yet: the picture while a page wakes, pins and spaces,
  sites kept awake, and Search's own typed check (an interim one until forms, M5).
- [x] Site icons (2026-11-04), from `Icons.swift`: when a page finishes, Tap's own
  content world reads its `<link rel=…icon…>`; `tap_core::icons::rank` orders them as
  Search does (a crisp 32–64 px icon, the touch icon, an SVG one, then `/favicon.ico`;
  the other look's icon skipped) and the first one that is really a PNG, ICO, JPEG,
  GIF or WebP (its header looked at first, 1 MB at most) is drawn into a 32-pixel
  square and kept as `icons/<sha256 of its address>.png`. An SVG icon (by its mime
  or its first bytes, 256 KB at most) is drawn in an isolated web view of its own
  (`tap-shell/src/svg.rs`, Search's `SVGIcons`: a non-persistent store, page scripts
  off, only `about:blank` allowed in) and kept the same way. `icons/pages.json` is Search's index (site with
  its port, then page, then icon; 4000 pages, the oldest forgotten), so a tab restored
  from yesterday has its icon before its page, and a page with none borrows the nearest
  of its site's unless one host is several apps. 16 pt in the strip, left of the title,
  alone in a narrow tab. Checked in a test world: GitHub, Wikipedia and rust-lang.org
  kept and drawn, example.com (no icon) left plain, all three back after a relaunch.
  SVG (2026-11-05): platform.qianwenai.com, whose only icon is an SVG, keeps and
  draws it. Not yet: inline icons (Search reads the page's own `data:` address),
  the dark-look icon, the letter before an icon arrives, icons in the field's rows.
- [x] Links from other apps (2026-11-04), from `Links.swift`: `Info.plist` claims http
  and https, `public.html` and `public.xhtml` (what macOS asks before it lists a default
  web browser), and Search's Alternate list (PDF, pictures by name, plain text, JSON) for
  Open With. `application:openURLs:` takes http, https and file addresses; the model's
  `arrive` puts the first into the new tab in front unless something is typed in it,
  else a tab of its own; the window comes out of the Dock and Tap in front, asking again
  the pre-macOS 14 way when the app that asked didn't hand the front over. A test run
  takes nothing from outside; the bench's `link` hands one in. Checked through the bench
  (`ftp:` and `javascript:` refused) and, outside a test run with HOME pointed at a
  scratch folder, by `open -a`: a cold launch with a link, a second link, and an HTML
  file. Not yet: the small window for links, the Dock menu's New Window, staying open
  with no window (Tap quits with its window), Settings' "Make Tap the default browser",
  `.webloc` files, and the crash log (`watchForTrouble`).
- M2 is done for 0.1.0; what each slice leaves out is listed with it.
- Tab model and host, KVO, `TapWebDelegate`, dialogs, downloads
- Address parsing, engines, keywords, site search, history suggestions; the omnibox
  with inline completion (IME-safe, finding 3.5)
- Basic top tab strip, session restore, history, find on page, sleeping tabs, icons,
  Web Inspector
- **#849:**
  - `CrashCount` and `PageEnd` in `tap_core::tab`
  - the SPI selector
  - the log line `A page's process ended (<reason>) on <private>: <action>`
  - "This page kept crashing." / "…kept running out of memory." in the failure view
  - Try again reloads through `loadAndVerify`; the delayed check never reloads behind
    the message

**Done when:**
- The Rust ports of these tests pass: Address, AddressRegression, CleanLink,
  EngineSuggest, TabClose, CloseTabsBelow, TabSelection, **CrashCount, CrashLoop**
  (including SIGKILL ×3 on the WebContent pid), DownloadLifecycle.
- The matching Python tests are green on both apps: close_last_tab, closing_tabs,
  completion_keys, dialog_keys, link_place, new_tab_*, tab_switcher.

### 0.1.1 — Settings, profile files, tab groups and the updater (brought forward)

At the user's request (2026-11-04: Settings like Search's in the main menu; profile
export and import in #845's format, tested on Search's own main profile; the profile's
tab groups and their tabs opening on the new Mac in one go; Kil's Check for Updates and
restart flow; then a patch release). Pieces of M3, M4, M6 and M8, each as narrow as
that asks:

- [x] **Settings** (Tap › Settings…, ⌘,), Tap's own page in a tab (`tap://settings`,
  `tap_core::internal`; no scheme handler, never WebKit's). `tap_core::settings_index`
  ports `SettingsIndex.swift` whole: Search's pages, groups and rows, keys, words,
  merged controls (`TabsLayout`, `SleepChoice`, `FloatOwn`, pins keep, `PeekChoice`),
  `applies`/`needs`, the find field. Translation's row is left out (§2). Values are
  Search's keys and Search's defaults in NSUserDefaults; a test run keeps its own suite
  (`com.timwuhaotian.tap.test.<world>`, `tap_shell::prefs`), never yours. The page
  (`tap_shell::settings`) is the rail of pages with the find field, then each group's
  card of rows with real AppKit controls (switch, segments, pop-up, buttons). **Live
  now:** Make default (http and https), Import Profile…/Export Tap Profile…, Confirm
  before quitting, Save to, Appearance, Sleep tabs, Search with (custom address too),
  site shortcuts (listed; used by the field), Clear history/cookies/cache, Updates
  (Check for Updates…, Install updates on its own), What's new. Every other row shows
  what is kept under its keys, dimmed, "Not in Tap yet; kept for when it is"; the
  layout, tab look and floating-⌘T rows show what Tap does for now (`fixed`). Not yet:
  editing site shortcuts, the (i) popovers (the long text is the row's tooltip), the
  Extensions page's list, Search's shortcut editor.
- [x] **Profile files (#845)**: File › Export Tap Profile… and Import Profile…, and the
  General row's buttons. `tap-import` (new crate) packs and reads #845's layout exactly
  (`profile::pack`/`unpack`, `zip` with deflate only): manifest `format` 1 and scope
  "profile", the store files byte for byte, `prefs.json` allowlisted by the index (77
  keys: Search's 78 less `translate.pages`, checked against `SettingsIndex.swift`),
  `installed.json`. Reading never unpacks to disk: flat names only, `__MACOSX` and paths
  skipped, 64 MB a file and 256 MB in all. Export: a save sheet named
  `Tap-profile-<day>.tapprofile`, then the share picker (AirDrop to the other Mac).
  Import: `.tapprofile`, `.searchprofile`, or a `.zip` with a profile's manifest, from
  the panel or opened from the Finder/AirDrop (Info.plist declares both types; asked
  first). Its sessions' tabs come in after this row's, asleep, in their groups (fresh
  group identities, folded as they were), the first session's front tab in front, a
  lone new tab giving way; history merged as Search's `History.take` (larger count, not
  the sum); bookmarks kept in `bookmarks.json` without doubles for M3; settings set and
  applied at once. Not yet (said in the result line where it applies): spaces and
  profiles of their own (each space's tabs come in one after another), pins as pins
  (they come as tabs), reinstalling extensions, Search importing `.tapprofile` (it takes
  the file renamed `.searchprofile`).
- [x] **Tab groups in the row**: `session::TabGroup` typed (lenient, as Search's), kept
  by the model (`tap_core::groups`): the row's one order is pins, each group's tabs in
  the groups' order, then the rest (Search's top bar draws it so); a folded group shows
  its name and only the tab in front if it is its own; a click on the name folds or
  opens it; a link from a group stays in it; a group goes with its last tab; the
  session writes them back. A narrow tab with no icon yet shows its first letter
  (Search's "Letters"). Not yet: making, naming, dragging groups; the row scrolling when
  too long (it stops at the doors).
- [x] **The updater**, Kil's `update.rs` ported (`tap_shell::update`, `update_window`),
  as §12 says. The version compared is the bundle's own. Tap › Check for Updates…
  opens the window (checking, downloading x of y MB, verifying, ready with Later and
  Restart Now, up to date, or why not); the item becomes "Restart to Update to x.y.z"
  once one is staged. Restart Now and the bench's `quit` put any open sheet away first:
  AppKit won't quit under one.
- Checked end to end through the bench in test worlds:
  - Search's real main profile, packed as `SearchProfile.collectCurrentProfile` packs
    it (from `~/Library/Application Support/Search` and its defaults; a scratch fixture,
    never committed): 27 tabs in 3 groups (软件开发 and 娱乐 folded, AI Providers open),
    2,000 places, 368 bookmarks and 14 settings came in, the front tab in front and the
    only one loaded, the strip drawn by eye. They came back the same after a relaunch.
  - Tap's own export unpacks with `ditto -x -k` (what Search's import uses) into #845's
    layout. A fresh world importing it got the same tabs, groups and folds, and the
    settings landed in its own suite.
  - Settings: by eye, page by page; Search with DuckDuckGo sent the field's words there;
    the find field ("dark") shows Search's results under PAGE › GROUP headings.
  - Updater: a copy of the build relabelled 0.0.9 and signed with the Developer ID
    (team 43VLF3KTFZ), pointed with `TAP_UPDATE_BASE` at `releases/`, found 0.1.0,
    downloaded it, checked its checksum, bundle id, version, team and notarization, and
    showed it ready. Restart Now swapped the bundle (0.1.0, "Notarized Developer ID",
    nothing left beside it) and reopened it in the same test world. It did the same with
    the export's save sheet open. The unit test stages the committed DMG through every
    check, as Kil's does.
- Found on the way, in Search's #845 (the user's own PR): its import keeps only sessions
  of spaces listed in `spaces.json`, and Search writes no `spaces.json` while Personal
  has one space. So a profile from this Mac's Search would bring no tabs into Search.
  Tap reads `session.json` as the first space's whether or not the list is there.
  Fixed on both sides after 0.1.1 (below).

### 0.1.2 — Settings that work, #845's space list, Search's profile brought over

At the user's request (2026-11-04: fix #845's `spaces.json`; export this Mac's Search
profile in Tap's format and bring it into the installed Tap, groups and all; "most of
Settings can't really be used: fix it"), then released as 0.1.2 ("发版").

- [x] **#845's space list, fixed in Search** (branch `export-search-profile`, commit
  `0df6942`, kept local: the user wants no more PRs, see §15): the export stages the list `Spaces.read` makes on the
  spot when there is no `spaces.json`, and the import reads a file without one as
  Personal's one space, so files already written bring their tabs too.
  `SearchProfileTests` (2 tests) fail without it and pass with it; the ImportFile and
  Profiles tests stay green. **Tap's export does the same** (`profile::pack` always
  writes `spaces.json`, Personal's `Space.first()` when the store has none), so a
  `.tapprofile` renamed `.searchprofile` comes into the fixed Search with its tabs.
- [x] **Search's own profile as a Tap profile file**: `tap_import::from_search::pack`
  reads Search's store folder as `collectCurrentProfile` does (Personal's first
  space, history, bookmarks, the shared files, extension ids, every file byte for
  byte) and packs it with `profile::pack`; `prefs::for_profile_of` reads Search's own
  defaults domain under the same allowlist; `cargo xtask search-profile <file>` writes
  it. This Mac's Search went into the installed Tap 0.1.1 through File › Import's
  path (opened from the Finder, Bring In): 27 tabs in 3 groups (软件开发 and 娱乐
  folded, AI Providers open, as in Search), 2,000 places, 368 bookmarks, 14 settings;
  the strip checked by eye. Still to come with M6: the in-app "Bring things over from
  Search" (§7) without a file, other spaces and profiles, pins as pins.
- [x] **Settings shows only what Tap does.** 0.1.1 showed Search's whole index with
  every row Tap didn't act on dimmed; most of the page couldn't be used. Now the page,
  the rail and the find field show only `settings_index::live` rows (27 of 81), and the
  two privacy rows only where this Mac's WebKit has the feature (macOS 27, as Search
  hides them). Pages with none (Passwords, Extensions, Keyboard & Mouse) leave the rail.
  The other rows' keys still travel in profiles (`profile_keys` is the whole index).
  `fixed`, `FIXED` and `NOT_YET` are gone. Newly live, each ported from Search:
  - `tabs.newPlace` (`placeForNew`/`placeForFront`: a link from another app beside the
    tab in front, first after pins, or at the end; ⌘T's at the end unless Leftmost;
    ⌘-click links after their page whatever it says), `tabs.newAfterLast` (with
    Search's rule that the last loose tab beside groups always leaves a new tab),
    `tabs.closeGoesBack` (the tab viewed before, from a list of tabs left this run;
    Tap's own pages always go back), `tabs.lazy` (a background link waits asleep),
    `commandbar` ("settings"/"preferences", "new tab"; the rest of Search's commands
    need pages Tap hasn't got), all in `Browser::behaviour`, set by `prefs::apply`.
  - `tabs.close.after` (`Closing.swift`): `close_left_alone` on the sleep timer and at
    launch; a tab that isn't on screen, pinned, named, grouped or blank closes as ⌘W
    would once the shell finds it idle (`sleep::try_retire`: sound, call, download,
    question, something typed). `close.after` in seconds for the bench.
  - `start.fresh`: `Shape::fresh` (Search's `Session.fresh`) at launch.
  - `downloads.ask`: a save panel per download (never on a test run); closing it
    cancels the download.
  - `media.click` (`.all`, else `.audio`, as Search) and `pageZoom` (every view, at
    once on change; per-site ⌘+/⌘− isn't in Tap yet), `privacy.clickIDs` (WebKit's
    `FilterLinkDecorationByDefaultEnabled` flag on each view's preferences, reaching
    open tabs on their next load) and `privacy.gpc` (`setGlobalPrivacyControlEnabled:`
    on the configuration a view is made from: tabs made or woken from then on).
- Checked: model tests for each (`tap-core`, 86); through the bench in a test world,
  gclid stripped with utm_ kept and kept again with the switch off, zoom 125% (1263 →
  1011 CSS px) and back, `navigator.globalPrivacyControl` true then false in a new tab,
  idle tabs closing in under a minute with `close.after` 5 and ⇧⌘T bringing one back,
  "settings" in the field, a fresh start keeping the imported 22 grouped and 2 named
  tabs; the Tabs page by eye. Not checked on a real video: `media.click`.

### 0.1.3 — Twenty-six more Settings rows live

At the user's request (2026-11-04: "Settings 大部分不能用的功能帮我全部实现",
then release a patch). `settings_index::LIVE` goes 27 → 53 of 81; the bench
`settings` answer lists every one `shown`, on a macOS 27 Mac (the two WebKit-gated
privacy rows included).

- [x] **Page level** (`web.rs`, new `shield.rs`): `tabs.popupWindows`, `sites.keep`
  (WebKit's `_setResourceLoadStatisticsEnabled:` + cookie policy, as Search),
  `notifications.ask` (the private permission delegate, answers kept as
  `capture.<origin>|notifications`), `pages.120` (the 60 FPS feature flag + IOKit
  power watch), `pages.dusk` (Search's filter fallback only: root/body luminance,
  no sampling engine, no per-site memory), `shield` (a built-in
  `WKContentRuleListStore` list, per-site `shield.paused`, YouTube page-data
  stripping + skipper), `fullscreen.escapeTwice` (an event monitor; WebKit has no
  key hook in the UI delegate), `autocorrect` (`WebAutomaticSpellingCorrectionEnabled`,
  as Search), `autoscroll` (Search's `AutoScroll.swift` script, `__tap` names),
  `motion.light` (the strip's give lands at once while Low Power Mode or a
  serious thermal state holds; Tap has no other chrome springs yet).
- [x] **Tab bar** (`strip.rs`, read from `Kept` on every draw): `glyph`,
  `tabs.reading` (pushed by `forms.js` on scroll, once a frame at most, as Search's
  `ScrollRelay`; it was a 200 ms poll through Tap's content world until 0.2.12, and
  each script the app runs in a page ends that page's user activation, so video
  full-screen buttons were refused "without transient activation" — no periodic
  script may run in a shown page),
  `tabs.give`, `tabs.shortcuts`, `tabs.address` (a registrable-domain table, not
  the Public Suffix List; IDN stays punycode, never decoded).
- [x] **Actions** (`settings.rs`, `bench.rs`): `about.feedback` (a GitHub draft
  with the version; Search's mailto goes to Search's author, Tap's contact is
  still the §14 open item), `privacy.forget` (deletes `capture.*`/`nofloat.*`
  as Search does — no writer of those keys exists yet, so it is a correct no-op
  until the capture delegate lands), `passwords.open` (the macOS Passwords app,
  not a Tap page: there is no vault yet), `bench` (the switch gates the real
  socket; a test world's always listens). **Bench fix found by hand:** the
  nonblocking listener's accepted sockets inherited nonblocking on macOS, so a
  reply past the socket buffer (a full `settings` answer) was cut at 8 KB;
  `serve` puts its socket back to blocking.
- [x] **Window and field** (`window.rs`, `downloads.rs`): `search.suggest`
  (Search's endpoints and sendability rules, a card under the offers — the
  model's offer list has no injection point, so arrow keys don't reach them),
  `newtab.floating` (gated at `window::update`, the one place all ⌘T paths
  pass), `downloads.button` (the folder, there being no Downloads page yet).
- [x] **Reader and float** (new `reader.rs`, `float.rs`): `reader.twins`
  (⇧⌘R; Search's same-origin Markdown twin, else a scored prose guess),
  `reader.follows` (the article-corner look switch is Search's stage overlay
  and isn't in; a kept `reader.look` is honoured), `float.style` (system PiP,
  or Tap's panel with the page's own view moved into it), `float.own` (a 0.5 s
  poll of the active tab, Search's known-players table only).
- Still hidden, with their triggers: the sidebar family and `pins.list`
  (`tabs.layout` — M3's column), `bookmarks.bar` (no bookmarks surface — M3),
  `links.little`/`links.peek` (off-row web views — M3, hooks named in
  `links.rs`/`window.rs`), `swipe.history` (WebKit owns the gesture — M5),
  `arc.setup`/`arc.import`, `spaces`, `profiles`, `pins.keep` (M4/M6),
  `passwords.save`/`passwords.fill`/`passkeys`/`passwords.apple` (M5's vault),
  Extensions (M7), and the `note.*` find-only rows whose always-there features
  (Split View, side panel, keep-awake, tab-to-search, shared pins, pin-back,
  switcher keys, link bubble, flick) aren't Tap's yet. Their keys still travel.
- Checked: `cargo test` (tap-core 86, tap-shell 37, tap-import 8), clippy
  `-D warnings`, nightly fmt; through the bench in a test world: 53 live = 53
  shown, autoscroll arriving and leaving an open page, `__tapDusk` on a new tab
  in dark, `navigator.globalPrivacyControl` true, Settings open at Appearance.
  Not checked by hand: popup window sizes, 120 Hz feel, Esc-twice, the
  notification sheet, Reader and float on real video sites, shield on YouTube.

### 0.1.4 — Bring things over from the browsers on this Mac

At the user's request (2026-11-04: "tap bring things over 理论上应该是能自动侦测到本地的
chrome/search 等类似的profil数据的，但目前没有"): Settings › General › Bring things
over finds the browsers on this Mac by itself and brings one over, as Search's
`ImportPanel` does, cut to what Tap holds.

- [x] **Found on their own** (`tap_import::found`): Search (its store folder, when it
  has a `session.json`) first, then Search's Chromium list in Search's order (Dia,
  Chrome, ego lite, Arc, Brave, Edge, Vivaldi, Chromium, Chrome's other channels,
  Opera, Opera GX, Helium, Comet). A Chromium browser counts when a profile folder,
  or its own folder (Opera), holds `Bookmarks`, `History`, `Sessions` or
  `Current Session`; Guest and System profiles never count; a folder a browser's
  helpers left (`NativeMessagingHosts` alone, as Arc's, Brave's and Edge's are on the
  user's Mac) doesn't either. A test run reads `<world>/Import` instead, never a real
  browser's (Search's `Chromium.base`).
- [x] **The row** (`settings.rs`): its words name what was found ("from Search and
  Chrome on this Mac"); Bring Over is a pull-down of them, then From a Profile File…
  (the old Import Profile…). Picking one reads it off the main thread
  (`bring::offer`), says what there is (tabs and groups, places, bookmarks, settings,
  and the profile when the browser has several), and brings it in on Bring In.
- [x] **Chromium** (`tap_import::chromium`, from `Import.swift`): the profile used
  last (`Local State`'s `last_used`, then `last_active_profiles`, then the folder
  touched last, as Search's `usual`); its open tabs and groups through the #841 SNSS
  reader (`tap_import::snss`, `ImportSession.swift` ported line for line, its
  `SessionImportTests` with it); the 3,000 newest places from a copy of `History`
  with its `-wal`/`-shm`, read with `rusqlite` against the system's libsqlite3
  (§5.3); bookmarks from `Bookmarks`, the bar first, then Other and Mobile. A link
  can't lead a read out of the browser's folder; files are capped at 64 MB.
- [x] **Search** without a file (§7): `from_search::read` is `pack` then `unpack`, so
  it merges exactly as the file does; Search's allowlisted settings come too (not on
  a test run).
- [x] **One merge for all** (`profile::bring_in`): what a browser has comes out as an
  `Incoming`, as a profile file does. Tabs already open here stay out, with the groups
  that leaves empty (`Incoming::leave_out`, as Search's `takeSession` skips them), so
  bringing a browser over again adds only what is new. History keeps the larger
  count, as before.
- Deviations: pinned tabs come as tabs (no pins yet); a Chromium browser's bookmarks
  always go in a folder of its name whose identity is the browser's, so a second time
  adds no second folder (Search files them at the top when it has none; Tap shows no
  bookmarks yet either way); a group a returning tab was in comes as a new group of
  the same name, as Search's `takeSession` does.
- Not yet, with their triggers: choosing a profile (Search's picker), Firefox and the
  other Mozilla browsers, Safari (its data is kept from other apps), passwords
  (no vault — M5), extensions (M7), Arc's spaces and pins (`arc.import`, M4), icons
  from the browser's `Favicons`, the File menu's own Bring Things Over…, and the
  Welcome's short version.
- Checked: `cargo test` (tap-core 86, tap-import 18, tap-shell 37), clippy
  `-D warnings`, nightly fmt. Through the bench in a test world holding a made-up
  Chrome (two profiles, `Local State` naming them, a session with two groups and a
  `chrome://newtab`, 41 places, three bookmarks) and a copy of this Mac's Search
  store: `bring` found `["Search", "Chrome"]` and the row said so; Chrome brought 4
  tabs in 2 groups (Rust 学习 folded, Reading open), 40 places and 3 bookmarks in a
  Chrome folder; again, no tabs and no bookmarks; Search brought 27 tabs in 3 groups
  (软件开发 and 娱乐 folded, AI Providers open), 2,000 places and 368 bookmarks; two
  closed Chrome tabs came back through the pull-down's own path (`bring menu=true`).
  Not checked by eye: the pull-down and the confirming sheet (the shell here can't
  reach the window), and a real Chrome profile (none on this Mac: only Chrome for
  Testing's crash folder), which M6 still asks for.

### 0.1.5 — Passwords: brought over, filled in, saved

At the user's request (2026-11-04: "所有的登陆名/密码等，好像没办法从search/chrome迁移进来？
bring things over应该也包括这些吧（自动填充）", then "brings things over / 密钥迁移 功能请你直接
照抄 ../search 的"): the core of M5's Forms and Vault and M6's password import,
brought forward as a patch release.

- [x] **Bring things over is Search's `ImportPanel`** (`bring.rs`, a sheet on the
  window, opened by Settings' Bring Over): the browsers found, each counted off the
  main thread from its files alone (no key asked, nothing decrypted); the profile
  picker ("Most recent: X", each profile, All profiles; `chromium::Pick`); a switch
  each for Passwords, Bookmarks, History, Tabs & tab groups (and Settings, for
  Search); Bring them in; the said lines; File… for a passwords CSV or a profile file.
- [x] **Chromium passwords** (`tap_import::chromium::passwords`, from `Import.swift`):
  `Login Data` and `Login Data For Account`, copied before reading; PBKDF2-SHA1
  ("saltysalt", 1003 rounds), AES-128-CBC with sixteen spaces, `v10` only, the 32-byte
  site hash skipped when present; never-sites kept. The key comes from the keychain
  (`<Name> Safe Storage`), one macOS question; a test world's browser keeps it in a
  `Safe Storage` file beside its profiles, Search's rule.
- [x] **The vault** (`keychain.rs`, `vault.rs`, from `Vault.swift`): internet
  passwords under the label `Tap` (`Tap (<world>)` on a test run), Search's
  dictionaries one to one, through Security.framework directly (a deviation from
  §5.3's `security-framework`: the calls are four, and the dictionaries are Foundation
  ones bridged). **Tap's items carry a security domain** (their label): the keychain
  knows an internet password by server, account, protocol, kind, port, path and
  security domain, never by label, so without it saving an account Search also keeps
  failed as a duplicate (found in the test world, `errSecDuplicateItem`). Registrable
  domains use CFNetwork's `_CFHostIsDomainTopLevel`, as Search's `Registrable.swift`.
- [x] **Search's passwords on first use** (§7): Bring things over from Search turns
  on `passwords.fromSearch`; its drawer's accounts are listed beside Tap's (names
  only); a pick reads the secret off the main thread (macOS asks once) and copies it
  into Tap's drawer.
- [x] **Forms** (`forms.js` is based on Search's script;
  `forms.rs`, `signin.rs`): the accounts hang under the box (five at most, newest
  first, the name the first step sent first; only plain-http ones on a plain-http
  page; a half second before a click counts), a pick fills through the field's own
  setter with the page's scheme and host checked when the script runs, and a sign-in
  that took is offered a place: Save / Update, Not now, Never here, in a bar at the
  bottom. `passwords.save` and `passwords.fill` are live (55 of 81).
- [x] **Settings' rail lines up** (the user's screenshot, 2026-11-04): each page's
  icon sits at the left of a fixed column and its name starts at one x, rather than a
  button's leading image pushing each name over by its own icon's width.
- Deviations: no Passwords page yet (Open… opens Keychain Access, where Tap's are
  listed), no LocalAuthentication, no Apple Passwords, no passkeys; the list takes no
  arrow keys; private tabs don't exist yet, so nothing checks for one.
- Checked: `cargo test` (tap-core 86, tap-import 31, tap-shell 39), clippy
  `-D warnings`, nightly fmt. In a test world, through the bench and screenshots,
  with a made-up Chrome (an encrypted `Login Data`: two localhost accounts, one
  github.com, one never-site) and a local sign-in page: the sheet counted "1 bookmark
  · 3 passwords"; Bring them in said "3 passwords" and the bookmark; the email box
  hung alice and bob; a pick filled `alice@example.com | Tr0ub4dor&3`; a new sign-in
  was offered and saved; the same password again offered nothing; a changed one
  offered Update; Never here stopped the offers; a seeded Search item was listed
  "from Search", filled, and moved into Tap's drawer; after a pick the list came back
  on the next visit and went on blur. Found and fixed on the way: a script message's
  `null` (NSNull) dropped the whole message; the save check read the keychain on the
  main thread (now off it). Not checked: a real Chrome (none on this Mac) and the
  macOS prompt for a real Search item.

### 0.1.6 — The strip moves, groups have colours, and an address bar

At the user's request (2026-11-04: "tab groups 应该支持animations/transition移动顺序；tab同理；
tab group展开，收缩应该有对应的统一风格的过渡效果；tab栏下方增加永久展示的一个（可配置隐藏）address
bar，跟随当前打开的tab更新对应的url等信息（SSL etc 行业标配）", then "直接推送main分支，发布patch
版本", "tab group好像还不能删除, 右键菜单audit一遍", "tab group 本身应该支持类似chrome一样的自动
分配不同的底色", "back, forward, refresh button跟着新增的地址栏，放到它的左侧（经典排版）").
Part of M3's "tab drag within the strip" and the tab menu, brought forward as a patch
release.

- [x] **One motion for the whole row** (`motion.rs`, Search's `Motion`): every slot
  of the strip (tab, heading, plus) is keyed by identity, not place, and springs to
  where the layout puts it on `settle` (0.30, 0.86). Search's
  `.animation(Motion.settle, value: tabs.map(\.id))`, run by hand since the strip
  is one drawn view: a row whose slots changed (opened, closed, moved, folded) goes
  there on the spring; a row whose slots didn't (a resize, the field widening the live
  tab) goes at once, anything in flight kept in flight. A slot that leaves fades where
  it stood, one that comes fades in. Reduce Motion: no motion. Settings › Reduce
  animations in Low Power Mode, while the Mac saves power: a short damped spring
  (0.12, 1.0) in place of Search's 0.16 s ease-out (`fixed-deviation`: one curve
  family, one integrator). Frames come from the strip's `CADisplayLink`, running only
  while something moves.
- [x] **Tabs drag** (Search's `Carried`): pressed and moved 5 points, a tab follows
  the hand exactly, lifted on its pill with Search's shadow, and passes a neighbour
  once its leading edge crosses the neighbour's middle; the others glide aside. Let
  go, it springs into its slot. Only within its own block (the pins, its group, the
  tabs in no group): `Browser::move_tab(id, place)`. A drop into another group is
  Move to Group's, below.
- [x] **Groups drag**: a heading carries its group, heading and tabs, past the other
  groups the same way (`Browser::move_group(from, to)`). `fixed-deviation` in feel,
  not outcome: Search reorders a group by dropping it on another's heading
  (`moveTabGroup`); here it moves along the row like a tab, so both read as one. A
  click on a heading (or on the live tab) acts on release, so a press can become a
  carry.
- [x] **Fold and unfold** on the same spring: a folding group's tabs slide back under
  its heading as they fade, and come out from under it when it opens.
- [x] **Group colours** (Chrome's; Search's groups have none): each group wears one
  of nine colours, given when it is made, one no group has yet or else the least used
  (`groups::next_colour`, Chrome's `GetNextColor`), kept in the session as Tap's own
  `color` key (Search's reader passes over it). Groups that come without one (Search's
  sessions, an older Tap's) are given theirs in order; a Chrome session's group brings
  its own (`SetTabGroupMetadata2`'s colour, read now). The heading is a chip of the
  colour and a line of it runs under the group's tabs. Not yet: choosing a colour.
- [x] **Address bar** (`address_bar.rs`; Tap's own, Search has none: its live tab is
  the address): a 38-point bar under the strip, on by default, Settings › Appearance ›
  Window › Show the address bar (`toolbar.address`, Tap's own key; 56 of 82 rows
  live). Back, forward and reload move onto its left while it is up (the classic
  layout), and back to the strip's end when it isn't. It follows the tab in front:
  its address, the host in ink and the rest dim; a badge for the connection
  (`tap_core::location::security`): lock for https whose every resource was (WebKit's
  `hasOnlySecureContent`, a new `TabFields::SECURITY`), a slashed lock where some
  came over plain http, a warning and "Not secure" on plain http, and marks for a
  file, this Mac and Tap's own pages; the load's progress along its foot. A click on
  the address opens the field there (⌘L too), the suggestions hanging under it; a
  click on the badge says what the connection is and, for https, the certificate from
  the page's `serverTrust`: issued to, issued by, valid until. With the bar up the
  live tab keeps its title (the bar says where), so Show the address on the tab
  you're on only applies with the bar off, and a click on the live tab no longer opens
  the field there.
- [x] **The right-click menus, audited against Search's** (`TabMenu`,
  `GroupHeadingShared`): the heading's menu had only Collapse/Expand, so a group could
  never go. Now Rename Group…, Collapse/Expand Group, Ungroup Tabs (`removeTabGroup`)
  and Close Group (`closeTabGroup`: every tab, closed as ⌘W closes them and remembered
  together, so one ⇧⌘T brings the group back whole, name, colour, fold and place). The
  tab's menu gains Move to Group (New Group, each group, Remove from Group), Rename…
  and Put to Sleep, and its close items are in Search's order (Close Tab, Close Other
  Tabs, Close Tabs to the Right, Reopen). Renaming is a sheet, not Search's in-place
  field (`fixed-deviation`: the strip draws its tabs, it has no field per tab). Still
  waiting on their subsystems: Pin, Mute, Copy Clean Link, Site Information, spaces,
  Split View, windows.
- Checked: `cargo test` (tap-core 100, tap-import 31, tap-shell 53), clippy
  `-D warnings`, nightly fmt. In a test world through the bench, real mouse events
  sent through the window (`drag`, `mouseup`) and screenshots: two groups made came
  out blue and red and kept them across a relaunch; 60 ms after a fold the groups to
  the right were still gliding left and the folded tab fading, settled by 0.8 s; a tab
  carried 120 points rode lifted over its neighbour, which slid aside, and sprang home
  on release; a group carried by its heading moved past the other; a click on a folded
  heading opened it on release; https://example.com showed the lock and its menu "Connection
  is secure", http://neverssl.com the warning and progress; ⌘L opened the field in the
  bar with its suggestions under it; Show the address bar off put the helm back at
  the strip's end and the live tab said "Not secure — neverssl…" again. Found on the
  way: a test run's events reach the window only while Tap is the active app.

### 0.1.7 — A group's colour chosen, the live tab glides, tabs picked together

At the user's requests after 0.1.6 (2026-11-04: "tab group 支持手动选择颜色"; "因为我们有了
独立的地址栏，所以tab点击后就不用拓宽展示url了，就保持原样即可，加个active效果就够了，push changes for
patch release"; "tabs要支持多选（click+shift+click)…cmd+click+click 多选不相邻的tab 选择后…cmd+c
…把所有圈选的tab的url作为list粘贴到粘贴板").

- [x] **Color in the heading's menu** (Chrome's group editor, as a submenu): the nine
  colours, each with its dot drawn for the menu's light or dark, the group's ticked;
  a pick is the group's from then on (`Browser::set_group_colour`, only one of
  `groups::COLOURS`), kept in the session. Two groups may share a colour once chosen;
  a new group is still handed one nobody has.
- Checked: `cargo test` (tap-core 101, tap-import 31, tap-shell 54), clippy
  `-D warnings`, nightly fmt. In a test world: a right-click on a heading (a real
  event through the window) opened the menu with Color › Blue ticked; Purple, picked
  from the keyboard, turned the chip and its line purple, and it was still purple
  after a relaunch.
- [x] **The live tab stays as it is** with the address bar up: no field widening it
  and no address in place of its title (0.1.6 already, rechecked: a click on the live
  tab does nothing, ⌘T's and ⌘L's field opens in the bar). A widening seen after the
  update is a Tap started before it: an update installs when Tap quits.
- [x] **The live tab's pill glides** from the tab that was live to the one picked
  (Search's `matchedGeometryEffect("live")` on `glide`, 0.34/0.82, `motion::Glide`),
  riding on both tabs where they are drawn, so it stays on them while the row moves;
  and it sits lifted, a soft shadow in a light window and a faint edge in a dark one.
  Under the pointer the live tab keeps it rather than the hover grey.
- [x] **Tabs picked together** (Search's `extendTabSelection`, `selectedTabIDs`,
  `copySelectedTabLinks`): ⌘-click toggles a tab (the live one joins as a selection
  starts), ⇧-click picks the run the row shows from the last tab clicked, ⌘⇧ adds a
  run; picked tabs wear the accent lightly. While any are picked the row takes the keys
  (`acceptsFirstResponder`), so **⌘C copies their addresses, one to a line, in the
  row's order** (Tap's addition: Search copies them only from the menu) and Esc lets
  them go; a plain click, or the keys going elsewhere, lets them go too. Right-clicked
  among them, the tab's menu ends with Copy N Addresses. Not yet: closing or moving
  the picked tabs together.
- Checked: `cargo test` (tap-core 102, tap-import 31, tap-shell 56), clippy
  `-D warnings`, nightly fmt. In a test world with real mouse events: the pill caught
  between two tabs 60 ms after a switch and on the new one by 0.9 s, lifted in a light
  window; two ⌘-clicks picked three tabs and gave the row the keys, ⌘C (through the
  Edit menu) put their three addresses on the pasteboard one to a line, a ⇧-click
  picked the run from the last clicked, and a plain click let them go and gave the page
  the keys back.

### 0.1.8 — Picked tabs close and move together

At the user's request (2026-11-04: "多选的tab支持一起关闭和拖动"), after 0.1.7.

- [x] **Closing** (Search's `closeFront` and `closePickedTabs`): ⌘W closes the picked
  tabs when any are, else the tab in front; right-clicked among them, Close N Tabs
  stands where Close Tab was. They close as ⌘W closes each, from the right and the
  one on screen last, remembered as one batch, so one ⇧⌘T brings them all back where
  they were (and a group they emptied with them). The cross and the wheel still close
  just their own tab.
- [x] **Carrying** (Chrome's way; Search carries one tab only): a carry from one of
  several picked tabs gathers the others picked in its block beside it, in the row's
  order and round where it stands (`Browser::gather`), and the run goes as one, passing
  a neighbour at its middle (`Browser::move_tabs`; `move_tab` is a run of one). Picked
  tabs in another block (a group, the pins) stay where they are. The picked stay picked
  after the drop, and the row keeps the keys. Pressing one of several picked no longer
  picks it or lets the others go until it is let go without a carry: picking a tab
  gives the page the keys, and the row losing them let the selection go before the
  carry could take it (found in the test world).
- Checked: `cargo test` (tap-core 104, tap-import 31, tap-shell 56), clippy
  `-D warnings`, nightly fmt. In a test world with real mouse events: a, c and e picked
  of a–e, c carried right gathered a and e beside it and moved the three past d
  (b d a c e), lifted together on the way and still picked after; ⌘W closed the three
  (b d left, d in front); one ⇧⌘T brought them back in place, a in front.

### 0.2.0 — Chrome polish

At the user's request (2026-11-05: "polish ui ux"), one polish-only pass over the
tab row, address bar, menus, Settings rail, find bar and window chrome. No behaviour
changes, no new features; the two noted deviations from Search's menu are deliberate
(a count in Copy N Addresses, a Color submenu Search's groups lack).

- [x] **Strip** (`strip.rs`): the plus always draws (only its hover fill gates on the
  pointer); the press give lands at once under Reduce Motion as well as Low Power;
  the live tab keeps a hover wash; a narrow tab's letter tile reads in a light window;
  the load line reads on the live pill.
- [x] **Address bar** (`address_bar.rs`, `offers.rs`): progress shows only while
  loading and not editing, from zero with no stub; the badge menu anchors to the
  capsule; the offer row keeps its full gap; the certificate lines stay readable to
  VoiceOver.
- [x] **Menus** (`tabmenu.rs`, `browser.rs`): no stray divider after Move to Group;
  Copy N Addresses dims when no picked tab has a page (`Browser::can_copy_selected`,
  tested); Color sits between dividers; the group rename sheet carries the tab's
  helper line; the picked colour's tick is drawn into its swatch (AppKit hides a tick
  beside an image).
- [x] **Settings rail** (`settings.rs`): hidden-until-found rows tint fully when dimmed;
  an info button carries the tooltip text for pointer and keyboard alike (tooltips stay);
  the empty state says only handled settings are listed; find headings breathe; rail
  icons sit on the pill rect, wide-row icons on the text.
- [x] **Find bar** (`findbar.rs`): the count sizes itself and grows left, never under
  the arrows; clicks pass through outside the card; previous/next dim with no matches;
  the options button tints while a switch is on; one 26-point pitch throughout.
- Checked: `cargo test` (tap-core 105, tap-import 31, tap-shell 56), clippy
  `-D warnings`, nightly fmt. Not checked by eye: light/dark feel of each wash and
  the VoiceOver pass over the badge menu.

### 0.2.0 — Agents use Tap

At the user's request (2026-11-05: "我们的tap 要专门支持agent协作…mcp还是配套自动安装的
cli？我希望我们所有的agent都能知道使用Tap来做必要的browser tool使用"). Coding agents on
this Mac (Claude Code, Codex, Gemini, opencode) drive Tap through a `tap` command, and
learn to through a skill Tap installs. Tap is WebKit, so CDP tools (Chrome DevTools,
Playwright, agent-browser) can't drive it; the bench is already the way in, so this is a
client and a few verbs on it, not a server. Delivered as a PR at the user's request.

- [x] **`tap`, the CLI** (`tap-app/src/cli.rs`): the app's own binary, a client when it
  has arguments and the app when it has none, so there is no second binary to bundle
  and sign. Text out for agents, `--json` for the bench's answer. Starts Tap in the
  background (`open -g -b`) when nothing listens and waits for the socket. Every verb
  but `open` names its tab: an agent never acts on the tab in front by accident.
  Verbs: `tabs`, `open`, `go`, `close`, `select`, `wait`, `text`, `snapshot`, `click`,
  `fill`, `press`, `eval`. Pure std and `serde_json`; the world (`TAP_PROBE`) as the app.
- [x] **`snapshot` and `act`** (`bench.rs`, `assets/js/agent.js`): the page's visible
  controls as numbered refs (`[e3] button "Sign in"`), and click, fill or press on one.
  Run in Tap's content world through `call_in_world`, so the page sees neither the
  refs nor the ref text, and the DOM isn't marked. Clicks and keys are dispatched
  events, not the window's: an agent's tab stays behind yours.
- [x] **On by default** (`settings_index.rs`): the `bench` row reads "Let agents use
  Tap" and defaults on (`fixed-deviation`: Search's is off and behind a keychain
  mark). The guard stays the same-uid check on a 0600 socket.
- [x] **Installed by Tap** (`tap-shell/src/agents.rs`): at launch and when the switch
  moves, in real runs from inside the app: `~/.local/bin/tap` → the app's binary,
  `~/.agents/skills/tap/SKILL.md` (`assets/agent/SKILL.md`, rewritten when it
  changes), `~/.claude/skills/tap` → that folder. Never replaces what isn't Tap's; off
  takes away only Tap's.
- [x] **A page read wakes a sleeping tab where it stands** (`Browser::wake`, now public;
  bench `with_page`): a session's tabs come back asleep, and an agent reading one of
  yours shouldn't have to bring it to the front first. `snapshot`, `act`, `text` and
  `eval` wait up to 15 s for it to load.
- Checked: `cargo test` (tap-app 5, tap-core 106, tap-import 31, tap-shell 61), clippy
  `-D warnings`, nightly fmt; the installer's ownership rules against a home of its own
  (a foreign command, skill and Claude link left alone on and off; a moved app
  repointed; a changed skill rewritten), and seen failing with the skill's mark check
  broken. In a test world, driven by the built `tap` itself against a local page: open
  (behind the tab in front), snapshot (the hidden button left out, the password shown
  as dots), fill with 世界, a checkbox clicked, a list's option picked, Enter in the
  field submitting its form (`Sent hello 世界 / true / b`), a link followed to the next
  page, a stale ref refused with exit 1, and a restored, sleeping tab read without
  leaving the tab in front. Not checked here, the user's Tap being open on the same
  data: the real run's launch (on by default, the install from `/Applications`) and
  `tap` starting Tap; both on the release's install.
- Not in: screenshots (a background tab's WKWebView has nothing drawn to snapshot), a
  mark on agents' tabs, handing a tab back for a sign-in, MCP (a `tap mcp` verb on
  the same client, if an agent without a shell needs one).
### 0.2.0 — A click on any tab puts the cursor in the field

At the user's request (2026-11-05: "每当点击任何一个tab，甚至是new tab，自动focus/选中地址栏的url" —
the cursor, not the pointer), a deliberate departure from Search's `DragStrip`:
Search opens the field only on the live tab, and only while the address bar is away.

- [x] **Any plain click opens the field** (`strip.rs`): a plain press on any tab —
  the live one in either mode, or one behind — sets `Click::Edit`, so on release
  `begin_editing(false)` opens the field on the tab now in front, its address selected
  (`place_field`'s `makeFirstResponder`); a blank one opens the field empty with the
  cursor in it, as the plus already does. The press still selects a tab behind at
  once; a carry clears the click as before, and ⌘/⇧ picks and the unpick release keep
  the page's keys.
- Checked: `cargo test` (tap-core 107, tap-import 31, tap-shell 57), clippy
  `-D warnings`, nightly fmt. Not checked by eye: a real click in a running app
  (`scripts/bench click` over the tab row, then `field`).

### Unreleased — One line rule for the chrome's text and pills

At the user's request (2026-11-05: "全app审查这类alignment和高度不一致的问题（比如tab和group
title）地址栏url和SSL锁icon等等"), audited in a running test world (`TAP_PROBE`, screenshots
measured by pixel row) rather than by reading offsets.

Found: a group's chip was 22 points tall beside a 30-point tab pill; the field opened
1.5–2 points above the line it replaced, in the address bar (`y − 1.5`, its text
above the lock) and in the tab row (`+5` against the title's `+7`); and every line of
chrome text carried its own hand-picked offset (`+7`/`17`, `+5.5`, `(ROW − 17)/2`,
`−0.5`), right by luck in some places and half a point off in others.

- [x] **One line rule** (`palette.rs`): `line_height` is the chrome font's own line as
  `draw_text` lays it out; `text_line(rect, left, right)` centres one line in any row;
  `field_frame(line)` puts a borderless field's text exactly on that drawn line. Tests:
  `a_line_sits_centred_in_any_row`, `the_field_opens_on_the_drawn_line`.
- [x] **Strip** (`strip.rs`): a heading's chip is the tab's own pill (full row, the
  chrome radius), so chip and tab share top and foot; titles, digits, the letter
  tile's letter, a gone slot's label and the field all go through the rule.
- [x] **Address bar** (`address_bar.rs`, `window.rs`): the address and the open field
  share one line, level with the badge; the field uses `palette::font`.
- [x] **Suggestions and offers** (`window.rs`, `offers.rs`): rows centre by the rule.
- Left as is: sign-in's two-line rows (two lines, not one centred); in icons mode the
  field still starts where the icon was, as before.
- Checked: `cargo test` (tap-core 114, tap-import 31, tap-shell 68), clippy
  `-D warnings`, nightly fmt; by pixel row at 2x, the address's x-height at rest and
  open both start on row 138, the lock's centre on the cap height, and the tab-row
  field on the title's rows 43–65.

### Unreleased — → takes the grey ending too

At the user's request (2026-11-05: "right arrow key 应该也可以实现自动完成补全不仅仅是tab按钮，比如地址栏的url自动完成").

- [x] **→ takes the ending** (`browser.rs` `accept_ending_on_right`,
  `right_takes_ending`; `window.rs` `moveRight:`): as Tab does, but only from the end
  of the typed text with nothing selected, as Chrome and Safari do; anywhere else →
  moves the caret as ever. Test: `right_arrow_takes_the_ending_only_from_the_end`.
- [x] **Bench**: `fieldkey` takes `left` and `right`.
- Checked: `cargo test` (tap-core 117, tap-import 31, tap-shell 68), clippy
  `-D warnings`, nightly fmt; in a running app through the bench, "exa" + → became
  example.com, and ← then → moved the caret without taking it.

### 0.2.17 — Agents see and fill more of the page

At the user's request (2026-11-07: "检查tap浏览器对ai agent协作的友好度，找优化点和bug",
then "根据你的建议来优化"), an audit of the agent channel, fixed in the page script and
the bench, verified in a `TAP_PROBE` world against a local probe.

- [x] **A background tab's block controls stay visible** (`assets/js/agent.js` `shown`):
  a tab behind the user's lays block elements out at zero width, so the old size check
  hid every `contenteditable` and `div[role]` control an agent needs. `shown` now trusts
  `checkVisibility({checkVisibilityCSS:true})`, which answers without laid-out size and
  catches an ancestor's `display:none` the old fallback missed.
- [x] **Open shadow roots are listed** (`agent.js` `collect`): snapshot walks every
  element and recurses into open shadow roots, so a web component's buttons and fields
  get refs and can be clicked and filled; closed roots stay encapsulated.
- [x] **A file input is labelled `file`** (`agent.js` `role`, `fill`): no longer a
  nameless textbox; filling it is refused clearly instead of throwing.
- [x] **`eval` awaits** (`bench.rs` `evaluate`): runs in the page's own world through
  `callAsyncJavaScript` (`pageWorld`), so a promise (`fetch`, …) resolves to its value
  instead of `{}`; the page's variables stay reachable and errors still report.
- [x] **Regression test** (`scripts/bench test-agent`): forces a hidden tab (the
  zero-width condition), checks the block/shadow/file controls are listed and fillable
  and that `eval` awaits; seen failing without the `shown` fix.
- Checked: `cargo test` (tap-app 7, tap-core 130, tap-import 35, tap-shell 101);
  `test-agent` (12 checks) and `test-forms` (36) in WebKit; clippy clean on the changed
  crates. The workspace `clippy -D warnings` gate hits a pre-existing unknown lint
  (`icons.rs` allows `clippy::duration_suboptimal_units`, unknown to the installed clippy
  0.1.94), unrelated to this change.
- Left as is (found, not fixed here): refs are positional, so a re-render can point one
  at a different control; iframe controls are still out; `press` has no native key
  defaults beyond Enter; a request that times out may still act.

### 0.2.15 — A second Tap no longer kills the agent channel

From an agent's test report (2026-11-07): starting a second Tap permanently wedged
the first run's bench socket — `listen()` removed and rebound `bench.sock`
unconditionally, so the first run was left answering on an unlinked inode and every
`tap` command failed with Connection refused until Tap restarted. The updater's
restart and running a build from the tree are the same scene.

- [x] **A live socket is left alone** (`bench.rs` `take_socket`): before binding, a
  connect probe — someone answering there means another run holds it, and this one
  stays quiet; only a stale file (there but nobody answers) is removed and bound.
  Test: `a_second_run_leaves_a_live_socket_and_takes_a_stale_one`.
- [x] **`stop` only takes away its own file** (`OWNED`, by device and inode): quitting
  or switching the bench off never unlinks another run's socket.
- [x] **Quit takes the socket with it** (`menu.rs` `will_terminate` → `bench::stop`),
  so no stale file is left for the next launch to clear.
- [x] **The copy says what to do** (`cli.rs`, `SKILL.md`): Tap-isn't-listening now
  ends with restarting Tap (or flipping the switch off and on), and the skill's
  `tap`-not-found row carries the same third cause.
- The report's P1 (eval of `querySelector("h1")` throwing on example.com) is not a
  bug: example.com's page holds no `h1` anymore (a redesigned multilingual page, an
  SVG and paragraphs), so the throw is correct. Verified against the live page, with
  `querySelector("p")` answering as the control.
- Checked: `cargo test` (tap-core 129, tap-import 35, tap-shell 100, tap-app 7), clippy
  `-D warnings`, nightly fmt. In a scratch `TAP_PROBE` world through the built `tap`:
  a second instance logged "stays quiet" while the first answered throughout, before
  and after the second was killed; a graceful quit removed the socket file; after a
  kill, the relaunch took the stale socket over and answered.

### 0.2.14 — Login pages fill themselves in

At the user's request (2026-11-07: "访问某网站/登陆表单应该帮我先自动填入，而不是一定要等我
focus进输入框才显示选择提示"). A deliberate departure from Search (`fixed-deviation`):
Search fills nothing on its own ("the way Safari does it"); Tap puts the newest kept
account in when the form is found, and the list under a focused box is there to switch.

- [x] **Autofill on form found** (`forms.js` `autofill`, `forms.rs` `form`/`autofill`,
  `signin.rs` `autofill`/`page_finished`/`forget_tab`/`pick_for_autofill`): the page's
  `form` message, ignored until now, fills the newest account kept for the site
  (the name the step before was sent with first, as the list does), wide across the
  site's hosts, plain-http pages only what was kept in the clear, behind
  `passwords.fill`. The script fills only empty boxes — never a new password, never
  another name's box, never a pair the list would not hang from — with the page's
  scheme and host rechecked when it runs. Once per page, cleared for a new document;
  the secret is read off the main thread and a refused fill may try a later form.
  The list, the save bar and the picker are unchanged; `passwords.fill` now reads
  "A login page fills itself in; click a box to switch accounts".
- Not in: submitting the form, first-step (name-only) pages, passkeys, Apple Passwords.
- Checked: `cargo test` (tap-core 129, tap-import 35, tap-shell 99, tap-app 7), clippy
  `-D warnings`, nightly fmt, `scripts/bench test-forms` (36 checks, 6 new). In a test
  world through the bench against a local login page: saved alice once, the revisit
  held `alice|Tr0ub4dor&3` with no focus; `passwords.fill` off left the boxes empty
  and on filled them again; focusing the box still hung alice for a switch.

### 0.2.13 — Hand a page to an agent

At the user's request (2026-11-07: "从ai agent 友好角度出发…右键点击tab或者页面的时候，可以多一个
复制tab详情的功能…能绝对定位到某页面和浏览信息"), the first of a three-step list agreed in the same
conversation. Agents could already drive Tap (0.2.0); the person had no way to point one at
*this* tab or *this* element. Steps 2 and 3, still open: `tap console`/`errors` (a page-world
recorder, so only where switched on), a front-tab screenshot, `tap requests`; then a mark on
agents' tabs, handing a tab back after a sign-in, Copy Details for several picked tabs.

- [x] **Ids that outlast a restart** (`session.rs` `tapID`, `tapNextID`; `browser.rs`
  `tab_from`, `shape`): a tab comes back under the id it had, unless another tab already
  has it (a hand-edited or merged session); the next id is kept too, so a closed tab's
  id is never handed to another page and an agent holding it can't reach the wrong one.
  Tap's own keys: Search's reader passes over them. Test:
  `a_tab_keeps_its_id_across_a_restart_and_a_closed_one_s_is_never_reused`.
- [x] **Copy Tab Details** (`inspect.rs`, `tabmenu.rs`; the strip's tab menu and the
  page's): a plain-text card an agent can act on: `Tap tab <id> — <label>`, the URL
  (with a warning when a parameter's name says it carries a secret: `token`, `code`,
  `key`, …), state with the main document's HTTP status (`decidePolicyForNavigation
  Response`, main frame), group and place, then what the page says of itself (lang,
  content type, charset, canonical, description, og:*, viewport, scroll, selection), then
  the `tap` commands that reach it. A sleeping tab isn't woken for it: the card says
  asleep and carries the model's facts only.
- [x] **Copy Element Details** (the page's menu: `PageView`, a `WKWebView` subclass
  adding both items in `willOpenMenu:withEvent:`): the right-clicked element's unique
  selector (test ids and ids first, `:nth-of-type` only where tag and classes don't
  tell it apart), its shadow host, tag, role and name, text, link, box, computed style,
  and its markup cut at 800 characters; a `tap eval` line that finds it, shell-quoted.
  A recorder in Tap's world (`inspect::RECORDER`, main frame, document start) keeps
  what the `contextmenu` event was on, so it sees into shadow roots; when it wasn't at
  the menu's point (a click inside an iframe), `elementFromPoint` names what is there.
  Passwords and hidden inputs give no value.
- [x] **`tap info <id>`** (`bench.rs` `info`, `cli.rs`): the same card, waking a
  sleeping tab first as the other page reads do; a blank tab answers without one. The
  skill (`assets/agent/SKILL.md`) tells agents what a pasted card is and that its id
  is the one to use.
- Read only when asked (a menu item, `tap info`), never on a timer: every script the app
  runs in a page ends its user activation (0.2.12).
- Checked: `cargo test` (tap-core 129, tap-import 35, tap-shell 98, tap-app 7), clippy
  `-D warnings`, nightly fmt. In a test world against a local page: `tap info` on a
  loaded tab (HTTP 200, `access_token` warned, canonical, description, og:title,
  scroll and selection), on a blank tab, and on a tab behind (viewport "not on screen");
  quit and relaunched: the page's tab kept id 2 and the next tab got a fresh id; the
  element script through the bench in Tap's world, after a `contextmenu` dispatched in
  the page: the recorded button, a span inside an open shadow root (with `#host`), a
  point with no right-click (`elementFromPoint`), a point off the page (missing); the
  page world can't see the recorder's `__tapPointed`. **Not checked:** the two menus
  opened by a real right-click and the pasteboard written from them (a synthetic click
  landed on another app's window in front of the test world, so it was stopped).

### M3 — Window chrome parity
- Sidebar column and full tab bar, pins, groups, Fold, Peek, Little window
- Bookmarks, bookmarks bar and dropdown, the ⌘K switcher, site card, status line and
  link bubble
- Welcome and What's New
- Every internal page: Settings generated from the index, History, Downloads,
  Bookmarks
- Shortcuts and the menu table
- Tab drag within the strip and column (pins, groups, PinDrop)

#### Tab context menu (from scratch, Search's `TabMenu` as reference)

Tap's strip has no right-click at all; Search's menu (`TabBar.swift:1213`) has
30+ items, most riding on subsystems Tap hasn't got. First cut, only what Tap
can do honestly today:

- [x] `tap-core`: `reload_tab(id)` (⌘R delegates), `duplicate(id)` (the same
  page beside itself, in its group), `close_others(keep)` / `close_right(id)`
  (pinned tabs are kept, not closed — Search puts pins down, Tap has no put-down
  yet, so leaving them is the analogue), `can_reopen()`, `Tab::is_pinned()`.
  Each with a test that fails without it.
- [x] `tap-shell`: `rightMouseDown:` on the strip builds an `NSMenu` for the tab
  under the pointer (Reload Tab, Duplicate Tab · Copy Address, Copy as Markdown
  Link · Close Tabs to the Right, Close Other Tabs · Close Tab, Reopen Closed
  Tab), items enabled from the tab's state, dispatched by tag against a pending
  target tab; a group heading gets Collapse/Expand Group. No `NSMenuDelegate`:
  the menu is built fresh per click, as the find bar's is.
- [ ] Left for later, when their subsystems land: Pin/Unpin (no pin drawing
  yet), Mute (`muted` is only carried in the session, never applied), Rename,
  Move to/New Group, Spaces, Split, windows, multi-select, Site Information.

**Done when:**
- The PinDrag, PinCross, SharedPins, TabGroupDrop, BookmarksBarMenu, SettingsIndex and
  Shortcut ports pass.
- The Python tests for pins, groups, sidebar, bookmarks and `settings_layout` pass.
- Strip and column snapshot diffs are within tolerance.
- The `ax-dump` diff is reviewed.

### M4 — Windows, spaces, profiles, Split View
- `Windows.swift` (Browsers, Front, windows.json)
- Spaces with their own data stores (`Spaces.swift:174`) and space swipe
- Profiles (`Profiles.swift:81`)
- PaneStage, SplitView, SplitTabItem
- Dragging tabs to another window or onto a page edge

**Done when:** `split_view.py` (the harness's own suite), the spaces, profiles and
window Python tests, and the ProfilesTests port pass.

### M5 — Privacy and page features
- Shield (WKContentRuleListStore), Curtain and hidden elements, Dusk, Reader
- Forms and Vault with LocalAuthentication, the Passwords page, Accounts
- Passkeys, Apple Passwords
- GPC and click IDs; notifications, location, camera and microphone
- Autoplay, mute, keep awake, blocked popups, YouTube ads
- Float and native PiP, media keys, full-screen escape, swipe, auto-scroll, kept
  scroll, live rate, 120 Hz
- Substitutions, image menu, print and PDF

**Done when:**
- The Shield, YouTubeAds, Reader*, PasskeyAssertion, ApplePasswords*,
  ScreenPermission, MutedTabRestore and PDFExport ports pass.
- These Python tests are green: dark_pages, popups, autoplay_setting, passwords_list,
  remove_passwords, float_video, call_background, youtube_ads.

### M6 — Importers and migration
- Chrome, Safari, Firefox and Arc; ImportFile (html, csv, Safari zip)
- **#841:** open tabs and groups offered in Bring Things Over (in since 0.1.4, from the
  profile used last)
- **#845:** File › Export Tap Profile, and import of `.searchprofile` and `.tapprofile`
  (in since 0.1.1 for one space and one profile; spaces and profiles of their own come
  with M4)
- "From Search" (in since 0.1.4: Personal's first space, as the file carries it)

This needs M3 (SettingsIndex) and M4 (spaces and profiles) first.

**Done when:**
- The ImportFile, BrowserDiscovery, BrowserImportBoundary and ArcIcon ports and the
  `arc_*` and import Python tests pass.
- A scripted round trip leaves equal `state` answers on both sides: Search
  (parity-pin) exports, Tap imports, Tap exports, Search imports.
- #841 has been run once against a **real** Chrome profile (its PR caveat).

### M7 — Chrome extensions (macOS 15.4+)
- Controller, delegate and adapters
- CRX3 install and updates, unpacked install
- The shims and their native answers (`ExtensionShims.swift:3827-4786`)
- Popups, side panel (with its own `CrashCount`), offscreen pages, worker recovery
- External and native messaging, capture, the extensions UI and store relay

**Done when:** the extension_* Python tests (ubol, rule_redirect, worker_fetch,
module_await, chrome_version, encoding, external_messaging, private_extensions) and
all Extension* ports pass.

### M8 — Distribution and the rest
- Updater, Kil's design (§12), behind Search's "Check for Updates…" item and
  `update.install` setting (in since 0.1.1)
- AppleScript, sharing
- The AI code behind the off switch
- The remaining bench verbs

**Done when:**
- In a test world, a notarized 0.0.N build finds 0.0.N+1 through `TAP_UPDATE_BASE`
  (a local server; honoured only in test runs). It checks the SHA-256, the Developer ID
  team and the notarization, and installs the new build when Tap quits.
- `osascript -e 'tell application "Tap" to get URL of current tab of window 1'` works.

### M9 — Parity gate, then 1.0
**Done when:**
- Every row in Appendix A is `parity`, `dropped` or `fixed-deviation`.
- The Python matrix meets the gate (§9).
- The `ax-dump` diff is reviewed.
- Clean runs on macOS 14, 15 and current VMs.
- The perf budgets in §10 are met.

Then release **1.0.0** by following §12 step by step. That commit is the first
commit of `releases/`.

## 9. Testing and the parity gate

**Reference build.** In `~/code/Search`:

```
git switch -c parity-pin d1639aa && git merge export-search-profile && git merge feature/fix-self-reload-blank-page
./build.sh   # → build/Search.app
```

The commit hash is recorded at the top of this doc. Moving the pin later is a
deliberate decision, logged in §15.

**Porting the XCTests** (77 files plus the two #849 tests and the Swift harnesses):
- **About 50 model-level files** become `#[test]` in tap-core and tap-import, with a
  `FakeHost` that records effects and feeds back delegate facts. They include
  CloseTabsBelow, TabClose, TabSelection, PinDrag, PinCross, SharedPins, TabGroupDrop,
  SessionImport, ImportFile, Markdown, SettingsIndex, Shield, Address, CleanLink,
  EngineSuggest, ApplePasswords, Crx, YouTubeAds, VaultAndUpdater, WindowTitle,
  TabSwitcher and CrashCount.
- **WebKit-dependent tests** go in `tap-web/tests/*.rs` with `harness = false`, using
  `libtest-mimic`. They need NSApplication on the main thread, which cargo's default
  harness can't give. Examples: DownloadLifecycle (one process per case, as
  `run-ci-tests.py` does), FindStage, FormFieldCorpus, ReaderTwin, PageSave,
  PDFExport, CrashLoop. Search's `LocalPageServer` and `LocalDownloadServer` are
  ported.
- **The expectation-list harnesses** (`AddressRegression`, `HistoryRegression`,
  `RegistrableRegression`, `DuskHarness`, `PageFindHarness`) become Rust tests.
- **A differential URL test** runs the address corpus through `fnd::Url` and raw NSURL.

**The bench protocol, ported faithfully and early.**
- `bench.sock` in the store folder, mode 0600, with a `getpeereid` uid check.
- JSON lines, a 4 MB limit, one request per connection and always exactly one answer.
- Timeouts as in Search: 25 s; `wait` + 5 s; 120 s for `import-file`.
- Consent is a keychain mark, as in Search.
- `eval` accepts `"world":"search"` as a test-run alias for Tap's world.
- Each milestone ships its verbs. Search has 156 top-level verbs; their status is
  tracked in Appendix A.

**Running Search's Python suite against Tap.** `xtask sync-tests` copies
`Tests/*.py` from parity-pin into `tests/parity/` and applies `harness.patch`. The
patch turns each hard-coded value into a variable that defaults to Search's value, so
the same suite still runs against Search:

| Variable | Search default | Tap value |
|---|---|---|
| `PARITY_APP` | `build/Search.app` | `target/release/Tap.app` |
| `PARITY_NAME` | `Search` | `Tap` |
| `PARITY_SUITE_PREFIX` | `com.officecommun.search.test` | `com.timwuhaotian.tap.test` |
| `PARITY_WEBKIT` | `~/Library/WebKit/com.officecommun.search` | `~/Library/WebKit/com.timwuhaotian.tap` |
| `PARITY_STORE_PREFIX` | `5E4C` | `7A90` |
| `PARITY_PROBE_ENV`, `PARITY_MEASURE_ENV` | `SEARCH_*` | `TAP_*` |
| `PARITY_JS_PREFIX` | `__office` / `__search` | `__tap` |

The patch also fixes the literals listed in finding 3.8.

**The gate:** `xtask parity` runs the suite against both apps and prints a matrix.
**Every test that is green on Search must be green on Tap.** The only allow-listed
exceptions are `translate_page.py` and the translate rows `settings_layout.py`
expects.

**Behaviour diffing.**
- `xtask diff-run scenarios/*.jsonl` runs the same bench command scripts against both
  apps, normalizes IDs and times, and diffs the `state`, `tabs`, `rows`, `probe`,
  `settings` and `windows` answers and the JSON files each app wrote.
- Off-screen renders (`strip`, `column`, `fold`, `site`) are compared with
  `dssim-core` and reviewed by eye.
- `xtask ax-dump --pid` diffs AX trees (roles, labels, actions) between the two apps.

**CI is Kil's `ci.yml`**, plus the JS drift check. It runs on GitHub Actions
`macos-15`:
- `cargo +nightly fmt --all --check`
- `cargo +stable clippy --workspace --all-targets --locked -- -D warnings`
- `cargo +stable test --workspace --locked`
- `cargo xtask extract-js --check` (from M1)
- `cargo xtask icon`, then `git diff --exit-code`: the PNGs match the SVG

CI never builds releases. App-driven suites (the Python parity matrix, perf) run
locally on the user's Mac, because they need a window session.

## 10. Performance

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"            # never unwind through AppKit/WebKit frames
debug = "line-tables-only"
strip = false              # xtask runs dsymutil, then strip -x, keeping the dSYM
[profile.release.package.tap-import]
opt-level = "s"            # cold code
[profile.dev]
panic = "abort"
[profile.dev.package."*"]
opt-level = 2
```

- **Panics.** Rust panics: the hook writes `crash.log` and an os_log fault, then
  aborts. ObjC exceptions: `NSSetUncaughtExceptionHandler` (`Links.swift:93`), plus
  `objc2::exception::catch` at known throw sites, such as adding a script message
  handler twice (`Tab.swift:26-38`).
- **Codegen.** `-C target-cpu=apple-m1`; arm64 only, as Kil releases. System malloc,
  unless measurement says otherwise.
- **Launch order.** Window, chrome and the active tab's web view come first. After the
  first frame: history parse on a background thread, content rule list lookup (compile
  only if missing), the extension controller, icons, the update check. Keep Search's
  shared process pool (it measured 41–59 ms → 9–10 ms per new tab, `Tab.swift:68-77`).

**`cargo xtask perf --app target/release/Tap.app --app <Search parity-pin> --runs 15`** runs
the two apps interleaved, in probe worlds. The data goes to `perf/results/*.json`; the
summary goes in the table below.

| Metric | Method |
|---|---|
| Cold launch | `posix_spawn` to the first answered `probe` on `bench.sock`, the same definition for both apps |
| ⌘T to typing | `CGEventPostToPid` ⌘T plus one letter; poll `AXFocusedUIElement`/`AXValue` until the letter appears |
| Tab switch | `layouts` stream on one connection, `select` on another; time to the stage change |
| Memory per tab | N = 1, 5, 10, 20 fixture tabs; sum `phys_footprint` over the app and its WebContent pids; report the slope and the UI process alone |
| Binary size | Executable and bundle `du`; `cargo bloat --release --crates` |

**Budgets:** never worse than Search on any metric. Targets: 20% faster cold launch
and ⌘T-to-typing, arm64 binary ≤ 8.9 MB, memory per tab within 2% (WebKit dominates
it).

**Results** (filled in from M0 on):

| Date | Commit | Cold launch | ⌘T→type | Tab switch | Mem/tab | Binary |
|---|---|---|---|---|---|---|
| 2026-11-04 | M0 first window (placeholder, no browser yet) | — | — | — | — | 337 KB executable, 652 KB app, 448 KB DMG (arm64, release, stripped) |

### Form presence checks (2026-11-06)

`forms.js` shares the first positive-size password-box query between pairing and
presence-only checks (`hasPassword`, `named`, `tell`, and both `look` checks).
Only pairing walks the inputs in the password's scope; visibility and pairing rules
remain unchanged. `TAP_PROBE=<world> scripts/bench test-forms` exercises the script
in WebKit with synthetic events and controlled timers/observer callbacks; it needs a
running Tap in that test world and is not part of `cargo test`. It checks behaviour
and input-query counts, not end-to-end timing or real-site compatibility.

A clean runtime flush after the window's first update also skips building its view
snapshot and updating AppKit; effects and dirty changes still take the normal path.
This reduces redundant work, but has not been measured against real page-load times.

### The page-load path (audited 2026-11-05)

An audit of everything between Return and the page being drawn found the load's own
path thin (decide policy, commit, finish) and the waste concentrated where WebKit
reports progress: every KVO tick paid for work that often changed nothing. Seven
changes, each behaviour-preserving:

1. **The session's dirty flag is set only by a real change** (`browser.rs`).
   `page_url`/`page_title` set `session_changed` unconditionally, so each of a
   load's 20–50 progress ticks cloned every tab and encoded the whole session JSON
   on the main thread even when the URL and title were the same. `Tab::set` now
   answers whether it changed.
2. **Session and history files are encoded on the writer thread** (`store.rs`).
   `Effect::Save`/`SaveHistory` carry the `Shape`/`History` themselves; the writer
   thread encodes and replaces, as Search's `Store.swift` does. `save_session_now`
   still waits for the write before returning.
3. **A page that already wears a fresh icon isn't asked again** (`icons.rs`).
   `fetch` now returns early when the index has an icon for the page and its PNG
   is younger than 7 days (Search's `Favicons.fresh`), instead of re-probing,
   re-downloading, decoding, drawing and writing on the main thread on every visit.
   The PNG write goes through the writer thread.
4. **KVO changes are read per key** (`web.rs`). The observer matched on the key
   path and read only what it says, instead of re-reading all seven observed
   properties and calling all five model methods per tick.
5. **A progress notification marks a change only if its step is at least 1/128**
   (`browser.rs`), and `window::update` marks the strip for redraw only when a
   drawn thing changed — a background tab's progress line is drawn nowhere. The bar
   redraws only for window fields or the active tab. Small consecutive steps are
   compared with the last notification, so their accumulated movement can be missed.
6. **`window::update` does its always-work only on change** (`window.rs`). The
   reload/stop symbol is rebuilt and `setTitle` called only when loading or the
   title moved; the downloads door refresh moved to launch, Settings and download
   start/end, where it belongs.
7. **Draw constants are drawn once** (`palette.rs`, `address_bar.rs`). The text
   font and paragraph style, and the address bar's badge symbol image per
   (symbol, warns, look), are cached rather than rebuilt in every draw.

History's `encode` also decorates each visit with its score before sorting
(`score` ran inside the comparator, `O(n log n)` `exp` calls). Not taken, on
purpose: web view pooling (the shared process pool already measured at 9–10 ms
per view), WebContent prewarming (a first-navigation win to measure first), the
keyword and suggest keystroke paths, Shield's per-settings script rebuild, and
`tab_of`'s pointer scan — all smaller, for a later round.

## 11. Top technical risks

Each spike's result is written next to its risk.

| # | Risk | Retired by |
|---|---|---|
| 1 | WKWebExtension protocol conformance from Rust (identity, `respondsToSelector`, completion handlers) | **M1 spike:** `TapExtTab`/`TapExtWindow` with Search's method set, uBO Lite unpacked, `tabs.query`, `tabs.sendMessage`, a popup; one stable adapter per tab; `Reply` guards |
| 2 | URL semantics drift | `fnd::Url` from day 1; the differential corpus test in M1 |
| 3 | Re-entrancy and RefCell panics from synchronous AppKit callbacks | The no-AppKit-while-borrowed rule, effects, deferred re-entrant dispatch, a bench fuzz scenario (click and type during loads) |
| 4 | IME in the address field | NSTextField and its field editor, never a custom `NSTextInputClient`; no completion while `hasMarkedText`; UTF-16 ranges. **M1 spike** with pinyin and Japanese input sources |
| 5 | Tab drag and drop | In-strip reorder by hand-written mouse tracking; out of the strip via `NSDraggingSession` (type `com.timwuhaotian.tap.tab`); page-edge split bands (`TabDrag.swift:84-90`). **M1 prototype**; logic in `tap_core::tab_drag` |
| 6 | Swipe navigation | Search's own disc swipe (`Swipe.swift`), `trackSwipeEvent`, `_setRubberBandingEnabled:`. **M1 spike** in `TapPageView::scrollWheel` |
| 7 | Accessibility | Prefer real controls; `TapView` exposes role, label, press and children (33 labels to carry over); `ax-dump` diff at M3 and M9 |
| 8 | PiP and Float (moving a live WKWebView between windows) | float_video and float_settle in M5 |
| 9b | A named `WKContentWorld` let go between calls: WebKit tears the world down, and the next call gets a fresh one with none of its state | One world object kept for the app's life (`page_find::world`), as Search keeps `Web.world` |
| 9c | Each content world sees its own JavaScript wrapper of a shared DOM object (a `Highlight` read from the page world kept its old size after the find cleared it) | Check what the page shows by eye or from Tap's own world, not from the page world |
| 9a | WebKit objects that forward their messages (`WKNSURLAuthenticationChallenge`): objc2's debug checks look for methods on the wrapper's class and stop | Read through the wrapper's `forwardingTargetForSelector:` target (`dialogs::unwrapped`); every new WebKit object type is exercised through the bench in a debug build before it ships |
| 9 | Private SPI changing between macOS releases | `spi.rs` catalogue, `respondsToSelector` guards, objc2 debug verification, the bench `spi` verb, runs on the macOS 14 VM and the current beta |
| 10 | Weak linking on macOS 14 (extern statics from newer SDKs, such as `WKWebExtensionPermission*`) | `objc2::available!`, `-Wl,-weak_framework,WebKit`; **M1 VM spike** |
| 11 | Panics or exceptions across FFI | `panic = "abort"`, crash.log hook, `exception::catch` at known throw sites |
| 12 | Code size | M0 baseline, `cargo bloat` every milestone, `regex-lite`, system SQLite, NSURLSession instead of a TLS crate, `opt-level="s"` for cold crates |
| 13 | Toolchain mix-up (finding 3.6) | As Kil: Homebrew cargo 1.94 for builds, `rustup run nightly cargo fmt` for formatting, `rust-version = "1.94"` in Cargo.toml; xtask sets `DEVELOPER_DIR` and `--sdk macosx` |
| 14 | Passkey entitlement lead time | Request it in M0; build both entitlement variants from M0; turn on the PasskeyAssertion path once the profile arrives |
| 15 | Swift String semantics (canonical equality, grapheme `.count`) | NFC at comparison boundaries (`unicode-normalization`); `unicode-segmentation` where Search counts characters; property tests on the history and find corpora |

## 12. Release flow (copied from Kil)

Ported from `~/code/kil`: `RELEASING.md`, `scripts/release.sh`, `scripts/bundle.sh`
and `crates/kil/src/update.rs`. Releases are built, signed and notarized **locally**,
and CI never builds them. Users always get one file,
`https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg`, and never see
a version number. Versions are for maintainers: `Cargo.toml`, Info.plist,
`CHANGELOG.md`, commit messages and the About line.

### Credentials (checked on this Mac, 2026-11-04)

- **Signing.** `TAP_SIGN_IDENTITY`, default `"Developer ID Application"`. That is the
  one such identity in the login keychain: **HAOTIAN WU (43VLF3KTFZ)**, the same one
  Kil uses.
- **Notarization.** `release.sh` uses the first of these that is available, in the
  same order as Kil's:
  1. `TAP_NOTARY_KEY`, `TAP_NOTARY_KEY_ID` and `TAP_NOTARY_ISSUER`: an App Store
     Connect API key.
  2. `TAP_NOTARY_PROFILE`: a `xcrun notarytool store-credentials` profile.
  3. The `asc` CLI's stored key, through `asc notarization submit --file … --wait`.
     **This is the one used today:** `asc` 5.17.1 is installed with a default
     credential in the system keychain. No `KIL_NOTARY_*` variables are set, and the
     `kil-notary` profile doesn't exist.
  4. The notarytool profile `tap-notary`.
- **Entitlements.** Hardened runtime, no sandbox. Camera, audio-input and location,
  Search's set.

### `scripts/release.sh`, in order (as Kil's)

1. Refuse to start unless `README.md` links
   `https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg` exactly. This
   check runs before the long build.
2. `scripts/bundle.sh release`. The version is read **afterwards** with
   `cargo pkgid -p tap-app`, because the lock file only updates during the build.
3. `codesign --force --timestamp --options runtime --entitlements … --sign "$identity"`,
   then `codesign --verify --strict --verbose=2`.
4. `ditto -c -k --keepParent` the app into a zip, notarize it, then
   `xcrun stapler staple` the app. The app is stapled first, so a copy dragged out of
   the DMG carries its own ticket.
5. Make the DMG: the app plus a `/Applications` symlink, `hdiutil create -volname Tap
   -fs HFS+ -format UDZO`. Codesign the DMG, notarize it, staple it.
6. `spctl --assess` both. Write `releases/Tap-arm64.dmg.sha256` and
   `releases/latest.toml` (`version = "…"`, `sha256 = "…"`). Fail unless Apple
   accepted both.

### Releasing, step by step

This is Kil's `RELEASING.md`. `CLAUDE.md` tells agents that "release", "发版" or "打包"
means following these steps.

1. **Start clean and green on `main`:**
   - `rustup run nightly cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets --locked -- -D warnings`
   - `cargo test --workspace --locked`
   - `cargo xtask check`
2. **Bump** `[workspace.package] version` in the root `Cargo.toml`. A patch bump
   changes the last number.
3. **Changelog.** Add `## x.y.z — YYYY-MM-DD` under `## Unreleased`.
4. **README.** It describes what this release does, keeps the exact download link,
   and contains no version number.
5. **Run `scripts/release.sh`.** Notarization takes a few minutes.
6. **Install it from the DMG, as a user would:**
   - `hdiutil attach`, then `ditto` the app to `/Applications/Tap.app`, then
     `hdiutil detach`.
   - `spctl -a -vv /Applications/Tap.app` must say **Notarized Developer ID**.
7. **Commit on `main`** everything at once: version, lock, changelog, README, DMG,
   `.sha256` and `latest.toml`. The message is `Tap x.y.z: <what changed>`, with a
   body that lists the changes and the notarization result.
8. **Push** `main`. Check that CI passes and that
   `curl -sIL …/Tap-arm64.dmg | grep -i content-length` matches
   `stat -f %z releases/Tap-arm64.dmg`.

**From 0.1.0** `releases/` is committed with every release, as Kil's is, and the repo
`timwuhaotian/tap` stays public so the raw download link works. Releases before 1.0 are
early ones: each says in the README what it doesn't do yet. From 0.1.1 the updater
reads `latest.toml`; 0.1.0 has no updater, so it is the one release whose users
download again.

### Updater (port of Kil's `update.rs`, replacing Search's appcast updater)

- At launch and every six hours, it reads
  `https://raw.githubusercontent.com/timwuhaotian/tap/main/releases/latest.toml`.
- If the version there is newer:
  1. Download `…/raw/main/releases/Tap-arm64.dmg` into the cache and check its
     SHA-256 against `latest.toml`.
  2. Mount it and verify the app inside: the bundle id; the version; `codesign`
     against `anchor apple generic and certificate leaf[subject.OU] = "<team of the
     running app>"`; and `spctl`, which confirms notarization.
  3. Stage it, and install it **when Tap quits**. Nothing restarts on its own.
- Ad-hoc builds (no team) never update.
- Search's **Check for Updates…** menu item and the `update.install` setting (on by
  default) stay, for parity. Off, Tap checks only when asked (Kil's meaning, not
  Search's "tells you and waits").
- **Restart Now** (the update window) and **Restart to Update to x.y.z** (the Tap menu,
  once staged) quit at once, install, and open the new Tap (`open` after this process
  is gone; a test run's `TAP_PROBE` and `TAP_UPDATE_BASE` go with it).
- `TAP_UPDATE_BASE` overrides both URLs, and is honoured in test runs only.
- Not carried over from Search: the signed appcast, the same-host rule, the Homebrew
  cask, the Intel build and the DMG background art.

### Info.plist (`packaging/Info.plist`, with `__VERSION__` filled in by `bundle.sh`)

Search's keys (`build.sh:180-262`), with Tap's name:
- usage strings for camera, microphone, location and Downloads;
  `NSAllowsArbitraryLoads`
- `LSApplicationCategoryType` productivity, `LSMinimumSystemVersion` 14.0
- **Added by the milestone that handles them**, so a build never claims what it can't
  do yet: URL types http and https and the web, XHTML and alternate document types
  (M2), and `NSAppleScriptEnabled` with `OSAScriptingDefinition` `Tap.sdef` (M8).

## 13. Icon: "Bolt T"

The mark is a heavy T. Its stem breaks partway down into a lightning-bolt zig-zag that
ends in a point. It is a single closed path in `packaging/icon.svg`
(`M262 232 H762 V382 H592 V518 L650 518 L470 800 L512 590 L432 590 V382 H262 Z`, with a
24-unit round-joined stroke), so the icon and the in-app logo (About header, Welcome)
are the same shape.

- **Plate.** Apple's grid: 824/1024 rounded square, corner radius 22.37%, soft shadow
  (as `Icon/icon.swift:89-117`).
- **Colour.** A warm gradient from top-left amber `#FFB238` to bottom-right
  orange-red `#FF5A36`. The mark is near-white `#FFF8F0`.
- **Dev build.** A graphite plate, so a worktree build is never mistaken for the
  installed app.
- **Files, as Kil does them.** `cargo xtask icon` renders `packaging/icon.svg` into
  `packaging/app-icon.png` (1024²), `app-icon-dev.png` and `docs/icon.png`, and they're
  committed.
  `bundle.sh` turns the PNG into `AppIcon.icns` with `sips` and `iconutil`. There are
  no Icon Composer dark or tinted variants; Kil has none either.
- **Done** (2026-11-04): it reads clearly at 32 px, and the graphite dev variant is
  distinct.

## 14. Open items for the user

1. **Request the browser passkey entitlement** from Apple for `com.timwuhaotian.tap`
   on team 43VLF3KTFZ, then create the Developer ID provisioning profile and save it
   as `packaging/Tap.provisionprofile`.
2. **The contact address** shown in Tap (feedback mail, `SECURITY.md`).

Resolved 2026-11-04, by copying Kil's setup:
- Team ID: 43VLF3KTFZ.
- Notarization: the `asc` stored key.
- Toolchain: Homebrew cargo 1.94, with nightly rustfmt through rustup.
- GitHub: https://github.com/timwuhaotian/tap was created **public** (empty, default
  branch `main`); `~/code/tap` has `origin` set to it.

## 15. Decision log

| Date | Decision |
|---|---|
| 2026-11-04 | Full Rust rewrite that copies Search; pure Rust (translation dropped); full parity before release; Developer ID from the start; Bolt T icon; WebKit over Servo and egui; one plan doc (this file). |
| 2026-11-04 | Credentials and the release flow are copied from Kil: Developer ID HAOTIAN WU (43VLF3KTFZ); notarization through the `asc` stored key; local release builds; one `releases/Tap-arm64.dmg` in git on `main`; the `latest.toml` updater; arm64 only; `scripts/bundle.sh` and `release.sh` instead of xtask release commands; no Homebrew cask, GitHub Pages or signed appcast. |
| 2026-11-04 | At the user's request (0.1.6, §8): the strip animates every change on Search's `settle` spring, tabs and groups reorder by hand, groups wear Chrome's colours, the group and tab menus gain what Search's have and Tap can do (a group can now be ungrouped or closed), and an address bar of Tap's own (Search has none) sits under the strip with back, forward and reload on its left, on by default and hidden by Settings › Show the address bar. `objc2-core-graphics` and `objc2-quartz-core` (already in the lock through `objc2-app-kit`) become direct dependencies for fading and frames. |
| 2026-11-04 | The bench came forward from M1 to check the browsing build by, and found three bugs no unit test could: `selectText:` ending the field's own editing session, focus handed out before the page was in the window, and shifted key equivalents that a real keyboard never matches (⇧⌘T must be "T"; ⌃⇧Tab is the back-tab character). |
| 2026-11-04 | Browsing came before M1's foundations, at the user's request: the core model, runtime, window, tabs, field and web views are in (M2 progress). The bench moves up next: driving the UI with synthetic keystrokes is unreliable and can reach other apps. |
| 2026-11-04 | The GitHub repo `timwuhaotian/tap` is public from day one, not only from 1.0. Nothing is downloadable until `releases/` is first committed at 1.0. |
| 2026-11-04 | At the user's request, 0.1.1 brings forward Settings (Search's whole index, the rows Tap acts on live, the rest dimmed with their values), #845 profile files (export, import, opened from the Finder), tab groups in the row, and Kil's updater (§8 "0.1.1"), then ships as a patch release. Profile files are read in memory with the `zip` crate in a new `tap-import` crate, as §4 and §7 planned. |
| 2026-11-04 | No more pull requests ("后面不要PR了，改动直接在main分支推送发布版本"): changes are committed on `main`, pushed and shipped as a release. The upstream Search PRs (#841, #845, #849) aren't updated; their behaviour lives in Tap, and Search-side fixes stay local. |
| 2026-11-04 | At the user's request ("为什么settings里面的大部分选项都没办法真实的使用，修复优化"), Settings shows only the rows Tap acts on, replacing 0.1.1's dimmed rows, and twelve more rows are made live (§8 "0.1.2"). The rest come back as their features land; their values are kept and travel meanwhile. |
| 2026-11-04 | At the user's request ("全部完成后，发布0.1.0版本"), 0.1.0 is released once M2 is done, rather than waiting for full parity at 1.0: browsing, the field, history, page questions, downloads, find, sleeping tabs, session, site icons, links from other apps and #849. `releases/` is committed from 0.1.0; the README says what isn't in yet. Full parity stays the bar for 1.0. Not in 0.1.0 and needed before people rely on it: the updater (M8 in the plan, ported from Kil). |
| 2026-11-04 | At the user's request ("Settings 大部分不能用的功能帮我全部实现", then a patch release), 0.1.3 makes 26 more Settings rows live (53 of 81, §8 "0.1.3"). The rows behind whole milestones (sidebar column, Spaces/profiles/pins, the passwords vault, extensions) stay hidden until those land; reduced forms are marked where they are (dusk, passwords.open, downloads.button, search.suggest). |
| 2026-11-04 | At the user's request ("所有的登陆名/密码等…bring things over应该也包括这些吧（自动填充）", then "照抄 ../search 的"), 0.1.5 brings forward passwords: Search's ImportPanel sheet with Chromium passwords (one macOS question per browser), the vault with a security domain of its own, Search's form script, the account list and the save bar (§8 "0.1.5"). Search's own passwords come on first use, not all at once: macOS asks per item for another app's items, and Search keeps 569 here (§7). `aes`, `cbc`, `pbkdf2`, `sha1` join `tap-import`, as §5.3 planned; the keychain is reached directly rather than through `security-framework`. |
| 2026-11-04 | At the user's request ("tap bring things over 理论上应该是能自动侦测到本地的chrome/search 等类似的profil数据的，但目前没有"), 0.1.4 brings forward the core of M6: Bring things over finds Search and the Chromium browsers on this Mac and brings one's open tabs and groups (#841), history and bookmarks through the profile merge (§8 "0.1.4"). `rusqlite` against the system's libsqlite3 joins `tap-import`, as §5.3 planned. |
| 2026-11-04 | At the user's request, the empty tab row opens a new tab on a double click (Search's `DragStrip.onDoubleClick`, replacing double-click-to-zoom there; a single press still drags the window), and the strip's layout reserves the downloads door beside the helm and pins the plus before those doors when tabs overflow, so the plus never runs under them (`fixed-deviation` from the overflow running to the helm's edge). Delivered as a PR at the user's request, against the 2026-11-04 no-PR decision above. |
| 2026-11-05 | At the user's request (New Tab should open many, and its cross should close it), new tabs pile up: +/⌘T and the "new tab" field command always open a fresh blank tab instead of reusing the blank one in front, and closing the only tab when it is already new closes the window as Search's `close` does (`fixed-deviation` from Search's `newTab` reusing a blank tab). Core asks with a new `Effect::CloseWindow`, carried out by the shell. Delivered as a PR at the user's request, against the 2026-11-04 no-PR decision above. |
| 2026-11-05 | The page-load path is audited end to end at the user's request and tightened in seven places (§10 "The page-load path"), all behaviour-preserving, as a PR like the one before it. |
| 2026-11-05 | At the user's request, agents drive Tap through a `tap` CLI and a skill Tap installs, not MCP (§8 "Agents use Tap"): every agent here has a shell, a skill loads only when a task needs it, and one install in `~/.agents/skills` reaches them all. The bench switch is on by default (`fixed-deviation`). Delivered as a PR at the user's request. |
| 2026-11-05 | At the user's request ("tab的拖拽应该是整个tab都可以拖拽 而不只是四周；…tab拖拽会容易跟着整个app拖拽…tab拖拽到group，自动可以加入这个group"): a press takes a strip slot by its whole column, the strip's full 52-point height and the gap to its right, not just its 30-point pill. The bands above, below and between the pills had been empty title bar, so a press meant for a tab dragged the window. Only the row's truly empty ends still move the window. Tabs carried over another group (heading or tabs) stop moving along their own block, the group shows a plate of its colour, and on release they join it beside the tab under the hand, or at its end over the heading or a folded group (Search's `GroupDropFrames.resolve` and `move(_:toGroup:)`: `groups::drop_target`, `Browser::drop_into_group`). Not done: dragging a grouped tab out among the loose tabs (Remove from Group does that). The bench's `row` now reports each slot's rect. Delivered as a PR at the user's request, against the 2026-11-04 no-PR decision above. |
| 2026-11-05 | `cargo xtask check`, step 1 of §12's release steps, is built. The plan had required it since 0.1.0, but it never existed, so releases ran without it. It checks the icon PNGs against the SVG and the parity tracker against Search, and on its first run it found `SearchProfile.swift` at 355 lines in the reference build, not 350. xtask no longer depends on `tap-shell`: it is quick to build, and `search-profile` is gone. |
| 2026-11-05 | At the user's request, strip hit testing follows the tabs' animated positions so dragging a moving tab does not move the window, and the empty row's context menu offers Reopen Closed Tab. Closing a blank tab beside pins or groups leaves the window open; closing the last window quits without the explicit-quit confirmation, avoiding a repeated prompt after Cancel. |
| 2026-11-05 | At the user's request, releases follow Kil and Kiled's signing flow directly, using the regular entitlements without a provisioning-profile check. |
| 2026-11-05 | At the user's request ("拖拽tab会导致整个app移动的问题还是没解决"), the window is made unmovable and only the strip's truly empty ends move it, through `performWindowDragWithEvent`: AppKit's titlebar drag had been stealing presses in the row's top band even with the slot hit areas fixed, because `mouseDownCanMoveWindow` is not consulted there. The double-click on the empty row that opened a new tab (Search's `DragStrip.onDoubleClick`) is removed at the same request; the empty row is the titlebar, not a button. |
| 2026-11-06 | At the user's request, Return uses the typed address unless a suggestion was explicitly picked; Tab or → accepts an inline ending. History offers match URL paths and queries as well as hosts (`fixed-deviation`). The offers card and tab strip use native spring transitions, respecting Reduce Motion and light motion. |
| 2026-11-06 | At the user's request ("地址栏加载效果还不够丝滑，还是一段一段的感觉 … 加载页面设计一个漂亮丝滑的等待过场，tab各种操作的效果 … 全部audit一遍；重新设计new tab默认背景，现在的太大了"), a motion audit. Found: the load line drew WebKit's coarse `estimatedProgress` steps as they came and vanished at the end; the press give eased on the 20 Hz tick (visible steps), and its ease back up never showed because the sink was dropped on release; hover washes, the hover cross and the live tab's title/address swap snapped. Now `motion.rs` has `Follow` (one value springing to a target that may change), `Loading` (the line chases the larger of WebKit's report and a slow creep capped at 0.92 on `CREEP` (0.6, 1.0), never back, then runs to the end and fades on `FADE` (0.34, 1.0)), and `Frames` (the one display-link helper the strip, the bar and the ground share). The bar and the strip draw the line through `Loading`, redrawing only its rect between model changes; the give and the hover washes (`QUICK`, 0.16/1.0) run on the strip's frames, and the wash follows whatever tab is under the pointer on every draw. The blank tab's isometric bench (about 60% of the stage) becomes a small ripple mark (~80 pt): a dot with three rings rippling out every 7.2 s. The same view is the first-load transition: a tab that has never committed a page (a link opened in a new tab, a restored tab, an address typed into a blank one) shows the mark with quicker rings (2.1 s), the accent dot and the host under it instead of WebKit's empty white, and fades away over the page when it commits (`TabFields::COMMITTED`, `Tab::committed`). A later navigation in the same tab keeps the old page until the new one commits, as WebKit does. Reduce Motion and light motion get a still mark and no fades. Tab switches stay instant (a crossfade would read as lag). `fixed-deviation`: Search leaves both blank. |
| 2026-11-07 | At the user's request ("点击tab栏周围空白区域应该允许自动放大（适应屏幕尺寸）/还原整个浏览器的尺寸"), a double click on the tab row's empty ends does what the system's title-bar double click does: zoom to the screen and back by default, Minimize or nothing when System Settings › Desktop & Dock says so (`AppleActionOnDoubleClick`). A single press still moves the window. The strip covers the title bar and the window is unmovable (2026-11-05), so AppKit never saw this double click; the strip handles it. The bench's `click` takes a `count` and answers with the window's frame. |

## Appendix A — Parity tracker

Every Swift file in Search at parity-pin appears exactly once. `xtask check` verifies
this and re-reads the line counts. The milestone is when the file's behaviour lands.

Status moves `todo` → `partial` (some of the file is in) → `ported` → `tested` →
`parity` (Python suite green and reviewed side by side). The other statuses are
`dropped` and `fixed-deviation`. Until `tap-ui` and `tap-web` are split out, their
modules live in `tap-shell` (`strip`, `window`, `palette`, `web`, `rt`, `menu`,
`store`, `crash`).

Crates: c = tap-core, i = tap-import, a = assets, sys = tap-sys, ui = tap-ui,
w = tap-web, s = tap-shell.

The tables for bench verbs, XCTest → Rust test, and Python test → expected result are
added below this one as M1 starts.

| Swift file | LOC | Rust module(s) | Milestone | Status |
|---|---:|---|---|---|
| Accounts.swift | 172 | s/ui/accounts | M5 | todo |
| Address.swift | 137 | c/address | M2 | tested |
| AddressCommands.swift | 70 | c/browser (`Command`: settings, new tab) | M2 | partial |
| AI.swift | 187 | c/ai, s/ai_keys | M8 | todo |
| AIAssist.swift | 616 | c/ai/assist, s/ui/ai_assist | M8 | todo |
| AIClient.swift | 312 | c/ai/client | M8 | todo |
| AIEngine.swift | 580 | c/ai/engine_feed, s/ai_engine (C++ helper not built) | M8 | todo |
| AIPage.swift | 372 | a/ai-page, c/ai/page | M8 | todo |
| AISignIn.swift | 99 | c/ai/sign_in | M8 | todo |
| App.swift | 1804 | s/app/menus, s/ui/window_content | M0→M3 | partial |
| ApplePasswords.swift | 427 | s/apple_passwords | M5 | todo |
| ApplePasswordsOffer.swift | 114 | c/apple_passwords/offer | M5 | todo |
| ApplePasswordsProtocol.swift | 581 | c/apple_passwords/{srp,protocol} | M1 | todo |
| ApplePasswordsSettings.swift | 182 | s/ui/settings/apple_passwords | M5 | todo |
| AtRest.swift | 333 | c/at_rest | M2 | todo |
| AtRestLine.swift | 92 | s/ui/at_rest_line | M3 | todo |
| AutoScroll.swift | 107 | a/auto-scroll, w/auto_scroll | M5 | todo |
| Bench.swift | 4631 | c/bench/proto, s/bench/* | M1→M8 | partial |
| BlockedPopups.swift | 247 | c/blocked_popups, s/ui/blocked_popups | M5 | todo |
| Bookmarks.swift | 1735 | c/bookmarks, s/ui/bookmarks/* | M3 | todo |
| BookmarksBar.swift | 160 | s/ui/bookmarks_bar | M3 | todo |
| BookmarksFile.swift | 165 | c/bookmarks_file | M1 | todo |
| Browser.swift | 7518 | c/browser/*, w/delegate, w/downloads, w/print | M2→M5 | partial |
| CleanLink.swift | 75 | c/clean_link | M2 | todo |
| Closing.swift | 108 | c/browser (`close_left_alone`, `retire`), s/sleep | M3 | ported |
| Crx.swift | 240 | c/crx | M1 | todo |
| Curtain.swift | 379 | c/curtain, a/curtain, w/relays | M5 | todo |
| Design.swift | 474 | ui/design, c/mark | M0 | partial |
| Dialogs.swift | 433 | w/dialogs (#849) | M2 | partial |
| Dusk.swift | 737 | a/dusk, w/dusk | M5 | todo |
| DuskColours.swift | 694 | a/dusk-colours | M5 | todo |
| Engine.swift | 81 | c/engine | M2 | tested |
| EngineSuggest.swift | 204 | c/engine_suggest | M2 | todo |
| ExtensionCapture.swift | 498 | w/ext/capture | M7 | todo |
| ExtensionExternal.swift | 128 | w/ext/external, a | M7 | todo |
| ExtensionFiles.swift | 42 | c/extensions/files | M7 | todo |
| ExtensionNative.swift | 353 | c/extensions/host_pipe, w/ext/native | M7 | todo |
| ExtensionOffscreen.swift | 284 | w/ext/offscreen | M7 | todo |
| ExtensionPopup.swift | 392 | w/ext/popup, a | M7 | todo |
| ExtensionRedirectPolicy.swift | 127 | c/extensions/redirect_policy | M7 | todo |
| ExtensionReturnNavigation.swift | 106 | w/ext/return_navigation | M7 | todo |
| Extensions.swift | 2383 | w/ext/{controller,delegate,tab,window,install}, c/extensions/installed, s/ui/extension_slot | M7 | todo |
| ExtensionShims.swift | 5375 | a/ext-shim*, c/extensions/shims, w/ext/answer/* | M7 | todo |
| ExtensionSocket.swift | 164 | w/ext/socket | M7 | todo |
| ExtensionsUI.swift | 342 | s/ui/extensions_ui | M7 | todo |
| ExtensionWorkerRecovery.swift | 187 | w/ext/worker_recovery | M7 | todo |
| Fetching.swift | 673 | c/fetching, s/ui/downloads_page | M2 | partial |
| Find.swift | 135 | s/ui/find_bar | M2 | ported |
| Float.swift | 1367 | w/float, a/float | M5 | todo |
| Fold.swift | 554 | s/ui/fold | M3 | todo |
| Forms.swift | 544 | a/forms, w/relays/form | M5 | partial (0.1.5: `forms.js`, `forms.rs`, `signin.rs`; no passkeys path; 0.2.14: autofill on form found, `fixed-deviation` from Search's fill-nothing-on-its-own) |
| FrameRate.swift | 156 | sys/power, w/frame_rate | M5 | todo |
| FullScreenEscape.swift | 396 | s/ui/full_screen_escape | M5 | todo |
| Hidden.swift | 87 | s/ui/hidden | M5 | todo |
| History.swift | 639 | c/history | M2 | partial |
| Icons.swift | 859 | c/icons_index, w/icons, a | M2 | partial |
| ImageMenu.swift | 290 | w/image_menu, a | M5 | todo |
| Import.swift | 1576 | i/{found,chromium,safari,firefox} | M6 | partial (0.1.4: found, Chromium history and bookmarks; 0.1.5: Chromium passwords, profiles; icons, Mozilla to come) |
| ImportArc.swift | 289 | i/arc | M6 | todo |
| ImportFile.swift | 398 | i/file; #845 part: i/profile | M6 | partial |
| ImportPanel.swift | 572 | s/bring, s/settings (Bring Over) | M6 | partial (0.1.5: the sheet, profile picker, choices, said lines, File…; no records, Arc, extensions or folder chooser) |
| ImportRecord.swift | 60 | i/records | M6 | todo |
| ImportSession.swift | 367 | i/snss (#841) | M1/M6 | tested (0.1.4, with `SessionImportTests`) |
| Inspector.swift | 59 | w/inspector | M2 | todo |
| InternalPages.swift | 168 | c/internal, s/window (stage) | M3 | partial |
| KeptScroll.swift | 36 | a/kept-scroll, w/kept_scroll | M5 | todo |
| KeyLayout.swift | 65 | sys/key_layout | M1 | todo |
| Keyword.swift | 73 | c/keyword | M2 | partial |
| Lights.swift | 154 | s/ui/lights | M0 | ported |
| Links.swift | 372 | s/app/delegate, s/links | M2 | partial |
| Little.swift | 480 | s/ui/little | M3 | todo |
| LiveRate.swift | 59 | a/live-rate, c/live_rate | M5 | todo |
| LocalText.swift | 23 | c/local_text | M2 | todo |
| Loot.swift | 68 | c/loot | M2 | todo |
| Markdown.swift | 1074 | c/markdown, s/ui/markdown_render | M1/M3 | todo |
| MediaKeys.swift | 113 | w/media_keys, a | M5 | todo |
| NativePiP.swift | 99 | w/native_pip, a | M5 | todo |
| Notifications.swift | 480 | w/notifications | M5 | todo |
| Omnibox.swift | 617 | s/ui/{omnibox,address_field} | M2 | partial |
| PageFind.swift | 829 | a/page-find, c/page_find, w/page_find | M2 | ported |
| PageQuestions.swift | 176 | s/ui/page_questions | M5 | todo |
| PaneStage.swift | 1040 | s/ui/pane_stage | M4 | todo |
| Passkeys.swift | 1189 | c/passkeys, w/passkeys, a | M5 | todo |
| Passwords.swift | 286 | s/ui/passwords_page | M5 | todo |
| Peek.swift | 324 | s/ui/peek | M3 | todo |
| PinDrop.swift | 142 | s/ui/pin_drop | M3 | todo |
| Pins.swift | 190 | c/pins | M3 | todo |
| Plate.swift | 271 | ui/plate | M3 | todo |
| PopupGesture.swift | 156 | c/popup_gesture, w/relays, a | M5 | todo |
| Prefs.swift | 818 | c/settings_index (keys, defaults, merged), s/prefs | M1 | partial |
| Privacy.swift | 108 | s/web (`Pages`, `set_feature`, `offers`) | M5 | partial |
| Profiles.swift | 343 | c/profiles, w/stores, s/ui/profiles | M4 | todo |
| Reader.swift | 594 | a/reader, c/reader, s/ui/reader_look | M5 | todo |
| Recall.swift | 482 | s/ui/history_page | M3 | todo |
| RecordingIndicator.swift | 217 | s/ui/recording_indicator | M5 | todo |
| Registrable.swift | 25 | c/registrable | M1 | done (0.1.5, in `vault.rs`) |
| Scripting.swift | 80 | s/app/scripting | M8 | todo |
| SearchProfile.swift (#845 branch) | 355 | i/profile, i/from_search, s/profile (#845) | M6 | partial |
| Session.swift | 195 | c/session | M1/M2 | partial |
| Settings.swift | 1454 | s/settings | M3 | partial |
| SettingsIndex.swift | 656 | c/settings_index | M3 | tested |
| Sharing.swift | 23 | s/app/sharing | M8 | todo |
| Shield.swift | 178 | c/shield, w/shield | M5 | todo |
| Shortcuts.swift | 502 | c/shortcuts, s/app/shortcuts | M3 | partial |
| ShortcutsPage.swift | 200 | s/ui/settings/shortcuts | M3 | todo |
| Side.swift | 1535 | s/ui/side/* | M3 | todo |
| SideIcons.swift | 318 | s/ui/side/icons | M3 | todo |
| SidePanel.swift | 738 | s/ui/side_panel, w/site_panel (CrashCount) | M7 | todo |
| SiteCard.swift | 665 | s/ui/site_card | M3 | todo |
| SiteLocation.swift | 228 | sys/geo, w/site_location | M5 | todo |
| SiteSearch.swift | 297 | c/site_search, a | M2 | todo |
| Sleep.swift | 245 | c/sleep, w/sleep | M2 | todo |
| Spaces.swift | 794 | c/spaces, w/stores, s/ui/space_dot | M4 | todo |
| SpaceSwipe.swift | 385 | s/ui/space_swipe | M4 | todo |
| Split.swift | 78 | c/split | M4 | todo |
| SplitTabItem.swift | 348 | s/ui/split_tab_item | M4 | todo |
| SplitView.swift | 358 | s/ui/split_view | M4 | todo |
| Stage.swift | 759 | s/ui/stage | M2 | partial |
| StatusLine.swift | 157 | s/ui/status_line, w/relays, a | M3 | todo |
| Store.swift | 239 | c/store, w/stores | M1 | partial |
| StoreRelay.swift | 193 | w/relays/store, a | M7 | todo |
| Substitutions.swift | 93 | s/app/substitutions | M5 | todo |
| Swipe.swift | 153 | w/swipe, a | M5 | todo |
| Tab.swift | 3012 | c/tab, w/{config,page_view,tab_host,spi,relays} | M2 | partial |
| TabBar.swift | 1771 | s/ui/tab_bar/* | M2→M3 | partial |
| TabDrag.swift | 288 | c/tab_drag, s/ui/tab_drag | M3 | partial: reorder within the strip (`strip.rs`, 0.1.6); no drag out, no split bands |
| TabGroup.swift | 32 | c/session (TabGroup), c/groups | M3 | ported |
| TabGroups.swift | 233 | c/groups, s/strip (headings), s/tabmenu | M3 | partial: fold, rename, ungroup, close, move, reorder by drag (0.1.6), colours chosen, a tab dropped on a group joins it; no in-place rename |
| TabOffers.swift | 146 | s/ui/tab_offers | M2 | todo |
| TabShortcutHint.swift | 26 | s/ui/tab_shortcut_hint | M3 | todo |
| TabSwitcher.swift | 631 | c/tab_switcher, s/ui/tab_switcher | M3 | todo |
| Translate.swift | 1076 | — (Swift-only Translation API) | — | dropped |
| Trim.swift | 61 | sys/trim | M2 | todo |
| Updater.swift | 605 | s/update, s/update_window (Kil's design, §12) | M8 | fixed-deviation |
| Vault.swift | 408 | sys/keychain, w/vault, i/csv | M5 | partial (0.1.5: `keychain.rs`, `vault.rs`, `passwords::csv`; no LocalAuthentication) |
| Welcome.swift | 460 | s/ui/welcome | M3 | todo |
| WhatsNew.swift | 513 | c/whats_new, s/ui/whats_new | M3 | todo |
| Windows.swift | 504 | c/windows, s/app/windows | M1/M4 | todo |
| WindowTitle.swift | 46 | c/window_title, s/ui/window_title | M2 | todo |
| YouTubeAds.swift | 216 | c/youtube_ads, a | M5 | todo |
| **140 files** | **75793** | | | |
