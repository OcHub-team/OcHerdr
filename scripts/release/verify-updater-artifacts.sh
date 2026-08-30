#!/usr/bin/env bash
set -euo pipefail

out_dir="${1:-dist}"
if [[ -z "${OCHERDR_UPDATER_PUBKEY:-}" ]]; then
    printf 'OCHERDR_UPDATER_PUBKEY is required to verify application updates.\n' >&2
    exit 1
fi
if ! command -v minisign >/dev/null; then
    printf 'minisign is required to verify application updates.\n' >&2
    exit 1
fi

mapfile_compatible_find() {
    find "${out_dir}" -maxdepth 1 -type f -name '*.app.tar.gz' | sort
}

payloads=()
while IFS= read -r payload; do
    payloads+=("${payload}")
done < <(mapfile_compatible_find)
if [[ "${#payloads[@]}" -ne 1 ]]; then
    printf 'expected one updater payload in %s, found %d\n' \
        "${out_dir}" "${#payloads[@]}" >&2
    exit 1
fi

payload="${payloads[0]}"
encoded_signature="${payload}.sig"
if [[ ! -f "${encoded_signature}" ]]; then
    printf 'missing updater signature: %s\n' "${encoded_signature}" >&2
    exit 1
fi

temporary="$(mktemp -d "${TMPDIR:-/tmp}/ocherdr-signature.XXXXXX")"
public_key="${temporary}/ocherdr.pub"
signature="${temporary}/payload.sig"
cleanup() {
    rm -f "${public_key}" "${signature}"
    rmdir "${temporary}" 2>/dev/null || true
}
trap cleanup EXIT

printf '%s' "${OCHERDR_UPDATER_PUBKEY}" | base64 --decode >"${public_key}"
base64 --decode <"${encoded_signature}" >"${signature}"
minisign -Vm "${payload}" -x "${signature}" -p "${public_key}"
