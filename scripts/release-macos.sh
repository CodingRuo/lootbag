#!/bin/sh
# Builds, signs, notarizes and staples Lootbag.app on this Mac.
# One-time setup: xcrun notarytool store-credentials lootbag --apple-id <id> --team-id <team>
set -eu
cd "$(dirname "$0")/.."
PROFILE="${NOTARY_PROFILE:-lootbag}"
# The certificate hash from the keychain: keeps names out of the repo, and Tauri fails on non-ASCII identity names.
APPLE_SIGNING_IDENTITY="$(security find-identity -v -p codesigning | awk '/Developer ID Application/ {print $2; exit}')"
[ -n "$APPLE_SIGNING_IDENTITY" ] || { echo "No Developer ID Application certificate in the keychain." >&2; exit 1; }
export APPLE_SIGNING_IDENTITY
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null || { echo "Notary profile '$PROFILE' missing, see the setup line at the top." >&2; exit 1; }

pnpm test
pnpm tauri build --bundles app
APP=src-tauri/target/release/bundle/macos/Lootbag.app
VERSION="$(sed -n 's/^ *"version": "\(.*\)",/\1/p' src-tauri/tauri.conf.json | head -n 1)"
ZIP="src-tauri/target/release/bundle/Lootbag-$VERSION-macos.zip"
codesign --verify --strict --deep "$APP"

ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
# Re-zip so the download carries the stapled ticket and opens offline.
rm "$ZIP" && ditto -c -k --keepParent "$APP" "$ZIP"
spctl --assess --type execute --verbose "$APP"
echo "Ready: $ZIP"
