#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$REPO_ROOT/vendor/ghosttykit/version.env"
ARCHIVE_NAME="GhosttyKit.xcframework.tar.gz"
TAG="xcframework-${GHOSTTY_COMMIT}-${GHOSTTY_BUILD_FLAVOR}"
DOWNLOAD_URL="https://github.com/${GHOSTTY_REPOSITORY}/releases/download/${TAG}/${ARCHIVE_NAME}"
OUTPUT_PARENT="$REPO_ROOT/vendor/ghosttykit"
OUTPUT_DIR="$OUTPUT_PARENT/GhosttyKit.xcframework"

if [ -d "$OUTPUT_DIR" ]; then
  INSTALLED_COMMIT="$(cat "$OUTPUT_DIR/.ghostty_sha" 2>/dev/null || true)"
  INSTALLED_FLAVOR="$(cat "$OUTPUT_DIR/.ghostty_build_flavor" 2>/dev/null || true)"
  if [ "$INSTALLED_COMMIT" = "$GHOSTTY_COMMIT" ] \
    && [ "$INSTALLED_FLAVOR" = "$GHOSTTY_BUILD_FLAVOR" ] \
    && [ -f "$OUTPUT_DIR/macos-arm64_x86_64/ghostty-internal.a" ] \
    && [ -f "$OUTPUT_DIR/macos-arm64_x86_64/Headers/ghostty.h" ]; then
    echo "GhosttyKit $GHOSTTY_COMMIT is already available at $OUTPUT_DIR"
    exit 0
  fi
  echo "Updating GhosttyKit at $OUTPUT_DIR"
fi

DOWNLOAD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ocherdr-ghosttykit.XXXXXX")"
trap 'rm -rf "$DOWNLOAD_DIR"' EXIT
ARCHIVE_PATH="$DOWNLOAD_DIR/$ARCHIVE_NAME"
EXTRACT_DIR="$DOWNLOAD_DIR/extract"
mkdir -p "$EXTRACT_DIR" "$OUTPUT_PARENT"

curl --fail --show-error --location \
  --connect-timeout 15 \
  --max-time 600 \
  --retry 5 \
  --retry-all-errors \
  --output "$ARCHIVE_PATH" \
  "$DOWNLOAD_URL"

ACTUAL_SHA256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"
if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
  echo "GhosttyKit checksum mismatch" >&2
  echo "expected: $EXPECTED_SHA256" >&2
  echo "actual:   $ACTUAL_SHA256" >&2
  exit 1
fi

tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"
if [ ! -f "$EXTRACT_DIR/GhosttyKit.xcframework/macos-arm64_x86_64/ghostty-internal.a" ] \
  || [ ! -f "$EXTRACT_DIR/GhosttyKit.xcframework/macos-arm64_x86_64/Headers/ghostty.h" ] \
  || [ "$(cat "$EXTRACT_DIR/GhosttyKit.xcframework/.ghostty_sha" 2>/dev/null || true)" != "$GHOSTTY_COMMIT" ] \
  || [ "$(cat "$EXTRACT_DIR/GhosttyKit.xcframework/.ghostty_build_flavor" 2>/dev/null || true)" != "$GHOSTTY_BUILD_FLAVOR" ]; then
  echo "GhosttyKit archive is incomplete or has mismatched build metadata" >&2
  exit 1
fi

# Keep the previous framework intact until download and verification succeed.
if [ -d "$OUTPUT_DIR" ]; then
  mv "$OUTPUT_DIR" "$DOWNLOAD_DIR/previous.xcframework"
fi
if ! mv "$EXTRACT_DIR/GhosttyKit.xcframework" "$OUTPUT_DIR"; then
  if [ -d "$DOWNLOAD_DIR/previous.xcframework" ]; then
    mv "$DOWNLOAD_DIR/previous.xcframework" "$OUTPUT_DIR"
  fi
  exit 1
fi
echo "Installed GhosttyKit $GHOSTTY_COMMIT at $OUTPUT_DIR"
