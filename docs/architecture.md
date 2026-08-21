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
6. Open `terminal session control --takeover` for the focused pane and `observe` for
   sibling panes. Release and terminate every bridge when focus or connection changes.
7. Feed decoded ANSI bytes into Ghostty's `manualMirror` surface. Ghostty owns VT
   state, shaping, glyph/image rendering, and produces a leased BGRA IOSurface.
8. Wrap that IOSurface as a CoreVideo pixel buffer without copying it. GPUI samples
   the native frame in its Metal pass and releases the Ghostty frame token only after
   the command buffer completes.
9. Carry Ghostty's Display-P3 metadata with the frame. GPUI converts P3 to sRGB in
   the fragment shader while preserving premultiplied alpha, and Ghostty follows the
   application's effective light/dark appearance.

## SSH policy

OcHerdr delegates configuration, keys, agents, proxies, known hosts, and authentication
to `/usr/bin/ssh`. Background commands use BatchMode and bounded connection timeouts so
the GUI cannot hang on an invisible prompt. First-use authentication and installation
are deliberately opened in the user's system Terminal.

Remote command arguments are POSIX-quoted before OpenSSH constructs its remote command
string. User labels and paths used for topology mutations travel as JSON through the
forwarded public socket instead of through a shell.

## Terminal policy

Only one controller may own a terminal. OcHerdr always requests takeover for the pane
selected by the user. All other visible panes are observers. A sequence gap in a delta
frame invalidates the local terminal state and requires a fresh bridge. A full frame is
an ANSI redraw and is applied to the existing Ghostty surface; it does not destroy or
recreate renderer state.

Terminal input follows the reverse path: GPUI key or committed-text events enter
Ghostty's native input encoder, and its exact output bytes are base64-encoded into
Herdr's public `terminal.input` command. This preserves application-cursor mode,
bracketed paste, modifier protocols, and other terminal modes.
