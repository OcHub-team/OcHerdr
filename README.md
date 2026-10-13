# OcHerdr

OcHerdr is a native macOS, Linux, and Windows client for [Herdr](https://herdr.dev). It presents Herdr's
`Session → Workspace → Tab → Pane` model as a connection-aware desktop workspace,
while Herdr remains the owner of PTYs, processes, persistence, layouts, and agent state.

OcHerdr is intentionally a client, not another multiplexer:

- state and mutations use Herdr's public JSON socket API;
- local and remote sessions share the same typed model;
- remote transport uses the system OpenSSH client and SSH config;
- terminal frames and semantic input use Herdr's stable endpoint generation 1;
- terminal rendering uses Ghostty Metal on macOS and a portable styled VT renderer on Linux and Windows;
- macOS pins the Ghostty development line toward 1.4, with [audited embedding sources and checksummed binaries](vendor/ghosttykit/README.md);
- application controls come from [`ochub-ui`](https://github.com/OcHub-team/ochub-ui).

## Status

The repository targets macOS, Linux, Windows, and Herdr `0.9.0+`, with the endpoint
contract checked against `0.9.4`. The first milestone includes:

- local and SSH-host session discovery;
- an in-app host center for filtering, organizing, diagnosing, and switching SSH hosts;
- live workspace, tab, pane, layout, and agent status rendering;
- one persistent endpoint connection per session, with input addressed to individual panes;
- a bounded cache of hidden tab surfaces, with inactive hosts pausing surface streaming;
- workspace/tab creation, rename, close, and pane operations through the public API;
- native context menus for workspace, tab, and pane actions;
- platform-native shortcuts plus Herdr's `Ctrl+B` prefix workflow;
- local clipboard image paste, plus SSH-host paste through the platform clipboard shortcut;
- a dockable right-side file manager for local workspaces and SSH hosts over SFTP,
  with typed paths, drag-and-drop transfer, progress and cancellation, contextual actions,
  and safe external-editor synchronization;
- theme families, native blur/clear backdrops, and adjustable shell opacity;
- runtime internationalization with system-language detection, English, and Simplified Chinese;
- OcHerdr-owned agent completion and attention alerts through the host notification center;
- Herdr settings guidance through the toolbar;
- stopped-session guidance through the system Terminal.

OcHerdr does not link or modify Herdr. It negotiates `endpoint.hello.v1` and the
snapshot, surface, semantic-input, and blob codecs over `herdr-client.sock` or
Herdr's SSH stdio bridge. The shared binary envelope is protocol 22; endpoint
compatibility is independent of the numbered private protocol. Unsupported
generations or core codecs fail with an upgrade error. SSH passwords and private
keys are never stored.

## Keyboard discovery and shared UI

Click **Keyboard shortcuts** in the status bar or press **Cmd+/** on macOS
(**Ctrl+Shift+/** elsewhere) to search the shortcut reference. Pressing Escape
returns focus to the previous control. Hold Command or Control to reveal tab
numbers without shifting the tab titles. `Ctrl+B`, then `M`, enters keyboard
pane movement; the reference also lists the prefix navigation keys.

Text fields use `ochub-ui` directly, including native word/line selection,
undo, Find, macOS `Ctrl+T` transpose, `Cmd+Shift+V` plain-text paste, and input
method composition protection. Terminal focus keeps terminal control sequences.

OcHub and OcHerdr share `ochub-ui` 0.7.0 and the same pinned GPUI revision.
For an unpublished local UI commit, run `just use-local-ui ../ochub-ui` before
`just check` or `just qa-app`. The ignored exact-source snapshot preserves the
Git revision in `Cargo.lock`; the normal upstream dependency remains intact.
Direct Cargo commands also need `--config .cargo/local-ui.toml` in this mode.
Set `OCHERDR_CONFIG_DIR` to use a separate configuration directory for QA;
the override covers connections, settings, themes, and window state.

## Requirements

- macOS 14 or newer, a current x86_64 Linux desktop, or Windows 10/11 x86_64
- Rust 1.98.1 (selected by `rust-toolchain.toml`)
- Xcode Command Line Tools on macOS; standard Wayland/X11 development libraries on Linux
- Herdr 0.9.0 or newer (tested with 0.9.4)

## Install

macOS builds can be installed and upgraded through the OcHub Homebrew tap:

```sh
brew tap OcHub-team/tap
brew install --cask ocherdr
```

DMGs, a Windows installer, Linux AppImage and Debian package, and portable archives are available from
[GitHub Releases](https://github.com/OcHub-team/OcHerdr/releases). OcHerdr checks for a
new signed release once per day and also exposes **OcHerdr → Check for Updates…**.
On macOS, application replacement is offered only when both the updater minisign signature and
the macOS Developer ID signature are valid; ad-hoc-signed releases, source builds, and
binaries launched outside an app bundle fall back to the release page.

On macOS, install the pinned GhosttyKit artifact once before the first build:

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

On macOS, OcHerdr supports `Cmd+T` (new tab), `Cmd+W` (close pane; last pane in a tab
closes the tab), `Cmd+Shift+W` (close workspace), `Cmd+Shift+N` (new workspace),
`Cmd+1…9` (switch tab), `Ctrl+Tab` (cycle tabs), `Cmd+Shift+E` (toggle files),
`Cmd+L` (enter a path while the file panel is open), `F2` (rename), and `Cmd+,`
(open OcHerdr appearance settings). Click the status-bar host to switch machines; `Hosts` in the
toolbar opens the connection manager. Switching machines parks the previous Herdr connection
without detaching its panes or restarting SSH. The switcher shows each connection's live status;
its red close button explicitly disconnects only that host. `Cmd+W` is for panes, not hosts. On Linux and
Windows, terminal-safe desktop equivalents use `Ctrl+Shift` (for example
`Ctrl+Shift+T`, `Ctrl+Shift+W`, `Ctrl+Shift+C`, and `Ctrl+Shift+L`) so `Ctrl+C`,
`Ctrl+W`, and `Ctrl+L` still reach the shell.

In the file panel, single-click selects, double-click opens a folder or hands a file
to the configured external editor, and right-click exposes transfer, path, rename,
and delete actions. The system-associated app is the default; **Choose editor…** in
the file menu can persist a `.app` or executable. Drop local files or folders onto the
file tree (or a specific folder row) to upload them. The transfer drawer reports byte
progress, completion, failures, and cancellation, while the download action can save a
remote file or directory anywhere on the Mac.

Remote files opened in an editor use a writable OcHerdr-owned temporary copy. Stable
saves are uploaded automatically with an observed-version check and a temporary-file
replacement, so a remote change pauses synchronization instead of being overwritten.
These editor copies are removed when OcHerdr exits.

With an image-only or file-backed image clipboard (including PixPin and Finder),
`Cmd+V` stays native for local panes. For an SSH pane, either `Cmd+V` or `Ctrl+V`
reads and validates the local image in the background, then sends one `ClipboardImage`
message over the session's existing endpoint connection. Herdr stages the bytes on the target
host and pastes its path. No extra SSH command, remote shell utility, X11 clipboard, or
Herdr server modification is involved. Switching machines keeps that same session connection
alive, so the client-scoped staged image remains valid until the host is explicitly disconnected.

The native Herdr prefix also works: press `Ctrl+B`, then use `C` for a tab,
`Shift+N` for a workspace, `N/P` to cycle tabs, `Shift+T/W/P` to rename,
`Shift+X/D` to close, `H/J/K/L` to focus panes, `1…9` to switch tabs, or `S`
to open Herdr settings guidance. The gear opens the same panel; the palette and
`Cmd+,` open OcHerdr's own appearance settings. Herdr's full TUI settings are not
embedded by the endpoint client; use Herdr on the connected host to edit them.

On macOS, text fields (including dynamically created Find fields) support:

- `Cmd+Left/Right`: line start/end; `Cmd+Up/Down`: document start/end.
- `Option+Left/Right`: move by Unicode word; add `Shift` to extend the selection.
- `Shift+Up/Down` and `Cmd+Shift+arrows`: extend the selection by line/document.
- `Cmd+Backspace/Delete`: delete to line start/end; `Option+Backspace/Delete`:
  delete by word. Existing selections take precedence.
- `Ctrl+A/E/B/F/P/N`: line/character/vertical movement; `Ctrl+H/D`: backward/forward
  deletion; `Ctrl+K`: delete to line end (or the newline); `Ctrl+T`: transpose characters.
- `Cmd+A/C/X/V`, `Cmd+Shift+V` (plain text), `Cmd+Z`, `Cmd+Shift+Z`: selection,
  clipboard, undo and redo. Composed Unicode characters and IME preedit are preserved.

The file panel's **Show hidden files** button reveals dotfiles and directories
such as `.env` and `.git`, locally and over SFTP. Toggle it with `Cmd+Shift+.` on
macOS (`Ctrl+H` elsewhere) while the file panel is open. The preference is saved;
changing it refreshes expanded directories and invalidates collapsed-directory caches.

OcHerdr receives semantic agent completion and attention notifications over the
session endpoint. The pane currently visible in the frontmost OcHerdr window does
not produce an alert. Agent notifications can be disabled
under General settings. They use Notification Center on macOS, native Toast
notifications on Windows, and the XDG notification service on Linux; remote Herdr
sessions notify on the computer running OcHerdr. Clicking a notification activates
OcHerdr.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `ocherdr-app` | GPUI shell, `ochub-ui` composition, selection and terminal surfaces |
| `ocherdr-core` | Connection/session hierarchy, layout snapshots, compatibility model |
| `ocherdr-files` | Unified local/SFTP filesystem operations and recursive transfers |
| `ocherdr-herdr` | Public JSON socket, OpenSSH API forwarding, endpoint stdio bridge, and generation-1 codecs |
| `ocherdr-terminal` | GhosttyKit/Metal renderer on macOS and portable styled VT renderer on Linux/Windows |

On macOS, GhosttyKit is pinned and checksum-verified by `scripts/bootstrap-ghosttykit.sh`. GPUI
is pinned to OcHerdr's leased-BGRA surface extension in the OcHub-team Zed fork. The
surface path keeps Ghostty's frame lease alive through Metal completion and preserves
the leased BGRA IOSurface's Display P3 color-space metadata. The macOS window uses
color-managed Display P3 output; ordinary sRGB UI colors and images are converted
before compositing. Core Animation handles the destination display profile.

Terminal colors and fonts remain independently configurable. To match standalone
Ghostty, use **Import Ghostty Config** in appearance settings: this imports its
terminal theme and font settings without replacing the OcHerdr UI theme. Previously
imported themes should be imported again to restore background, foreground, cursor,
and selection colors that older versions omitted. See [color rendering](docs/color-rendering.md).

Trackpad scrolling uses logical display units, coalesces high-frequency wheel input,
and reuses cached application chrome during terminal-only updates. See
[scroll performance and regression measurements](docs/scroll-performance.md).

## License

OcHerdr source is dual-licensed under MIT or Apache-2.0. See `NOTICE` for third-party
licensing details. Binary distributors must audit the complete GPUI dependency graph;
some revisions can introduce GPL-licensed transitive code even though OcHerdr and
`ochub-ui` source are permissively licensed.

Maintainer release setup and the tag-to-Homebrew flow are documented in
[`docs/releasing.md`](docs/releasing.md).

If the pinned GPUI commit is also local, run
`python3 scripts/use-local-ui.py ../ochub-ui --gpui-repository ../zed`.
The Git URL mapping applies only to that fetch; lockfile source identities and global Git settings remain intact.

See [the shared UI integration record](docs/shared-ui-2026-10-14.md) for component ownership and validation.
