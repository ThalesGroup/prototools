<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0480 — fdp-scan inflates zip and gzip members

Status: draft
App: fdp-scan, protoscan, git-seriate-fdp
Refs: docs/protoscan/scan.md §5 (compressed descriptors: Go's APIv1
        gzips them, no surveyed tool reads them, the pre-pass sketched),
      docs/specs/0448-fdp-scan-undeclared-fields-and-rejections.md (S7,
        `Explained`, which `scan` must keep agreeing with),
      docs/specs/0449-fdp-scan-dependency-indices-and-field-order.md (S7,
        `ScanOptions`, `order_cuts` recorded by extract),
      docs/specs/0453-git-seriate-fdp-descriptors-from-executables.md (S2, S5: the extractor and the
        snapshot it writes)

## Background

The scanner reads the input's bytes as they are. Descriptors that were
compressed before being embedded are invisible to it:

- **Go's APIv1** (`github.com/golang/protobuf`) registered each file
  descriptor as a gzip stream (`proto.RegisterFile`, `fileDescGZIP`).
  A binary built against it shows no descriptor in the clear (scan.md
  §5.2). Multi-year collections of executables contain such builds.
- **Descriptor sets shipped as data**, for gRPC reflection, transcoding
  or a schema registry, are often embedded as a member of a zip archive
  (`go:embed`, a resource bundle, a jar). Deflated, they are invisible.
  Stored (method 0), they are found, but nothing says they came from
  an archive rather than from compiled code.

Verified in the code (2026-11-25): `fdp-scan` depends on `prototext` and
`prototext-graph` only. Nothing in `fdp-scan/src/lib.rs`,
`protoscan/src/main.rs` or `git-seriate-fdp/src/extract.rs` decompresses
anything. `flate2` is in the workspace, but only `git-seriate`'s archive
writer uses it.

The second point matters beyond recall. A descriptor set embedded as
data was produced by its own protoc or buf run, from its own tree at its
own date. A downstream pass that groups an executable's descriptors by
the Go module that compiled them (the offline split into module-scoped
snapshots) must tell such sets apart from compiled-in descriptors. It
can only do so if extraction recorded where each descriptor was found.

## Goals

- **G1. Find descriptors inside gzip streams and zip members,** deflated
  or stored, nested to a bounded depth.
- **G2. Report each descriptor's origin:** the chain of containers it
  was found in, and its range in the innermost one.
- **G3. Nothing changes unless asked.** Without the option, every output
  (`scan`, `Explained`, protoscan's, extract's snapshots) is
  byte-identical to today's.
- **G4. Bounded cost on hostile input.** Inputs are untrusted binaries:
  decompression is capped in depth, per member and in total output.
- **G5. Bounded memory at scale.** Peak memory follows the largest
  member at each nesting level, never the sum of all members.

## Non-goals

- **N1. No zlib or raw deflate streams without a container.** zlib's
  two-byte header (`78 01`, `78 9c`, `78 da`…) occurs every few
  kilobytes in any binary, and raw deflate has no header at all. Trying
  each would multiply the cost for no known source of descriptors.
- **N2. No other compression methods** (bzip2, xz, zstd, zip methods
  other than 0 and 8), and no encrypted zip members. They are counted
  as skipped (S5), not read.
- **N3. No split by origin here.** Turning an embedded descriptor set
  into a snapshot of its own is the offline split's job. This spec only
  records what that split needs.
- **N4. The Python extension is unchanged.** `fdp_scan_lib.scan` keeps
  returning ranges into its input. An inflating variant can follow when
  a Python caller needs one.
- **N5. No encoded descriptors.** Some generated code holds the
  descriptor encoded, not compressed, and inflating does not reveal it:
  - **Java:** `descriptorData` string constants, which a class file
    stores as modified UTF-8 (bytes ≥ 0x80 take two bytes, NUL is
    `C0 80`). An inflated jar still shows no raw descriptor.
  - **C#:** a base64 string literal.
  - **Python sources:** an escaped bytes literal in `_pb2.py`. Compiled
    `.pyc` files marshal the raw bytes, though, so a wheel or `.pyz` of
    compiled modules does gain descriptors once inflated.

  Each would need a decoder of its own. How often they occur is not
  measured.
- **N6. No change of default for extract.** Inflating changes snapshots
  (new items, `origin` entries, files found in more than one place). A
  kit turns it on deliberately, as a versioned choice; it is never
  inherited.

## Specification

- **S1. Containers recognized.** With the option on, a pre-pass looks
  for two signatures in the buffer being scanned:
  - a **zip local file header** (`50 4b 03 04`): the 30-byte header is
    parsed by hand (no zip crate: embedded archives are carved from the
    middle of a file, and the central directory's offsets are relative
    to an archive start nobody knows). Method 0 is read as it is,
    method 8 inflated. When general-purpose bit 3 is set (sizes in a
    trailing data descriptor), the member's end is where the deflate
    stream ends.
  - a **gzip member header** (`1f 8b 08`), with its reserved flag bits
    zero, inflated through its optional fields.

  `flate2` with the `zlib-rs` back end, already a workspace dependency,
  does the inflating.
- **S2. Only valid members count.** A gzip member is kept only if its
  CRC-32 and size trailer match the inflated bytes. A zip member is kept
  only if its CRC-32 and uncompressed size match, when the header gives
  them. Anything else is a signature found by chance, dropped silently:
  random `1f 8b 08` bytes are common in code, and the trailer check is
  what rejects them.
- **S3. Each kept member is scanned like the input,** depth-first: its
  bytes go through the same pre-pass and the same scanner, with the same
  schema and options. A stored member's bytes are a slice of its
  parent's, never copied. A deflated member is inflated into a buffer
  kept per nesting level and reused for the next member at that level.
  Descriptors found in a member's range are reported with that member,
  not with the parent.
- **S4. Limits** (`InflateLimits`), all settable:
  - a depth of 3 containers (a gzip in a zip in a zip);
  - 256 MiB inflated per member, which bounds memory (G5);
  - 1 GiB inflated per input, which bounds time.

  Inflating a member stops as soon as a limit is reached, and the member
  is skipped (S5): its partial bytes are not scanned.
- **S5. The API: a visitor.** Nothing is held once visited (G5):

  ```rust
  pub struct ScanOptions {
      pub increasing_field_numbers: bool,
      /// Look into zip and gzip members (spec 0480). `None`: as before.
      pub inflate: Option<InflateLimits>,
  }

  /// One scanned buffer: the input, or a member.
  pub struct Visit<'a> {
      pub origin: &'a Origin,     // the chain of containers; empty for the input
      pub data: &'a [u8],         // valid during the call only
      pub explained: &'a Explained, // as today: ranges into `data`
      pub skipped: &'a [Skipped], // members of `data` not read, and why
  }

  pub fn scan_inflating(
      data: &[u8],
      graph: &LoadedGraph,
      root: &str,
      options: &ScanOptions,
      visit: impl FnMut(&Visit),
  );

  pub struct Skipped { pub at: usize, pub kind: MemberKind, pub reason: SkipReason }
  // MemberKind: Zip { name: String } | Gzip
  // SkipReason: Encrypted, Method(u16), DepthLimit, MemberLimit, InputLimit
  ```

  The visitor is called once per buffer: the input first, then each
  member depth-first, a member's own members right after it. A caller
  wanting a tree or a flat list builds it in the visitor. `Explained`
  is unchanged. `scan`, `scan_explained` and the other entry points are
  unchanged and never inflate, so `scan` and `scan_explained(...).found`
  still agree (0448 S7).
- **S6. Origin, as text.** An optional prefix given by the caller, a
  chain of containers from the input inward, then the range in the
  innermost buffer. Offsets are in decimal, as `order_cuts` records them
  (0449 S7):

  ```
  origin = [ prefix "!" ] { layer "!" } start ".." end
  layer  = "zip@" offset "!" member-name | "gzip@" offset
  ```

  - **The offset of `zip@N`** is that of the member's local file header
    (its `P`). **The offset of `gzip@N`** is that of its `1f 8b`. Both
    are relative to the buffer holding the header.
  - **The range** is relative to the member's data: the stored bytes
    after the header, its name and its extra field, or the inflated
    bytes.
  - **The prefix** continues a name the caller already gives the input,
    such as `<file>/<member>` for a whole-file archive the caller
    expanded itself. `Origin::text(prefix)` writes it; fdp-scan never
    invents one.
  - **In a member name,** `%` and `!` are written `%25` and `%21`, so
    `!` always separates.

  Examples:
  - `1048576..1050112`: in the clear, in the input;
  - `gzip@2203648!0..1536`: an APIv1 descriptor, the whole gzip stream;
  - `zip@7340032!assets/api.binpb!88..4210`: in a descriptor set, a
    zip member;
  - `zip@7340032!lib/x.jar!zip@0!x.desc!0..900`: a jar inside an
    archive.
- **S7. protoscan.**
  - `--inflate`: looks into members (S1–S4, default limits).
  - `--origin`: each line is `NAME<TAB>ORIGIN`. It works without
    `--inflate` too, where every origin is a plain range.
  - Skipped members are reported on stderr, one line each: the member's
    origin and the reason.
  - `--proto-out` writes descriptors from members the same way.
    Several descriptors with the same name and different bytes are
    written `<name>~2.pb`, `<name>~3.pb`…, the rule `git-seriate`'s store
    uses. The same bytes again are written once. This also changes the
    case without `--inflate`, where today the last descriptor of a name
    silently overwrites the earlier ones; `CHANGELOG.md` says so.
  - Off by default. Whether protoscan, an exploratory tool, inflates by
    default is decided once test 11 is measured, in a later spec.
- **S8. git-seriate-fdp extract.**
  - `--inflate` turns the option on. Off by default, for good (N6).
  - Item paths stay the descriptor's name (`a/b.pb`), whatever its
    origin. The same file found in the clear in one executable and in a
    zip in another is the same item, and comparisons across snapshots
    go by path.
  - A descriptor not found in the clear gets one record entry:
    `origin <stored path>` = its origin (S6). Descriptors in the clear
    get none, so snapshots of modern executables gain nothing.
  - **The store names the stored path.** `store::Item` gains
    `record: Vec<(String, String)>`, entries about that item. The store
    writes each as `<key> <stored path>` = value, once it has placed the
    item, `~2` variant included. Only the store knows its naming rule,
    so a caller can't compute a path a later change of that rule would
    make wrong. An item dropped as the same bytes again keeps its
    entries, under the path of the copy kept.
  - Skipped members add `skipped <origin of the member>` = the reason, on
    the snapshot, not only on stderr.
- **S9. Cost when off: none.** The option is checked once, before the
  pre-pass; the scan loop is untouched.

### Expected cost when on

Not measured yet. It's measured in the test plan, before any decision
to make the option a default. The model:

- **The pre-pass** looks for two short signatures with
  `memchr::memmem` (already in the lock file), which skips through the
  buffer with SIMD. The scanner already visits every byte and decodes a
  varint at every `0x0A`. The pre-pass should cost a small fraction of
  that.
- **False signatures** cost a header parse each, plus, for gzip, an
  inflate until the deflate stream errors or ends. The trailer check
  rejects them. Most error within a few bytes.
- **Real members** cost an inflate, then a scan of the inflated bytes at
  the scanner's usual rate. So, roughly: today's cost × (1 + inflated
  bytes / input bytes).
  - **A modern Go binary:** a handful of members (the `gh` binary has 3
    gzip members, scan.md §5.2), so close to today.
  - **An APIv1 binary:** one gzip stream per `.proto` file, a few
    megabytes inflated in all, so a small addition. This is the case
    the option exists for.
  - **A binary that is mostly a deflated archive** (UI bundles, embedded
    installers): the inflated size, up to the 1 GiB cap, dominates.
- **Memory:** one buffer per nesting level, each at most the largest
  member inflated at that level (256 MiB at most), so 768 MiB in the
  worst case at depth 3. Stored members are slices of their parent.

## Alternatives considered

- **Inflate everything into one big buffer, then scan it once.**
  Offsets would then mean nothing outside that buffer, and a descriptor
  would lose the chain of containers it was found in. That chain is the
  point of G2.
- **A separate pre-pass tool feeding fdp-scan.** It would copy inflated
  bytes out and lose their offsets. A container walker inside fdp-scan,
  calling the unchanged scanner on each buffer, keeps both, and leaves
  the byte scanner as it is.
- **A tree of members in `Explained`, or a flat list.** Either holds
  every inflated member until the caller is done: up to the 1 GiB limit
  in memory at once, against G5. Both are easy to build from the
  visitor.
- **A ratio cap (inflated ÷ compressed).** Deflate cannot exceed about
  1032:1 on one stream, so a cap near 1000× catches almost nothing, and
  a lower one refuses honest sparse data. Bombs work through nesting or
  through zip members overlapping the same bytes: the depth and total
  limits stop those.
- **`store::place_items` made public,** extract placing the items itself
  to key its records. Callers would then rely on the store's naming
  rule, and break silently when it changes.
- **The `zip` crate.** It reads archives through their central
  directory, which needs the archive's start. An archive carved out of
  the middle of an executable doesn't have one. A local header is 30
  bytes plus two variable-length fields, simpler to parse than to adapt
  a crate to.
- **Origin as part of the item path** (`zip@…/assets/api.binpb/a/b.pb`).
  The same descriptor would then be a different item depending on how
  it was shipped, and seriation would no longer see it as the same file.
- **Inflation on by default in extract.** It changes snapshot contents
  for inputs holding compressed descriptors (N6).
- **A cargo feature for `flate2` in `fdp-scan`.** `flate2` with
  `zlib-rs` is pure Rust and already built for `git-seriate`. A feature
  would only add a build variant to test.

## Test plan

1. `inflate_off_is_unchanged`: the existing corpus (`fdp-scan/tests/corpus`)
   scans identically with `inflate: None`, and `scan` equals
   `scan_explained(...).found`.
2. `gzip_member_found`: a descriptor gzipped and embedded among noise
   is found, its origin `gzip@N!0..len`.
3. `zip_members_stored_and_deflated`: one archive, one stored and one
   deflated member, each holding descriptors. Both are found with their
   origins, and the stored member's descriptors are no longer in the
   top-level `found`.
4. `bit3_data_descriptor`: a deflated member with sizes in a trailing
   data descriptor is read to the end of its stream.
5. `false_signatures_dropped`: random `1f 8b 08` and `PK\3\4` bytes, and
   a member with a bad CRC, yield nothing.
6. `limits`: an archive nested 4 deep, a member over the per-member
   limit and an input over the total limit each stop at their limit,
   reported in `skipped`.
7. `visit_order_and_reuse`: the visitor sees the input, then members
   depth-first; peak allocation stays one buffer per level.
8. `origin_text`: a member name with `!` and `%` round-trips, and a
   prefix heads the chain.
9. protoscan CLI: `--origin` with and without `--inflate`, skipped
   members on stderr, and `--proto-out` writing `~2` for a second
   descriptor of the same name.
10. extract: an executable with a zipped descriptor set gets
    `origin <path>` records under the paths the store chose, `~2`
    included, `skipped` entries for refused members, and no entries for
    descriptors in the clear.
11. Measurement, on public binaries, inflation off then on: `gh` (APIv2),
    an executable built against APIv1 (an old release of a Go
    consumer), and one embedding a large deflated archive. Report time,
    peak memory and descriptors found.

## Measured outcome

Filled in at implementation.
