#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
target="${OCHERDR_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
out_dir="${1:-dist}"

cd "${repo_root}"
mkdir -p "${out_dir}"
out_dir="$(cd "${out_dir}" && pwd)"

version="$(
    cargo metadata --no-deps --format-version 1 |
        jq -r '.packages[] | select(.name == "ocherdr") | .version'
)"

cargo build --release --locked --target "${target}" -p ocherdr

case "${target}" in
aarch64-apple-darwin) arch="aarch64" ;;
x86_64-apple-darwin) arch="x86_64" ;;
*)
    printf 'unsupported release target: %s\n' "${target}" >&2
    exit 1
    ;;
esac

signing_mode="${MACOS_SIGNING_MODE:-auto}"
case "${signing_mode}" in
auto | required | adhoc) ;;
*)
    printf 'unsupported MACOS_SIGNING_MODE: %s (expected auto, required, or adhoc)\n' \
        "${signing_mode}" >&2
    exit 1
    ;;
esac

apple_credentials=(
    APPLE_SIGNING_IDENTITY
    APPLE_CERTIFICATE
    APPLE_CERTIFICATE_PASSWORD
    APPLE_TEAM_ID
)
missing_apple_credentials=()
for variable in "${apple_credentials[@]}"; do
    if [[ -z "${!variable:-}" ]]; then
        missing_apple_credentials+=("${variable}")
    fi
done

signing_identity=""
developer_id_signing=false
notarize=false
if [[ "${signing_mode}" != adhoc && ${#missing_apple_credentials[@]} -eq 0 ]]; then
    signing_identity="${APPLE_SIGNING_IDENTITY}"
    developer_id_signing=true
    if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" ]]; then
        notarize=true
    elif [[ -n "${APPLE_ID:-}" || -n "${APPLE_PASSWORD:-}" ]]; then
        printf 'APPLE_ID and APPLE_PASSWORD must be configured together.\n' >&2
        exit 1
    else
        unset APPLE_ID APPLE_PASSWORD APPLE_KEYCHAIN_PROFILE
        unset APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
    fi
elif [[ "${signing_mode}" == required ]]; then
    printf 'Developer ID signing is required, but these credentials are missing: %s\n' \
        "${missing_apple_credentials[*]}" >&2
    exit 1
else
    if [[ "${signing_mode}" == auto && ${#missing_apple_credentials[@]} -gt 0 ]]; then
        printf 'Apple signing credentials are incomplete (%s); using ad-hoc signing.\n' \
            "${missing_apple_credentials[*]}"
    else
        printf 'Using ad-hoc macOS signing.\n'
    fi
    signing_identity=""
    unset APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
    unset APPLE_TEAM_ID APPLE_ID APPLE_PASSWORD APPLE_KEYCHAIN_PROFILE
    unset APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
fi

config_json="$(
    jq -cn \
        --arg version "${version}" \
        --arg target "${target}" \
        --arg binaries_dir "${repo_root}/target/${target}/release" \
        --arg out_dir "${out_dir}" \
        --arg icon "${repo_root}/packaging/macos/OcHerdr.icns" \
        --arg info_plist "${repo_root}/packaging/macos/Info.plist" \
        --arg entitlements "${repo_root}/packaging/macos/entitlements.plist" \
        --arg identity "${signing_identity}" \
        '{
            productName: "OcHerdr",
            version: $version,
            identifier: "io.github.ochub-team.ocherdr",
            category: "DeveloperTool",
            description: "Native macOS client for Herdr",
            authors: ["OcHub contributors"],
            publisher: "OcHub contributors",
            binaries: [{ path: "ocherdr", main: true }],
            binariesDir: $binaries_dir,
            outDir: $out_dir,
            targetTriple: $target,
            icons: [$icon],
            macos: ({
                minimumSystemVersion: "14.0",
                entitlements: $entitlements,
                infoPlistPath: $info_plist
            } + if $identity == "" then {} else { signingIdentity: $identity } end),
            dmg: {
                windowSize: { width: 660, height: 420 },
                appPosition: { x: 180, y: 210 },
                appFolderPosition: { x: 480, y: 210 }
            }
        }'
)"

cargo packager --config "${config_json}" --formats app,dmg

app_path="$(find "${out_dir}" -maxdepth 1 -type d -name 'OcHerdr.app' -print -quit)"
dmg_path="$(find "${out_dir}" -maxdepth 1 -type f -name '*.dmg' -print -quit)"
if [[ -z "${app_path}" || -z "${dmg_path}" ]]; then
    printf 'cargo-packager did not produce both OcHerdr.app and a DMG\n' >&2
    exit 1
fi

if [[ "${developer_id_signing}" == true ]]; then
    apple_requirement="=anchor apple generic and certificate leaf[subject.OU] = \"${APPLE_TEAM_ID}\""
    for artifact in "${app_path}" "${dmg_path}"; do
        codesign --verify --deep --strict --verbose=2 "${artifact}"
        codesign --verify --deep --strict -R "${apple_requirement}" "${artifact}"
        details="$(codesign -dv --verbose=4 "${artifact}" 2>&1)"
        grep -q '^Authority=Developer ID Application:' <<<"${details}"
        grep -Fxq "TeamIdentifier=${APPLE_TEAM_ID}" <<<"${details}"
    done
else
    codesign --force --deep --sign - "${app_path}"
    codesign --force --sign - "${dmg_path}"
fi

if [[ "${notarize}" == true ]]; then
    xcrun notarytool submit "${dmg_path}" \
        --apple-id "${APPLE_ID}" \
        --password "${APPLE_PASSWORD}" \
        --team-id "${APPLE_TEAM_ID}" \
        --wait
    xcrun stapler staple "${dmg_path}"
    xcrun stapler validate "${dmg_path}"
fi

tarball="${out_dir}/OcHerdr_${version}_macos_${arch}.app.tar.gz"
tar -czf "${tarball}" -C "${out_dir}" "$(basename "${app_path}")"
rm -rf "${app_path}"

printf 'release DMG: %s\n' "${dmg_path}"
printf 'updater payload: %s\n' "${tarball}"
