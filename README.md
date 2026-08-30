# OcHerdr

OcHerdr is a native macOS client for [Herdr](https://herdr.dev). It presents Herdr's
`Session → Workspace → Tab → Pane` model as a connection-aware desktop workspace,
while Herdr remains the owner of PTYs, processes, persistence, layouts, and agent state.

OcHerdr is intentionally a client, not another multiplexer:

- state and mutations use Herdr's public JSON socket API;
- local and remote sessions share the same typed model;
- remote transport uses the system OpenSSH client and SSH config;
- terminal frames and input use a versioned facade over Herdr's private client protocol;
- ANSI state, font shaping, colors, images, and terminal rendering use native Ghostty Metal;
- application controls come from [`ochub-ui`](https://github.com/OcHub-team/ochub-ui).

## Status

The repository currently targets macOS and Herdr `0.8.1+`. The first milestone includes:

- local and SSH-host session discovery;
- an in-app host center for filtering, organizing, diagnosing, and switching SSH hosts;
- live workspace, tab, pane, layout, and agent status rendering;
- independent `--takeover` control for each visible pane after a click, wheel, or input action;
- read-only observation of untouched panes, with a bounded cache of hidden tab surfaces;
- workspace/tab creation, rename, close, and pane operations through the public API;
- native context menus for workspace, tab, and pane actions;
- macOS shortcuts plus Herdr's `Ctrl+B` prefix workflow;
- local clipboard image paste with `Cmd+V`, plus SSH-host paste with `Cmd+V` or `Ctrl+V`;
- theme families, native blur/clear backdrops, and adjustable shell opacity;
- runtime internationalization with system-language detection, English, and Simplified Chinese;
- an Open TUI handoff for Herdr settings through the system Terminal;
- stopped-session guidance through the system Terminal.

OcHerdr does not link or modify Herdr. It implements protocol 20 of Herdr's private
`herdr-client.sock` wire format behind an isolated codec, and never stores SSH passwords
or private keys. Unknown protocol versions fail closed with an upgrade error.

## Requirements

- macOS 14 or newer
- Rust 1.97.1 (selected by `rust-toolchain.toml`)
- Xcode Command Line Tools
- Herdr 0.8.1 or newer

## Install

Published builds can be installed and upgraded through the OcHub Homebrew tap:

```sh
brew tap OcHub-team/tap
brew install --cask ocherdr
```

The same signed DMGs are available from
[GitHub Releases](https://github.com/OcHub-team/OcHerdr/releases). OcHerdr checks for a
new signed release once per day and also exposes **OcHerdr → Check for Updates…**.
Application replacement is offered only when both the updater minisign signature and
the macOS code-signing identity are valid; source builds and binaries launched outside
an app bundle fall back to the release page.

Install the pinned GhosttyKit artifact once before the first build:

```sh
./scripts/bootstrap-ghosttykit.sh
```

## Run

```sh
cargo run -p ocherdr
```

For local acceptance, install [`just`](https://github.com/casey/just) and run:

```sh
just accept
```

This checks the toolchain, runs the same format/Clippy/test gate as CI, creates an
ad-hoc signed `target/qa/OcHerdr.app`, and opens a fresh instance. Use `just --list`
for the shorter development commands.

The left sidebar starts directly with Herdr workspaces. OcHerdr automatically prefers
the running `default` session on each host; if it is unavailable, it falls back to the
first running session. Interface language can be changed without restarting under
Appearance → Language; the choice is stored alongside the existing connection and
appearance settings.

Start Herdr first if no running session appears:

```sh
herdr
```

SSH hosts are read from `~/.ssh/config` and its `Include` fragments. The files stay
read-only: OcHerdr stores only local favorites, groups, tags, and selected overrides.
Authentication and host-key enrollment remain with OpenSSH and the system Terminal.

## Keyboard

OcHerdr supports `Cmd+T` (new tab), `Cmd+W` (close pane; last pane in a tab
closes the tab), `Cmd+Shift+W` (close workspace), `Cmd+Shift+N` (new workspace),
`Cmd+1…9` (switch tab), `Ctrl+Tab` (cycle tabs), `F2` (rename), and `Cmd+,`
(open Herdr settings in Terminal). Click the status-bar host to switch machines; `Hosts` in the
toolbar opens the connection manager. `Cmd+W` is for panes, not hosts.

With an image-only or file-backed image clipboard (including PixPin and Finder),
`Cmd+V` stays native for local panes. For an SSH pane, either `Cmd+V` or `Ctrl+V`
reads and validates the local image in the background, then sends one `ClipboardImage`
message over the pane's existing Herdr connection. Herdr stages the bytes on the target
host and pastes its path. No extra SSH command, remote shell utility, X11 clipboard, or
Herdr server modification is involved.

The native Herdr prefix also works: press `Ctrl+B`, then use `C` for a tab,
`Shift+N` for a workspace, `N/P` to cycle tabs, `Shift+T/W/P` to rename,
`Shift+X/D` to close, `H/J/K/L` to focus panes, `1…9` to switch tabs, or `S`
to open Herdr settings in Terminal.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `ocherdr-app` | GPUI shell, `ochub-ui` composition, selection and terminal surfaces |
| `ocherdr-core` | Connection/session hierarchy, layout snapshots, compatibility model |
| `ocherdr-herdr` | Public JSON socket, dual-socket OpenSSH tunnel, and versioned terminal protocol codecs |
| `ocherdr-terminal` | GhosttyKit runtime, leased IOSurface frames, and native input encoding |

GhosttyKit is pinned and checksum-verified by `scripts/bootstrap-ghosttykit.sh`. GPUI
is pinned to OcHerdr's leased-BGRA surface extension in the OcHub-team Zed fork. The
surface path keeps Ghostty's frame lease alive through Metal completion and samples
the leased BGRA IOSurface as sRGB.

## License

OcHerdr source is dual-licensed under MIT or Apache-2.0. See `NOTICE` for third-party
licensing details. Binary distributors must audit the complete GPUI dependency graph;
some revisions can introduce GPL-licensed transitive code even though OcHerdr and
`ochub-ui` source are permissively licensed.

Maintainer release setup and the tag-to-Homebrew flow are documented in
[`docs/releasing.md`](docs/releasing.md).
