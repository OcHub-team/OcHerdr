# Known issues

## One OcHerdr instance per session

Every pane of the visible tab holds a `terminal session control --takeover`
stream, so its PTY is sized from the grid OcHerdr renders for it (Herdr only
resizes the PTY for attached clients; an observe stream just records the
viewport). Takeover is per terminal and unconditional: a second OcHerdr, or
any other `--takeover` client, on the same session takes those streams away
from the first, whose panes then stop updating until it re-attaches. Before
this change the same was true of the selected pane alone; now it covers the
whole visible tab. Also see the "multiple instances fight" note in the
project memory: two instances make Herdr's PTY sizes flip between the two
layouts, which looks like a rendering regression and is not.

## Debug builds spin in `HashMap` unless the whole dev profile is optimised

`[profile.dev] opt-level = 1` in the workspace manifest is load-bearing. Without
it a debug build never reaches its first window.

**Symptom.** The process starts, uses 100% of one core, and never opens a window.
A sample shows it inside `gpui::ActionRegistry::load_actions` →
`HashMap::contains_key` → `hashbrown::RawTable::find_inner`, forever.

**What it actually is.** The `HashMap`'s table gets its first two fields
clobbered. Read out of a hung process with lldb, after five successful inserts:

```
ctrl        = 0xa3ec18310   -> the bytes there are ARM64 machine code, not control tags
bucket_mask = 3             -> four buckets
growth_left = 1
items       = 5             -> five entries in four buckets
```

`growth_left` and `items` are correct; `ctrl` and `bucket_mask` are not. The
control bytes therefore contain no `EMPTY` tag, so the probe loop never finds a
free slot. `ProbeSeq::move_next` has a `debug_assert!` that would catch this,
but this `HashMap` comes from std's own precompiled copy of hashbrown, which is
built with assertions off — so nothing fires and the process just spins.

**What triggers it.** Two conditions together: `opt-level = 0` *and* GhosttyKit
linked into the binary. A minimal reproduction, with no gpui involved:

```rust
let mut map = std::collections::HashMap::new();
for name in names {           // 30 distinct &str keys
    map.contains_key(name);   // the sixth call never returns
    map.insert(name, ());
}
```

The same test compiled into `ocherdr-core` (0 GhosttyKit symbols) passes
instantly. Compiled into `ocherdr-app` (137 GhosttyKit symbols) it hangs.

**It is not specific to gpui.** Optimising only dependencies
(`[profile.dev.package."*"]`) moves the failure rather than removing it: gpui's
registry then builds at opt-level 1 and the window opens, but the first
`HashMap` that this workspace's own crates grow spins instead — observed on the
main thread inside `find_or_find_insert_index_inner`, codegen'd into
`ocherdr-herdr` and `ocherdr-core`, with the UI frozen on "discovering Herdr
sessions" at 100% CPU. Any crate that instantiates hashbrown at opt-level 0 is
exposed, so the profile setting has to cover the workspace's own crates too.

**Ruled out, by measurement rather than reasoning.** GhosttyKit exports exactly
one symbol that shadows libc, `_memset`, and it wins the link — in a debug
binary `_memset` is a local definition and libSystem's is never imported. It is
nevertheless correct: called directly through the linked symbol across 17
lengths and 3 fill values, it fills correctly, writes nothing out of bounds, and
returns `dest`. There is no second copy of gpui in the lock file.

**Status.** `opt-level = 1` changes codegen enough that the write lands
somewhere harmless. That is a workaround, not a fix — the underlying write is
still happening somewhere, and release builds may simply be getting lucky. The
next step to identify the writer is a hardware watchpoint on the table's first
16 bytes.
