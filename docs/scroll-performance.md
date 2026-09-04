# Terminal scrolling and repainting

OcHerdr 0.2.13 converts GPUI wheel events using the measured terminal body's
**logical** height and row count. Terminal backing buffers use physical pixels;
mixing the two previously halved trackpad scrolling sensitivity on 2× Retina
displays. Fractional line deltas are accumulated, and ordinary line-wheel events
keep their existing behavior.

The terminal command queue merges adjacent, same-direction wheel events before
writing to the socket. The first event does not wait; steady scrolling is grouped
in windows of at most 8 ms. This is a fixed deadline, not a debounce that can keep
postponing input. Direction reversals, keys, resize and release are ordering
barriers and flush queued scrolls. Counts exceeding the protocol's `u16` limit
are split rather than wrapped. Each session owns its queue, so delayed input
cannot leak to a replacement session.

Terminal frames now use a separate GPUI paint invalidation signal. The root still
traverses terminal layout, paints current frames and updates terminal accessibility
text. Cached sidebar, tab-bar and file-panel views reuse their paint state for
terminal-only updates. Ordinary model notifications, interaction, animations,
changed bounds and global window refreshes still invalidate the relevant views.

## Local regression measurements

`controller::gpui_tests::rendering` drives the production GPUI window and compares
120 paints with the old root-notification path against 120 terminal-only paints:
each of the three chrome regions rebuilds 120 times with the former and zero times
with the latter, after initial layout. A following model notification rebuilds
all three. The existing file-panel tests exercise docking, overlay layout,
resizing, hidden-file toggling and input through these cached views.

The queue tests verify that a first one-line event followed by a burst of 120
same-direction events becomes two commands totaling 121 lines, with no loss or
reordering across barriers. Controller tests cover logical wheel units at display
scales 1×, 1.5×, 2× and 3×, including the live pane handler's Retina remainder.

These are render-work and input-queue measurements, **not an FPS benchmark** of a
live Herdr workload. Herdr remains the owner of scrolling and TUI mouse behavior;
the protocol still scrolls in whole lines. This change does not add client-local
pixel scrolling, alter the server's frame cadence or promise a fixed frame rate.
