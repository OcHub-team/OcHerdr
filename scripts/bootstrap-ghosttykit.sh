#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GHOSTTY_COMMIT="3da10da73ae848c0310e3e0f0cb29e509c2f6963"
GHOSTTY_BUILD_FLAVOR="crashsubdir-cmux-crash-sentry-off-v1"
EXPECTED_SHA256="6a02a2ec3794de79a02af993083292a89517d2533eb20c746deca377f23456bd"
ARCHIVE_NAME="GhosttyKit.xcframework.tar.gz"
TAG="xcframework-${GHOSTTY_COMMIT}-${GHOSTTY_BUILD_FLAVOR}"
DOWNLOAD_URL="https://github.com/manaflow-ai/ghostty/releases/download/${TAG}/${ARCHIVE_NAME}"
OUTPUT_PARENT="$REPO_ROOT/vendor/ghosttykit"
OUTPUT_DIR="$OUTPUT_PARENT/GhosttyKit.xcframework"

if [ -d "$OUTPUT_DIR" ]; then
  INSTALLED_COMMIT="$(cat "$OUTPUT_DIR/.ghostty_sha" 2>/dev/null || true)"
  if [ "$INSTALLED_COMMIT" = "$GHOSTTY_COMMIT" ] \
    && [ -f "$OUTPUT_DIR/macos-arm64_x86_64/ghostty-internal.a" ] \
    && [ -f "$OUTPUT_DIR/macos-arm64_x86_64/Headers/ghostty.h" ]; then
    echo "GhosttyKit $GHOSTTY_COMMIT is already available at $OUTPUT_DIR"
    exit 0
  fi
  echo "GhosttyKit at $OUTPUT_DIR is incomplete or belongs to another commit" >&2
  echo "move it aside, then rerun this script" >&2
  exit 1
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
if [ ! -d "$EXTRACT_DIR/GhosttyKit.xcframework" ]; then
  echo "GhosttyKit archive does not contain GhosttyKit.xcframework" >&2
  exit 1
fi

mv "$EXTRACT_DIR/GhosttyKit.xcframework" "$OUTPUT_DIR"
echo "Installed GhosttyKit $GHOSTTY_COMMIT at $OUTPUT_DIR"
