<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0099 — Just-in-time descriptor loading for `Any` (and future `MessageSet`)

**Status:** implemented
**Implemented in:** 2026-06-16
**App:** prototext, prototext-core

---

## Background

`prototext decode` uses a `LazyPool` when a `protodb` sidecar (`index.rkyv`)
is present alongside the descriptor file.  The lazy pool starts empty and loads
`FileDescriptorProto` entries on demand: when the root message type is
determined (either from `--type` or by auto-inference), its FDP and transitive
dependencies are loaded into the pool.

The pool is then snapshotted into a flat `HashMap<String, Arc<MessageDescriptor>>`
(built in `decode_and_render` by `build_descriptor_map`) that the renderer uses
for all schema lookups during rendering.

`google.protobuf.Any` expansion (spec 0089) resolves the FQDN extracted from
`type_url` against this snapshot map.  If the referenced type was not
transitively reachable from the root message type, it is absent from the pool
and the map — so expansion silently falls back to raw two-field rendering.

In practice, `Any.value` routinely wraps types that are entirely unrelated to
the root message (e.g. `proxy.Sample` contains `Any` fields whose
`type_url` references `Request` and `proxy.InfoRequest`).
These types live in the same descriptor set but are not loaded because the root
type does not depend on them.

The same problem will affect `MessageSet` (spec 0089 §9) once implemented.

---

## Goals

1. When `render_any_expansion` cannot find a FQDN in `all_schemas`, attempt to
   load it on demand via a thread-local loader callback, then retry the lookup.
2. Keep the renderer (`prototext-core`) decoupled from `LazyPool` and `run.rs`:
   the callback is an opaque `FnMut(&str) -> Option<Arc<MessageDescriptor>>`
   stored in a thread-local, set by the caller before rendering and cleared
   after.
3. Work for both the eager pool path (no `index.rkyv`) and the lazy pool path.
   On the eager path the callback still tries `pool.get_message_by_name`; if
   the type is absent from the file the fallback renders raw bytes silently (no
   change from today).
4. Generalise naturally to `MessageSet` with zero additional infrastructure.

---

## Non-goals

- Pre-scanning binaries to discover `type_url` values before rendering.
- Multi-threaded rendering (the loader uses `RefCell`, not `Mutex`).
- Emitting warnings when a `type_url` cannot be resolved.

---

## Specification

### §1 — Thread-local loader in `prototext-core`

Add to `render_text/mod.rs`:

```rust
thread_local! {
    pub(super) static ANY_LOADER:
        RefCell<Option<Box<dyn FnMut(&str) -> Option<Arc<MessageDescriptor>>>>>
        = const { RefCell::new(None) };
}
```

Two public functions install and clear the loader:

```rust
pub fn set_any_loader(loader: Box<dyn FnMut(&str) -> Option<Arc<MessageDescriptor>>>) {
    ANY_LOADER.with(|l| *l.borrow_mut() = Some(loader));
}

pub fn clear_any_loader() {
    ANY_LOADER.with(|l| *l.borrow_mut() = None);
}
```

Both are re-exported from `prototext_core::lib.rs`.  No changes to
`decode_and_render`, `render_as_text`, or `RenderOpts` — the signatures are
unchanged and the public API is unaffected.

### §2 — JIT lookup in `render_any_expansion`

When `all_schemas.and_then(|m| m.get(fqdn))` returns `None`, attempt JIT
loading before giving up:

```rust
let resolved_desc: Option<Arc<MessageDescriptor>> =
    all_schemas.and_then(|m| m.get(fqdn)).cloned().or_else(|| {
        ANY_LOADER.with(|l| {
            l.borrow_mut()
                .as_mut()
                .and_then(|f| f(fqdn))
        })
    });
let resolved_desc = match resolved_desc {
    Some(d) => d,
    None => return false,
};
```

No changes to `all_schemas` itself — it remains a `&HashMap` snapshot.  The
loader is the fallback; the map is the fast path.

Note: after a successful JIT load, the newly loaded type is available in the
underlying pool (which is `Arc`-shared), so if the same FQDN appears again in
a sibling field the `all_schemas` map will still miss it on the second
occurrence — but the loader will succeed again instantly (the FDP is already in
the pool, `get_message_by_name` is O(1)).  This is acceptable; caching loader
results into `all_schemas` would require interior mutability on the map and is
not worth the complexity.

### §3 — Loader construction in `run.rs`

`install_any_loader` takes a raw pointer to `DescriptorContext` — valid because
the render call is synchronous and `desc_ctx` always outlives the closure.
`clear_any_loader()` is called before the stack frame that holds `desc_ctx`
returns, so the pointer is never dangling.

```rust
fn install_any_loader(desc_ctx: &mut DescriptorContext) {
    let ctx_ptr: *mut DescriptorContext = desc_ctx as *mut DescriptorContext;
    set_any_loader(Box::new(move |fqdn: &str| {
        let ctx = unsafe { &mut *ctx_ptr };
        if let Some(lazy) = ctx.lazy.as_mut() {
            let _ = lazy.get_message(fqdn);
        }
        ctx.pool()
            .get_message_by_name(fqdn)
            .map(std::sync::Arc::new)
    }));
}
```

On the lazy-pool path, `lazy.get_message(fqdn)` loads the FDP into `lazy.pool`
on demand.  `ctx.pool()` then returns a reference to the same `lazy.pool`, so
`get_message_by_name` finds the newly loaded type.  On the eager-pool path,
`ctx.lazy` is `None` and `ctx.pool()` queries the pre-populated pool directly.

### §4 — Call sites in `run.rs`

`install_any_loader` / `clear_any_loader` bracket every schema-aware `process`
call that has access to a `DescriptorContext`.  This covers:

- Explicit `--type` path (stdin and single-file)
- Auto-infer path (stdin, single-file)
- `run_batch_infer` second pass

The `--raw` path and `encode` path are unchanged (no schema context).

---

## Testing

- TC-1: Decode a binary whose root type is `proxy.Sample` with a
  lazy pool containing `Request` and `proxy.InfoRequest`.
  Assert that `Any` fields expand to named message blocks rather than raw bytes.
- TC-2: Same binary, eager pool.  Assert same expansion (types present).
- TC-3: Same binary, eager pool with only the root type's transitive deps.
  Assert graceful raw-bytes fallback (types absent from pool).
- TC-4: `--no-expand-any` suppresses all expansion regardless of loader.

---

## Notes

- The thread-local pattern is consistent with `EXPAND_ANY`, `ANNOTATIONS`, etc.
  already in `render_text/mod.rs`.
- The `unsafe` raw pointer in `install_any_loader` is localised to one helper
  in `run.rs` and is trivially auditable: `desc_ctx` always outlives the
  synchronous render call, and `clear_any_loader()` is always called before
  the function holding `desc_ctx` returns.
- `MessageSet` expansion (future) will call the same `ANY_LOADER` thread-local
  with the FQDN extracted from `MessageSet` extension type URLs — zero
  additional infrastructure needed.
