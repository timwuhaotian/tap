#!/bin/sh
# Builds Tap, signs it with Developer ID, notarizes and staples it, and packs it in a DMG under
# releases/. Kil's scripts/release.sh, ported (docs/PLAN.md §12). Run locally; CI never builds
# releases.
# Usage: scripts/release.sh   → releases/Tap-arm64.dmg (+ .sha256, latest.toml), replacing the
# last release: downloads are always the latest, and the version is only for us (Info.plist,
# git history) and for the app's updater, which reads latest.toml. Until 1.0, releases/ stays out
# of git (.gitignore); this proves the pipeline.
#
# Signing: TAP_SIGN_IDENTITY, default "Developer ID Application" (the one such identity in the
# login keychain).
# Passkeys: with packaging/Tap.provisionprofile present, it is embedded and the app is signed with
# packaging/tap.passkeys.entitlements; without it, the release refuses unless
# TAP_ALLOW_NO_PASSKEYS=1, and passkeys stay unavailable in that build.
# Notarization credentials, the first of:
#   TAP_NOTARY_KEY, TAP_NOTARY_KEY_ID, TAP_NOTARY_ISSUER   an App Store Connect API key
#   TAP_NOTARY_PROFILE   a `xcrun notarytool store-credentials` profile
#   the `asc` CLI's stored App Store Connect key, if `asc` is installed
#   the notarytool profile `tap-notary`
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

identity="${TAP_SIGN_IDENTITY:-Developer ID Application}"
work="target/release/dist"
profile="packaging/Tap.provisionprofile"

# Submits $1 and waits; fails unless Apple accepts it.
notarize() {
  if [ -n "${TAP_NOTARY_KEY:-}" ]; then
    result="$(xcrun notarytool submit "$1" --key "$TAP_NOTARY_KEY" --key-id "$TAP_NOTARY_KEY_ID" \
      --issuer "$TAP_NOTARY_ISSUER" --wait --output-format json)"
  elif [ -z "${TAP_NOTARY_PROFILE:-}" ] && command -v asc >/dev/null; then
    result="$(asc notarization submit --file "$1" --wait)"
  else
    result="$(xcrun notarytool submit "$1" --keychain-profile "${TAP_NOTARY_PROFILE:-tap-notary}" \
      --wait --output-format json)"
  fi
  echo "$result"
  case "$result" in
    *'"status":"Accepted"'*) ;;
    *) echo "notarization of $1 was not accepted" >&2; exit 1 ;;
  esac
}

dmg="releases/Tap-arm64.dmg"
# README's download link must give this file, checked before the long build and notarization.
link="https://github.com/timwuhaotian/tap/raw/main/$dmg"
if ! grep -qF "($link)" README.md; then
  echo "README.md must link the release as $link" >&2
  exit 1
fi
if [ -f "$profile" ]; then
  entitlements="packaging/tap.passkeys.entitlements"
elif [ "${TAP_ALLOW_NO_PASSKEYS:-}" = 1 ]; then
  entitlements="packaging/tap.entitlements"
else
  echo "no $profile: passkeys would not work in this build." >&2
  echo "Add the Developer ID provisioning profile, or set TAP_ALLOW_NO_PASSKEYS=1." >&2
  exit 1
fi

app="$(sh scripts/bundle.sh release | tail -n 1)"
# After the build: before it, `cargo pkgid` still reports the version in Cargo.lock, and a
# bumped Cargo.toml only reaches the lock there. Read it late, or the last line reports the
# version it replaces.
version="$(cargo pkgid -p tap-app | sed 's/.*[#@]//')"
if [ -f "$profile" ]; then
  cp "$profile" "$app/Contents/embedded.provisionprofile"
fi
codesign --force --timestamp --options runtime \
  --entitlements "$entitlements" \
  --sign "$identity" "$app"
codesign --verify --strict --verbose=2 "$app"

# The app first, so a copy dragged out of the DMG carries its own ticket, even offline.
rm -rf "$work"
mkdir -p "$work/dmg" releases
ditto -c -k --keepParent "$app" "$work/Tap.zip"
notarize "$work/Tap.zip"
xcrun stapler staple "$app"

ditto "$app" "$work/dmg/Tap.app"
ln -s /Applications "$work/dmg/Applications"
rm -f "$dmg"
hdiutil create -volname "Tap" -srcfolder "$work/dmg" -fs HFS+ -format UDZO "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"

spctl --assess --type execute --verbose=2 "$app"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
(cd releases && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")
printf 'version = "%s"\nsha256 = "%s"\n' "$version" "$(awk '{print $1}' "$dmg.sha256")" \
  > releases/latest.toml
echo "$dmg ($version)"
