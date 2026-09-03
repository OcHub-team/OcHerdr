# Architecture

## Ownership boundary

Herdr is the source of truth for sessions, PTYs, processes, workspaces, tabs, panes,
layouts, persistence, and agent detection. OcHerdr projects that state into a native
window and sends explicit public API mutations back to Herdr.

OcHerdr does not link or modify Herdr. It mirrors the serialization-relevant v20
private client schema in a frozen codec and exposes only version-neutral terminal
commands and events to the rest of the application.

## Data flow

1. Run `herdr session list --json` locally or through `ssh -T`.
2. For a local session, connect directly to its public `herdr.sock`.
3. For a remote session, one persistent OpenSSH process uses two StreamLocal forwards
   to map both the public API socket and private client socket into an owner-private
   `/tmp/ocherdr-*` directory.
4. Bootstrap state with `session.snapshot`; unknown fields are ignored for forward
   compatibility.
5. Use public API methods for Workspace, Tab, Pane, and layout mutations.
6. Open a measured pane through the native private socket as `ObserveTerminal`. A
   click, wheel gesture, or terminal input replaces that pane's connection with
   `ControlTerminal { takeover: true }` without releasing other controlled panes.
   Only the selected pane receives keyboard/IME input and terminal focus; wheel input
   targets the pane under the pointer. Hidden panes retain their existing stream mode,
   while recently visited Ghostty surfaces and frames remain in a bounded LRU cache.
   Release every private connection when its pane is evicted or the session changes.
7. Feed decoded ANSI bytes into Ghostty's `manualMirror` surface. Ghostty owns VT
   state, shaping, glyph/image rendering, and produces a leased BGRA IOSurface.
8. Wrap that IOSurface as a CoreVideo pixel buffer without copying it and without
   attaching color-space metadata (`CVPixelBuffer::from_io_surface(..., None)`).
   GPUI samples the BGRA frame as sRGB (`surface()` defaults to sRGB; the
   Display-P3 fragment conversion in GPUI is unused) and releases the Ghostty
   frame token only after the command buffer completes. GhosttyKit labels leased
   Metal frames as Display P3, but OcHerdr does not forward that tag.
9. Ghostty follows the application's effective light/dark appearance.

## Host residency

The status-bar host selector changes which host runtime is visible; it does not replace the
runtime. A live background host retains its `SessionConnection`, OpenSSH tunnel, event worker,
snapshot, selection, pane runtimes, and private terminal streams in a profile-keyed parking map.
Terminal and event workers carry a `(profile_id, session_name)` owner key, so equal pane ids on
different hosts cannot route frames into the active host by mistake. Events for a parked host are
drained as connection liveness signals, and the current snapshot is fetched again when that host
returns to the foreground.

Only an explicit disconnect action, profile removal/reconfiguration, session replacement, or app
shutdown releases a parked runtime. Normal tab and host switches therefore preserve Herdr's
client id and client-scoped resources such as staged clipboard images. OcHerdr never evicts a live
host connection automatically; the switcher exposes a disconnect button for that resource choice.

## SSH policy

OcHerdr delegates configuration, keys, agents, proxies, known hosts, and authentication
to `/usr/bin/ssh`. Background commands use BatchMode and bounded connection timeouts so
the GUI cannot hang on an invisible prompt. First-use authentication and installation
are deliberately opened in the user's system Terminal.

Remote command arguments are POSIX-quoted before OpenSSH constructs its remote command
string. User labels and paths used for topology mutations travel as JSON through the
forwarded public socket instead of through a shell.

An image-only or single file-backed image pasted with `Cmd+V` or `Ctrl+V` follows the
same local-client boundary as Herdr remote attach. File-backed clipboard providers
such as PixPin are recognized before GPUI's path-to-text fallback; OcHerdr reads the
local file on a background executor, validates its signature and 16 MiB limit, then
sends `ClipboardImage` over the selected pane's existing private connection. Herdr
stages and pastes the path on the host that owns the PTY. No second SSH process, remote
shell, X11 clipboard, `cat`, or `rm` is required.

## Terminal policy

Only one controller may own one terminal, but a single OcHerdr may independently
control multiple terminals. Panes begin as observers and direct interaction promotes
the target pane while preserving the other controlled panes. Stream mode is separate
from focus: keyboard, IME, and focus-in/out reporting go to the selected pane, while
wheel input goes to the pane under the pointer. A takeover by another client demotes
only the affected pane locally; it is promoted again only by another direct
interaction. Untouched panes on hidden tabs remain observers. Panes already controlled
by OcHerdr retain that control stream across tab switches, and all hidden panes retain
their most recent surface until LRU eviction. A sequence gap in a delta frame
invalidates the local terminal state and requires a fresh bridge. A full frame is an
ANSI redraw and is applied to the existing Ghostty surface; it does not destroy or
recreate renderer state.

Terminal input follows the reverse path: GPUI key or committed-text events enter
Ghostty's native input encoder, and its exact output bytes are sent losslessly in the
private protocol's `Input` message. This preserves application-cursor mode, bracketed
paste, modifier protocols, and other terminal modes.

Key presses, repeats, and releases go through `ghostty_surface_key`. The surface runs
in `GHOSTTY_SURFACE_IO_MANUAL_MIRROR` mode with an `io_write_cb`, so everything
Ghostty wants to write to the pty (encoded keys, pastes, mouse reports, query
replies) lands in one per-surface queue that `Terminal::try_input` drains; the
controller forwards it to the selected pane's control stream after each key and on
every frame and event poll. OcHerdr's own shortcuts (`handle_app_shortcut`: ⌘
combos, the Ctrl+B prefix mode, overlay keys) are claimed before the terminal sees
the key, and the matching key-up is swallowed too. IME-composed text keeps its own
commit path and is written as-is.

The event Ghostty receives mirrors what Ghostty.app builds from an `NSEvent`:
`keycode` is the macOS virtual keycode (mapped back from GPUI's key name through the
US ANSI layout, since GPUI does not expose the hardware code), `text` is GPUI's
`key_char` for plain keys and the chord's own character for Ctrl/Alt chords,
`unshifted_codepoint` is the key name when it is a single character, and Shift is
reported as consumed when it produced the text. Ghostty then tracks the kitty
keyboard protocol and modifyOtherKeys state itself, so Shift+Enter is
`ESC [27;2;13~` in the legacy encoding and `ESC [13;2u` once an application such as
Claude Code enables the kitty protocol.

Ghostty's default keybindings are application actions (tabs, splits, windows, font
size, search, inspector) that OcHerdr either implements itself or does not offer,
and a bound key never reaches the pty. `ocherdr-terminal` therefore starts every
Ghostty config it loads with `keybind = clear` and restores only the macOS defaults
that are pty writes (`super+left/right/backspace` → `^A`/`^E`/`^U`,
`alt+left/right` → `ESC b`/`ESC f`), so a key OcHerdr does not claim is encoded
exactly as Ghostty would encode it with those bindings. The same base config sets
`macos-option-as-alt = true`: Option is Alt, prefixing `ESC` rather than typing the
macOS symbol, which is what OcHerdr sent before it used Ghostty's encoder.

## Private protocol evolution

The snapshot's `protocol` value selects a codec before a terminal socket is opened.
Unknown versions are rejected; OcHerdr never guesses a layout or falls back to the
process/NDJSON adapter.

`ocherdr-herdr/src/private_v20.rs` is a frozen wire schema. Enum and field order are
part of bincode's ABI and are guarded by golden byte fixtures. Application code cannot
import it. `private_protocol.rs` owns the explicit version registry, handshake, limits,
and mapping between wire values and the stable `TerminalCommand` / `TerminalEvent`
facade.

`Notify` messages are also part of the stable facade. Herdr `Toast` messages use
OcHerdr's in-app notification host, while `SystemToast` messages are posted through
GPUI's platform notification API. On macOS that API uses `UNUserNotificationCenter`,
requests alert permission lazily, retains notifications in Notification Center, and
opts into foreground banner/list presentation. The packaged app's bundle identifier
provides its notification identity; source binaries run outside an app bundle safely
leave system delivery disabled.

For a future v21, add a new `private_v21.rs`, copy the released Herdr schema exactly,
add independent golden fixtures, register one new codec variant, and implement only
the facade mappings that changed. The SSH tunnel, pane lifecycle, clipboard reader,
Ghostty integration, and controllers must not depend on a versioned wire type and
therefore should require no edits. The end-to-end fake-socket test must run once per
registered codec before support is advertised.

## Agent status events

OcHerdr opens two independent `events.subscribe` connections.

The session-wide EventHub types are subscribed once at connect and never
rebuilt. Herdr starts those at sequence 0 and, after the subscribe ACK,
replays retained history. OcHerdr never applies that startup burst to the
rendered snapshot: every batch extends a short quiet-period deadline. Once
the stream is quiet, OcHerdr fetches and installs one authoritative snapshot.
Events arriving while that snapshot is in flight request another snapshot,
so historical `layout.updated` states are never presented as a startup
animation and a concurrent live change is not silently lost.

The per-pane `pane.agent_status_changed` subscription is rebuilt when the
snapshot pane set changes. Herdr starts those parameterized entries at the
hub's current sequence, so a rebuild does not replay status history.

Herdr does not merge the two connections into a globally ordered stream:
detect/release on the session subscription and status on the per-pane
subscription can arrive out of order.

OcHerdr therefore treats a name mismatch between a status event and the pane's
current agent as `Resync`. That catches cross-subscription reordering of
*different* agents. It cannot distinguish two instances of the same kind
(`grok` then another `grok` in the same pane). In that case a stale status
event can apply to the new generation and stay until the next status event
or a resync; it is not necessarily brief.

Known limitation (session replay vs snapshot): the quiet-period deadline is a
presentation barrier, not a protocol watermark. It prevents retained history
from being rendered and converges through a final snapshot, but a continuously
busy stream can postpone startup convergence. Herdr should still provide
either a replay-complete event or monotonic event sequences with an atomic
snapshot watermark for an exact protocol-level barrier.

A complete fix for cross-stream reordering still needs Herdr to provide a
globally sequenced aggregate subscription and a snapshot barrier. Until
then the same-kind restart race is an accepted limitation; OcHerdr does
not add local generation tracking for it.
