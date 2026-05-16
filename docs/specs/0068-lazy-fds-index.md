<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0068 — Lazy FDS index builder (`reproto --build-schema-db`)

**Status:** implemented
**Implemented in:** 2026-05-15
**App:** reproto, prototext, score-graph

---

## Purpose

Loading a large `FileDescriptorSet` (FDS) at startup currently takes ~3
seconds because `prost-reflect` must build full symbol tables and
cross-references for every FDP in the pool.  In practice, `prototext`
only needs a tiny fraction of those FDPs for any given invocation.

This spec extends `reproto --build-schema-db` to also produce a sidecar
`<stem>/index.rkyv` file alongside the existing `<stem>/hopcroft.rkyv`.  At runtime,
`prototext` loads the index in near-zero time (two `mmap` calls + a
pointer cast) and decodes FDPs on demand, one per type lookup.

The design follows `docs/lazy_fds_design.md`.  This spec covers the
**reproto side only** (index building).  The `prototext` runtime
(`LazyPool`) is a separate spec.

---

## Observations

### O1 — reproto already has the data

`reproto` performs a full FDS decode during normal operation.  It walks
message types recursively, resolves dependency graphs, and has access to
the raw `.pb` bytes.  All three pieces needed for the index
(`type_to_file`, `file_to_span`, `dep_graph`) are already computed or
trivially derivable.

### O2 — Piggyback on `--build-schema-db`

`reproto --build-schema-db` already writes a sidecar scoring graph at
`<stem>/hopcroft.rkyv`.  `index.rkyv` is a second output of the same
pass — no new CLI flag needed.  When `--build-schema-db` is given,
reproto writes both `<stem>/hopcroft.rkyv` and `<stem>/index.rkyv`.

### O3 — Sidecar location convention

The design doc proposed `schema.pb.idx`.  The preferred convention in
this codebase is `<stem>/` subdirectory for sidecars (consistent with
`hopcroft.rkyv`).  The index file will therefore be written to
`<stem>/index.rkyv` where `<stem>` is the descriptor path with its
extension stripped (e.g. `foo.desc` → `foo/index.rkyv`).

### O4 — Byte span extraction

Each FDP in a `FileDescriptorSet` is encoded as a length-delimited field
(tag + varint length + raw bytes).  The byte span `(start, end)` for FDP
`i` can be extracted during a single linear scan of the raw `.pb` bytes
using `prost`'s wire-format primitives, without fully decoding each FDP.
Alternatively, it can be derived after a full decode by re-encoding each
FDP — acceptable since this is a build-time step.

### O5 — rkyv alignment

The `FdsIndex` struct will be serialized with `rkyv::to_bytes()` and stored
with the same 24-byte PTSGRAPH-style header used by `hopcroft.rkyv`
(magic, version, root offset).  This lets `prototext` validate the file
before casting and keeps the format consistent across sidecar files.

### O6 — Self-contained FDS

`reproto --build-schema-db` (and `protoc --include_imports`) produces a
self-contained FDS that includes all transitive dependencies, including
WKT files.  The index must faithfully represent this complete FDS: every
file gets an entry in `type_to_file`, `file_to_span`, and `dep_graph`,
WKT files included.

**Note for the runtime spec:** the `LazyPool` must use `DescriptorPool::new()`
(empty pool), not `DescriptorPool::default()` (which pre-populates system
WKT types).  Using `default()` would silently satisfy WKT dependencies from
the host system rather than from the embedded FDS, breaking reproducibility.

### O7 — rkyv is Rust-only: use a PyO3 extension

`rkyv` is a Rust crate; Python cannot serialize rkyv natively.  The
codebase already solves this same problem for `hopcroft.rkyv`: the
`score-graph-pyo3` crate exposes `build_graph()` as a PyO3 extension
(`scoring_graph_lib`) that Python calls with in-memory data and receives
back the serialized rkyv bytes.

The same pattern applies here.  A new `build_fds_index(...)` function is
added to `score-graph-pyo3`, accepting the three maps as Python dicts and
returning `bytes`.  `reproto` calls this function (already a dependency)
instead of writing rkyv itself.  No new crate is needed.

---

## Goals

1. `reproto --build-schema-db` writes both `<stem>/hopcroft.rkyv` and
   `<stem>/index.rkyv` in one pass — no new CLI flag.
2. The index encodes `type_to_file`, `file_to_span`, and `dep_graph` for
   every file in the FDS (including WKT files) in rkyv format with the
   standard PTSGRAPH header.

---

## Non-goals

- The `prototext` runtime `LazyPool` implementation (separate spec).
- Incremental index updates (full rebuild on any FDS change).
- Compression of the index or `.pb` bytes.
- Thread-safety of the index builder itself.

---

## Specification

### S1 — Index data structure (Rust, in `score-graph`)

```rust
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FdsIndex {
    /// Fully-qualified type name (no leading dot) → proto file name.
    /// Covers top-level messages, nested messages (recursively), and enums
    /// for every file in the FDS, including WKT files.
    pub type_to_file: HashMap<String, String>,

    /// Proto file name → (start, end) byte offsets within the raw .pb file.
    /// The slice raw[start..end] is the wire encoding of that
    /// FileDescriptorProto.  Covers every file in the FDS including WKT.
    /// u64 (not usize) for portability: rkyv archives usize as pointer-sized.
    pub file_to_span: HashMap<String, (u64, u64)>,

    /// Proto file name → list of direct import file names (as recorded in
    /// FileDescriptorProto.dependency).  Covers every file in the FDS.
    ///
    /// Invariant: the FDS is self-contained (built with --include_imports),
    /// so every name appearing in any value list is also a key in this map
    /// and has a span in file_to_span.  The runtime can recurse blindly.
    pub dep_graph: HashMap<String, Vec<String>>,
}
```

`FdsIndex` is defined in `score-graph` (which already owns `CompiledGraph`
and the PTSGRAPH header logic) and re-exported from `score-graph-pyo3`.

### S1b — PyO3 function in `score-graph-pyo3`

A new `build_fds_index` function is added to the `scoring_graph_lib`
PyO3 extension (alongside the existing `build_graph`):

```rust
/// Serialize an FdsIndex to rkyv bytes with the PTSGRAPH header.
///
/// Parameters
/// ----------
/// type_to_file : dict[str, str]
///     Fully-qualified type name → proto file name.
/// file_to_span : dict[str, tuple[int, int]]
///     Proto file name → (start, end) byte offsets in the raw .pb.
/// dep_graph : dict[str, list[str]]
///     Proto file name → list of direct import file names.
///
/// Returns
/// -------
/// bytes
///     Serialized index.rkyv content (PTSGRAPH header + rkyv payload).
///
/// Raises
/// ------
/// RuntimeError
///     If serialization fails.
#[pyfunction]
fn build_fds_index<'py>(
    py: Python<'py>,
    type_to_file: HashMap<String, String>,
    file_to_span: HashMap<String, (u64, u64)>,
    dep_graph: HashMap<String, Vec<String>>,
) -> PyResult<Bound<'py, PyBytes>> { ... }
```

Python calls this function with the three dicts computed in
`build_index.py` and writes the returned bytes to `<stem>/index.rkyv`.

### S2 — File format

The `index.rkyv` file uses the same PTSGRAPH binary envelope as
`hopcroft.rkyv`:

```
bytes  0.. 8   magic:       b"PTSGRAPH"
bytes  8..12   version:     u32 LE = 3   (new version for FdsIndex)
bytes 12..16   reserved:    u32 LE = 0
bytes 16..24   root_offset: u64 LE       (byte offset of rkyv root)
bytes 24..     rkyv payload
```

Version 3 distinguishes this file type from the Hopcroft graph (version
2) so that a mismatched load fails with a clear error.

### S3 — Byte span extraction

During index building, iterate over the raw FDS bytes using prost wire
primitives to record the `(start, end)` span of each FDP without
redundant re-encoding:

```python
# Python pseudocode (reproto is Python)
def extract_spans(raw_pb_bytes):
    """Yield (file_index, start, end) for each FDP in the FDS."""
    pos = 0
    i = 0
    while pos < len(raw_pb_bytes):
        tag_byte, n = decode_varint(raw_pb_bytes, pos)
        pos += n
        field_number = tag_byte >> 3
        wire_type    = tag_byte & 0x7
        assert field_number == 1 and wire_type == 2  # repeated FileDescriptorProto
        length, n = decode_varint(raw_pb_bytes, pos)
        pos += n
        start = pos
        end   = pos + length
        yield i, start, end
        pos = end
        i += 1
```

### S4 — `reproto` CLI changes

No new CLI flag.  The existing `--build-schema-db PATH` flag gains an
additional side effect: alongside `<stem>/hopcroft.rkyv` it also writes
`<stem>/index.rkyv`.  The help text for `--build-schema-db` is updated
to document the new output.

### S5 — Index builder implementation (`reproto/src/reproto/build_index.py`)

New module `build_index.py`:

```python
def build_fds_index(raw_pb_bytes: bytes, fds: FileDescriptorSet) -> bytes:
    """
    Build and serialize an FdsIndex from the raw .pb bytes and the decoded FDS.

    Computes type_to_file, file_to_span, and dep_graph for every file in
    the FDS (including WKT files), then calls
    scoring_graph_lib.build_fds_index() to serialize to rkyv with the
    PTSGRAPH header.

    Returns the serialized index.rkyv content as bytes.

    The FDS is assumed to be self-contained (produced with --include_imports).
    """
    ...
```

Called from `reproto()` in `reproto.py` when `--build-schema-db` is set,
after the normal FDS processing pass.  The returned bytes are written
directly to `<stem>/index.rkyv`.

### S6 — Output path

`--build-schema-db=foo.desc` writes `foo.desc` (the FDS) and
`foo/hopcroft.rkyv` (the scoring graph).  The index follows the same
convention:

```
--build-schema-db=foo.desc   →  writes  foo/hopcroft.rkyv  (existing)
                                         foo/index.rkyv      (new)
```

The stem directory is created if absent.  reproto derives the path:

```python
stem = Path(build_schema_db).with_suffix('')   # e.g. foo.desc → foo
index_path = stem / 'index.rkyv'
```

This is consistent with how `prototext --descriptor foo.pb` will locate
the sidecar (it will look for `foo/index.rkyv` alongside `foo.pb`).

### S7 — Nix integration

The `wktRkyv` derivation in `default.nix` requires no change to its
`reproto` invocation — `--build-schema-db` now automatically writes
both sidecars:

```nix
python -m reproto.cli \
  --build-schema-db="$out/schemas.desc" \
  -O "$TMPDIR/reproto-out" \
  -I "$out" \
  wkt.desc
# results: $out/schemas/hopcroft.rkyv  (existing)
#          $out/schemas/index.rkyv      (new)
```

The `wktRkyv` derivation gains a `cp` step to expose the new sidecar:

```bash
cp "$out/schemas/index.rkyv" "$out/wkt_index.rkyv"
```

The `prototext` full build in `nix/rust.nix` gains `WKT_INDEX = "${wktRkyv}/wkt_index.rkyv"`
alongside `WKT_RKYV`, and `build.rs` copies both files into `$OUT_DIR`.

---

## Files changed

- `score-graph/src/fds_index.rs` — new: `FdsIndex` struct + `to_bytes()` + `write()`
- `score-graph/src/lib.rs` — re-export `fds_index` module
- `score-graph-pyo3/src/lib.rs` — new `build_fds_index` PyO3 function
- `reproto/src/reproto/build_index.py` — new module
- `reproto/src/reproto/cli.py` — update `--build-schema-db` help text
- `reproto/src/reproto/reproto.py` — call `build_fds_index` when `--build-schema-db` is set
- `default.nix` — add `cp schemas/index.rkyv wkt_index.rkyv` to `wktRkyv` derivation
- `nix/rust.nix` — pass `WKT_INDEX = "${wktRkyv}/wkt_index.rkyv"` to `prototext` full build
- `prototext/build.rs` — copy `WKT_INDEX` into `$OUT_DIR/index.rkyv`
- `docs/specs/0068-lazy-fds-index.md` — this file

---

## Implementation order

1. Define `FdsIndex` struct in `score-graph` and its `to_bytes()`/`write()`
   functions (S1, S2).
2. Add `build_fds_index` PyO3 function to `score-graph-pyo3` (S1b).
3. Implement `build_index.py` with span extraction calling the PyO3 function
   (S3, S5).
4. Wire into `cli.py` and `reproto.py` (S4, S6).
5. Update Nix build (S7).
6. Update spec status to `implemented`.

---

## Test plan

- Unit test `extract_spans` against a known small FDS (verify offsets
  round-trip: `FileDescriptorProto::decode(raw[start..end])` succeeds).
- Unit test completeness: WKT files (`google/protobuf/*.proto`) appear
  as keys in `type_to_file`, `file_to_span`, and `dep_graph`.
- Integration test: run `reproto --build-schema-db` on a fixture FDS, load
  the resulting `index.rkyv`, verify a known FQDN (including a WKT type)
  resolves to the correct file and span.
- Nix CI: `wktRkyv` derivation produces `$out/schemas/index.rkyv`.
