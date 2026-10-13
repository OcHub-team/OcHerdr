#!/usr/bin/env bash
# Build the pinned source checkout into the release asset used by bootstrap.
set -euo pipefail
export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$REPO_ROOT/vendor/ghosttykit/version.env"
SOURCE_DIR="${1:?usage: build-ghosttykit.sh /path/to/ghostty-checkout [output-directory]}"
OUTPUT_DIR="${2:-$REPO_ROOT/target/ghosttykit}"
SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd)"
if [ "$(git -C "$SOURCE_DIR" rev-parse HEAD)" != "$GHOSTTY_COMMIT" ]; then
  echo "Ghostty checkout must be at $GHOSTTY_COMMIT" >&2
  exit 1
fi
if ! git -C "$SOURCE_DIR" diff --quiet HEAD; then
  echo "Ghostty checkout has modified tracked files" >&2
  exit 1
fi
(
  cd "$SOURCE_DIR"
  zig build -Demit-exe=false -Demit-macos-app=false -Demit-xcframework=true \
    -Dxcframework-target=universal -Doptimize=ReleaseFast -Dsentry=false
)
PACKAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ocherdr-ghosttykit-package.XXXXXX")"
trap 'rm -rf "$PACKAGE_DIR"' EXIT
mkdir -p "$OUTPUT_DIR"
cp -R "$SOURCE_DIR/macos/GhosttyKit.xcframework" "$PACKAGE_DIR/"
# Normalize upstream's lib-prefixed name to the established OcHerdr layout.
SLICE="$PACKAGE_DIR/GhosttyKit.xcframework/macos-arm64_x86_64"
if [ -f "$SLICE/libghostty-internal.a" ]; then
  mv "$SLICE/libghostty-internal.a" "$SLICE/ghostty-internal.a"
  /usr/libexec/PlistBuddy -c 'Set :AvailableLibraries:0:LibraryPath ghostty-internal.a' \
    "$PACKAGE_DIR/GhosttyKit.xcframework/Info.plist"
fi
printf '%s\n' "$GHOSTTY_COMMIT" > "$PACKAGE_DIR/GhosttyKit.xcframework/.ghostty_sha"
printf '%s\n' "$GHOSTTY_BUILD_FLAVOR" > "$PACKAGE_DIR/GhosttyKit.xcframework/.ghostty_build_flavor"
COPYFILE_DISABLE=1 tar -czf "$OUTPUT_DIR/GhosttyKit.xcframework.tar.gz" \
  -C "$PACKAGE_DIR" GhosttyKit.xcframework
shasum -a 256 "$OUTPUT_DIR/GhosttyKit.xcframework.tar.gz"
