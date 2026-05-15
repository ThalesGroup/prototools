<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0067 — Built-in WKT schema database (auto-inference for google.protobuf.*)

**Status:** implemented
**Implemented in:** 2026-05-14
**App:** prototext, prototext-core, score-graph, build system

---

## Purpose

When no `--descriptor` is supplied, `prototext` falls back to the
embedded `descriptor.pb` (covering `google.protobuf.*` types) for
rendering, but auto-inference and `list-schemas` are unavailable because
no Hopcroft scoring graph exists for those types.  This spec adds a
pre-built WKT schema database — a compiled `FileDescriptorSet` plus a
Hopcroft rkyv graph — embedded directly in the binary, so that
`prototext list-schemas` and type auto-inference work out of the box for
all well-known types without requiring a `--descriptor` file.

A secondary goal is to establish the `prototext-bare` / `prototext-full`
split needed to break the circular build dependency that would otherwise
arise (`prototext-full` → `reproto` → `prototext-bare`), and to lay the
groundwork for the future mmap-based large-schema performance improvement.

---

## Observations

### O1 — Circular build dependency

`reproto --build-schema-db` calls `prototext` internally (to decode `.pb`
files during codegen).  If `prototext` in turn requires `reproto` to
generate its embedded WKT database, a circular dependency arises.

The break point: a `prototext-bare` build (current behaviour, no embedded
scoring graph) satisfies `reproto`'s dependency; `prototext-full` (new,
with embedded WKT database) depends on `reproto`.

### O2 — `DescriptorContext::load` is the WKT resolution entry point

`DescriptorContext::load` in `prototext/src/run.rs` is the single
function that resolves `--descriptor`.  When `path` is `None` it already
uses the embedded `descriptor.pb`.  The graph is always `None` in that
case.  This is the correct place to add WKT graph loading.

### O3 — `LoadedGraph` is mmap-backed; embedded bytes need a parallel path

`load_graph()` in `score-graph/src/score/load.rs` mmaps a file and holds
`_mmap: Mmap` to keep the backing memory alive.  The `graph` field is a
`&'static ArchivedCompiledGraph` achieved by `unsafe` lifetime extension
tied to the mmap.

For embedded bytes (`&'static [u8]` from `include_bytes!`) no mmap is
needed.  rkyv's `access()` works identically against any `&[u8]`, so the
graph view can be constructed directly from the static slice.  A new
`LoadedGraph::from_static_bytes()` constructor handles this case; all
downstream consumers of `LoadedGraph` are unaffected.

### O4 — Future mmap path for large `schemas.desc`

A forthcoming spec will add mmap-based loading of large user-supplied
`schemas.desc` files and their `schemas/hopcroft.rkyv` sidecars, for
performance with large corpora.  The WKT path (small, static,
`include_bytes!`) must not block that work.  The design here keeps both
paths unified behind `LoadedGraph`: the embedded case owns a
`EmbeddedBytes` variant; the mmap case owns the existing `Mmap` variant.
Callers receive the same `&ArchivedCompiledGraph` reference in both cases.

### O5 — WKT source list committed to the repo

The set of `.proto` files needed for the WKT database is small and
stable.  It is committed as `prototext/wkt/SOURCES` — a plain text file,
one path per line, relative to the protoc include root (e.g.
`google/protobuf/descriptor.proto`).  This file is the single source of
truth read by both `build.rs` (to locate embedded bytes in CI/release
builds) and the Nix derivation (to drive `protoc` for `wkt.desc`
generation).

### O6 — Cargo feature `wkt-db`

The embedded WKT database is gated behind a Cargo feature `wkt-db`
(enabled by default).  When disabled, the build produces `prototext-bare`
behaviour: WKT descriptor for rendering only, no scoring graph.  This
feature flag is what `reproto`'s build dependency on `prototext` uses
(`--no-default-features`), breaking the circular dependency.

---

## Goals

1. Add `LoadedGraph::from_static_bytes(bytes: &'static [u8])` to
   `score-graph`, constructing the graph view directly from embedded
   bytes without mmap.
2. Update `DescriptorContext::load` so that when `path` is `None` and
   the `wkt-db` feature is enabled, it loads the embedded WKT graph via
   `from_static_bytes` and returns it in `self.graph`.
3. Commit `prototext/wkt/SOURCES` listing the WKT `.proto` files needed.
4. Add a `build.rs` step in `prototext` (gated on `wkt-db`) that reads
   `SOURCES`, locates the `.proto` files from the protoc include path,
   compiles them into `wkt.desc` via `protoc` (or `prost-build`), then
   calls a `reproto`-equivalent API (or invokes `reproto` as a subprocess)
   to produce `wkt.rkyv`, and emits `include_bytes!`-ready paths via
   `cargo:rustc-env`.
5. Update the Nix build:
   - Add `prototext-bare` attribute: builds `prototext` with
     `--no-default-features` (no `wkt-db`); this is what `reproto`
     depends on.
   - The existing `prototext` attribute becomes `prototext-full` in the
     Nix `let` bindings (retaining the public attribute name `prototext`
     for backwards compatibility); it gains a new build step that runs
     `reproto --build-schema-db` over the WKT descriptor to produce
     `wkt.rkyv`, then passes it to the Crane `buildPackage` via an env
     var.
6. Verify that `prototext list-schemas` with no `--descriptor` returns
   the expected set of `google.protobuf.*` type candidates.

---

## Non-goals

- Mmap-based loading of user-supplied `schemas.desc` (future spec).
- Embedding scoring graphs for non-WKT types.
- Any change to the `--descriptor` resolution logic for user-supplied
  descriptors.
- The release bundle derivation (`releaseBundle`) described in D6 of
  spec 0066.

---

## Specification

### S1 — `LoadedGraph` backing variants

Add an internal enum to `score-graph/src/score/load.rs`:

```rust
enum GraphBacking {
    Mmap(Mmap),
    Static,   // bytes are 'static — no owned handle needed
}

pub struct LoadedGraph {
    _backing: GraphBacking,
    pub graph: &'static ArchivedCompiledGraph,
}
```

Add a constructor:

```rust
impl LoadedGraph {
    /// Construct a `LoadedGraph` from a `'static` byte slice (e.g. from
    /// `include_bytes!`).  No mmap is created; the slice is used directly.
    /// Safety: same as `load_graph` — caller guarantees the bytes are a
    /// valid rkyv-serialised `CompiledGraph` with the correct magic/version.
    pub fn from_static_bytes(bytes: &'static [u8])
        -> Result<Self, Box<dyn std::error::Error>>
    {
        // Validate magic and version (same checks as load_graph).
        // Then: access::<ArchivedCompiledGraph, rkyv::rancor::Error>(...)
        // Lifetime is already 'static — no unsafe extension needed.
    }
}
```

`load_graph` is updated to use `GraphBacking::Mmap`; the rest of the
codebase is unchanged.

### S2 — `DescriptorContext::load` WKT graph path

In `prototext/src/run.rs`, update `DescriptorContext::load`:

```rust
pub fn load(path: Option<&Path>) -> Result<Self, String> {
    let (desc_bytes, graph) = match path {
        None => {
            let bytes = EMBEDDED_DESCRIPTOR.to_vec();
            #[cfg(feature = "wkt-db")]
            let graph = Some(
                LoadedGraph::from_static_bytes(WKT_GRAPH)
                    .map_err(|e| format!("wkt graph: {e}"))?,
            );
            #[cfg(not(feature = "wkt-db"))]
            let graph = None;
            (bytes, graph)
        }
        Some(p) => {
            let bytes = read_descriptor_file(p)?;
            let graph = /* existing stem/hopcroft.rkyv logic */ ...;
            (bytes, graph)
        }
    };
    let pool = decode_pool(&desc_bytes).map_err(|e| format!("descriptor: {e}"))?;
    Ok(DescriptorContext { pool, graph })
}
```

`WKT_GRAPH` is a `&'static [u8]` defined in `lib.rs`:

```rust
#[cfg(feature = "wkt-db")]
pub static WKT_GRAPH: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/wkt.rkyv"));
```

### S3 — `prototext/wkt/SOURCES`

A committed plain-text file, one proto path per line:

```
google/protobuf/any.proto
google/protobuf/api.proto
google/protobuf/descriptor.proto
google/protobuf/duration.proto
google/protobuf/empty.proto
google/protobuf/field_mask.proto
google/protobuf/source_context.proto
google/protobuf/struct.proto
google/protobuf/timestamp.proto
google/protobuf/type.proto
google/protobuf/wrappers.proto
```

### S4 — `build.rs` (feature `wkt-db`)

When `wkt-db` is enabled, `build.rs`:

1. Reads `wkt/SOURCES`.
2. Compiles all listed `.proto` files into a merged `FileDescriptorSet`
   using `protoc` (via `prost-build` or a direct `Command`) with the
   protoc well-known include path.  Writes `$OUT_DIR/wkt.desc`.
3. Invokes `reproto` (found via `PATH` or a build-time env var
   `REPROTO_BIN`) with `--build-schema-db=$OUT_DIR/wkt.rkyv` over
   `$OUT_DIR/wkt.desc`.
4. Emits `cargo:rerun-if-changed=wkt/SOURCES` and
   `cargo:rerun-if-changed=build.rs`.

In the Nix build, `REPROTO_BIN` points at the `reprotoBare` store path;
`protoc` is in `nativeBuildInputs`.

### S5 — Nix build changes

```
nix/rust.nix bindings (new):

  prototextBare = crane.buildPackage (protocArgs // {
    pname          = "prototext-bare";
    cargoArtifacts = rustTests;
    cargoExtraArgs = "--no-default-features -p prototext";
    ...
  });

  wktRkyv = pkgs.runCommand "wkt-rkyv" {
    buildInputs = [ pkgs.protobuf reprotoBare ];
  } ''
    set -euo pipefail
    # compile wkt.desc from SOURCES
    ...
    reproto --build-schema-db=$out/wkt.rkyv -I$PWD/wkt.desc .
  '';

  prototext = crane.buildPackage (protocArgs // {
    pname          = "prototext";
    cargoArtifacts = rustTests;
    cargoExtraArgs = "--features wkt-db --no-default-features -p prototext";
    WKT_RKYV       = "${wktRkyv}/wkt.rkyv";
    ...
  });
```

`reproto` (in `nix/python.nix`) continues to depend on `prototextBare`
(not `prototext`), breaking the cycle.

### S7 — Runtime use of the embedded WKT graph in `run()`

`run()` in `prototext/src/run.rs` checks `desc_ctx.graph.is_none()` at
three sites before dispatching graph-dependent subcommands.  Once S2 is
implemented, `desc_ctx.graph` is `Some(...)` whenever no `--descriptor`
is given (WKT built-in) **or** a user descriptor with a sibling
`hopcroft.rkyv` is given.  The three sites require two updates:

**1. `decode` auto-inference guard (line 322)**

Current error message:
```
"decode auto-inference requires a DB-backed descriptor \
 (no hopcroft.rkyv found alongside the descriptor file)"
```

This message is wrong when `--descriptor` is absent and `wkt-db` is
enabled: there is no "descriptor file" to speak of.  Update the guard
to distinguish the two cases:

```rust
if auto_infer && desc_ctx.graph.is_none() {
    return Err(if cli.descriptor.is_some() {
        "decode auto-inference requires a DB-backed descriptor \
         (no hopcroft.rkyv found alongside the descriptor file)".into()
    } else {
        "decode auto-inference requires --descriptor with a sibling \
         hopcroft.rkyv, or a wkt-db-enabled build".into()
    });
}
```

When `wkt-db` is enabled and no `--descriptor` is given,
`desc_ctx.graph` is `Some(WKT_GRAPH)` and this guard is not reached.
Auto-inference proceeds normally via `infer_type(pb_bytes, graph)`,
scoring against the WKT Hopcroft graph.

**2. `list-schemas` guard (line 363) and `score` guard (line 408)**

Same treatment: update the error string to distinguish "no `--descriptor`
given" from "descriptor given but no sibling rkyv":

```rust
let graph = desc_ctx.graph.as_ref().ok_or_else(|| {
    if cli.descriptor.is_some() {
        "list-schemas requires a DB-backed descriptor \
         (no hopcroft.rkyv found alongside the descriptor file)"
    } else {
        "list-schemas requires --descriptor with a sibling hopcroft.rkyv, \
         or a wkt-db-enabled build"
    }
})?;
```

When `wkt-db` is enabled and no `--descriptor` is given, both
`list-schemas` and `score` work against the WKT graph transparently —
no code path changes, only the error message fallback.

**Summary of runtime behaviour after this spec:**

| Invocation | `desc_ctx.pool` | `desc_ctx.graph` |
|---|---|---|
| No `--descriptor`, `wkt-db` on | WKT FDS | `Some(WKT_GRAPH)` |
| No `--descriptor`, `wkt-db` off (bare) | WKT FDS | `None` |
| `--descriptor foo.desc`, no rkyv | user FDS | `None` |
| `--descriptor foo.desc`, rkyv present | user FDS | `Some(mmap)` |

### S6 — Compatibility with future mmap path

The `GraphBacking` enum introduced in S1 is the hook for the future mmap
spec.  When that spec adds mmap loading of `schemas.desc`:

- `load_graph()` uses `GraphBacking::Mmap` (already the case).
- The WKT path uses `GraphBacking::Static` (added here).
- All consumers receive `&ArchivedCompiledGraph` regardless of backing.

No changes to S1 are expected when the mmap spec lands.

---

## Files changed

| File | Change |
|---|---|
| `score-graph/src/score/load.rs` | Add `GraphBacking` enum; add `LoadedGraph::from_static_bytes()` |
| `prototext/src/lib.rs` | Add `WKT_GRAPH` static (feature-gated) |
| `prototext/src/run.rs` | Update `DescriptorContext::load` to load WKT graph |
| `prototext/build.rs` | Add `wkt-db`-gated step: compile `wkt.desc`, run reproto, emit `wkt.rkyv` |
| `prototext/Cargo.toml` | Add `wkt-db` feature (default); add `prost-build` build-dep |
| `prototext/wkt/SOURCES` | New — committed list of WKT `.proto` paths |
| `nix/rust.nix` | Add `prototextBare`, `wktRkyv`; update `prototext` to use `wkt-db` feature |
| `nix/python.nix` | Update `reproto` dep chain to use `prototextBare` |

---

## Implementation order

1. Add `GraphBacking` + `LoadedGraph::from_static_bytes()` to
   `score-graph` (S1).
2. Add `wkt/SOURCES` to `prototext/` (S3).
3. Add `wkt-db` feature to `prototext/Cargo.toml`; add `WKT_GRAPH`
   static to `lib.rs` (S2 prerequisite).
4. Write `build.rs` step for `wkt-db` (S4); verify it produces a valid
   `wkt.rkyv` in a local dev build.
5. Update `DescriptorContext::load` (S2).
6. Update `nix/rust.nix` and `nix/python.nix` (S5); verify
   `nix-build -A ci` passes.
7. Verify `prototext list-schemas` with no `--descriptor` returns
   `google.protobuf.*` candidates ranked by score.

---

## Test plan

- `prototext list-schemas` with no `--descriptor` and a `google.protobuf.Timestamp`
  binary input returns `google.protobuf.Timestamp` as the top candidate.
- `prototext decode` with no `--descriptor` and `--type google.protobuf.Timestamp`
  renders field names correctly (existing behaviour — must not regress).
- `cargo build --no-default-features -p prototext` (bare build) succeeds
  and produces a binary with no embedded graph.
- `nix-build -A ci` passes on all four CI platforms.
- `reuse lint` passes.
