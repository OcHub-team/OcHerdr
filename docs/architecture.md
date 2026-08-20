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
7. Decode base64 ANSI frames into native libghostty-vt instances. GPUI paints the
   resulting viewport and application chrome.

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
frame invalidates the local terminal state and requires a fresh bridge; full frames reset
the libghostty-vt state before replay.
