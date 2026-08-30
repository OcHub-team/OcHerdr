#!/usr/bin/env bash
set -euo pipefail

out_dir="${1:-dist}"
if [[ -z "${OCHERDR_SIGNING_PRIVATE_KEY:-}" ]]; then
    printf 'OCHERDR_SIGNING_PRIVATE_KEY is required for application updates.\n' >&2
    exit 1
fi

export CARGO_PACKAGER_SIGN_PRIVATE_KEY="${OCHERDR_SIGNING_PRIVATE_KEY}"
export CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD="${OCHERDR_SIGNING_PRIVATE_KEY_PASSWORD:-}"

signed=0
while IFS= read -r artifact; do
    printf 'signing %s\n' "${artifact}"
    cargo packager signer sign "${artifact}"
    signed=$((signed + 1))
done < <(find "${out_dir}" -maxdepth 1 -type f -name '*.app.tar.gz' | sort)

if [[ "${signed}" -ne 1 ]]; then
    printf 'expected one updater payload in %s, found %d\n' "${out_dir}" "${signed}" >&2
    exit 1
fi
