<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0069 — LazyPool: on-demand FDP loading for prototext

**Status:** implemented
**Implemented in:** 2026-05-16
**App:** prototext, scoring-graph

---

## Purpose

`prototext --descriptor foo.desc` currently calls `decode_pool(&bytes)`,
which feeds the entire `FileDescriptorSet` into `prost-reflect` upfront.
For a large corpus descriptor (e.g. `stress.desc`, ~8 000 FDPs) this
takes ~3 seconds dominated by symbol-table construction — before any
user work is done.

Spec 0068 produced a sidecar `<stem>/index.rkyv` alongside each
descriptor built with `reproto --build-schema-db`.  That index encodes
`type_to_file`, `file_to_span`, and `dep_graph` for every FDP.  This
spec wires the index into `prototext` as a `LazyPool` that mmaps both
files and decodes FDPs on demand, reducing startup to two `mmap` calls +
a pointer cast.

The design follows `docs/lazy_fds_design.md`.

---

## Observations

### O1 — `DescriptorContext::load` is the single entry point

`DescriptorContext::load` in `prototext/src/run.rs` is the only place
that resolves `--descriptor`.  It already checks for
`<stem>/hopcroft.rkyv` alongside the descriptor.  Adding a check for
`<stem>/index.rkyv` in the same function is the natural integration
point.

### O2 — `FdsIndex` already exists in `scoring-graph`

`scoring-graph/src/fds_index.rs` defines `FdsIndex` with
`rkyv::Archive`.  `scoring-graph` is already a dependency of `prototext`
via `Cargo.toml`.  `LazyPool` reads the archived form via
`rkyv::access_unchecked`.

### O3 — `DescriptorPool::new()` always; `::default()` never

Every FDS in this codebase — whether a user-supplied `--descriptor` or
the stress corpus — is self-contained (built with `--include_imports`).
`LazyPool` always starts with `DescriptorPool::new()` (empty pool) and
loads WKT types from the FDS spans like any other type.
`DescriptorPool::default()` pre-populates `google.protobuf.*` from
`prost-reflect`'s built-in descriptors, which would silently shadow the
FDS-embedded copies and break reproducibility.  It is never appropriate
inside `LazyPool`.

### O4 — `LazyPool` replaces `decode_pool` only when index is present

When `<stem>/index.rkyv` is absent (bare `.pb`, no `--build-schema-db`
run), `DescriptorContext::load` falls back to the current eager
`decode_pool` path unchanged.  No regression for existing users.

### O5 — `DescriptorPool` is `Arc`-backed; `.clone()` is a ref-count bump

`prost-reflect::DescriptorPool` wraps `Arc<DescriptorPoolInner>`.
`.clone()` produces a second handle to the same pool — mutations through
either handle are visible through the other.  There is therefore no
reason to hold both `ctx.pool` and `ctx.lazy.pool` separately: when
`lazy` is `Some`, all pool access goes through `lazy.pool` directly.
`DescriptorContext.pool` is only populated when `lazy` is `None` (eager
path).

### O6 — The `None` arm (no `--descriptor`) stays eager

When no `--descriptor` is given, `DescriptorContext::load(None)` uses
`EMBEDDED_DESCRIPTOR` (the compiled `descriptor.proto`, covering only
`google.protobuf.*`) plus, when the `wkt-db` feature is enabled, the
pre-built WKT scoring graph `WKT_GRAPH`.

`WKT_INDEX` (spec 0068) encodes spans into `schemas.desc` — the full
WKT FDS, a different and larger file.  Feeding `EMBEDDED_DESCRIPTOR`
as the raw bytes to a `LazyPool` constructed from `WKT_INDEX` would be
incorrect: the spans would not correspond to the bytes.  Embedding a
second static `WKT_FDS` blob is possible but unnecessary — the WKT FDS
is small (~few MB) and eagerly loading it takes well under 100 ms, far
from the 3 s problem.  The `None` arm therefore keeps the current eager
`decode_pool` path unchanged and is out of scope for this spec.

### O7 — `encode` never touches the descriptor pool

`run_encode` passes `None` for schema throughout — it is a pure
wire-format textproto → binary conversion with no type resolution.
It never accesses the pool.  No `load_all` call is needed for `encode`.

### O8 — Defensive cycle guard in `ensure_loaded`

Protobuf import graphs are acyclic by construction, but a malformed or
hand-crafted FDS could contain cycles.  Without a guard, a cycle would
cause infinite recursion in `ensure_loaded`.  An `in_progress:
HashSet<String>` field tracks files currently being loaded (mid-DFS);
detecting a file already in `in_progress` returns an error immediately.

### O9 — `memmap2` and `rkyv` already available

Both crates are already dependencies of `scoring-graph`, which `prototext`
depends on.  No new crate additions needed.

### O10 — Thread safety is out of scope

`LazyPool` takes `&mut self` during resolution and is not `Sync`.
`prototext` is single-threaded.  No locking is needed.

---

## Goals

1. When `<stem>/index.rkyv` exists alongside `--descriptor <stem>.desc`,
   `DescriptorContext::load` constructs a `LazyPool` instead of calling
   `decode_pool`.  Startup time drops from ~3 s to < 1 ms for large
   descriptors.
2. `DescriptorContext` exposes `lazy: Option<LazyPool>` directly; callers
   use `lazy.pool` when lazy is `Some`, `pool` when `None`.
3. The existing eager path is preserved when `index.rkyv` is absent.
4. The `None` arm (no `--descriptor`) is unchanged.

---

## Non-goals

- Lazy loading for the embedded WKT path (`wkt-db` feature, `None` arm).
- Thread safety / `Sync` impl for `LazyPool`.
- Incremental index updates.
- Compression of `index.rkyv` or the `.pb` bytes.

---

## Specification

### S1 — `LazyPool` struct (in `scoring-graph/src/lazy_pool.rs`)

```rust
pub struct LazyPool {
    /// Mmapped raw .pb bytes.
    raw: memmap2::Mmap,

    /// Mmapped index.rkyv bytes.
    _idx_mmap: memmap2::Mmap,

    /// Zero-copy typed view into _idx_mmap.
    index: &'static ArchivedFdsIndex,

    /// The prost-reflect pool.  Starts empty (DescriptorPool::new()).
    pub pool: DescriptorPool,

    /// Files already fully added to the pool.
    loaded: HashSet<String>,

    /// Files currently being loaded (mid-DFS); used to detect cycles.
    in_progress: HashSet<String>,
}
```

`ArchivedFdsIndex` is the rkyv-archived form of `FdsIndex` from
`scoring-graph/src/fds_index.rs`.

### S2 — Constructor

```rust
impl LazyPool {
    /// Open a lazy pool from a .pb file and its index.rkyv sidecar.
    /// The pool starts empty (DescriptorPool::new()).
    pub fn open(pb_path: &Path, idx_path: &Path)
        -> Result<Self, Box<dyn Error>>;
}
```

`open` validates the PTSGRAPH header on `idx_path` (magic `b"PTSGRAPH"`,
version `3u32 LE`) before the pointer cast, returning an error if wrong.

### S3 — On-demand resolution

```rust
impl LazyPool {
    /// Ensure the FDP defining `fqdn` and all its transitive deps are
    /// in the pool, then return the MessageDescriptor.
    pub fn get_message(&mut self, fqdn: &str)
        -> Result<Option<MessageDescriptor>, Box<dyn Error>>;

    /// Same for enum types.
    pub fn get_enum(&mut self, fqdn: &str)
        -> Result<Option<EnumDescriptor>, Box<dyn Error>>;

    /// Ensure every FDP in the index is loaded into the pool.
    /// Used when a full pool is required (list-schemas, tab-completion).
    pub fn load_all(&mut self) -> Result<(), Box<dyn Error>>;
}
```

`get_message` and `get_enum` call `ensure_loaded(file)`:

```
fn ensure_loaded(&mut self, file: &str) -> Result<()>:
    if loaded.contains(file): return Ok(())
    if in_progress.contains(file): return Err("cycle detected: {file}")
    in_progress.insert(file)
    for dep in index.dep_graph[file]:
        ensure_loaded(dep)?
    (start, end) = index.file_to_span[file]
    fdp = FileDescriptorProto::decode(&raw[start..end])?
    pool.add_file_descriptor_proto(fdp)?
    in_progress.remove(file)
    loaded.insert(file)
    Ok(())
```

`load_all` calls `ensure_loaded` on every file in `file_to_span`.

### S4 — Integration into `DescriptorContext`

`DescriptorContext` gains a `lazy` field and `pool` becomes
`Option<DescriptorPool>`:

```rust
pub struct DescriptorContext {
    /// Populated only on the eager path (no index.rkyv).
    pub pool: Option<prost_reflect::DescriptorPool>,
    pub graph: Option<LoadedGraph>,
    pub lazy: Option<LazyPool>,
}
```

Callers access the pool via a helper:

```rust
impl DescriptorContext {
    pub fn pool(&self) -> &DescriptorPool {
        if let Some(lazy) = &self.lazy { &lazy.pool }
        else { self.pool.as_ref().unwrap() }
    }
    pub fn pool_mut(&mut self) -> &mut DescriptorPool {
        if let Some(lazy) = &mut self.lazy { &mut lazy.pool }
        else { self.pool.as_mut().unwrap() }
    }
}
```

`DescriptorContext::load` logic when `path` is `Some(p)`:

```
stem       = p.with_extension("")
rkyv_path  = stem / "hopcroft.rkyv"
index_path = stem / "index.rkyv"
graph      = load_graph(rkyv_path) if exists
if index_path.exists():
    lazy = LazyPool::open(p, index_path)?
    return DescriptorContext { pool: None, graph, lazy: Some(lazy) }
else:
    bytes = read_descriptor_file(p)?
    pool  = decode_pool(&bytes)?
    return DescriptorContext { pool: Some(pool), graph, lazy: None }
```

The `None` arm (no `--descriptor`) is unchanged from the current code.

### S5 — Call-site changes in `run.rs`

All `desc_ctx.pool` accesses become `desc_ctx.pool()`.  Subcommands that
need the full type namespace call `lazy.load_all()` first:

| Subcommand | Strategy |
|---|---|
| `decode --type T` | `lazy.get_message(T)` then use pool |
| `decode` (auto-infer) | score on wire bytes; `get_message(winner)` after |
| `encode` | no pool access needed — unchanged |
| `instantiate-schema` | `get_message(T)` per requested type |
| `score --type T` | `get_message(T)` |
| `list-schemas` | `load_all` |
| tab-completion of type names | `load_all` |

### S6 — PTSGRAPH header validation

```
bytes[0..8]  == b"PTSGRAPH"    # magic
bytes[8..12] == 3u32 LE        # version (FdsIndex = 3, not Hopcroft = 2)
```

Returns `Err` with a descriptive message on mismatch.

### S7 — Module placement

`LazyPool` is defined in `prototext/src/lazy_pool.rs` and declared as
`pub mod lazy_pool` in `prototext/src/lib.rs`.  (`scoring-graph` does
not depend on `prost-reflect`, so `LazyPool` cannot live there.)

### S8 — Nix: no changes required

`wktRkyv` already produces `wkt_index.rkyv` (spec 0068) and
`prototext/build.rs` already copies it to `$OUT_DIR`.  `WKT_INDEX` is
already embedded in `lib.rs`.  No Nix changes needed for this spec.
`WKT_INDEX` remains unused until a future spec extends lazy loading to
the embedded WKT path.

---

## Files changed

- `prototext/src/lazy_pool.rs` — new: `LazyPool` struct, constructor, resolution
- `prototext/src/lib.rs` — add `pub mod lazy_pool`
- `prototext/src/run.rs` — integrate `LazyPool` into `DescriptorContext`; update call sites per S5
- `docs/specs/0069-lazy-pool.md` — this file

---

## Test plan

- Unit test `LazyPool::open`: open `stress.desc` + `stress/index.rkyv`,
  call `get_message("google.type.LatLng")`, assert the descriptor is
  returned and only the required FDPs were loaded (`loaded.len()` is
  small, not 8 000).
- Unit test `load_all`: assert that after `load_all`, `pool` contains
  every type present in the index.
- Unit test header validation: corrupt the version byte, assert `open`
  returns `Err`.
- Unit test cycle detection: construct a minimal fake index with a
  two-file cycle in `dep_graph`; assert `ensure_loaded` returns `Err`.
- Integration: `prototext --descriptor stress.desc decode PostalAddress.pb`
  completes well under 1 s (vs. ~3 s eager).
- Regression: all existing `prototext` tests pass unchanged (eager path
  exercised when `index.rkyv` is absent).
