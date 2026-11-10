#!/usr/bin/env bash
# Builds a universal (Apple Silicon + Intel) macOS release and packs it:
#   dist/lohtty-<version>-macos-universal.dmg        lohtty.app, with a link to
#                                                    /Applications to drag it to
#   dist/lohtty-<version>-macos-universal-dsym.zip   lohtty.dSYM, the symbols
#                                                    stripped from the app
#
#   scripts/package-macos.sh
#
# Needs Xcode, not just the Command Line Tools: gpui compiles its Metal
# shaders with `xcrun metal`. The app is ad-hoc signed, not notarized.
#
# Prints the paths of the files it made, one per line.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
targets=(aarch64-apple-darwin x86_64-apple-darwin)
# The oldest macOS the app declares; the build targets it too.
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add "${targets[@]}" >&2
for target in "${targets[@]}"; do
    # --locked: build exactly the dependency versions in Cargo.lock.
    cargo build --release --locked -p lohtty --target "$target" >&2
done
version="$(cargo pkgid -p lohtty)"
version="${version##*[#@]}"
name="lohtty-$version-macos-universal"
out="${CARGO_TARGET_DIR:-$root/target}"
dist="$root/dist"
mkdir -p "$dist"

stage="$dist/$name"
rm -rf "$stage"
app="$stage/lohtty.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/lohtty" \
    "$out/${targets[0]}/release/lohtty" "$out/${targets[1]}/release/lohtty"
# The line tables go to lohtty.dSYM (dsymutil reads them from the object
# files); the binary keeps its symbol table for panic backtraces.
dsym="$dist/lohtty.dSYM"
rm -rf "$dsym"
dsymutil "$app/Contents/MacOS/lohtty" -o "$dsym"
strip -S "$app/Contents/MacOS/lohtty"
sed -e "s/@VERSION@/$version/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" \
    assets/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >&2
cp assets/icons/lohtty.icns "$app/Contents/Resources/"
# Ad-hoc signature: required for Apple Silicon to run the app at all. Without
# notarization, Gatekeeper still asks the user to allow it on first launch.
codesign --force --sign - "$app"
codesign --verify --strict "$app" >&2

ln -s /Applications "$stage/Applications"
cp README.md LICENSE CHANGELOG.md "$stage/"
dmg="$dist/$name.dmg"
rm -f "$dmg"
hdiutil create -volname "lohtty $version" -srcfolder "$stage" -format UDZO -ov "$dmg" >&2
(cd "$dist" && ditto -c -k --keepParent lohtty.dSYM "$name-dsym.zip")
rm -rf "$dsym"
echo "$dmg"
echo "$dist/$name-dsym.zip"
