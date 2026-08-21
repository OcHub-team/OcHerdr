# OcHerdr

OcHerdr is a native macOS client for [Herdr](https://herdr.dev). It presents Herdr's
`Session → Workspace → Tab → Pane` model as a connection-aware desktop workspace,
while Herdr remains the owner of PTYs, processes, persistence, layouts, and agent state.

OcHerdr is intentionally a client, not another multiplexer:

- state and mutations use Herdr's public JSON socket API;
- local and remote sessions share the same typed model;
- remote transport uses the system OpenSSH client and SSH config;
- terminal frames use Herdr's public NDJSON terminal bridge;
- ANSI state, scrollback, Unicode handling, and reflow use native `libghostty-vt`;
- application controls come from [`ochub-ui`](https://github.com/OcHub-team/ochub-ui).

## Status

The repository currently targets macOS and Herdr `0.8.1+`. The first milestone includes:

- local and SSH-host session discovery;
- an OcHub-style node manager for switching, adding, and removing SSH nodes;
- live workspace, tab, pane, layout, and agent status rendering;
- interactive `--takeover` control for the focused pane;
- read-only observation of the other panes in the selected tab;
- workspace/tab creation, rename, close, and pane operations through the public API;
- native context menus for workspace, tab, and pane actions;
- macOS shortcuts plus Herdr's `Ctrl+B` prefix workflow;
- theme families, native blur/clear backdrops, and adjustable shell opacity;
- runtime internationalization with system-language detection, English, and Simplified Chinese;
- a read-only mirror of the native TUI settings sections with an Open TUI handoff;
- stopped-session guidance through the system Terminal.

OcHerdr never reads Herdr's private `herdr-client.sock` protocol and never stores SSH
passwords or private keys.

## Requirements

- macOS 14 or newer
- Rust 1.97.1 (selected by `rust-toolchain.toml`)
- Zig 0.15.2
- Herdr 0.8.1 or newer

Homebrew can install the build dependency without replacing a newer global Zig:

```sh
brew install zig@0.15
```

The build script automatically uses `/opt/homebrew/opt/zig@0.15/bin/zig` when present.
Otherwise set `ZIG` to a Zig 0.15.2 executable.

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

The left sidebar names its top-level Herdr endpoints **Connections**. Interface
language can be changed without restarting under Appearance → Language; the choice
is stored alongside the existing connection and appearance settings.

Start Herdr first if no running session appears:

```sh
herdr
```

SSH hosts are read from `~/.ssh/config`. Authentication and host-key enrollment stay
with OpenSSH; establish them in Terminal before using a host in OcHerdr.

## Keyboard

OcHerdr supports `Cmd+T` (new tab), `Cmd+W` (close tab), `Cmd+Shift+W` (close
workspace), `Cmd+Shift+N` (new workspace), `Cmd+1…9` (switch tab), `Ctrl+Tab`
(cycle tabs), `F2` (rename), and `Cmd+,` (Herdr settings).

The native Herdr prefix also works: press `Ctrl+B`, then use `C` for a tab,
`Shift+N` for a workspace, `N/P` to cycle tabs, `Shift+T/W/P` to rename,
`Shift+X/D` to close, `H/J/K/L` to focus panes, `1…9` to switch tabs, or `S`
to open Herdr settings.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `ocherdr-app` | GPUI shell, `ochub-ui` composition, selection and terminal surfaces |
| `ocherdr-core` | Connection/session hierarchy, layout snapshots, compatibility model |
| `ocherdr-herdr` | Public JSON socket, OpenSSH tunneling, CLI and terminal NDJSON streams |
| `ocherdr-terminal` | Safe owned wrapper around vendored native `libghostty-vt` |

The vendored Ghostty source is pinned in `vendor/libghostty-vt.vendor.json`.

## License

OcHerdr source is dual-licensed under MIT or Apache-2.0. See `NOTICE` for third-party
licensing details. Binary distributors must audit the complete GPUI dependency graph;
some revisions can introduce GPL-licensed transitive code even though OcHerdr and
`ochub-ui` source are permissively licensed.
