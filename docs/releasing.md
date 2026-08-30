# Releasing OcHerdr

OcHerdr releases are tag-driven. Pushing `v<workspace-version>` runs the release
workflow; a tag whose version does not exactly match `Cargo.toml` is rejected before
any macOS build starts.

The pipeline builds Apple Silicon and Intel apps on their native GitHub runners,
signs each updater archive with a separate minisign key, generates checksums and a
versioned `latest.json`, attests every asset, and publishes one GitHub Release. When
all Developer ID credentials are configured, it Developer ID-signs each app and DMG
and optionally notarizes the DMG. Otherwise it produces an explicitly ad-hoc-signed
release. A published release is immutable from this workflow: a manual rerun may
resume a draft but will not overwrite a release that is already public.

## Repository configuration

Configure these GitHub Actions secrets in `OcHub-team/OcHerdr` before pushing a tag:

| Name | Required | Purpose |
| --- | --- | --- |
| `APPLE_SIGNING_IDENTITY` | optional set | Exact `Developer ID Application: …` identity imported from the certificate |
| `APPLE_CERTIFICATE` | optional set | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | optional set | Password used when the `.p12` was exported |
| `APPLE_TEAM_ID` | optional set | Apple Developer Team ID; release validation requires every artifact to match it |
| `OCHERDR_SIGNING_PRIVATE_KEY` | yes | Contents of the dedicated cargo-packager/minisign private key |
| `OCHERDR_SIGNING_PRIVATE_KEY_PASSWORD` | if set | Password chosen for the updater key |
| `APPLE_ID` | recommended | Apple account used by `notarytool` |
| `APPLE_PASSWORD` | with `APPLE_ID` | App-specific password for notarization |
| `HOMEBREW_TAP_DISPATCH_TOKEN` | optional | Fine-grained token for `OcHub-team/homebrew-tap`, with Contents write access, used for an immediate repository dispatch |

Also configure the Actions variable `OCHERDR_UPDATER_PUBKEY` with the contents of the
matching public updater key. The release build compiles this value into the app; it is
not downloaded at runtime.

Generate a dedicated key once with the pinned packaging tool and store its private
half outside the repository:

```sh
cargo install cargo-packager --version 0.11.8 --locked
cargo packager signer generate --ci --path /secure/path/ocherdr-updater.key
gh secret set OCHERDR_SIGNING_PRIVATE_KEY \
  --repo OcHub-team/OcHerdr < /secure/path/ocherdr-updater.key
gh variable set OCHERDR_UPDATER_PUBKEY \
  --repo OcHub-team/OcHerdr < /secure/path/ocherdr-updater.key.pub
```

Add `--password` when generating the key and configure the corresponding password
secret if the private key should be encrypted. Never commit either the `.p12` or the
updater private key.

`HOMEBREW_TAP_DISPATCH_TOKEN` only reduces latency. The tap's
`update-ocherdr-cask.yml` also checks the latest public release every day, so a missing
cross-repository token does not fail or block an OcHerdr release.

The four Developer ID values form one optional set. If any member is absent, the
release workflow falls back to ad-hoc signing and skips notarization. Homebrew can
still install and upgrade that build, but Gatekeeper may require explicit approval on
first launch and the in-app updater will open the release page instead of replacing
the application automatically. Set `MACOS_SIGNING_MODE=required` when running the
packaging script to fail closed instead of falling back.

## Publish a version

1. Update `[workspace.package].version` in `Cargo.toml` and refresh `Cargo.lock`.
2. Run `just ci` and, when packaging changed, run
   `scripts/release/package-macos.sh` locally as well.
3. Merge the tested commit to `main`.
4. Create and push an exact version tag:

   ```sh
   git tag -s v0.2.0 -m "OcHerdr 0.2.0"
   git push origin v0.2.0
   ```

5. Confirm the Release workflow publishes two DMGs, two `.app.tar.gz` updater
   payloads, their `.sig` files, `SHA256SUMS`, and `latest.json`.
6. Confirm `OcHub-team/homebrew-tap` updates `Casks/ocherdr.rb`. If immediate dispatch
   is not configured, run its workflow manually or wait for the daily schedule.

## Update protocol and key rotation

`latest.json` currently uses `schema_version: 1` and independent platform entries for
`darwin-aarch64` and `darwin-x86_64`. Readers ignore additive JSON fields but fail
closed on an unsupported schema version. This leaves room for future channels,
rollouts, delta formats, or extra platforms without changing version 1 consumers; a
breaking manifest change must increment the schema and ship reader support before a
writer starts emitting it.

The updater accepts only archives signed by the public key compiled into that build.
For a planned key rotation, first ship a reader capable of trusting both the old and
new key IDs, then switch release signing, and only remove the old key after the entire
supported upgrade population has crossed the bridge release. An emergency loss of the
current private key requires a manual DMG/Homebrew update because older binaries must
not learn replacement trust roots from the network.
