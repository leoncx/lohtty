#!/usr/bin/env bash
# Builds a Linux release for this machine's architecture and packs it:
#   dist/lohtty-<version>-linux-<arch>.tar.gz        lohtty, .desktop entry, icon,
#                                                    README, LICENSE, CHANGELOG
#   dist/lohtty-<version>-linux-<arch>-debug.tar.gz  lohtty.debug, the symbols
#                                                    stripped from the binary
#   dist/lohtty-<version>-linux-<arch>.AppImage      with --appimage
#
#   scripts/package-linux.sh [--appimage]
#
# The binary needs at least the glibc it was built against, so release builds
# run on the oldest supported distribution (Ubuntu 22.04: glibc 2.35).
# --appimage needs patchelf and downloads two pinned tools (appimagetool and
# the AppImage runtime, checked against the hashes below) into
# $LOHTTY_TOOLS (default: target/appimage-tools). It works without FUSE.
#
# Prints the paths of the files it made, one per line.
set -euo pipefail

appimage=false
for arg in "$@"; do
    case "$arg" in
        --appimage) appimage=true ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
arch="$(uname -m)"
case "$arch" in
    x86_64 | aarch64) ;;
    *) echo "unsupported architecture $arch" >&2; exit 1 ;;
esac

# --locked: build exactly the dependency versions in Cargo.lock.
cargo build --release --locked -p lohtty >&2
version="$(cargo pkgid -p lohtty)"
version="${version##*[#@]}"
name="lohtty-$version-linux-$arch"
bin="${CARGO_TARGET_DIR:-$root/target}/release/lohtty"
dist="$root/dist"

stage="$dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
cp "$bin" "$stage/lohtty"
# The line tables go to lohtty.debug; the binary keeps its symbol table (for
# panic backtraces) and a debuglink to the debug file.
objcopy --only-keep-debug "$stage/lohtty" "$dist/lohtty.debug"
objcopy --strip-debug --add-gnu-debuglink="$dist/lohtty.debug" "$stage/lohtty"
cp assets/linux/lohtty.desktop assets/icons/lohtty.png README.md LICENSE CHANGELOG.md "$stage/"
chmod 0755 "$stage/lohtty"
chmod 0644 "$stage"/{lohtty.desktop,lohtty.png,README.md,LICENSE,CHANGELOG.md}
tar -C "$dist" -czf "$dist/$name.tar.gz" "$name"
tar -C "$dist" -czf "$dist/$name-debug.tar.gz" lohtty.debug
rm "$dist/lohtty.debug"
echo "$dist/$name.tar.gz"
echo "$dist/$name-debug.tar.gz"

$appimage || exit 0

# Pinned AppImage tools: appimagetool 1.9.1 and type2-runtime 20251108.
declare -A sha256=(
    [appimagetool-x86_64.AppImage]=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    [appimagetool-aarch64.AppImage]=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    [runtime-x86_64]=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    [runtime-aarch64]=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
)
tools="${LOHTTY_TOOLS:-${CARGO_TARGET_DIR:-$root/target}/appimage-tools}"
mkdir -p "$tools"
fetch() { # file url
    local file="$tools/$1"
    if [ ! -f "$file" ] || ! echo "${sha256[$1]}  $file" | sha256sum -c --quiet - 2>/dev/null; then
        curl -sSfL -o "$file" "$2"
        echo "${sha256[$1]}  $file" | sha256sum -c --quiet - || {
            echo "checksum mismatch for $1" >&2
            rm -f "$file"
            exit 1
        }
    fi
    chmod +x "$file"
}
fetch "appimagetool-$arch.AppImage" \
    "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage"
fetch "runtime-$arch" \
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch"

appdir="$dist/$name.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/lib" "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps"
cp "$stage/lohtty" "$appdir/usr/bin/"
# Bundle the keyboard libraries, which some systems (Wayland-only, minimal
# installs) lack. They're found through the rpath, not LD_LIBRARY_PATH, which
# would leak into the shells lohtty starts. libc, libxcb and the GPU drivers
# come from the system.
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1; do
    path="$(ldd "$appdir/usr/bin/lohtty" | awk -v l="$lib" '$1 == l { print $3 }')"
    [ -n "$path" ] || { echo "$lib not found" >&2; exit 1; }
    cp -L "$path" "$appdir/usr/lib/"
    # RUNPATH isn't inherited: each bundled library finds the others itself.
    patchelf --set-rpath '$ORIGIN' "$appdir/usr/lib/$lib"
done
patchelf --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/lohtty"
cp assets/linux/lohtty.desktop "$appdir/usr/share/applications/"
cp assets/linux/lohtty.desktop "$appdir/"
cp assets/icons/lohtty.png "$appdir/usr/share/icons/hicolor/256x256/apps/"
cp assets/icons/lohtty.png "$appdir/lohtty.png"
ln -s lohtty.png "$appdir/.DirIcon"
ln -s usr/bin/lohtty "$appdir/AppRun"

out="$dist/$name.AppImage"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$arch" "$tools/appimagetool-$arch.AppImage" \
    --no-appstream --runtime-file "$tools/runtime-$arch" "$appdir" "$out" >&2
rm -rf "$appdir"
echo "$out"
