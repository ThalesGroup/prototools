<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# `score_all` — flaws report

*last verified: 2026-07-25*

Findings from a fresh-eyes review of the multi-entry scoring walk
(`prototext-graph/src/score/`), the component that answers "which of
these 100,000 root message types best explains this blob?". Its design
is described in [schema-match.md](schema-match.md) and
[build-scoring-graph-impl-notes.md](build-scoring-graph-impl-notes.md).

`score_all` is on protolens's critical path twice over — once in
`resolve_root_winner_fqdn` (`protolens/src/decode.rs:188-199`) and again,
with a byte-identical comparator, in `override_pane::inferred_candidates`
(`protolens/src/tui/override_pane.rs:56-73`) — and it is the background
heat-cue sweep's entire cost. It also runs against **adversarial input by
construction**: the whole point is to score blobs whose schema is
unknown, which includes blobs that are not valid protobuf at all.

Ranked in three bands: **correctness bug** (wrong verdict, panic, hang,
or UB), **perf cliff** (correct but degrades superlinearly or blocks the
UI), and **minor / doc drift**.

Findings marked **[unverified]** come from the audit but were not
confirmed against source in this pass; they are recorded so they are not
lost, but must be re-checked before any code is written.

---

## Correctness bugs

Numbered in discovery order, not impact order. By impact the ranking is
**C8** (undefined behavior, and it is reachable from a file on disk),
then **C1/C3** (crashes on hostile input, on the path whose declared job
is to eat hostile input), then **C5/C6** (silently wrong answers, which
is worse than a crash for a tool whose output a human trusts).

### C1. `parse_group_blind` recurses without a depth cap

**Where:** `prototext-graph/src/score/walk.rs:421-429`, self-recursing at
`:459`.

**What happens.** The main walk *is* capped —
`MAX_SCORE_DEPTH = 1000` (`walk.rs:54`), checked on entry at `:770`,
vetoing all active entries and bailing to `buflen`. The blind group walk
is not:

```rust
fn parse_group_blind(buf: &[u8], mut pos: usize, expected_field: u64) -> Option<usize> {
    ...
        WT_START_GROUP => {
            pos = parse_group_blind(buf, pos, tag.field_number)?;   // :459
        }
```

There is no `depth` parameter to thread. `parse_group_blind` is reached
on the Unknown verdict and as the all-vetoed fallback — i.e. precisely
when the input does *not* match any schema, which is the common case for
the heat-cue sweep. A blob of *n* repeated START_GROUP tags costs *n*
stack frames. Each byte can open a group, so a 1 MB blob buys ~1 M
frames. Stack overflow is a `SIGSEGV`: no unwinding, no panic hook, no
message — the TUI just dies.

The doc comment on `MAX_SCORE_DEPTH` (`walk.rs:44-53`) already
acknowledges this class of risk in the abstract ("tens of thousands of
levels for an ordinary-sized document — a legitimate (if unlikely)
stack-overflow risk on its own") but the cap it introduces does not reach
this function.

**Proposed correction.** Thread `depth: usize` through
`parse_group_blind` and return `None` past `MAX_SCORE_DEPTH`, matching
the main walk. `None` already means "structural error, stop here", so
every call site handles it. Note the loop guard at `:423` is
`if pos == buflen` where `>=` is the safer form — see C3, which can drive
`pos` past `buflen`.

### C2. `MAX_SCORE_DEPTH = 1000` is uncalibrated **[unverified]**

**Where:** `prototext-graph/src/score/walk.rs:54`.

The constant is justified by "far beyond any legitimate schema's nesting
depth while staying comfortably inside any thread's stack budget", but
"comfortably" is asserted, not measured. `walk_message`'s frame carries a
`Vec<ActiveEntry>` by value plus several locals; if the frame is ~2 KB,
1000 frames is 2 MB, which is the *entire* default stack of a spawned
thread (`std` default: 2 MiB) — and the heat-cue sweep runs on a spawned
thread, not the main one.

**Proposed correction.** Measure the frame with a stack-address delta in
a `#[test]`, then either lower the constant or set an explicit
`stack_size` on the sweep thread and assert the relationship between the
two in a test. Do not leave the safety margin implicit.

### C3. Length-prefix arithmetic overflows before the bounds check

**Where:** `prototext-graph/src/score/walk.rs:453`, `:669`, `:734`,
`:1031` — four independent copies of the same shape.

**What happens.** Each site reads a length as a `u64` varint, casts to
`usize`, and guards with an addition:

```rust
let length = lr.value as usize;
if pos + length > buflen {          // :1031
    veto_all(&mut active, ws, "LEN body extends past end of buffer");
    return buflen;
}
let payload = &buf[pos..pos + length];   // :1035
```

A varint may legitimately encode up to `u64::MAX`. In release builds —
and this project builds release exclusively — `pos + length` wraps. With
`length = u64::MAX`, `pos + length == pos - 1`, which is `<= buflen`, so
the guard *passes*. Execution then reaches `&buf[pos..pos + length]`,
whose `start > end` panics with `slice index starts at N but ends at M`.

The three sibling sites (`:453` in `parse_group_blind`, `:669` and `:734`
in the Any `type_url` / `value` extractors) have the same guard and
follow it with `pos += len` — there the wrap does not panic immediately
but corrupts `pos`, which then defeats the `pos == buflen` loop guards
(C1) and spins or reads garbage.

This is the same defect as C1 in the prototext-core decode report
(`docs/prototext/decode-flaws.md`) — the two crates made the identical
mistake independently, which is the strongest possible argument for a
shared helper.

**Proposed correction.** The overflow-free form, at all four sites:

```rust
if length > buflen - pos { ... }
```

`pos <= buflen` is an invariant of every one of these loops, so
`buflen - pos` cannot underflow. Better still, hoist it:

```rust
/// Bounds-check a LEN payload without overflowing. `len` is attacker-
/// controlled up to `u64::MAX`, so the natural `pos + len > buflen`
/// wraps in release builds and *passes*.
fn len_fits(len: u64, pos: usize, buflen: usize) -> bool {
    len <= (buflen - pos) as u64
}
```

### C4. Out-of-range field numbers are counted, then truncated into a false match

> **Resolved (2026-07-25).** Spec 0172 S1. `score_message_multi`'s verdict
> loop now short-circuits to `Verdict::Unknown` whenever `tag.out_of_range`
> is set, so `find_transition` is never reached with a number the wire
> format forbids. The `non_canonical` charge is unchanged, and
> `field_number as u32` in the surviving branch is sound by construction.

**Where:** `prototext-graph/src/score/walk.rs:817-828` and `:836`.

**What happens.** `parse_wiretag` correctly detects illegal field
numbers:

```rust
let field_number = raw >> 3;
let oor = field_number == 0 || field_number >= (1 << 29);   // :403
```

But the walk only *scores* the flag — it does not skip the field:

```rust
if tag.overhang > 0 || tag.out_of_range {          // :817
    for ae in &active {
        for &e in &ae.entries {
            ...
            if tag.out_of_range {
                ws.scores[e as usize].non_canonical += 1;
            }
```

Control falls through to the schema lookup, which truncates:

```rust
let v = match find_transition(ws.graph, ae.state_id, field_number as u32) {   // :836
```

A field number of `2^32 + 1` is `>= 2^29`, so it is flagged
non-canonical (`-20`), and then looked up as field **1** — where it may
find a transition and score a `match` (`+1`). A crafted blob can
therefore steer scoring toward an arbitrary entry by encoding field
numbers that alias onto that entry's low field numbers modulo `2^32`.

**Proposed correction.** An out-of-range field number is not a field.
Treat it as the structural error it is — veto, or at minimum skip the
lookup:

```rust
if tag.out_of_range {
    // A field number of 0 or >= 2^29 is not representable in protobuf.
    // Falling through would truncate it into `u32` and can alias onto a
    // real, low field number, scoring a spurious match.
    veto_all(&mut active, ws, "field number out of protobuf range");
    return buflen;
}
```

### C5. A canonically-encoded negative enum is *always* vetoed

> **Resolved (2026-07-25).** Spec 0172 S2. The RANGE arm now mirrors the
> INT32 arm: it vetoes only the genuinely impossible gap
> `0xFFFF_FFFF < val < 0xFFFF_FFFF_8000_0000` and decodes everything else
> through `val as u32 as i32 as i64`, so the canonical ten-byte negative
> and its four-byte truncation are finally treated the same way round.

**Where:** `prototext-graph/src/score/walk.rs:933-961`.

**What happens.** Negative `int32`/enum values are wire-encoded
sign-extended to a full 10-byte varint, i.e. the decoded `u64` lands in
`0xFFFF_FFFF_8000_0000..=0xFFFF_FFFF_FFFF_FFFF`. The `RANGE` arm rejects
that before it can be interpreted:

```rust
0 if ri != 0xFFFF => {
    // RANGE (bool / enum)
    if val >= (1u64 << 32) {
        do_veto = true;                          // :935 — unconditional
    } else {
        if (0x8000_0000u64..=0xFFFF_FFFFu64).contains(&val) {
            ... non_canonical += 1;              // :938
        }
        ...
        let signed = val as i32 as i64;          // :949
        if signed < min || signed > max {
            if ws.strict_ranges { do_veto = true; }
            else { ... non_canonical += 1; }
        }
```

Two things are wrong at once, and they compound:

1. The veto at `:935` **ignores `strict_ranges`**, unlike every other
   range rejection in the same block (`:951`). There is no way to turn it
   off.
2. Line `:949` (`val as i32 as i64`) exists specifically to sign-extend —
   but line `:935` makes it **unreachable for genuinely negative
   values**. The only path that reaches `:949` with a negative `signed`
   is the *five*-byte truncated encoding (`0x8000_0000..=0xFFFF_FFFF`),
   which `:938` flags as non-canonical.

So the treatment is exactly inverted: the **non-canonical** encoding of a
negative enum is tolerated and merely penalized, while the
**canonical, protoc-emitted** encoding is fatal. Any schema with a
negative enumerator (`UNKNOWN = -1` is a common idiom) is unmatchable
against real data.

**Proposed correction.** Sign-extend first, range-check second, and route
the result through `strict_ranges` like everything else:

```rust
0 if ri != 0xFFFF => {
    // Negative enum/int32 values are sign-extended to 10 bytes on the
    // wire, so `val` is in 0xFFFF_FFFF_8000_0000..=u64::MAX. Reject only
    // the genuinely unrepresentable gap in between; everything else
    // sign-extends and goes to the declared range check.
    let signed: i64 = match val {
        v if v <= 0x7FFF_FFFF => v as i64,
        v if (0x8000_0000..=0xFFFF_FFFF).contains(&v) => {
            non_canonical += 1;             // 5-byte truncated negative
            v as i32 as i64
        }
        v if v >= 0xFFFF_FFFF_8000_0000 => v as i32 as i64,   // canonical
        _ => { do_veto = true; 0 }          // unrepresentable gap
    };
```

Note the `INT32` arm at `:918` already gets this right — it vetoes only
the gap `val > 0xFFFF_FFFF && val < 0xFFFF_FFFF_8000_0000`. The `RANGE`
arm should mirror it.

### C6. Enum range checking has no notion of proto3 open enums

> **Resolved (2026-07-26).** Spec 0176. An **open** enum now emits
> `type: int32` and therefore carries no range at all, so there is nothing left
> for a range check to be strict about. A **closed** enum keeps its range and
> keeps its full discriminating power, so the precision loss noted in the
> interim below is also gone. No compiled-graph format change and no version
> bump: D-g is *answered*, not implemented. The "record syntax as one bit per
> enum" proposal below was the wrong shape — an open enum does not have a range
> that needs qualifying, it has **no range**, and `int32` states exactly that.
>
> **The 2026-07-25 interim was incomplete, and on the path that matters most.**
> Spec 0172 S3 flipped `ScoringOpts::default()`'s `strict_ranges` to `false`,
> which is what `protolens` reads — but the `prototext` CLI never consults that
> default. All three of its scoring entry points compute
> `strict_ranges: !relax_ranges` (`prototext/src/run.rs:423`, `:500`, `:532`)
> from a bare `clap` boolean, so **the CLI's default is strict** and C6 stayed
> fully live on the primary user-facing path for a day. This is the general
> hazard: a `Default` impl is not the shipped behavior when a CLI builds the
> struct field by field. Spec 0176 closes C6 at the source instead, which is
> immune to it.

**Where:** `prototext-graph/src/score/walk.rs:943-959`, and the absence
is repo-wide: grepping `prototext-graph/src` for
`syntax|proto3|open_enum|closed` returns **zero matches**.

**What happens.** `strict_ranges` defaults to `true`
(`walk.rs:178`), and the compiled graph stores one flat `(min, max)` pair
per enum with no record of the declaring file's syntax. In proto3, enums
are *open*: a value outside the declared set is legal, is preserved on
round-trip, and is exactly what a newer sender emits to an older reader.
Forward compatibility is the feature.

`score_all` vetoes it. The consequence is worse than a lost point: veto
is absorbing, so a single unknown enum value **eliminates the correct
FQDN entirely** and hands the win to some structurally-similar
alternative. This is a silently wrong answer on ordinary, valid,
non-adversarial input — the failure mode most damaging to a tool whose
job is inference.

**Proposed correction.** Two steps, in order:

1. Record syntax at graph-build time — one bit per enum in the compiled
   graph, set from the declaring `FileDescriptorProto`'s `syntax` field.
   This is a format change, so it belongs with the next graph version
   bump.
2. At scoring time, an out-of-range value on an *open* enum is
   `non_canonical += 1` (a soft signal — it is still weak evidence the
   guess is wrong), never `do_veto`. Only closed (proto2) enums may veto,
   and only under `strict_ranges`.

Until step 1 lands, the conservative interim is to demote the
out-of-range enum veto to `non_canonical` unconditionally. Losing the
discriminating power of a proto2 enum veto costs precision; keeping it
costs correctness.

**Scheduling.** Step 1 was **deferred** on 2026-07-25 as decision D-g in
[protolens/rendering-worklist.md](protolens/rendering-worklist.md), then
**dropped** on 2026-07-26: spec 0176 obtains the same outcome in `reproto`
alone, so the graph format was never opened. See C12 for the part of the
question that genuinely remains.

### C7. Packed vs. unpacked repeated fields **[confirmed 2026-07-25]**

> **Resolved (2026-07-26).** Spec 0175. Both encodings now match, in both
> directions, with no compiled-graph format change and no version bump — the
> premise below that "it needs the graph to know a leaf is a repeated scalar
> (which today it does not)" was wrong on the second half. `label` has been on
> every `TransitionEntry` since spec 0045, and spec 0173 already routed it into
> the verdict loop as `tr.label`. The only genuinely missing piece was the
> *element type*, which `reproto` was discarding — collapsing all seven packable
> types to `LEN_PACKED` — in exchange for `is_packed`, a bit that carries no
> information a scorer may act on precisely because both encodings are always
> legal. Deleting the collapse gave back the element type; `Verdict::FoundPacked`
> reads the run and validates it, which is where the discriminating power lost to
> the collapse comes back.

The audit reports that a repeated scalar field encoded in the form the
schema does not name (packed where the graph expects unpacked, or the
reverse) is treated as a wire-type mismatch and vetoed. Both encodings
are legal for any repeated scalar in both proto2 and proto3, and readers
are required to accept both.

**Confirmed.** A repeated scalar's leaf carries the *element's* wire type
— `graph.rs:19-28` assigns INT32 `wire_type=9`, UINT32 `8`, UINT64 `0`,
and `node_wire_type` (`walk.rs:539-542`) maps the internal 8/9
discriminants back to protobuf wire type 0. Nothing anywhere records that
the field is repeated and therefore packable. A packed encoding arrives
as wire type 2, so `walk.rs:865-869` returns `Verdict::Mismatch` and
`:882-892` vetoes the candidate outright.

This is the same class as C6 — an absorbing veto on valid input — but not
the same fix: it lives in the verdict loop rather than the RANGE arm, and
it needs the graph to know a leaf is a repeated scalar (which today it
does not). Spec 0172 deliberately left it alone; it wants its own spec.

### C8. `root_offset` is used unvalidated in a `from_raw_parts`

> **Resolved (2026-07-25).** Spec 0172 S4. `check_header` now rejects a
> `root_offset` past the end of the file, and `load_graph` slices
> (`&mmap[root_offset..]`) instead of fabricating the slice from a raw
> pointer — the bounds are established rather than asserted. The only
> remaining `unsafe` on that path is the lifetime extension, whose
> soundness rests on `LoadedGraph` owning both the mapping and the view.

**Where:** `prototext-graph/src/score/load.rs:34-89`.

**What happens.** The `.rkyv` sidecar's 24-byte header is validated for
magic and version, but the offset it carries is returned raw:

```rust
fn check_header(bytes: &[u8], label: &str) -> Result<usize, Box<dyn Error>> {
    if bytes.len() < 24 { return Err(...) }
    if &bytes[0..8] != MAGIC { return Err(...) }
    let version = u32::from_le_bytes(bytes[8..12].try_into()?);
    if version != 2 { return Err(...) }
    let root_offset = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
    Ok(root_offset)          // no bounds check
}
```

and then fed straight into pointer arithmetic:

```rust
let bytes: &'static [u8] =
    std::slice::from_raw_parts(mmap.as_ptr().add(root_offset), mmap.len() - root_offset);
```

If `root_offset > mmap.len()`, `.add()` produces a pointer outside the
allocation — **undefined behavior by itself**, before any read — and
`mmap.len() - root_offset` underflows to a near-`usize::MAX` length. The
resulting slice covers most of the address space and `access::<...>()`
reads from it.

The trigger is a truncated or corrupt `.rkyv` file, not necessarily a
malicious one: the magic and version occupy the first 12 bytes, so a file
truncated at 24 bytes passes every existing check.

Note the asymmetry: the sibling `from_static_bytes` path (`:61`) does
`&bytes[root_offset..]`, which panics cleanly. Only the mmap path is
unsound — and the mmap path is the one protolens uses.

**Proposed correction.** Validate inside `check_header`, so both callers
inherit the check and it cannot be forgotten at a future third call site:

```rust
if root_offset > bytes.len() {
    return Err(format!(
        "{label}: root_offset {root_offset} exceeds file length {}",
        bytes.len()
    ).into());
}
```

Also require enough room for the archived root itself, not merely
`<= len`, and document on the `unsafe` block *which* precondition
`check_header` is discharging — right now the `unsafe` has no safety
comment tying it to a validated invariant, which is why the gap was
invisible.

### C9. The loaded graph is handed out as `&'static` **[unverified]**

Corroborates the existing A5 / S4(1) finding in the protolens rendering
review: the `'static` lifetime is fabricated from an mmap whose lifetime
is a struct field, and the 2026-07-25 teardown segfault
(commit `60a1673`) was exactly this class of bug. Cross-reference rather
than duplicate; the fix belongs to that campaign.

### C10. Root count is enforced with `assert!`

> **Resolved (2026-07-25).** Spec 0172 S5. `load::check_root_count` rejects
> an oversized graph at load time on both the mmap and the `include_bytes!`
> path, and `score_all`'s guard is now a `debug_assert!` naming that site.
> The 65 535-root ceiling itself stands — widening `ActiveEntry::entries`
> is deferred decision D-h.

**Where:** `prototext-graph/src/score/walk.rs:187-191`.

```rust
assert!(
    graph.roots.len() <= u16::MAX as usize,
    "entry count {} exceeds u16::MAX",
    graph.roots.len()
);
```

`u16` is an internal representation choice (`ActiveEntry::entries` holds
`u16` indices). A graph built from more than 65,535 roots is a
*supported-in-principle* input — [schema-match.md](schema-match.md)
targets "100,000+ FDPs" — that the current encoding cannot represent.
Aborting the whole TUI with a panic is the wrong report for "this file is
too big for this build".

**Proposed correction.** Move the check to load time
(`load.rs::check_header`, where the root count is already available) and
return the existing `Result`. The panic then becomes an error message the
TUI can show. Separately: the 65,535 ceiling contradicts the design
document's stated scale target, so either the design doc or the index
width is wrong — resolve that explicitly rather than leaving an `assert!`
to discover it.

**Scheduling (2026-07-25).** The `assert!`→`Err` conversion is in scope
and is correct whichever way the ceiling is resolved. **Resolving the
ceiling is deferred** — decision D-h in
[protolens/rendering-worklist.md](protolens/rendering-worklist.md), which
also notes that widening `ActiveEntry::entries` to `u32` is not free: it
is the hottest structure in the walk, so it wants a measurement rather
than an assumption.

### C11. HashMap iteration order **[unverified]**

The audit reports at least one place where scoring results depend on
`HashMap` iteration order, which is randomized per process by
`RandomState`. If confirmed, this means two runs of protolens on the same
blob can rank tied candidates differently — a reproducibility bug that
would also make any regression test on candidate ordering flaky. Verify,
and if real, switch the affected map to `BTreeMap` or sort before
consuming.

### C12. The two surviving range vetoes are not vetoes of the impossible **[open]**

*Raised 2026-07-26 by spec 0176, which deliberately does not fix it.*

**Where:** `prototext-graph/src/score/walk.rs`, the `Range` arm of
`check_varint_value`, gated on `ScoringOpts::strict_ranges`.

After spec 0176 the `Range` leaf reaches exactly two kinds of field: `bool`,
with range `(0, 1)`, and a **closed** enum. Under `strict_ranges` an
out-of-range value on either vetoes. Neither value is impossible on the wire:

- A `bool` is `value != 0` in every generated parser, so any nonzero varint
  is a legal `bool`. `2` parses to `true`.
- A closed enum moves an unrecognized number to the unknown-field set rather
  than failing the parse (proto2 semantics), so the message still parses.

By the governing principle below — veto only for what the wire format makes
impossible — both should be penalties, not vetoes. They remain vetoes because
they are strong evidence and because `--relax-ranges` turns them off, so
nothing is trapped. The `non_canonical` **penalty** on the same values is
separately correct and is not part of this entry: see the posture note below.

Note the asymmetry this leaves. `--relax-ranges` is the only escape, and it
is coarse: it disables the check for bool and closed enum together. If this is
ever revisited, the shape to prefer is demoting both to `non_canonical`
unconditionally and deleting the knob, rather than adding a second knob.

---

## Perf cliffs

### P1. The verdict table is a `Vec` scanned linearly, once per active entry, per wire tag

*Fixed by spec 0173 S1, 2026-07-26 — the verdict moved onto `ActiveEntry`.*

**Where:** `prototext-graph/src/score/walk.rs:834-882`.

**What happens.** For each wire tag, the walk builds a verdict per active
*state*, then re-finds it per active *entry group* by linear scan — twice
over:

```rust
verdicts.clear();
for ae in active.iter() {
    let v = match find_transition(ws.graph, ae.state_id, field_number as u32) { ... };
    verdicts.push((ae.state_id, v));
}

for ae in active.iter_mut() {
    let v = verdicts.iter().find(|(sid, _)| *sid == ae.state_id)...;   // O(A)
    ...
}
...
let verdict_for = |sid: u32| -> Verdict {
    verdicts.iter().find(|(s, _)| *s == sid)...                        // O(A), again
};
```

With *A* active entry-groups the cost is **O(A²) per wire tag**, and
`verdict_for` is called again inside the LEN handler (`:1041`), so the
constant is at least 2. *A* is large exactly when scoring is hardest —
early in the walk, before vetoes have pruned, every root is active. This
is the leading explanation for the measured 533 ms sweep.

The irony is that `verdicts` is *built* by iterating `active` in order,
so the *i*-th verdict already corresponds to the *i*-th `ActiveEntry`.
The lookup is reconstructing information the loop just destroyed.

**Proposed correction.** Index positionally — `verdicts[i]` for
`active[i]` — which is O(1) and needs no key at all:

```rust
// `verdicts` is built by iterating `active` in order, so index i is
// active[i]'s verdict. Keying by state_id and re-scanning was O(A²) per
// wire tag, which dominates the sweep on graphs with many live roots.
for (ae, &(_, v)) in active.iter_mut().zip(verdicts.iter()) { ... }
```

The second consumer (`verdict_for`, called from the LEN arm) needs the
same treatment: pass the index, not the `state_id`. If a keyed lookup
turns out to be genuinely necessary somewhere, the fallback is a
`HashMap<u32, Verdict>` reused across tags (cleared, not reallocated) —
but positional indexing should make that unnecessary.

**Measure before and after.** This is the one change in this report with
a large enough expected effect to justify a dedicated benchmark on
`googleapis.desc`.

### P2. Per-state side tables **[unverified]**

Reported as allocated per walk rather than reused. Same shape as P3;
verify together.

### P3. Per-frame allocations in the walk **[unverified]**

`walk_message` allocates `Vec`s (`verdicts`, `child_pairs` at `:1038`,
the regrouped `active`) on every recursion. `child_pairs` in particular
is `Vec::new()`'d inside the LEN arm, i.e. once per LEN field in the
entire blob. A scratch buffer threaded through `WalkState` and cleared
per use would remove the whole class. Verify the actual allocation count
before optimizing — this is a plausible-but-unmeasured claim.

### P4. `EntryScore.fqdn` copies every root FQDN on every call

*Fixed by spec 0173 S3, 2026-07-26 — `pub fqdn: &'g str`.*

**Where:** `prototext-graph/src/score/walk.rs:60` (the field) and `:197`
(the allocation).

```rust
pub struct EntryScore {
    pub fqdn: String,
    ...
}
...
.map(|r| EntryScore {
    fqdn: r.fqdn.as_str().to_owned(),      // :197
    ...
})
```

One heap allocation and one memcpy **per root, per `score_all` call**.
With the 100,000-FDP target in [schema-match.md](schema-match.md) that is
100,000 allocations before a single byte of the blob is examined — and
protolens calls `score_all` more than once per user action (see the
duplicated sweep in `resolve_root_winner_fqdn` /
`inferred_candidates`). It also defeats the entire premise of the rkyv
zero-copy load: the FQDN is already in the mmap, laid out contiguously,
and never mutated.

**Proposed correction.** Borrow from the graph:

```rust
pub struct EntryScore<'g> {
    /// Borrowed from the archived graph — the point of the rkyv mmap is
    /// that these strings need never be copied.
    pub fqdn: &'g str,
    ...
}
```

This is a signature change with a blast radius (`score_all`'s return
type, both protolens call sites, `reproto`'s), so it is a deliberate
refactor rather than a local fix. If the lifetime proves awkward at the
protolens boundary — where the winner's FQDN outlives the sweep — clone
*the winner only*, at the one place that needs ownership, instead of all
100,000 up front.

### P5. The veto reason is formatted eagerly, inside the per-entry loop

*Fixed by spec 0173 S2, 2026-07-26 — `set_vetoed` takes a closure.*

**Where:** `prototext-graph/src/score/walk.rs:860-862`.

```rust
for &e in &ae.entries {
    ws.set_vetoed(e, &format!("wire-type mismatch on field {field_number} (wire_type={wire_type})"));
}
```

The `format!` is *inside* the `for &e` loop, so an entry group of *k*
members builds *k* identical strings. Worse, vetoing is the **common**
outcome — the walk's whole job is to eliminate 99,999 of 100,000
candidates, so this is the hot path, not the error path.

**Proposed correction.** Two independent wins, take both:

1. Hoist the `format!` out of the inner loop — the string does not depend
   on `e`.
2. Make the reason lazy. `set_vetoed` takes `&str`; if the reason is only
   ever read for diagnostics, take `impl FnOnce() -> String` or a
   `&'static str` discriminant plus the two numbers, and format only when
   something actually renders it.

Check whether the reason is read at all on the protolens path before
investing in (2) — if it is diagnostics-only, (2) removes 100% of the
cost.

### P6. No pruning of hopeless candidates **[unverified]**

Reported: entries whose score has fallen far enough that no remaining
input could recover a win are still carried through the walk. Since
`score()` is monotonically non-increasing in the penalty terms
(`walk.rs:69-74`), a branch-and-bound cut is available in principle.
Verify the claim, then weigh it against P1 — if P1 is the real cost,
pruning may be unnecessary complexity.

### P7. **[unverified]**

Recorded in the audit; not yet re-derived. Re-check before acting.

---

## Cross-cutting observations

**The two crates made the same overflow mistake independently.** C3 here
and C1 in [prototext/decode-flaws.md](prototext/decode-flaws.md) are the
same defect — `pos + len > buflen` on an attacker-controlled `len` —
written twice, in two crates, by the same hands. Both crates also
independently wrote an uncapped blind group-skipping recursion
(C1 here, C2 there). These are not coincidences; they are what happens
when the wire-format bounds-checking idiom lives in a code reviewer's
head rather than in a function. **The highest-leverage fix in either
report is a single shared, tested, documented helper module for
wire-format bounds arithmetic**, used by both.

**Veto is absorbing, and it is used too freely.** C5, C6 and C7 are all the
same failure: a *soft* signal — an encoding choice, a forward-compatible
unknown value — is treated as *proof* the candidate is wrong. Because veto
cannot be recovered from, one such signal anywhere in a large blob eliminates
the correct answer. The scoring model already has a graded penalty
(`non_canonical`, `-20`) designed for exactly this. The rule worth stating
explicitly in the design doc: **veto only for what the wire format makes
impossible; score everything that is merely unlikely.** (All three are fixed
as of 2026-07-26: specs 0172, 0175 and 0176.)

**That principle bounds veto, not penalty (confirmed 2026-07-26).** The
scoring heuristic **deliberately penalizes suspicious serialization as much as
erroneous serialization** — that is a voluntary posture, and it is the entire
reason `non_canonical` exists. Legal-but-no-conformant-writer-emits-it is
exactly what it is for: a 5-byte negative `int32`, a varint with overhang, a
`bool` of `2`, a zero-length packed run. Do not "fix" a `non_canonical`
penalty by citing the principle above; only vetoes are in question. The two
vetoes the principle *does* still indict are C12.

The test for "suspicious" is: **would a conformant writer of the schema under
test produce it?** If no, penalize. If yes routinely, it must cost nothing —
which is what makes an out-of-set value on an *open* enum unpenalizable (it is
the designed forward-compatibility mechanism, C6), and likewise the expanded
encoding of a default-packed proto3 repeated scalar, or the packed encoding of
a `[packed=false]` one (writers routinely do either, C7). Penalizing those
charges the *correct* schema for ordinary valid traffic, which is C6's failure
mode in penalty form.

**Adversarial input is the design point, not an edge case.** Every
correctness bug here (C1, C3, C4, C8) is a crash or UB on malformed
input. `score_all` exists to be pointed at bytes of unknown provenance;
"the blob was malformed" is its normal operating condition, and it should
be fuzzed. A `cargo-fuzz` target taking arbitrary bytes into
`score_all` against a fixed small graph would have found C1, C3, and C4
in minutes.

---

## Checked and clean

- **The multi-entry walk really is a single traversal.** Spec 0048's
  central claim holds: candidates are carried in parallel through one
  pass (`ActiveEntry` groups those sharing a DFA state), not re-parsed
  per candidate. The O(A²) of P1 is *within* one traversal, not a hidden
  second one.
- **`MAX_SCORE_DEPTH` is enforced on the main walk** (`:770`) and vetoes
  rather than panicking. C1 is a gap in coverage, not an absent design.
- **`parse_wiretag` rejects wire types > 5** (`:377`) before doing
  anything else, and treats them as end-of-buffer garbage rather than
  attempting a lookup.
- **`out_of_range` is detected correctly** (`:403`) — the flaw in C4 is
  in what the caller does with it, not in the predicate.
- **Header magic and version are checked** before any archived access
  (`load.rs:34-45`). C8 is one missing field in an otherwise present
  validation, which is why it is worth fixing rather than rewriting.
