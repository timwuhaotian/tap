# SVG favicon (platform.qianwenai.com shows no tab icon) — 2026-11-05

## Acceptance
- Visiting `https://platform.qianwenai.com/home/` shows its site icon in the tab.
- Observable cause: the page declares exactly one icon,
  `<link rel="icon" type="image/svg+xml" href="https://img.alicdn.com/…28-28.svg">`;
  `tap_core::icons::rank` skips every SVG, the candidate list falls back to
  `/favicon.ico`, which the site doesn't serve as an icon → blank tab.

## Non-goals
- `data:` inline icons (Search's `inline()`), dark-look second icon, letter
  placeholder, icons in field rows. Still "Not yet", unchanged.

## Design (approved: Search-parity isolated WebView)
Port Search's `Icons.swift` SVG path (`SVGIcons`, #680):
1. `tap-core/src/icons.rs` — `rank` scores SVG 35 instead of skipping
   (Search's `rank`: `sizes == "any" || type.contains("svg") ||
   pathExtension == "svg"` → 35). Touch icon (40) still beats it, crisp
   32–64px raster (45/50) still beats it.
2. `tap-shell` — fetch keeps raster fast path (`raster()` + `NSImage`).
   On raster failure, `looks_like_svg(bytes, mime)` (mime
   `image/svg+xml`, else first 1KB contains `<svg`, cap 256KB) routes to a
   shared hidden `WKWebView` (`svg.rs`): non-persistent store, page JS off,
   only `about:blank`/empty CSP page allowed to load; SVG bytes as
   `data:image/svg+xml;base64,…` drawn via `<img>` + canvas at 128px
   (`toDataURL('image/png')`, 4s patience); PNG base64 decoded with
   `NSData initWithBase64EncodedString:` (no new crates); view released
   after 10s idle. PNG bytes re-enter existing `keep()` → same
   `square_png`/index path, kept under the icon address's ident.
3. `docs/PLAN.md` — update the Site-icons row: SVG done via isolated
   WebView; remaining "Not yet" is inline `data:` icons, dark-look icon,
   letter, field rows.

## Tasks (each: fail → pass → commit)
1. `tap-core/src/icons.rs` `rank`: score SVG 35, don't skip. Update
   `a_crisp_icon_first_then_the_touch_icon_then_the_root_file` to expect
   `[32.png, touch.png, i.svg, 16.png, favicon.ico]`.
   `cargo test -p tap-core --lib icons` green.
2. `tap-shell/src/svg.rs` (new, `mod svg` in `lib.rs`): shared hidden
   renderer as above; pure helper `looks_like_svg(&[u8], Option<&str>) -> bool`
   with `#[cfg(test)]` cases (mime hit, `<svg` sniff, over-limit miss,
   non-SVG miss). `cargo test -p tap-shell --lib svg` green.
   (WebView draw itself needs main thread + WebKit: covered by manual proof.)
3. `tap-shell/src/icons.rs` `try_next`: read `NSURLResponse.MIMEType()`;
   on `!keep(page, address, &bytes)` and `looks_like_svg` → `svg::draw`
   on main → `keep(page, address, &png)` → `icon_arrived`.
   `cargo clippy` + full `cargo test --workspace --locked` green.
4. `docs/PLAN.md` Site-icons row + `cargo +nightly fmt --all`.
   Manual proof: debug build, open the site, tab shows icon; confirm no
   regression on a PNG-icon site. Things only a person can check go in the
   PR body.
