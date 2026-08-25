# Architecture

## Ownership boundary

Herdr is the source of truth for sessions, PTYs, processes, workspaces, tabs, panes,
layouts, persistence, and agent detection. OcHerdr projects that state into a native
window and sends explicit public API mutations back to Herdr.

OcHerdr does not link Herdr, open the private client socket, or decode the private
bincode protocol.

## Data flow

1. Run `herdr session list --json` locally or through `ssh -T`.
2. For a local session, connect directly to its public `herdr.sock`.
3. For a remote session, use OpenSSH StreamLocal forwarding to map the remote public
   socket into a private `/tmp/ocherdr-*` directory.
4. Bootstrap state with `session.snapshot`; unknown fields are ignored for forward
   compatibility.
5. Use public API methods for Workspace, Tab, Pane, and layout mutations.
6. Open `terminal session control --takeover` for every pane of the visible tab and
   `observe` for panes of hidden tabs. Only the selected pane receives input and
   terminal focus; the other control streams exist so Herdr sizes each PTY from the
   grid OcHerdr renders for it (an observe stream only records the viewport). Release
   and terminate every bridge when the visible tab or connection changes.
7. Feed decoded ANSI bytes into Ghostty's `manualMirror` surface. Ghostty owns VT
   state, shaping, glyph/image rendering, and produces a leased BGRA IOSurface.
8. Wrap that IOSurface as a CoreVideo pixel buffer without copying it and without
   attaching color-space metadata (`CVPixelBuffer::from_io_surface(..., None)`).
   GPUI samples the BGRA frame as sRGB (`surface()` defaults to sRGB; the
   Display-P3 fragment conversion in GPUI is unused) and releases the Ghostty
   frame token only after the command buffer completes. GhosttyKit labels leased
   Metal frames as Display P3, but OcHerdr does not forward that tag.
9. Ghostty follows the application's effective light/dark appearance.

## SSH policy

OcHerdr delegates configuration, keys, agents, proxies, known hosts, and authentication
to `/usr/bin/ssh`. Background commands use BatchMode and bounded connection timeouts so
the GUI cannot hang on an invisible prompt. First-use authentication and installation
are deliberately opened in the user's system Terminal.

Remote command arguments are POSIX-quoted before OpenSSH constructs its remote command
string. User labels and paths used for topology mutations travel as JSON through the
forwarded public socket instead of through a shell.

## Terminal policy

Only one controller may own a terminal. OcHerdr requests takeover for every pane of
the visible tab, so each PTY follows its on-screen grid after a window resize, divider
drag, or relocation; panes of hidden tabs are observers and Herdr keeps their size.
Stream mode is separate from focus: keyboard, IME, and mouse input and focus-in/out
reporting go to the selected pane only. Because takeover is per pane, a second
OcHerdr on the same session takes every visible pane's stream away from the first
(see `known-issues.md`). A sequence gap in a delta
frame invalidates the local terminal state and requires a fresh bridge. A full frame is
an ANSI redraw and is applied to the existing Ghostty surface; it does not destroy or
recreate renderer state.

Terminal input follows the reverse path: GPUI key or committed-text events enter
Ghostty's native input encoder, and its exact output bytes are base64-encoded into
Herdr's public `terminal.input` command. This preserves application-cursor mode,
bracketed paste, modifier protocols, and other terminal modes.

## Agent status events

OcHerdr opens two independent `events.subscribe` connections.

The session-wide EventHub types are subscribed once at connect and never
rebuilt. Herdr starts those at sequence 0 and, after the subscribe ACK,
replays retained history. There is no replay-complete marker and no
snapshot watermark, so OcHerdr cannot tell replayed history from live
events. A historical `pane.agent_detected` release then detect can land
*after* the post-connect snapshot and leave a pane as agent=X,
status=Unknown, presentation empty. An authoritative snapshot taken
after EventHub replay has drained would correct that. The client
limitation is that it cannot tell when replay has drained, so it
cannot wait for that snapshot.

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

Known limitation (session replay vs snapshot): OcHerdr cannot close this
race itself. Herdr needs to provide either a replay-complete / barrier
event after EventHub history is sent, or a monotonic sequence on every
event with `session.snapshot` atomically returning the matching EventHub
watermark, so the client can ignore history that belongs before the
snapshot it just installed. OcHerdr does not invent a local barrier.

A complete fix for cross-stream reordering still needs Herdr to provide a
globally sequenced aggregate subscription and a snapshot barrier. Until
then the same-kind restart race is an accepted limitation; OcHerdr does
not add local generation tracking for it.
