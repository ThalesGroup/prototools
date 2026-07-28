<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0197 — the descriptor set is loaded on demand

Status: draft
App: protolens
Refs: docs/specs/0068-lazy-fds-index.md (the `FdsIndex` artifact),
      docs/specs/0069-lazy-pool.md (`LazyPool`, the prototext original),
      docs/specs/0099-any-lazy-loader.md (the JIT `Any` loader),
      docs/specs/0100-message-set-expansion.md (§5, `ext_to_file`),
      docs/specs/0111-protolens-v1-decode-navigate-extract.md
        (Goal 2, "v1 always requires an explicit --descriptor-set"),
      docs/specs/0114-protolens-range-type-override.md (§3.2),
      docs/specs/0117-protolens-override-collection.md
        (§4, `descriptor_set_sha256`),
      docs/specs/0137-protolens-override-primitive-and-enum-candidates.md
        (enums in the lexicographic list),
      docs/specs/0157-protolens-optional-descriptor-set-schemaless-launch.md
        (G3, the schemaless context),
      docs/specs/0177-reproducible-schema-db-artifacts.md
        (`canonical_map`, why the archive is byte-stable)

## Background

### 1. Startup is one function call

`DescriptorContext::load` (`decode.rs:87-107`) reads the whole
`--descriptor-set` into a `Vec<u8>` and hands it to `decode_pool`, which
builds a complete `prost_reflect::DescriptorPool`. Nothing about that is
deferred. On the googleapis corpus — 25.6 MB, 7 771 files, 58 777 types —
that single call is **1 698 ms**, and it is essentially the entire
startup wait: a batch run against a 100-byte blob with an explicit
`--type` (so no inference sweep, no meaningful render) takes 2.96 s wall.

The module's own header comment already names this as a deliberate v1
simplification:

```rust
//! Mirrors (simplified) `prototext`'s own `DescriptorContext` /
//! `infer_type` machinery (`prototext/src/run.rs`) — no
//! `LazyPool`/`index.rkyv` fast path, no embedded-WKT-descriptor
//! fallback: spec 0111 v1 always requires an explicit
//! `--descriptor-set`.
```

`prototext` has not been paying this since spec 0069. The same
invocation through `prototext decode` takes **0.07 s**.

### 2. The pool costs almost as much to destroy as to build

`drop(pool)` on the same 7 771-file pool is **844 ms**. protolens pays it
at process exit, so quitting the TUI on a large schema stalls for most of
a second with nothing on screen to explain it. This does not show up in
any startup-shaped framing of the problem, which is why it has gone
unnoticed; it is nonetheless the second-largest single cost in the
program's life.

### 3. A root type needs a fraction of a percent of the schema

`index.rkyv` records, per proto file, the byte span of its
`FileDescriptorProto` inside the raw `.pb` and that file's direct
imports (`fds_index.rs:57-79`). The transitive import closure of a root
type's file is therefore computable without decoding anything, and
because protobuf requires every referenced type's file to be imported,
that closure contains every type reachable from the root. This is the
invariant `LazyPool` already relies on.

Measured closures on googleapis:

| root type | files | payload | share of FDS | build |
|---|---|---|---|---|
| `google.protobuf.FileDescriptorSet` | 1 | 0.02 MB | 0.07% | 0.5 ms |
| `google.spanner.v1.ResultSet` | 10 | 0.03 MB | 0.12% | 0.9 ms |
| `google.cloud.aiplatform.v1.Model` | 14 | 0.05 MB | 0.18% | 1.4 ms |
| `google.container.v1.NodeManagement` | 15 | 0.15 MB | 0.58% | 4.4 ms |
| `google.cloud.bigquery.v2.Job` | 44 | 0.15 MB | 0.57% | 4.3 ms |

### 4. Most pool uses are point lookups; three are not

Every call site that reads the pool falls into one of two classes.

**Point lookups by name.** `determine_root_type` (`decode.rs:284`,
`:297`), `origin_resolves`'s `FqdnField` branch
(`command_line.rs:772`), the override pane's resolution
(`override_select.rs:687`, `:692`, `:717`), the override applier
(`override_apply.rs:368`, `:517`, `:523`, `:670`, `:799`, `:830`), and
`v`'s declaration jump (`key_dispatch.rs:749`). Each names one FQDN and
wants one descriptor. A lazy pool serves these by loading that FQDN's
file closure first.

**Whole-namespace scans.** These enumerate the pool and would silently
return nothing against a pool that starts empty:

- `override_pane::all_type_fqdns` (`override_pane.rs:34-42`), called
  once from `App::new` (`mod.rs:1421`) and consumed both by the override
  pane's lexicographic mode (`override_select.rs:384`) and by `:type`
  command-line completion (`command_line.rs:225-245`).
- `export_descriptor::locate_file_descriptor_set_type`
  (`command_line.rs:635`), which searches for `descriptor.proto`'s own
  messages so `:export --descriptor --prototext` has a meta-schema.
- `complete::complete_type_names` (`complete.rs:69-77`), which today
  decodes the entire 25 MB pool on **every shell TAB** just to list
  names.

### 5. The index is exactly the lexicographic universe

The obvious worry about serving `all_type_fqdns` from `index.rkyv` is
that the sidecars might not cover what the pool covers — in particular
that enums, added to the lexicographic list by spec 0137, might be
missing. Measured on googleapis:

| | count |
|---|---|
| pool messages | 49 255 |
| pool enums | 9 522 |
| pool total (`all_type_fqdns`) | 58 777 |
| `index.rkyv` `type_to_file` | 58 777 |
| `hopcroft.rkyv` roots | 49 255 |

The symmetric difference between the pool's namespace and the index's is
**zero in both directions**. The index is not an approximation of the
lexicographic list; it *is* the lexicographic list.

The graph is a different set: `roots` is exactly the pool's messages
(zero messages missing, zero roots unknown to the pool) and contains
**no enums at all**, because scoring a byte range against an enum is
meaningless. The two sidecars answer different questions — the graph
answers "what could these bytes be", the index answers "what types
exist" — and are not interchangeable.

### 6. A 25 MB buffer kept for a hash nobody asks for

`DescriptorContext::raw_bytes` (`decode.rs:69`) holds the canonicalized
descriptor bytes for the whole session. Its only reader is
`target_hashes` (`command_line.rs:692-696`), which SHA-256s it so
`:save` can stamp `descriptor_set_sha256` into the YAML (spec 0117 §4).
Most sessions never run `:save`; the ones that do, run it once. SHA-256
over the 25 MB is 22.6 ms.

### 7. `register_wrapper` is fine, but only just

`register_wrapper` (`decode.rs:556-605`) adds a synthetic one-field
`FileDescriptorProto` to the pool. Measured on the full googleapis pool
it costs **0.1 ms** when the pool's `Arc` is unique — and **563 ms**
when a live clone forces prost-reflect's internal `Arc::make_mut` to
deep-clone every file. The `drop(target)` at `decode.rs:592` is what
keeps it on the fast side, and its doc comment records the 2026-07-20
bug that motivated it.

This is a cliff whose height is proportional to pool size.
`warm_visible_override_wrappers` (`override_select.rs:673`) calls
`register_wrapper` once per visible override-pane row, once per frame.
On a 15-file lazy pool the same cliff is 1.4 ms.

## Goals

- **G1.** When `--descriptor-set` has an `index.rkyv` sidecar, do not
  decode the whole descriptor set at startup. Decode only the file
  closure the resolved root type needs.
- **G2.** Serve the three whole-namespace queries (§4) from the index
  rather than from the pool, so they keep working — and get faster —
  against a pool that starts empty.
- **G3.** Make the fallback to eager loading **loud**: a user who waited
  three seconds must be told why, and told that re-running `reproto`
  fixes it.
- **G4.** Stop holding the descriptor bytes for the whole session.
- **G5.** Preserve every existing behavior exactly. No visible change to
  what protolens renders, what the override pane offers, or what
  `:save` writes.

## Non-goals

- **N1.** Making the *eager* path faster. When there is no index there
  is nothing to be lazy about, and `decode_pool` is prost-reflect's
  cost, not ours.
- **N2.** Sharing the pool with a background thread. `LazyPool` is
  `&mut`-only; the loading it does is a mutation. Today no protolens
  thread receives the pool — the scoring threads take
  `Arc<LoadedGraph>` — and this spec does not change that.
- **N3.** Lazy loading for a `#@ prototext` descriptor set. See §S7.
- **N4.** Generating `index.rkyv` from protolens. It is `reproto`'s
  artifact; protolens consumes it or does without.
- **N5.** Extending `FdsIndex`'s on-disk format. Everything this spec
  needs is already in it. Changing it would bump PTSGRAPH's version and
  invalidate every existing schema-db.
- **N6.** A trait abstracting "schema source". `prototext` solves the
  same problem with two fields and a pair of accessors
  (`run.rs:38-62`); a trait here would be an abstraction over exactly
  two implementations, one of which is a plain struct field.

## Specification

### S1 — `DescriptorContext` gains a lazy branch

Mirror `prototext`'s shape (`run.rs:38-43`) rather than inventing one:

```rust
pub struct DescriptorContext {
    /// Populated only on the eager path.
    pool: Option<DescriptorPool>,
    /// Populated only when an `index.rkyv` sidecar was accepted.
    lazy: Option<LazyPool>,
    pub graph: Option<Arc<LoadedGraph>>,
    /// Where the descriptor set was read from, for on-demand hashing (S6).
    source: Option<PathBuf>,
}
```

`pool()` and `pool_mut()` keep their current signatures and return
`&lazy.pool` on the lazy path. Every existing call site compiles
unchanged; what changes is *what is in* the pool at a given moment.

`LazyPool` moves from `prototext/src/lazy_pool.rs` to a crate both
binaries depend on. `prototext-core` is the obvious candidate — it
already owns `decode_pool` — but the type needs
`prototext_graph::fds_index::ArchivedFdsIndex`, and `prototext-core`
does not depend on `prototext-graph`. Putting it *in* `prototext-graph`
would work with no new crate, at the price of dragging `prost-reflect`
into a crate that is otherwise pure scoring and serialization.

**A new workspace member, `prototext-schema`, takes it instead.** Its
whole contents are `LazyPool` and whatever the two binaries need to
share around it. It depends on `prototext-graph` (for `ArchivedFdsIndex`),
`prost`/`prost-types`/`prost-reflect`, `memmap2`, `rkyv` and
`workspace-hack`; nothing depends on it but the two binaries. This keeps
`prototext-graph`'s dependency set as it is and gives the descriptor-
loading concern a name.

Mechanically the move requires: the member added to `Cargo.toml`'s
`members` list; a `[workspace.dependencies]` entry alongside
`prototext-core`'s; the crate registered in `default.nix` the same way
`prototext-graph` is; and `workspace-hack` regenerated. `prototext`
re-exports `LazyPool` from its old path so its own call sites are
untouched. Otherwise this is a pure move — no behavioral change, and
`prototext`'s existing tests are the regression suite for it.

One signature change is forced by the move. `LazyPool::open` decodes
`crate::EMBEDDED_DESCRIPTOR` for its WKT fallback (`lazy_pool.rs:120-126`),
and that constant is generated by `prototext`'s own `build.rs` into
`OUT_DIR` (`prototext/src/lib.rs:25`). Rather than give the new crate a
`build.rs` to reproduce it, `open` takes the fallback as a parameter:

```rust
pub fn open(pb_path: &Path, idx_path: &Path, wkt_fallback: &[u8])
    -> Result<Self, Box<dyn std::error::Error>>
```

`prototext` passes `EMBEDDED_DESCRIPTOR`. protolens passes `&[]`, which
is both correct and deliberate: spec 0111 Goal 2 says protolens has no
embedded-WKT fallback, and the fallback cannot fire anyway — `reproto`
builds self-contained descriptor sets with `--include_imports`, so every
dependency has a span in `file_to_span` and the WKT branch at
`lazy_pool.rs:183` is unreachable (the invariant is already stated at
`fds_index.rs:69-71`).

### S2 — selecting the path

`DescriptorContext::load` gains the same sidecar probe `prototext` uses
(`run.rs:88-119`), extended with S3's diagnostics:

1. `stem = path.with_extension("")`.
2. If `stem/hopcroft.rkyv` exists, load it as today.
3. If `stem/index.rkyv` exists **and** the descriptor is binary (S7),
   try `LazyPool::open`. On success, `pool` is `None`.
4. Otherwise read and `decode_pool` the whole file, as today.

`schemaless()` and the test constructors set both `pool` to an empty
`DescriptorPool` and `lazy` to `None`, so the schemaless path (spec
0157 G3) is untouched.

### S3 — the fallback is announced, not silent

Falling back costs the user seconds. Three distinguishable causes, each
with its own message:

| cause | message |
|---|---|
| no `index.rkyv` beside the descriptor | `no index.rkyv beside '<name>' — loading the whole descriptor set; re-run reproto to build one` |
| `index.rkyv` present but rejected | `'<path>': <error> — loading the whole descriptor set; re-run reproto to regenerate it` |
| descriptor is `#@ prototext` | `'<name>' is #@ prototext — loading the whole descriptor set; a binary .pb descriptor can be loaded on demand` |

Each is printed to stderr, prefixed `protolens: warning:`, immediately
after the existing `protolens: loading descriptor set '<name>'…` line
(`main.rs:308-311`) and before the wait it explains — so it is on screen
*while* the user waits, not after.

The same text is retained on `DescriptorContext` and rendered in the
splash pane, so a user who launched without watching stderr still finds
out. The splash line is styled as a warning, not as body text.

The splash pane alone is not enough exposure. It is a startup screen,
not a persistent panel: `track_splash_timeout` (`render.rs:785-788`)
dismisses it after `SPLASH_TIMEOUT = 3 s` (`mod.rs:110`), and the first
key (`key_dispatch.rs:314`) or mouse event (`mouse.rs:30`) dismisses it
sooner. A user who starts typing immediately never reads it. So the
warning is seeded into the status line as well (`app.message`), where it
stays until the next thing that writes there. Three channels, one text:
stderr for the scripted run, the splash for the idle start, the status
line for the impatient one.

The "rejected" case must not be fatal. A version-skewed `index.rkyv`
(`check_header` returns "unsupported version N") is exactly the state
every user's schema-db is in the moment this ships, and it must degrade
to today's behavior.

### S4 — the whole-namespace queries move to the index

`LazyPool` gains one read-only accessor:

```rust
/// Every type name the index knows, sorted. Messages and enums, nested
/// types included — see spec 0197 §5: this set is equal to the eager
/// pool's `all_messages() + all_enums()`.
pub fn all_type_fqdns(&self) -> Vec<String>
```

implemented as `self.index.type_to_file.keys()` collected and
`sort_unstable`'d. Measured at **11-13 ms** for 58 777 names, against
24 ms for the pool-based version, so it stays where it is —
eagerly in `App::new` (`mod.rs:1421`). No deferral, no `OnceCell`.

`override_pane::all_type_fqdns(&DescriptorPool)` stays for the eager
path. `App::new` picks:

```rust
let all_type_fqdns = ctx.all_type_fqdns();   // dispatches on the branch
```

`complete::complete_type_names` (`complete.rs:69-77`) takes the same
route: probe for the sidecar, read names from it, and only decode the
pool when there is no index. This is a latency fix in its own right —
shell completion currently decodes 25 MB per TAB.

Completion is the one place where the fallback of S3 is silent. It runs
in a subprocess whose stdout is a candidate list the shell parses; there
is no stderr a user reads and no `App` to hold a message. It falls back
without announcing it, which is acceptable: the user asked for names,
not for a schema, and the same descriptor produces the same warning on
the next real launch.

`export_descriptor::locate_file_descriptor_set_type`
(`command_line.rs:635`) is not a namespace query in disguise; it wants
one specific message. On the lazy path, JIT-load
`google.protobuf.FileDescriptorSet` by name first, then call the
existing function unchanged. Its "no descriptor.proto in the loaded
`--descriptor-set`" error message stays correct: a name absent from the
index is absent from the schema.

### S5 — point lookups JIT-load first

Introduce two shims on `DescriptorContext`, so no call site has to know
which branch it is on:

```rust
pub(crate) fn message(&mut self, fqdn: &str) -> Option<MessageDescriptor>
pub(crate) fn enumeration(&mut self, fqdn: &str) -> Option<EnumDescriptor>
```

Each calls `lazy.get_message`/`get_enum` when lazy (ignoring the load
error, exactly as `prototext`'s `install_any_loader` does — an
unresolvable name is a miss, not a crash) and then reads from
`pool()`. On the eager path they are a bare pool lookup.

Every point-lookup call site in §4 converts to these. Two need more:

- **`decode.rs:284/297`.** `determine_root_type` takes `&DescriptorContext`;
  it becomes `&mut`. Its two callers (`main.rs:363`, `decode`'s own
  `decode` at `decode.rs:833`) already hold a `&mut`.
- **`override_apply.rs:813/830`.** MessageSet resolution needs the
  *extension* JIT path, not the message one:
  `lazy.get_extension(extendee_fqdn, field_number)` before
  `get_message_by_name(extendee).get_extension(number)`. Add a third
  shim, `extension(&mut self, extendee: &str, number: u32)`, whose
  eager form is a no-op.

**The staleness rule.** prost-reflect's `add_file_descriptor_proto` uses
`Arc::make_mut`; when the pool forks, a `MessageDescriptor` obtained
before a later load is blind to symbols registered after it
(`prototext/src/run.rs:648-652`). protolens is safe today only because
it happens to store no descriptor in `App` and re-fetches at each use.
This spec makes that explicit: **`App` must not hold a
`MessageDescriptor` or `EnumDescriptor` across an event-loop
iteration.** A doc comment on `DescriptorContext` states it; the
existing shape already complies.

### S6 — the descriptor bytes are not retained

`raw_bytes: Vec<u8>` is replaced by `source: Option<PathBuf>`.

`target_hashes` (`command_line.rs:692`) computes
`descriptor_set_sha256` on demand: re-read the file through
`read_descriptor_file` (so a `#@` input is canonicalized to binary
exactly as it was at load time) and hash it. `:save` is a deliberate,
interactive, once-per-session action; ~55 ms there is invisible, and it
removes 25 MB of resident memory from every session.

`source` is `None` for the schemaless context, whose hash is that of the
empty byte string — the same value `sha256_hex(&[])` returns today.

### S7 — `#@ prototext` descriptors stay eager

`LazyPool` slices `FileDescriptorProto`s out of the mmapped file by byte
offset (`lazy_pool.rs:179-182`). Those offsets index the *binary* wire
encoding. `read_descriptor_file` (`decode.rs:157-180`) converts a `#@`
text descriptor to binary in memory, and that buffer was never indexed —
the offsets in `index.rkyv` do not describe the file on disk.

So when the descriptor starts with the `#@` magic, the lazy path is
skipped regardless of whether a sidecar is present, with S3's third
message. In practice this never fires: `reproto` writes a binary `.pb`
and only then builds the sidecars, so a `#@` descriptor has no
`index.rkyv` next to it. It is a guard against a hand-assembled
directory, not a supported configuration.

## Alternatives considered

**Hash the descriptor at load and keep only the digest.** 22.6 ms and 32
bytes, versus 25 MB. Strictly better than today, and simpler than S6.
Rejected because 22.6 ms is roughly a fifth of the post-lazification
startup budget, paid unconditionally by every session for a value most
sessions never read. S6 pays nothing until `:save`.

**Keep the mmap and hash from it.** `LazyPool` already holds the file
mapped (`lazy_pool.rs:34`), so hashing at `:save` time could read from
there for free. Rejected because it only works on the lazy branch; the
eager branch would need its own mmap, and `DescriptorContext` would need
a "bytes from either place" accessor. Re-reading the path works
identically on both branches with no new type.

**Serve the lexicographic list from `hopcroft.rkyv` instead.** The graph
is already loaded and already holds FQDN strings. Rejected on measured
grounds: `roots` is exactly the pool's *messages* and contains **zero
enums**, so this would silently drop 9 522 names from the override pane
and undo spec 0137.

**Move the eager `decode_pool` to a background thread.** Rejected:
`render_resolved` cannot produce a single line without the root's
`MessageDescriptor`, so the first frame depends on the pool. Only a
render-raw-then-upgrade design would help, which is a far larger change
for a worse result.

**Cache a prebuilt pool on disk.** `prost_reflect::DescriptorPool` is
not serializable, and `index.rkyv` already is the durable form of the
same information.

**A `SchemaSource` trait.** See N6.

## Test plan

1. **Both branches resolve the same root type.** Decode a fixture blob
   twice — once with a sidecar present, once with it hidden — and assert
   the rendered lines are byte-identical.
2. **The lazy pool starts empty and grows.** After `LazyPool::open`,
   `pool().all_messages().count() == 0`; after resolving one root type,
   it equals that root's file-closure type count and no more.
3. **A type outside the root's closure resolves on demand.** Look up an
   FQDN from an unrelated file through `ctx.message()`; assert it
   returns `Some` and that the pool grew.
4. **`all_type_fqdns` agrees across branches.** For a fixture schema
   with nested messages and enums, assert the index-sourced list equals
   the pool-sourced list exactly — same length, same order. This is
   §5's measured property, pinned as a test.
5. **Enums survive.** The fixture must contain a nested enum, and the
   lexicographic candidate list must contain it. Direct regression guard
   for spec 0137.
6. **Missing index falls back and warns.** No `index.rkyv`: the context
   is eager, and the recorded warning names the missing sidecar.
7. **Version-skewed index falls back and warns.** Corrupt the PTSGRAPH
   version byte; assert the context is eager, that loading did **not**
   error, and that the warning quotes the version complaint.
8. **A `#@` descriptor falls back even with a sidecar present.** Write a
   `#@` descriptor and a valid `index.rkyv` beside it; assert eager plus
   the third warning.
9. **The warning reaches the splash pane.** Render a frame after a
   fallback and assert the pane contains the text.
10. **The warning survives the splash.** After a fallback, dismiss the
    splash (a key event) and assert the status line still carries the
    warning — the ephemerality guard of §S3.
11. **`:save` hashes identically across branches.** The
    `descriptor_set_sha256` written from a lazy context must equal the
    one written from an eager context over the same descriptor, and both
    must equal today's value (pin the literal digest for the fixture).
12. **Schemaless is unaffected.** No `--descriptor-set`: empty pool, no
    lazy, no warning, and `target_hashes` returns `sha256_hex(&[])`.
13. **A MessageSet extension JIT-loads.** On the lazy branch, an
    extension whose declaring file is outside the root closure must
    still resolve through the `extension()` shim.
14. **Override selection across a fresh type.** Open the override pane,
    move onto a candidate whose file is not yet loaded, and assert the
    splice resolves — the end-to-end path through `warm_visible_override_wrappers`,
    `register_wrapper` and `splice_override`.
15. **Shell completion needs no pool.** `complete_type_names` against a
    sidecar-backed descriptor returns the full name list, and does not
    call `decode_pool`.

Performance checks, run manually against the googleapis corpus and
recorded in `Measured outcome` — not part of the regression suite, per
`docs/bench-process.md`:

16. Startup wall time for `--type <fqdn>` on a small blob, lazy vs eager.
17. Exit wall time (the `drop(pool)` cost).
18. Resident memory at steady state.
19. `register_wrapper` on a forked pool, lazy vs eager.

## Measured outcome

(To be filled in on implementation.)

Baseline, googleapis (`googleapis.desc`, 25.6 MB, 7 771 files, 58 777
types), release build, measured 2026-07-28:

| | |
|---|---|
| read the 25 MB `.pb` | 31 ms |
| `decode_pool` | 1 698 ms |
| `all_type_fqdns` from the pool | 24 ms |
| `drop(pool)` | 844 ms |
| `register_wrapper`, pool `Arc` unique | 0.1 ms |
| `register_wrapper`, pool `Arc` forked | 563 ms |
| SHA-256 of the 25 MB | 22.6 ms |
| open + mmap + `access` `index.rkyv` | 0.1 ms |
| sorted 58 777-name list from `type_to_file` | 11-13 ms |
| root-type file closure, decoded | 0.5-4.4 ms |
| `protolens --type … <100-byte blob> export`, wall | 2.96 s |
| `prototext --type … decode`, same input, wall | 0.07 s |
