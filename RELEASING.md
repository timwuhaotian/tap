# Releasing Tap

Releases are built locally on an Apple silicon Mac; CI only checks. Users download
`releases/Tap-arm64.dmg`, which overwrites the last release, and installed copies update
from `releases/latest.toml`.

1. **Start clean and green.** Everything to ship is committed on `main`.
2. **Bump the version** in the root `Cargo.toml` (`[workspace.package] version`);
   `Cargo.lock` follows during the build.
3. **Changelog.** In `CHANGELOG.md`, put a `## x.y.z — YYYY-MM-DD` heading under
   `## Unreleased`, so the release takes its entries and `Unreleased` starts empty.
4. **README.** It must describe what this release does, keep its download link exactly
   `https://github.com/timwuhaotian/tap/raw/main/releases/Tap-arm64.dmg`
   (step 5 refuses to start otherwise), and put no version number in it.
5. **Build:** `scripts/release.sh`. It signs with Developer ID, notarizes and staples
   the app, then the DMG, checks both with `spctl`, and overwrites
   `releases/Tap-arm64.dmg`, its `.sha256` and `releases/latest.toml`. It fails unless
   Apple accepts both.
6. **Install it from the DMG, as a user would:** `hdiutil attach`, `ditto` the app to
   `/Applications/Tap.app`, `hdiutil detach`; `spctl -a -vv` must say
   **Notarized Developer ID**.
7. **Commit and push** everything at once on `main`: version, lock file, changelog,
   README and `releases/`, as `Tap x.y.z: <what changed>`. Check that CI passes and the
   download link serves the new DMG.
8. **Clean up:** `cargo clean` to drop the local `target/` directory the release build left behind.

## Bad release

If a pushed release turns out broken, roll the pointer back: find the previous release
commit (`git log -- releases/latest.toml`), restore all three files from it
(`git checkout <commit> -- releases/Tap-arm64.dmg releases/Tap-arm64.dmg.sha256 releases/latest.toml`),
commit and push. Downloads and the updater then see the previous version again.
