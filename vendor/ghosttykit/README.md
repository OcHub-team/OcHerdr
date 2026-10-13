# GhosttyKit

OcHerdr uses the latest audited Ghostty development tree with a small embedding
patch maintained in [OcHub-team/ghostty](https://github.com/OcHub-team/ghostty/tree/ocherdr/ghosttykit-upstream).
The exact source and artifact checksum are pinned in [version.env](version.env):

- Upstream: `cc53940b4cf97876e48826e5d825833a77471c89`
- Embedding source: `b8cc75b0243d0489c0668892bfa2239af0a2409d`
- Upstream version: `1.3.2-dev`, the development line toward Ghostty 1.4;
  this is not a released Ghostty 1.4 build.
- Targets: macOS arm64 and x86_64; Sentry is disabled.

The embedding patch supports manual mirror I/O, leased external Metal frames,
authoritative grid sizing, visible-row selection, and a per-surface scrollback
limit. Herdr owns terminal protocol replies; Ghostty mirrors its output. The
header and library must be upgraded together because the development C API is
not stable. In particular, clipboard payloads now carry an explicit byte length.

Run `scripts/bootstrap-ghosttykit.sh` to download and verify the matching
`GhosttyKit.xcframework`. An existing framework is replaced only after the new
archive passes checksum, layout, and source-version checks. The generated
framework is not tracked by Git.

To rebuild with Zig 0.16.0 and a full Xcode installation:

```sh
git clone --branch ocherdr/ghosttykit-upstream https://github.com/OcHub-team/ghostty.git /tmp/ocherdr-ghostty
git -C /tmp/ocherdr-ghostty checkout b8cc75b0243d0489c0668892bfa2239af0a2409d
./scripts/build-ghosttykit.sh /tmp/ocherdr-ghostty
```

The build script produces `target/ghosttykit/GhosttyKit.xcframework.tar.gz` and
prints its SHA-256. Compiler and SDK differences can change the archive hash;
`version.env` verifies the published artifact, not arbitrary local rebuilds.
The source branch documents and includes Zig regression tests and a native
Metal integration test in `test/ocherdr/run.sh`.
