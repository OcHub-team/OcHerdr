# Known issues

## Debug builds hang before the first window unless dependencies are optimised

`[profile.dev.package."*"] opt-level = 1` in the workspace manifest is load-bearing.
Without it a debug build never reaches its first window.

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

**Ruled out, by measurement rather than reasoning.** GhosttyKit exports exactly
one symbol that shadows libc, `_memset`, and it wins the link — in a debug
binary `_memset` is a local definition and libSystem's is never imported. It is
nevertheless correct: called directly through the linked symbol across 17
lengths and 3 fill values, it fills correctly, writes nothing out of bounds, and
returns `dest`. There is no second copy of gpui in the lock file.

**Status.** `opt-level = 1` moves the layout enough that the write lands
somewhere harmless. That is a workaround, not a fix — the underlying write is
still happening somewhere, and release builds may simply be getting lucky. The
next step to identify the writer is a hardware watchpoint on the table's first
16 bytes.
