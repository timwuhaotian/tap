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

### Made by the user (not to be revisited)

| Topic | Decision |
|---|---|
| Language | **Pure Rust, no Swift at all.** Features that need Swift-only Apple APIs are dropped. Today that means page translation (`Translate.swift` uses `TranslationSession`). |
| Engine | **WebKit**, the system `WKWebView`, driven from Rust through `objc2`. The UI is AppKit. |
| Scope | **Full parity with Search before the first release** (minus translation). |
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
7. **The reference build is a local `parity-pin` branch** in Search (`main` + #845 +
   #849), built to `build/Search.app`. The installed `/Applications/Search.app` (1.0.8,
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

**`.searchprofile` (#845).** `tap_import::search_profile` ports the manifest,
`AnyCodable` (`$data`/`$date`), the allowlist, collect and import.
- Uses the `zip` crate with `enclosed_name()` against zip-slip, instead of `ditto`.
- Tap **reads** `.searchprofile` and Safari zips, and **writes** `.tapprofile` with the
  identical layout and `format: 1`.

**"Bring things over from Search"** (`tap_import::from_search`, new). If
`~/Library/Application Support/Search` exists, Search is offered as an import source.
- It builds the same `IncomingProfile` that #845's import reads, from the live folder
  plus `CFPreferencesCopyMultiple("com.officecommun.search")`, so both paths share all
  of the merge code.
- What comes over: profiles, spaces, sessions (open tabs, groups, splits), pins,
  bookmarks, history, hidden elements, search sites, allowlisted settings, and
  extension IDs (reinstalled).
- **Can't come over:**
  - cookies and sign-ins: WebKit keeps them per bundle id.
  - passwords without prompts: Search's keychain items are tied to its signature. An
    opt-in path reads them with one macOS prompt per item, with a clear warning.

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
- [ ] Still to come for M2: history and suggestions, find on page, downloads, dialogs
  (alert/confirm/prompt, uploads, sign-in), sleeping tabs, icons, the os_log line for
  #849 (now on stderr).
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

### M3 — Window chrome parity
- Sidebar column and full tab bar, pins, groups, Fold, Peek, Little window
- Bookmarks, bookmarks bar and dropdown, the ⌘K switcher, site card, status line and
  link bubble
- Welcome and What's New
- Every internal page: Settings generated from the index, History, Downloads,
  Bookmarks
- Shortcuts and the menu table
- Tab drag within the strip and column (pins, groups, PinDrop)

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
- **#841:** open tabs and groups offered in Bring Things Over
- **#845:** File › Export Tap Profile, and import of `.searchprofile` and `.tapprofile`
- "From Search"

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
  `update.install` setting
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
- **Passkeys** (an addition over Kil). If `packaging/Tap.provisionprofile` exists
  (git-ignored), `release.sh` embeds it as `Contents/embedded.provisionprofile` and
  signs with `packaging/tap.passkeys.entitlements`. That file holds:
  - `com.apple.application-identifier` = `43VLF3KTFZ.com.timwuhaotian.tap`
  - team `43VLF3KTFZ`
  - `com.apple.developer.web-browser.public-key-credential`

  Without the profile it signs with `packaging/tap.entitlements`, and passkeys report
  that they're unavailable. `release.sh` then refuses to run unless
  `TAP_ALLOW_NO_PASSKEYS=1`, which is Search's rule (`build.sh:288-291`).
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

**Until 1.0** (full parity comes first), `release.sh` runs at M0 and M8 only to prove
the pipeline. `releases/` isn't committed. Its first commit is the 1.0.0 release, and
from then on the repo `timwuhaotian/tap` must be public so the raw download links
work, as Kil's is.

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
- Search's **Check for Updates…** menu item and the `update.install` setting (install
  automatically, on by default) stay, for parity.
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
| 2026-11-04 | The bench came forward from M1 to check the browsing build by, and found three bugs no unit test could: `selectText:` ending the field's own editing session, focus handed out before the page was in the window, and shifted key equivalents that a real keyboard never matches (⇧⌘T must be "T"; ⌃⇧Tab is the back-tab character). |
| 2026-11-04 | Browsing came before M1's foundations, at the user's request: the core model, runtime, window, tabs, field and web views are in (M2 progress). The bench moves up next: driving the UI with synthetic keystrokes is unreliable and can reach other apps. |
| 2026-11-04 | The GitHub repo `timwuhaotian/tap` is public from day one, not only from 1.0. Nothing is downloadable until `releases/` is first committed at 1.0. |

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
| AddressCommands.swift | 70 | c/address_commands | M2 | todo |
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
| Closing.swift | 108 | c/closing | M3 | todo |
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
| Fetching.swift | 673 | c/fetching, s/ui/downloads_page | M2 | todo |
| Find.swift | 135 | s/ui/find_bar | M2 | todo |
| Float.swift | 1367 | w/float, a/float | M5 | todo |
| Fold.swift | 554 | s/ui/fold | M3 | todo |
| Forms.swift | 544 | a/forms, w/relays/form | M5 | todo |
| FrameRate.swift | 156 | sys/power, w/frame_rate | M5 | todo |
| FullScreenEscape.swift | 396 | s/ui/full_screen_escape | M5 | todo |
| Hidden.swift | 87 | s/ui/hidden | M5 | todo |
| History.swift | 639 | c/history | M2 | todo |
| Icons.swift | 859 | c/icons_index, w/icons, a | M2 | todo |
| ImageMenu.swift | 290 | w/image_menu, a | M5 | todo |
| Import.swift | 1576 | i/{discovery,chromium,safari,firefox} | M6 | todo |
| ImportArc.swift | 289 | i/arc | M6 | todo |
| ImportFile.swift | 398 | i/file | M6 | todo |
| ImportPanel.swift | 572 | s/ui/import_panel | M6 | todo |
| ImportRecord.swift | 60 | i/records | M6 | todo |
| ImportSession.swift | 367 | i/session_snss (#841) | M1/M6 | todo |
| Inspector.swift | 59 | w/inspector | M2 | todo |
| InternalPages.swift | 168 | c/internal_pages, s/ui/internal_pages | M3 | todo |
| KeptScroll.swift | 36 | a/kept-scroll, w/kept_scroll | M5 | todo |
| KeyLayout.swift | 65 | sys/key_layout | M1 | todo |
| Keyword.swift | 73 | c/keyword | M2 | partial |
| Lights.swift | 154 | s/ui/lights | M0 | ported |
| Links.swift | 372 | s/app/delegate | M2 | partial |
| Little.swift | 480 | s/ui/little | M3 | todo |
| LiveRate.swift | 59 | a/live-rate, c/live_rate | M5 | todo |
| LocalText.swift | 23 | c/local_text | M2 | todo |
| Loot.swift | 68 | c/loot | M2 | todo |
| Markdown.swift | 1074 | c/markdown, s/ui/markdown_render | M1/M3 | todo |
| MediaKeys.swift | 113 | w/media_keys, a | M5 | todo |
| NativePiP.swift | 99 | w/native_pip, a | M5 | todo |
| Notifications.swift | 480 | w/notifications | M5 | todo |
| Omnibox.swift | 617 | s/ui/{omnibox,address_field} | M2 | partial |
| PageFind.swift | 829 | a/page-find, c/page_find, w/page_find | M2 | todo |
| PageQuestions.swift | 176 | s/ui/page_questions | M5 | todo |
| PaneStage.swift | 1040 | s/ui/pane_stage | M4 | todo |
| Passkeys.swift | 1189 | c/passkeys, w/passkeys, a | M5 | todo |
| Passwords.swift | 286 | s/ui/passwords_page | M5 | todo |
| Peek.swift | 324 | s/ui/peek | M3 | todo |
| PinDrop.swift | 142 | s/ui/pin_drop | M3 | todo |
| Pins.swift | 190 | c/pins | M3 | todo |
| Plate.swift | 271 | ui/plate | M3 | todo |
| PopupGesture.swift | 156 | c/popup_gesture, w/relays, a | M5 | todo |
| Prefs.swift | 818 | c/prefs | M1 | todo |
| Privacy.swift | 108 | c/privacy, w/privacy | M5 | todo |
| Profiles.swift | 343 | c/profiles, w/stores, s/ui/profiles | M4 | todo |
| Reader.swift | 594 | a/reader, c/reader, s/ui/reader_look | M5 | todo |
| Recall.swift | 482 | s/ui/history_page | M3 | todo |
| RecordingIndicator.swift | 217 | s/ui/recording_indicator | M5 | todo |
| Registrable.swift | 25 | c/registrable | M1 | todo |
| Scripting.swift | 80 | s/app/scripting | M8 | todo |
| SearchProfile.swift (#845 branch) | 350 | i/search_profile (#845) | M6 | todo |
| Session.swift | 195 | c/session | M1/M2 | partial |
| Settings.swift | 1454 | s/ui/settings/* | M3 | todo |
| SettingsIndex.swift | 656 | c/settings_index | M3 | todo |
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
| TabDrag.swift | 288 | c/tab_drag, s/ui/tab_drag | M3 | todo |
| TabGroup.swift | 32 | c/tab_group | M3 | todo |
| TabGroups.swift | 233 | s/ui/tab_groups | M3 | todo |
| TabOffers.swift | 146 | s/ui/tab_offers | M2 | todo |
| TabShortcutHint.swift | 26 | s/ui/tab_shortcut_hint | M3 | todo |
| TabSwitcher.swift | 631 | c/tab_switcher, s/ui/tab_switcher | M3 | todo |
| Translate.swift | 1076 | — (Swift-only Translation API) | — | dropped |
| Trim.swift | 61 | sys/trim | M2 | todo |
| Updater.swift | 605 | c/updater, s/updater | M8 | todo |
| Vault.swift | 408 | sys/keychain, w/vault, i/csv | M5 | todo |
| Welcome.swift | 460 | s/ui/welcome | M3 | todo |
| WhatsNew.swift | 513 | c/whats_new, s/ui/whats_new | M3 | todo |
| Windows.swift | 504 | c/windows, s/app/windows | M1/M4 | todo |
| WindowTitle.swift | 46 | c/window_title, s/ui/window_title | M2 | todo |
| YouTubeAds.swift | 216 | c/youtube_ads, a | M5 | todo |
| **140 files** | **75788** | | | |
