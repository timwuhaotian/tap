# Changelog

Notable changes to Tap. The download is always the latest build
([`Tap-arm64.dmg`](https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg));
the versions below are how we track releases, and match `Cargo.toml` and the app's
Info.plist.

## Unreleased

- **The project.** Plan ([`docs/PLAN.md`](docs/PLAN.md)), license, contribution guide,
  code of conduct, security policy, and issue and pull request templates.
- **The icon.** "Bolt T": a heavy T whose stem breaks into a lightning bolt, on a warm
  plate. It is drawn from `packaging/icon.svg` with `cargo xtask icon`; debug builds get a
  graphite plate.
- **The release flow**, as in Kil: `scripts/bundle.sh` and `scripts/release.sh` build,
  sign with Developer ID, notarize and staple locally. CI checks formatting, lints and
  tests, and never builds releases.
