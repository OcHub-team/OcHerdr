#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
target="${OCHERDR_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
out_dir="${1:-dist}"

case "${target}" in
x86_64-unknown-linux-gnu)
    platform="linux"
    arch="x86_64"
    binary="ocherdr"
    formats="deb,appimage"
    ;;
aarch64-unknown-linux-gnu)
    platform="linux"
    arch="aarch64"
    binary="ocherdr"
    formats="deb,appimage"
    ;;
x86_64-pc-windows-msvc)
    platform="windows"
    arch="x86_64"
    binary="ocherdr.exe"
    formats="nsis"
    ;;
*)
    printf 'unsupported portable release target: %s\n' "${target}" >&2
    exit 1
    ;;
esac

cd "${repo_root}"
mkdir -p "${out_dir}"
out_dir="$(cd "${out_dir}" && pwd)"
version="$({
    cargo metadata --no-deps --format-version 1 |
        jq -r '.packages[] | select(.name == "ocherdr") | .version'
})"

cargo build --release --locked --target "${target}" -p ocherdr
binary_dir="${repo_root}/target/${target}/release"
test -f "${binary_dir}/${binary}"

config_json="$({
    jq -cn \
        --arg version "${version}" \
        --arg target "${target}" \
        --arg binaries_dir "${binary_dir}" \
        --arg out_dir "${out_dir}" \
        --arg icon "${repo_root}/packaging/macos/OcHerdr.png" \
        '{
            productName: "OcHerdr",
            version: $version,
            identifier: "io.github.ochub-team.ocherdr",
            category: "DeveloperTool",
            description: "Cross-platform client for Herdr",
            authors: ["OcHub contributors"],
            publisher: "OcHub contributors",
            homepage: "https://github.com/OcHub-team/OcHerdr",
            licenseFile: "LICENSE-APACHE",
            binaries: [{ path: "ocherdr", main: true }],
            binariesDir: $binaries_dir,
            outDir: $out_dir,
            targetTriple: $target,
            icons: [$icon],
            deb: {
                depends: [
                    "libasound2",
                    "libfontconfig1",
                    "libssl3",
                    "libwayland-client0",
                    "libx11-xcb1",
                    "libxkbcommon-x11-0"
                ]
            },
            nsis: { installMode: "perUser" }
        }'
})"

cargo packager --config "${config_json}" --formats "${formats}"

archive_root="$(mktemp -d "${TMPDIR:-/tmp}/ocherdr-package.XXXXXX")"
cleanup() {
    rm -rf "${archive_root}"
}
trap cleanup EXIT
mkdir -p "${archive_root}/OcHerdr"
cp "${binary_dir}/${binary}" "${archive_root}/OcHerdr/${binary}"
cp README.md LICENSE-APACHE LICENSE-MIT "${archive_root}/OcHerdr/"
tarball="${out_dir}/OcHerdr_${version}_${platform}_${arch}.tar.gz"
tar -czf "${tarball}" -C "${archive_root}" OcHerdr

case "${platform}" in
linux)
    find "${out_dir}" -maxdepth 1 -type f -name '*.deb' -print -quit | grep -q .
    find "${out_dir}" -maxdepth 1 -type f -name '*.AppImage' -print -quit | grep -q .
    ;;
windows)
    find "${out_dir}" -maxdepth 1 -type f -name '*-setup.exe' -print -quit | grep -q .
    ;;
esac

printf '%s packages: %s\n' "${platform}" "${out_dir}"
