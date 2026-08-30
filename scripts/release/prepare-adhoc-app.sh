#!/usr/bin/env bash
set -euo pipefail

if [[ "${CARGO_PACKAGER_FORMAT:-}" != dmg ]]; then
    exit 0
fi

app_path="${OCHERDR_PACKAGE_APP_PATH:?OCHERDR_PACKAGE_APP_PATH is required}"
main_binary="${app_path}/Contents/MacOS/ocherdr"

test -d "${app_path}"
test -f "${main_binary}"
codesign --remove-signature "${main_binary}"
codesign --force --deep --sign - "${app_path}"
test -f "${app_path}/Contents/_CodeSignature/CodeResources"
codesign --verify --deep --strict --verbose=2 "${app_path}"
