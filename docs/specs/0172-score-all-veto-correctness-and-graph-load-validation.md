<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0172 — prototext-graph: `score_all` veto correctness and scoring-graph load validation

Status: implemented
Implemented in: 2026-07-25
App: prototext-graph
Refs: docs/scoring-flaws.md (C4, C5, C6, C8, C10),
      docs/protolens/rendering-worklist.md (W29, W30)

## Background

`score_all` decides which root type a blob most likely is. It carries
every candidate through one traversal, and each candidate can end in one
of two ways: **scored** (counters accumulate, a comparator picks the
best) or **vetoed** (permanently eliminated, no score can save it).

The governing principle — stated in the cross-cutting section of
`docs/scoring-flaws.md` — is: *veto only for what the wire format makes
impossible; score everything that is merely unlikely.* Four defects
break it in both directions.

### C4 — an out-of-range field number aliases into a real field

`parse_wiretag` correctly detects the case (`walk.rs:403`):

```rust
let oor = field_number == 0 || field_number >= (1 << 29);
```

and `score_message_multi` correctly charges it (`walk.rs:817-828`,
`non_canonical += 1`). But it then looks the field up anyway
(`walk.rs:836`):

```rust
let v = match find_transition(ws.graph, ae.state_id, field_number as u32) { .. };
```

`field_number` is a `u64`. Field number `2^32 + 1` truncates to `1` and
finds the transition for field 1 — so a candidate can be awarded a
`match`, have its wire type checked against the wrong field, or be
*vetoed* for a mismatch, all on the strength of a field number no schema
can possibly declare. It is the only place in the walk where a `u64` off
the wire is narrowed without the range having been established.

### C5 — a canonically-encoded negative enum is always vetoed

Negative varints are sign-extended to 64 bits, so `-1` on the wire is
ten bytes decoding to `0xFFFF_FFFF_FFFF_FFFF`. The RANGE arm
(`walk.rs:933-961`, wire type 0 with `range_idx != 0xFFFF` — bool and
enum) begins:

```rust
if val >= (1u64 << 32) {
    do_veto = true;
} else {
    ...
    let signed = val as i32 as i64;
    if signed < min || signed > max { if ws.strict_ranges { do_veto = true; } .. }
}
```

Every canonical negative therefore takes the first branch and is vetoed
outright — the range is never consulted, and `strict_ranges` never gets
a say. Meanwhile the *non-canonical* four-byte truncation of the same
value (`0x8000_0000..=0xFFFF_FFFF`) reaches the `else` branch, is
decoded correctly by `val as i32`, and is merely penalized. The
treatment is exactly inverted: the correct encoding is fatal, the
sloppy one is tolerated.

The `INT32` arm two match-arms above (`walk.rs:916-925`) already gets
this right, vetoing only the genuinely impossible gap
`0xFFFF_FFFF < val < 0xFFFF_FFFF_8000_0000` — values that are neither a
u32 nor a sign-extended i32. The RANGE arm needs the same shape.

### C6 — proto3 open enums are eliminated, not penalized

Grepping `prototext-graph/src` for `syntax`, `proto3`, `open_enum` or
`closed` returns nothing: the compiled graph carries `wire_type`,
`is_string` and `range_idx` per node (`serial.rs:14-28`) and no syntax
information at all. `ScoringOpts::strict_ranges` defaults to `true`
(`walk.rs:178`), so an enum value outside the declared range vetoes the
candidate.

Under proto3 all enums are *open*: an unknown value is legal, is
preserved on round-trip, and is the normal consequence of reading a
message written by a newer build. Under proto2 the same value is
invalid. The graph cannot tell the two apart — so vetoing means that
scoring a message which merely carries a forward-compatible enum value
eliminates its own correct FQDN, and the winner becomes some unrelated
type that happened not to be vetoed.

Making this precise requires a syntax bit in the graph format, which is
a format version bump and is **deliberately deferred** (decision D-g,
recorded in `docs/protolens/rendering-worklist.md`). This spec ships the
interim, which is a strictly better default under the information the
graph actually has.

### C8 — an unvalidated `root_offset` is dereferenced

`check_header` (`load.rs:34-47`) validates the length, the magic and the
version, then reads a 64-bit `root_offset` out of the header and returns
it without a bounds check. `load_graph` (`load.rs:84-89`) then does:

```rust
let bytes: &'static [u8] =
    std::slice::from_raw_parts(mmap.as_ptr().add(root_offset), mmap.len() - root_offset);
```

`mmap.len() - root_offset` underflows, `add` produces a pointer outside
the mapping, and `from_raw_parts` fabricates a slice of nearly
`usize::MAX` bytes — undefined behavior *before* rkyv's `access`
validator ever runs. `from_static_bytes` (`load.rs:61`) is not affected:
`&bytes[root_offset..]` panics cleanly. The unsound path is the mmap
one, which is the one protolens uses.

### C10 — an `assert!` on the root count

`score_all` opens (`walk.rs:187-191`) with

```rust
assert!(graph.roots.len() <= u16::MAX as usize,
        "entry count {} exceeds u16::MAX", graph.roots.len());
```

because `ActiveEntry::entries` holds `u16` indices. The limit is real
and this spec keeps it (widening the index is decision D-h, also
deferred — `entries` is the hottest structure in the walk and wants a
measurement, not an assumption). But a `.desc` corpus with more than
65 535 message types is not a programming error in protolens; it is
input. Aborting the process in the middle of a background scoring thread
is the wrong response.

## Goals

- **G1**: A field number the wire format places outside the legal range
  never resolves to a schema field.
- **G2**: The RANGE arm decodes negative varints the way the INT32 arm
  already does, so the canonical encoding of a negative enum is treated
  no worse than its non-canonical form.
- **G3**: A value outside a declared enum/bool range is penalized, not
  vetoed, by default — because with no syntax bit in the graph it is
  merely unlikely, not impossible.
- **G4**: A malformed or oversized scoring-graph file is a load-time
  `Err`, never UB and never a panic from inside the walk.

## Non-goals

- **N1**: A syntax bit in the compiled-graph format (decision D-g). No
  change to `serial.rs`, no format version bump, no rebuild of existing
  `.rkyv` sidecars.
- **N2**: Widening `ActiveEntry::entries` beyond `u16` (decision D-h).
  The 65 535-root ceiling stands; G4 only changes how it is reported.
- **N3**: Removing `ScoringOpts::strict_ranges`. The knob keeps working
  and keeps meaning what it says; only its default changes.
- **N4**: Any change to the score comparator or to `EntryScore::score`'s
  weights.
- **N5**: Bounds arithmetic and recursion depth. That is spec 0171.
- **N6**: Validating `from_static_bytes`'s payload with rkyv's checked
  `access`. That path reads a build-time `include_bytes!` blob and is
  trusted by construction; only the file-backed path takes untrusted
  input.

## Specification

### S1. Out-of-range field numbers are Unknown (C4)

In `score_message_multi`'s verdict loop (`walk.rs:834-848`), when
`tag.out_of_range` is set, every active entry gets `Verdict::Unknown`
without any call to `find_transition`:

```rust
verdicts.clear();
for ae in active.iter() {
    // A field number of 0 or >= 2^29 cannot be declared by any schema,
    // so there is nothing to look it up against — and narrowing it to
    // u32 for the lookup would alias it onto a real field (e.g. 2^32+1
    // onto field 1), awarding a match or a wire-type-mismatch veto on
    // the strength of a number the wire format forbids.
    let v = if tag.out_of_range {
        Verdict::Unknown
    } else {
        match find_transition(ws.graph, ae.state_id, field_number as u32) { .. }
    };
    verdicts.push((ae.state_id, v));
}
```

The existing `non_canonical += 1` at `walk.rs:823-825` is unchanged: the
tag is still penalized, it just no longer resolves. `field_number as u32`
in the remaining branch is now sound by construction, since
`!out_of_range` establishes `1 <= field_number < 2^29`.

### S2. RANGE decodes negatives like INT32 (C5)

The RANGE arm (`walk.rs:933-961`) is restructured to mirror the INT32
arm:

```rust
0 if ri != 0xFFFF => {
    // Mirrors the INT32 arm above. A negative enum/bool value is
    // sign-extended to 64 bits on the wire, so -1 arrives as
    // 0xFFFF_FFFF_FFFF_FFFF; the only genuinely impossible values are
    // those in the gap between "too big for u32" and "smallest
    // sign-extended i32", which are neither encoding of any 32-bit
    // number.
    if val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000u64 {
        do_veto = true;
    } else {
        if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
            // Negative value written in the non-canonical 5-byte form.
            for &e in &ae.entries { ws.scores[e as usize].non_canonical += 1; }
        }
        let signed = val as u32 as i32 as i64;
        if let Some(range) = ws.graph.ranges.get(ri as usize) {
            let (min, max) = (range.0.to_native() as i64, range.1.to_native() as i64);
            if signed < min || signed > max {
                if ws.strict_ranges { do_veto = true; }
                else { for &e in &ae.entries { ws.scores[e as usize].non_canonical += 1; } }
            }
        }
    }
}
```

`val as u32 as i32 as i64` is written with the explicit `as u32` step
because it now receives both encodings; it yields the same result as the
old `val as i32` for the four-byte form and the correct one for the
sign-extended form.

### S3. `strict_ranges` defaults to `false` (C6)

```rust
impl Default for ScoringOpts {
    fn default() -> Self {
        Self {
            // Vetoing on an out-of-range enum value requires knowing the
            // enum is *closed*, and the compiled graph carries no syntax
            // information (`serial.rs`'s NodeEntry is wire_type +
            // is_string + range_idx). Under proto3 every enum is open, so
            // an unknown value is legal, forward-compatible, and common —
            // vetoing eliminates the blob's own correct FQDN and hands the
            // win to an unrelated type. Penalizing instead costs the right
            // answer 20 points and still lets it win.
            //
            // The same reasoning covers bool, whose range is 0..=1 but
            // whose wire encoding accepts any nonzero varint as `true`.
            //
            // Revisit when the graph format carries syntax per enum node
            // (deferred decision D-g), at which point closed enums can
            // veto again and this default can go back to `true`.
            strict_ranges: false,
            expand_any: true,
        }
    }
}
```

The field's own doc comment (`walk.rs:166-167`) is corrected: it
currently advertises a `--no-strict-ranges` flag that no binary in this
workspace exposes.

### S4. `root_offset` is validated, and `from_raw_parts` goes away (C8)

`check_header` gains the bounds check that makes its return value
trustworthy:

```rust
let root_offset = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
if root_offset > bytes.len() {
    return Err(format!(
        "{label}: root offset {root_offset} past end of file ({} bytes)",
        bytes.len()
    ).into());
}
Ok(root_offset)
```

and `load_graph` stops fabricating a slice from a raw pointer. Safe
slicing establishes the bounds; the only remaining `unsafe` is the
lifetime extension, which is the one thing that genuinely cannot be
expressed safely and whose soundness argument (`_mmap` outlives `graph`
because `LoadedGraph` owns both) the existing comment already makes:

```rust
let payload = &mmap[root_offset..];
let graph: &'static ArchivedCompiledGraph = unsafe {
    // Safety: `payload` borrows `mmap`, which `LoadedGraph` keeps alive
    // for exactly as long as `graph`. The bounds are established by the
    // slice above, not asserted — `root_offset` is attacker-controlled
    // and was validated in `check_header`.
    let payload: &'static [u8] = std::mem::transmute::<&[u8], &'static [u8]>(payload);
    access::<ArchivedCompiledGraph, rkyv::rancor::Error>(payload)
        .map_err(|e| format!("{}: rkyv access failed: {e}", path.display()))?
};
```

### S5. The root-count ceiling is a load error (C10)

A new private helper in `load.rs`, called by both `load_graph` and
`from_static_bytes` after the graph is materialized:

```rust
/// `score_all` addresses candidates by `u16` index (`ActiveEntry::entries`),
/// so a graph with more roots than that cannot be scored. Rejecting at load
/// is what makes the walk's `debug_assert!` a genuine invariant rather than
/// a live abort in a background thread: a corpus with more than 65 535
/// message types is input, not a programming error.
fn check_root_count(graph: &ArchivedCompiledGraph, label: &str)
    -> Result<(), Box<dyn std::error::Error>>
```

`score_all`'s `assert!` becomes a `debug_assert!` carrying the same
message plus a pointer to `check_root_count` as the enforcing site.

## Test plan

All in `prototext-graph`, alongside the existing `score::tests`.

- **C4** `out_of_range_field_number_does_not_alias` — a message whose
  only field carries tag field number `2^32 + 1` with the same wire type
  as the graph's field 1. Assert the candidate is neither vetoed nor
  credited a match, that `unknowns` is 1, and that `non_canonical` is 1.
  Assert the same blob with field number `1` *does* credit a match, so
  the test would fail if the lookup were simply skipped for every field.
- **C5** `canonical_negative_enum_is_not_vetoed` — an enum field whose
  declared range includes `-1`, encoded as ten bytes. Assert the
  candidate scores a match and is not vetoed. Companion
  `negative_enum_outside_range_is_penalized_not_vetoed` with `-99` and a
  range of `0..=3`.
- **C5** the impossible varint gap still vetoes — `0x1_0000_0000` on a
  RANGE field is neither a u32 nor a sign-extended i32, so S2 must not
  have simply removed the check. Already covered by the existing
  `tc77_04_range_32bit_overflow_always_veto`, whose value sits inside the
  narrowed gap; its doc comment is updated to say so rather than a
  duplicate test being added.
- **C5** regression `truncated_negative_enum_still_costs_exactly_one_penalty`
  — the four-byte-truncated-negative case still scores exactly one
  `non_canonical`, now that the canonical form reaches the same path.
- **C6** the proving test for S3: an enum value outside the range under
  `ScoringOpts::default()` leaves the candidate alive with
  `non_canonical == 1`. This is the retargeted
  `tc05_enum_out_of_range_penalized_by_default` (see the regression note
  below) rather than a new test.
- **C8** `graph_with_out_of_range_root_offset_is_rejected` — write a
  temp file with a valid magic and version and a `root_offset` of
  `u64::MAX`; assert `load_graph` returns `Err` mentioning the offset,
  under both debug and release.
- **C10** covered by `tc_of1_entry_count_over_u16_max_is_a_load_error`,
  which already builds a real 65 536-root corpus (the drafting assumption
  that doing so would be disproportionate was wrong — the test predates
  this spec) and now asserts an `Err` from `load_graph` where it
  previously asserted a panic from `score_all`.
- Regression: `hopcroft_suite` and every existing `score`/`score_all`
  test pass unchanged, except the two that already opt into
  `strict_ranges: false` (which now match the default) and those that
  relied on the old vetoing default. Each of the latter was re-examined
  individually rather than mechanically updated:
  - `tc77_01_bool_range_veto`, `tc77_02_enum_range_veto_strict` and
    `mt06_enum_oor_vetoes_only_enum_entry` were written to exercise the
    range check firing (and, for MT-06, a veto's *isolation* to the entry
    owning the offending leaf). They opt into `strict_ranges: true`
    explicitly, keeping their subject intact and serving as the
    "the knob is demonstrably live" companions.
  - `tc05_enum_out_of_range_veto` was written to assert the default, and
    the default is what S3 changes. It is retargeted to the new default
    (`tc05_enum_out_of_range_penalized_by_default`) and doubles as C6's
    proving test.
  - `tc77_12_bool_vs_int32_discrimination` guards the discrimination
    benefit spec 0077 bought — that `bool` and `int32` no longer collapse
    into one VARINT leaf. The veto was only how that discrimination
    happened to be expressed; the assertion moves to the resulting score
    gap, which S3 preserves.
