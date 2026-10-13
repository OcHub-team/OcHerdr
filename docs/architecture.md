# Architecture

## Ownership boundary

Herdr owns sessions, PTYs, processes, workspaces, tabs, panes, layouts,
persistence, and agent detection. OcHerdr projects that state into a native
window and sends explicit API operations back to Herdr.

OcHerdr requires Herdr 0.9.0 or newer. The endpoint contract is checked against
[Herdr 0.9.4](https://github.com/herdrdev/herdr/tree/7e26bad94044eba205033111a0ce8033ede7e24f).
It negotiates endpoint generation 1 and four named codecs independently of
Herdr's private binary protocol number. The current shared envelope is protocol
22; the application does not use the old per-pane protocol-20 attach path.

The macOS renderer uses the Ghostty development tree toward 1.4, pinned together
with its embedding patch and archive checksum in
[`vendor/ghosttykit/version.env`](../vendor/ghosttykit/version.env). Frames carry
explicit leases and layout contexts. Releases may run on a Metal completion
thread; final surface destruction is dispatched to the application thread.

## Data flow

1. Discover sessions with `herdr session list --json`, locally or through SSH.
2. Connect to the public API socket for `session.snapshot` and operations outside
   the endpoint's advertised methods. Remote API calls use a persistent OpenSSH
   StreamLocal forward in a private temporary directory.
3. Open one endpoint per session: the companion client socket locally, or
   `ssh -T … herdr --session … remote-client-bridge` remotely. Default sessions
   omit the `--session` option.
4. Negotiate `endpoint.hello.v1`, `shell.snapshot.v1`, `shell.surface.v1`,
   `shell.input.semantic.v1`, and `shell.blob.v1`. Reject unsupported generations
   and core codecs. Unknown optional named controls are ignored.
5. Use endpoint snapshot pushes as invalidation cues for an authoritative
   `session.snapshot` pull. Refreshes are single-flight, with one queued retry
   when another push arrives during a fetch.
6. Activate the visible endpoint surface and synchronize client focus and view
   selection. Herdr returns composited full frames and row patches with pane
   rectangles. `SurfaceDecoder` extracts each pane and emits styled ANSI updates.
7. Feed those updates to Ghostty's manual-mirror surfaces on macOS, or the
   portable VT renderer on Linux and Windows. Ghostty handles VT state, shaping,
   glyph rendering, and leased BGRA IOSurfaces.
8. Wrap the IOSurface as a CoreVideo pixel buffer without copying. Keep the
   frame lease until GPUI completes its Metal commands; see
   [color rendering](color-rendering.md) for the compositor color-space contract.

`surface_reuse`, `surface_delta`, and `surface_scroll` are optional encodings.
OcHerdr does not request them, so Herdr keeps sending full V1 surfaces and V1
row patches. A patch must match the retained surface revision before it applies.

## Host residency

Switching hosts parks the previous connection, snapshot, selection, and pane
renderers in a profile-keyed runtime. It disables that endpoint's surface
interest while retaining its session connection. Switching back reactivates the
surface and refreshes authoritative state.

Workers carry a `(profile_id, session_name)` owner key. Equal pane IDs on different
hosts cannot route frames or events into one another. Snapshot invalidations for
parked hosts also refresh their rows when the aggregate sidebar is visible.

An explicit disconnect, profile reconfiguration/removal, session replacement,
or shutdown releases a parked runtime. Ordinary tab and host switches keep the
client identity and client-scoped resources such as staged clipboard images.
Hidden pane renderers remain in a bounded cache.

## Input and clipboard

GPUI keys, releases, IME commits, paste, and mouse actions become semantic
`ClientPaneInputEvent` values addressed to a pane. Herdr encodes them for that
pane's actual terminal modes. App shortcuts and focused overlay inputs consume
events before terminal forwarding. Ghostty's locally encoded output is drained
without forwarding it a second time.

The server's pane-surface metadata is the authority for mouse capture. Shift
selection remains local; other captured pointer events include the pane's cell
and pixel geometry. Keyboard focus follows the selected pane, while wheel input
targets the pane beneath the pointer.

An image paste uses one `ClipboardImage` message on the existing session
endpoint. OcHerdr validates the local image and 16 MiB limit; Herdr stages it on
the host that owns the pane. No second SSH command or remote shell utility is
needed. Text and file-path paste retain the normal semantic-input path.

## SSH policy

OcHerdr delegates keys, agents, proxies, known hosts, and authentication to the
system OpenSSH client. Background commands use BatchMode and bounded connection
timeouts. First-use authentication and installation use the system Terminal.

Remote command arguments are POSIX-quoted. Topology operations travel as JSON,
not shell fragments. Endpoint requests are serialized to match Herdr's
single-flight request lane; failures remain scoped to their request.

## Protocol compatibility and notifications

`ocherdr-herdr/src/endpoint_v1.rs` preserves the enum and field order of Herdr's
shared bincode envelope. New endpoint behavior is named and negotiated rather
than appended arbitrarily to that envelope. Frozen upstream JSON fixtures and
a golden binary envelope cover compatibility. A live test can also exercise
handshake, input, split requests, full surfaces, and patches against an isolated
server selected by `OCHERDR_TEST_CLIENT_SOCKET`.

Semantic agent completion and attention notifications arrive on the endpoint.
OcHerdr posts them through the local operating system's notification service,
including for remote sessions, while suppressing the selected pane's alert when
its window is frontmost. Clicking an alert selects its host and pane.

Herdr's full TUI settings are not currently embedded through this endpoint path.
The toolbar explains that boundary; OcHerdr's own appearance settings remain
available in the app.
