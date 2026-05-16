<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Lazy FileDescriptorSet Loading — Design Document

## Problem

The program loads a large `FileDescriptorSet` (FDS) at startup via
`prost`/`prost-reflect`. With ~8000 `FileDescriptorProto` (FDP) entries,
this takes ~3 seconds, dominated by the cost of building
`DescriptorPool`'s internal symbol tables and cross-references —
not I/O.

At runtime, only a small fraction of the 8000 FDPs are ever needed
(the program uses the FDS to determine the correct schema for a given
protobuf message type). Paying the full 3-second cost upfront is
therefore wasteful.

Multiple FDS files exist (different schema versions); the one to use
is specified on the CLI at startup.

---

## Goals

- Startup time: near-zero (two `mmap` calls + a pointer cast)
- Per-type cost: one `FileDescriptorProto` decode + DFS over its deps
- No full deserialization of the FDS at startup
- Support multiple FDS files selected at runtime via CLI argument
- No changes to the `.pb` file format itself

---

## Design Overview

Split the work into two phases:

**Build time** — a tool (or `build.rs`) runs once per FDS file and
produces a sidecar index file (`schema.pb.idx`) alongside the original
`schema.pb`. This index encodes all the information needed to load any
type lazily, in a zero-copy binary format.

**Runtime** — `LazyPool` mmaps both files. The index is accessed
directly as typed memory (no deserialization). Individual FDPs are
decoded from the mmapped `.pb` only when a specific type is first
requested.

---

## Files

```
/path/to/schema_v1.pb       # original FDS, unchanged
/path/to/schema_v1.pb.idx   # sidecar: zero-copy rkyv-encoded index
```

The CLI argument specifies the `.pb` path; the loader appends `.idx`
to find the sidecar.

---

## Index Format

The index is serialized using [`rkyv`](https://github.com/rkyv/rkyv),
which produces a layout that can be accessed directly from mmapped
memory with no deserialization step (a single pointer cast).

```rust
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct Index {
    /// Fully-qualified type name → name of the file that defines it.
    /// e.g. "mypackage.MyMessage" → "mypackage/my_message.proto"
    type_to_file: HashMap<String, String>,

    /// File name → byte range (start, end) of its FileDescriptorProto
    /// within the raw .pb file.
    file_to_span: HashMap<String, (usize, usize)>,

    /// File name → list of file names it imports (the dep graph).
    dep_graph: HashMap<String, Vec<String>>,
}
```

### Notes on `type_to_file`

- Covers top-level messages, nested messages (recursively), and enums.
- Keys are fully-qualified names without a leading dot,
  e.g. `"pkg.Outer.Inner"`.
- Services can be added if needed, using the same pattern.

### Notes on `file_to_span`

The byte range `(start, end)` is the byte offset of the raw protobuf
encoding of a single `FileDescriptorProto` within the `.pb` file. This
is the slice passed to `FileDescriptorProto::decode()` at runtime.

Extracting these spans requires a single streaming pass over the FDS
wire format during index building (reading field tags and
length-delimited boundaries, without fully decoding each FDP).
Alternatively, decode the FDS fully once at build time to extract
spans — acceptable since build time cost doesn't matter.

---

## Runtime: `LazyPool`

```rust
struct LazyPool {
    /// Mmapped raw .pb bytes. Never decoded upfront.
    raw: memmap2::Mmap,

    /// Mmapped .idx bytes. Accessed via rkyv pointer cast.
    idx_mmap: memmap2::Mmap,

    /// Typed zero-copy view into idx_mmap.
    index: &'static ArchivedIndex,

    /// The prost-reflect pool. Starts empty (but DescriptorPool::default()
    /// pre-populates well-known types: google/protobuf/*.proto).
    pool: DescriptorPool,

    /// Files already added to the pool. Guards against double-adds.
    loaded: HashSet<String>,
}
```

### Startup

```rust
impl LazyPool {
    pub fn open(pb_path: &Path) -> Result<Self> {
        let idx_path = pb_path.with_extension("pb.idx");

        let raw = unsafe { memmap2::Mmap::map(
            &std::fs::File::open(pb_path)?
        )? };
        let idx_mmap = unsafe { memmap2::Mmap::map(
            &std::fs::File::open(idx_path)?
        )? };

        // Pointer cast — zero cost, no deserialization.
        let index = unsafe {
            rkyv::access_unchecked::<ArchivedIndex>(&idx_mmap)
        };

        Ok(Self {
            raw,
            idx_mmap,
            index,
            pool: DescriptorPool::default(),
            loaded: HashSet::new(),
        })
    }
}
```

Cost: two `mmap` syscalls + one pointer cast. No I/O, no parsing.

### On-demand type resolution

```rust
impl LazyPool {
    pub fn get_message(
        &mut self,
        full_name: &str,
    ) -> Option<MessageDescriptor> {
        let file = self.index.type_to_file.get(full_name)?;
        self.ensure_loaded(file.as_str()).ok()?;
        self.pool.get_message_by_name(full_name)
    }

    fn ensure_loaded(&mut self, file: &str) -> Result<()> {
        if self.loaded.contains(file) {
            return Ok(());
        }

        // Recurse into dependencies first (topological order).
        // DescriptorPool::default() already contains well-known types,
        // so google/protobuf/*.proto deps are pre-satisfied.
        let deps: Vec<String> = self.index
            .dep_graph
            .get(file)
            .map(|d| d.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();

        for dep in deps {
            self.ensure_loaded(&dep)?;
        }

        // Decode only this one FDP from the mmapped .pb bytes.
        let (start, end) = self.index.file_to_span[file];
        let fdp = FileDescriptorProto::decode(
            &self.raw[start..end]
        )?;

        // All deps are now in the pool — this will succeed.
        self.pool.add_file_descriptor_proto(fdp)?;
        self.loaded.insert(file.to_owned());
        Ok(())
    }
}
```

### Dependency resolution (DFS)

`ensure_loaded` performs a depth-first traversal of the dependency
graph. This guarantees that when `add_file_descriptor_proto(fdp)` is
called, all files listed in `fdp.dependency` are already present in
the pool — which is a hard requirement of `prost-reflect`.

Example for `"pkg.Foo"` defined in `"pkg/foo.proto"`, which depends
on `"common/base.proto"`:

```
ensure_loaded("pkg/foo.proto")
  → ensure_loaded("common/base.proto")   # not loaded yet
      → ensure_loaded("google/.../any.proto")  # already in pool → skip
      → decode raw[span of common/base.proto]
      → pool.add_file_descriptor_proto(...)
  → decode raw[span of pkg/foo.proto]
  → pool.add_file_descriptor_proto(...)
pool.get_message_by_name("pkg.Foo")   # succeeds
```

---

## Build-time Index Tool

A standalone binary (or `build.rs` step) that takes a `.pb` file and
writes the `.pb.idx` sidecar.

```
fds-index-builder schema_v1.pb
# writes schema_v1.pb.idx
```

Steps:

1. `FileDescriptorSet::decode(raw)` — full decode, acceptable at build
   time.
2. For each `FileDescriptorProto` at index `i`:
   - Record `file.name` → position `i`
   - Record `file.dependency` → dep graph
   - Record byte span of the FDP within the original `.pb` file
   - Walk `file.message_type` recursively to collect all FQDNs
     (including nested types) → `type_to_file`
3. Construct `Index`, serialize with `rkyv::to_bytes()`, write to
   `<input>.idx`.

### Extracting byte spans

`FileDescriptorSet` is a protobuf message with one repeated field
(`file`, field number 1). In the wire format, each FDP is encoded as:

```
tag (varint) | length (varint) | <length bytes of FDP>
```

The span for FDP `i` is `(start_of_FDP_bytes, start + length)`.
These offsets can be recorded during a single linear scan of the raw
bytes using `prost::encoding` primitives, or derived from the full
decode by re-encoding each FDP and searching — the former is cleaner.

---

## Memory layout at runtime

```
raw (Mmap)        → virtual mapping of schema.pb (~50–100 MB)
                    Pages loaded on demand by OS as FDP spans are accessed.
                    If only 5% of FDPs are used, only ~5% of the file
                    is ever read from disk.

idx_mmap (Mmap)   → virtual mapping of schema.pb.idx (small: strings +
                    integer pairs). Fully paged in after first few
                    lookups. Effectively in RAM.

pool              → DescriptorPool, grows incrementally.
                    Contains only FDPs touched in this run.
```

---

## Thread safety

`LazyPool` takes `&mut self` during `ensure_loaded`, so it is not
`Sync` as-is. Options:

- Wrap in `Mutex<LazyPool>` for shared access across threads.
- Or shard: one `LazyPool` per thread (each mmap is independent and
  cheap to open; the OS shares the underlying pages).

`DescriptorPool` itself is `Clone + Send + Sync` once built, so
descriptors can be extracted and shared freely after resolution.

---

## Dependencies

```toml
[dependencies]
prost            = "..."
prost-reflect    = "..."
prost-types      = "..."
memmap2          = "..."
rkyv             = { version = "...", features = ["std"] }
anyhow           = "..."

[build-dependencies]
# (same, for the index builder tool)
```

---

## Summary of costs

| Phase | Work done | Estimated cost |
|---|---|---|
| Build time (once) | Full FDS decode + index write | ~3 s (offline) |
| Startup | 2× mmap + pointer cast | < 1 ms |
| First use of type T | DFS decode of T's FDP chain | 1–10 ms |
| Subsequent use of T | Pool lookup | < 1 µs |
