# Changelog

Notable changes to Tap. The download is always the latest build
([`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg));
the versions below are how we track releases, and match `Cargo.toml` and the app's
Info.plist.

## Unreleased

- **Tap browses.** One window with Search's row of tabs across the top: the live tab on a
  grey pill, a cross to close, the plus for a new tab, back, forward and reload at the
  far end, and the traffic lights centred in the row. Click the live tab or press ⌘L to
  type an address or a search; ⌘T floats the field over the page and opens a tab only
  when you press Return. ⌘W, ⇧⌘T, ⌃Tab, ⌘1–⌘9, ⌘[ and ⌘] work as in Search. ⌘-click
  opens a link behind the page; a page's new window becomes a tab beside it.
- **Tabs come back.** The session is kept in Search's own format, written as it changes
  and when Tap quits; restored tabs cost nothing until you look at them.
- **A page that keeps crashing stops reloading itself** (Search #849): it is made again
  twice a minute at most, then says "This page kept crashing." with Try Again.
- **The project.** Plan ([`docs/PLAN.md`](docs/PLAN.md)), license, contribution guide,
  code of conduct, security policy, and issue and pull request templates.
- **The icon.** "Bolt T": a heavy T whose stem breaks into a lightning bolt, on a warm
  plate. It is drawn from `packaging/icon.svg` with `cargo xtask icon`; debug builds get a
  graphite plate.
- **The release flow**, as in Kil: `scripts/bundle.sh` and `scripts/release.sh` build,
  sign with Developer ID, notarize and staple locally. CI checks formatting, lints and
  tests, and never builds releases.
