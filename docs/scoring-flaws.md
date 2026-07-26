<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# `score_all` — flaws report

*last verified: 2026-07-26*

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

**No finding is `[unverified]` any more.** The six that were — C2, C9, P2,
P3, P6, P7 — were each checked against source on 2026-07-26 and now carry an
explicit verdict in their heading. The tally is worth keeping, because it is
the argument for auditing a report before acting on one:

| | |
|---|---|
| confirmed, and the numbers came out worse than reported | **P3** |
| confirmed open, but owned by another campaign | **C9** |
| partly confirmed — the cap holds everywhere shipped (3.6× at the tightest), but not in a debug build, and not for a library embedder | **C2** |
| subsumed by another finding | **P2** |
| rejected on a false premise | **P6** |
| unrecoverable; never had content | **P7** |

Only two of six survived as work to do, and one of those two was *already*
half-implemented before the audit began (C2's explicit `stack_size`). An
unverified finding is not a cheap finding: acting on P6 as written would have
produced an unsound cut.

The audit then made the same class of mistake it was correcting, twice, on
C2's stack margin: first measuring one of the two walkers that share the
constant and one of the two build profiles, then dividing one walker's
consumption by a stack the other walker runs on. **A verification pass needs
verifying too**, and the specific trap is a claim that sounds like a
measurement because it has a number attached.

---

## Correctness bugs

Numbered in discovery order, not impact order. By impact the ranking is
**C8** (undefined behavior, and it is reachable from a file on disk),
then **C1/C3** (crashes on hostile input, on the path whose declared job
is to eat hostile input), then **C5/C6** (silently wrong answers, which
is worse than a crash for a tool whose output a human trusts).

### C1. `parse_group_blind` recurses without a depth cap

> **Resolved by spec 0171 §S3.** Noticed while auditing C2, which cites the
> same constant. The fix is better than the correction proposed below: rather
> than threading a `depth` and capping it, `parse_group_blind`
> (`walk.rs:487`) is now **iterative**, matching group nesting with a
> `depth` counter instead of the call stack, so it cannot overflow and needs
> no cap at all. Guarded by `blind_group_walk_does_not_overflow_the_stack`
> (200 000 START_GROUP tags). One check was traded away — only the outermost
> closing field number is validated, since a `Vec<u64>` of open field numbers
> would put an allocation in a routine whose point is to have none; the
> reasoning is in the comment at `:479-485`.
>
> Two stale names below, both fixed by spec 0171: the constant is
> `MAX_WIRE_DEPTH` in `prototext-core/src/helpers/bounds.rs:69`, not
> `MAX_SCORE_DEPTH` in `walk.rs`, and the `pos == buflen` guard flagged at
> the end is safe as written because every length now goes through
> `payload_end`, which cannot drive `pos` past `buflen` (that is C3).

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

### C2. The recursion cap is uncalibrated **[measured 2026-07-26 — binding margin 3.6×, and negative in debug]**

**Where:** `prototext-core/src/helpers/bounds.rs:69` (`MAX_WIRE_DEPTH`),
consulted at `prototext-graph/src/score/walk.rs:940`.

The constant was justified by "far beyond any legitimate schema's nesting
depth while staying comfortably inside any thread's stack budget", but
"comfortably" was asserted, not measured. The worry was that
`score_message_multi`'s frame — which carries the active set by value plus
several locals — might be ~2 KB, making 1000 frames 2 MB: the *entire*
default stack of a spawned thread, and the heat-cue sweep runs on a spawned
thread, not the main one.

**Measured.** Bisecting an explicit `stack_size` against a nest of LEN
fields on a self-recursive schema, release build:

| frames | minimum stack that survives |
|---|---|
| 501 | 288 KiB |
| 1002 | 576 KiB |

≈ **590 bytes per frame** for `score_message_multi`, so the full cap costs
~576 KiB. `MAX_WIRE_DEPTH` is shared with `prototext-core`'s `render_message`
(spec 0171), which needs **~1408 KiB** for the same depth — 2.4× the scorer,
≈ 1.44 KiB per frame, and identical on both the schema'd path and the
schemaless one that makes two calls per level.

Those are the worst cases by construction, and that is the point of having a
cap: depth on the wire is bounded only by input length, so without
`MAX_WIRE_DEPTH` there is no finite figure to measure. A flat message costs one
frame; the numbers above are what a maximally nested one costs. Sizing against
them means every accepted input is covered.

**A consumption figure is not a margin, and the first two versions of this
verdict conflated them.** What a walker consumes is a property of the code;
what it has to spend is a property of the *thread it runs on*, and the two
walkers do not share threads. Per (walker, thread) pair:

| walker | thread | available | margin |
|---|---|---|---|
| scorer | `protolens`'s detached root-type thread (`tui/mod.rs:1680`, a plain `thread::spawn`) | 2 MiB | **3.6×** |
| scorer | `protolens`'s heat worker (explicit `stack_size`) | 16 MiB | 28× |
| renderer | main thread — `protolens`'s draw loop, and the `prototext` CLI | 8 MiB (`RLIMIT_STACK`) | **5.8×** |
| renderer | whatever thread Python calls `prototext-pyo3` from | `threading.stack_size()` — caller's choice | **unbounded** |

So the binding margin in the shipped binaries is **3.6×** — the scorer, on the
one call site that takes `std`'s default. The renderer, despite being the
heavier walker, has more room because it only ever runs on a main thread. The
genuine open exposure is the last row: `prototext-pyo3` is a library, a Python
caller may set any stack size it likes, and 1408 KiB is a lot to need from a
thread someone else sized.

**And in a debug build the margin is negative.** The same bisection puts the
scorer's frame at ≈ 4.8 KiB — 8× the release figure — so 1000 levels wants
~4.7 MiB and overflows a 2 MiB stack outright. This is why a debug
`cargo test` aborts in `prototext-core`'s `deeply_nested_len_*` tests, and it
means **C2's original estimate was correct for a build the first pass of this
audit did not measure**: "if the frame is ~2 KB, 1000 frames is 2 MB, which is
the entire default stack" understates debug rather than overstating release.
The repo builds release exclusively, so no shipped binary is at risk — but the
estimate was not the error; the single-configuration check was.

**Verdict: the cap holds in every shipped configuration, but it is not
obviously right.** 1000 levels is 10× protobuf's own reference default of 100,
buys nothing anyone needs, and leaves a library embedder no way to shrink its
appetite. Lowering it is a *behavior* change — the
cap is deliberately a constant because rendering must be a function of
`(bytes, schema)` alone — so it needs a spec and a decision, not a quiet edit.
Recorded here rather than acted on.

**Two things this audit found already done.** The entry's own proposed
correction asked for an explicit `stack_size` on the sweep thread — that
exists: `HEAT_WORKER_STACK_SIZE = 16 MiB`
(`protolens/src/tui/heat_worker.rs:49`). And the constant is no longer
`MAX_SCORE_DEPTH` in `walk.rs`: spec 0171 moved it to `prototext-core` so
both wire walkers refuse the same inputs, which is why grepping the cited
line finds nothing. The function is `score_message_multi`, not
`walk_message`.

**Closed by** recording the numbers where the claim lives — the doc comment
on `MAX_WIRE_DEPTH` now carries both walkers' per-frame figures, the
per-(walker, thread) margin table, and the release-only caveat — and by
`max_depth_walk_fits_in_a_default_thread_stack`
(`prototext-graph/src/score/tests.rs`), which walks `MAX_WIRE_DEPTH + 2`
levels on a thread pinned to `std`'s 2 MiB default. It asserts the match
count as well as survival, because a walk that stopped early would never
allocate the deep frames and would make the stack assertion vacuous. It is
`#[cfg(not(debug_assertions))]`: the contract is a release contract, and in
debug it would abort the test binary over a configuration nothing ships in.

**Generalizable lessons, and none of them is the obvious one.** This entry
took three passes, each wrong for a different reason: the first measured only
the scorer and published 3.5×; the second measured the renderer too and
published 1.45×; the third noticed that 1.45× divided the *renderer's* 1408 KiB
by `thread::spawn`'s 2 MiB — a stack the renderer never runs on. In order of
usefulness:

1. **A margin needs a named denominator.** "1.45× the default thread stack" was
   arithmetic on two numbers that never meet at runtime. Always write the pair:
   which walker, on which thread.
2. **A shared constant must be measured against every sharer.** The cap became
   `prototext-core`'s in spec 0171; measuring only the crate whose flaws report
   this is answered the wrong question.
3. **A margin measured in one build configuration is not a margin.** Release
   and debug differ 8× here, and the original estimate was right about debug.
   The prose flaw ("comfortably", naming no number) was real, but naming *one*
   number would have replaced an unreviewable claim with a falsely precise
   one.

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
alone, so the graph format was never opened. The part of the question that
genuinely remained — the two range vetoes left standing — became C12, and was
closed the same day by spec 0178.

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

### C9. The loaded graph is handed out as `&'static` **[fixed 2026-07-26]**

> **Resolved.** Spec
> [0180](specs/0180-own-the-scoring-graph-by-arc.md). `LoadedGraph::graph`
> is **private**, with a `graph(&self) -> &ArchivedCompiledGraph` accessor
> beside the existing `Deref`, so both ways out shorten the lifetime to
> `&self` and the `transmute` below is discharged by the module rather than
> asserted to callers. protolens holds `Arc<LoadedGraph>` and both of its
> background threads own a handle, so the never-joined detached thread at
> `mod.rs:1680` — the unmitigated instance this entry identified — is sound
> without being joined, which is what it wanted to be all along.
>
> The prediction below held: a compiler check replaced the field ordering,
> and `App`'s field-order comment is now a historical note. `load_graph`
> still returns a plain `LoadedGraph`, not an `Arc` — see the spec's N2.

**Where:** `prototext-graph/src/score/load.rs:138` fabricates the lifetime:

```rust
std::mem::transmute::<&[u8], &'static [u8]>(payload)
```

`LoadedGraph.graph` is then a `pub` `Copy` field, so the `Deref` impl at
`:27-31` — the thing that would otherwise tie a borrow to the owner — is
trivially bypassed by reading the field out. `protolens/src/tui/mod.rs:1660`
does exactly that (`let graph_ref = graph.graph;`) and moves the copy into a
`thread::spawn` at `:1680` that is **never joined**.

**Verified still open.** Commit `60a1673` fixed the *observable* 2026-07-25
teardown segfault, but only by ordering `App`'s fields so `heat_worker`
(`:738`) drops before `ctx` (`:742`). The `transmute` and the `pub` field are
untouched, so today's safety rests entirely on a field declaration order that
no compiler check and no test protects. The detached thread at `:1680` is a
second, unmitigated instance: it is never joined, and its comment claiming it
"holds only `'static`/`Arc`-owned data" is false — the `'static` there is the
fabricated one.

**Not fixed here, deliberately.** This is owned by **W8** in
[protolens/rendering-worklist.md](protolens/rendering-worklist.md), where the
A5 / S4(1) findings of the rendering review already scope it. Duplicating the
fix into the scoring report would put two descriptions of one lifetime
redesign in two documents. What this audit adds is only the confirmation that
W8 is *not* already discharged by `60a1673`, which the commit message could
easily be read as implying.

### C10. Root count is enforced with `assert!`

> **Resolved (2026-07-25).** Spec 0172 S5. `load::check_root_count` rejects
> an oversized graph at load time on both the mmap and the `include_bytes!`
> path, and `score_all`'s guard is now a `debug_assert!` naming that site.
> The 65 535-root ceiling itself stood until spec 0179 S1 (2026-07-26),
> which widened `ActiveEntry::entries` to `u32` — deferred decision D-h,
> now answered. The load-time check is *kept*, against `u32::MAX`, because
> `roots.len()` is a `usize` and a 64-bit target can still express more
> roots than the index addresses.

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

**Ceiling resolved (2026-07-26).** Spec 0179 S1. The measurement the
deferral asked for was made, and it says the width is not the variable
that matters — the *inline capacity* is. Holding capacity at 4, every
spill decision is bit-identical to the `u16` version, so the widening is
allocation-neutral; `SmallVec<[u32; 2]>` has the same `size_of` as
`[u16; 4]` and looks free for that reason, but measured **+21.9%
allocations**. The contradiction with `schema-match.md` was live rather
than theoretical: googleapis alone compiles to **49 255 roots**, 75% of
the old ceiling.

### C11. HashMap iteration order **[not a scoring bug; artifact half fixed 2026-07-26]**

The audit reported that scoring results depend on `HashMap` iteration order,
which is randomized per process by `RandomState`, and that two runs of
protolens on the same blob could therefore rank tied candidates differently.

**Measured, and the ranking claim is false.** The scoring walk contains no
`HashMap` at all — only the graph *builder* does. And all four consumers of
`score_all` apply the identical total order:

```rust
(false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(b.fqdn)),
```

`prototext/src/run.rs:194` (decode auto-infer), `:334` (list-schemas),
`protolens/src/decode.rs:194` (root winner), `protolens/src/override_pane.rs:62`
(candidate pane). FQDNs are unique, so the final tie-break is a total order and
the ranking is fully determined. There is no per-candidate cap or
first-match-wins anywhere in the walk that could make processing order
observable; every active entry accumulates its own score independently.

Verified end to end: `descriptor.proto` compiled four times in four separate
processes, then `list-schemas --top 15` run against each. All four outputs are
byte-identical, including the 8-way tie at score `-110`, which comes out in
FQDN order every time. Five consecutive runs against one DB are likewise
identical.

**What is real is narrower, and is not about scoring: the compiled artifacts
are not byte-reproducible.** The same four builds produced four *different*
`hopcroft.rkyv` files and four different `index.rkyv` files. Two independent
causes:

- **State-ID assignment.** `graph::build` numbers nodes by iterating
  `merged.states.keys()` (`graph.rs:198`), and Hopcroft's refinement loop
  allocates new block IDs while iterating `x_in_block`
  (`hopcroft.rs:231`), both `HashMap`s. Hopcroft is confluent — the coarsest
  partition is unique regardless of worklist order — so only the *numbering*
  moves, never the equivalence classes. That is exactly why the scores above
  are invariant.
- **`FdsIndex`.** Its four fields are `HashMap`s in an rkyv `Archive` struct
  (`fds_index.rs:22-40`), and rkyv serializes a `HashMap` in iteration order.
  All four are consumed only by `.get()` in `lazy_pool.rs:157-267`, never
  iterated, so again only the bytes move.

Both are invisible to behavior; what they cost is the ability to treat a
schema DB as content-addressable — you cannot verify one by digest, cache or
dedupe by digest, or diff two DBs to tell "the schema changed" from "someone
rebuilt it".

**Fixed** by `docs/specs/0177-reproducible-schema-db-artifacts.md`
(2026-07-26), and at no cost to the format: the `FdsIndex` half turned out not
to need `BTreeMap` at all, because `HashMap<K, V, S>` archives to
`ArchivedHashMap<K, V>` — the source hasher never reaches the archive, and
archived lookups use `FxHasher64` regardless. Fixing the source seed and
inserting in sorted key order makes the layout a function of the key set alone,
leaving the archived type, the reader, `VERSION` and lookup cost untouched, so
existing DBs stay readable.

Note that `docs/specs/0059-hopcroft-test-harness.md:175` is *not* an instance
of this. It requires two map-entry types to stay in distinct states "regardless
of HashMap iteration order", which is an assertion about the partition, not the
numbering — and the partition is invariant, so that assumption holds.

One lesson worth keeping: the graph half needed *three* sorts, not two. Sorting
`graph.rs:198` and `hopcroft.rs:231` left `hopcroft.rkyv` still varying, because
`LeafRegistry::range_sentinel` assigns each distinct range a `range_idx` in
first-seen order and the edge-build loop was walking `merged.states` unordered —
so the initial partition's leaf labels moved too. And a small schema does not
exercise any of this: a four-message input produced an identical
`hopcroft.rkyv` even before the fix, because Hopcroft never had to split a
block. The regression test therefore imports the WKTs.

### C12. The two surviving range vetoes are not vetoes of the impossible **[fixed 2026-07-26]**

*Raised 2026-07-26 by spec 0176, which deliberately does not fix it.*

> **Fixed** by [specs/0178-out-of-range-is-a-penalty-not-a-veto.md](specs/0178-out-of-range-is-a-penalty-not-a-veto.md).
> The out-of-range check is now a penalty in its own counter, `out_of_range`,
> weighted `-15`. `ScoringOpts::strict_ranges` and `--relax-ranges` are gone —
> the fix took the shape the note below recommended (demote both, delete the
> knob) rather than adding a second knob.

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

**What settled it.** The real cost of demoting a veto is not lost precision —
`non_canonical` already carried a `-20` weight, so the signal was never too
weak to rank on. It is lost **pruning**: a veto empties the candidate's
`ActiveEntry`, and when the active set empties the byte scan stops early. But
`ScoringOpts::default()` had `strict_ranges: false`, and that default is what
`protolens` reads — so the interactive, latency-sensitive consumer had *always*
run without this prune. Only the `prototext` CLI (which computed
`strict_ranges: !relax_ranges` from a bare flag) ever had it. A prune that the
hot path demonstrably does not need is not worth a wrong answer.

Spec 0178 also reordered the four other coefficients while it was there, having
found that `mismatches` — the *only* increment site of which is a declared
`required` field absent from the blob — was weighted `-10`, less than the
`-20` on a merely sloppy encoding. See its S1 for the evidence-split argument.

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

### P2. Per-state side tables **[not confirmed — no such table exists 2026-07-26]**

Reported as allocated per walk rather than reused. There is nothing left to
reuse: `WalkState` (`walk.rs:136-144`) holds `graph`, `scores`, `vetoed`,
`debug_fqdn` and `expand_any`, and none of them is keyed by state. The only
state-keyed side table the walk ever had was `verdicts`, which spec 0173 S1
deleted by moving the verdict onto `ActiveEntry` (that is P1). `vetoed` is
keyed by *entry*, not state, and is a flat bitset allocated exactly once per
`score_all` (`:157`) — already the reuse this entry was asking for.

P2 was recorded as "same shape as P3", and on the evidence it was a
restatement of P3 rather than a second finding. **Closed as subsumed;** the
live allocation work is entirely under P3.

### P3. Per-frame allocations in the walk **[quantified, then re-measured on a real corpus; largely fixed 2026-07-26]**

> **Resolved in part (2026-07-26).** Spec 0179 S2. `occurrences` became a
> `SmallVec<[(u32, u32); 2]>`, which removed **78.3%** of everything
> `score_all` allocates on googleapis (4 761 300 → 1 035 352). The scratch
> buffers proposed below are **declined**, on the measurement in "The
> synthetic bench inverts the profile" — they are 1.3% of the total, not the
> bulk of it. Read that subsection before acting on the numbers above it.

**Measured.** A counting global allocator around a single `score_all` over
the `benches/score` workload — an 886-byte blob, 64 nested-message records,
1024 roots — records **2828 allocations and 2.6 MB of allocation traffic**.
Scaling, holding one variable fixed:

| roots (A) | allocations per LEN record |
|---|---|
| 64 | 16 |
| 256 | 22 |
| 1024 | 28 |
| 4096 | 34 |

Exactly **+3 per doubling of A**, plus a fixed **~1 allocation per distinct
live state per frame**.

**The mechanism is worse than reported.** The `+3 per doubling` is the
signature of three `Vec`s grown from *zero* capacity, so each costs
O(log A) allocations rather than one:

- `child_pairs` (`walk.rs:1146`) — `Vec::new()` inside the LEN arm, one per
  LEN field in the blob, then pushed once per entry;
- `normal_pairs` (`:1273`) — the `partition` that separates `Any` candidates
  allocates *two* fresh `Vec`s and consumes `child_pairs`, which the original
  entry did not mention;
- `group_by_state`'s `collect` (`:190`) — a third copy of the same pairs,
  sorted in place.

The fixed per-frame term is `occurrences: Vec::new()` (`:204`): one
`ActiveEntry` per distinct live state, each of which allocates as soon as
`record_occurrence` pushes. At A = 4096 with a 13-byte blob that alone is
4144 allocations before the walk does anything interesting.

Two corrections to the entry as recorded: `verdicts` is **stale** (spec 0173
removed it, see P1), and the `child_pairs` line reference `:1038` predates
that commit.

**The synthetic bench inverts the profile (2026-07-26).** Everything above
was measured on `benches/score`, and that workload is not representative of
the thing it is standing in for. It gives every synthetic root a *unique*
field number, so Hopcroft can merge none of them: every `ActiveEntry` ends
up holding exactly one entry and `entries` never spills. And it holds A
large against only 64 records, which maximizes the O(log A) terms. The two
distortions push in the same direction, and the resulting picture — "the
three `Vec`s are the problem" — is the opposite of the truth.

Re-measured on `googleapis.desc` (49 255 roots), the split is:

| site | share of `score_all` allocations |
|---|---|
| `occurrences` | **81.6%** |
| `entries` spilling past its inline 4 | 17.1% |
| `child_pairs` + the partition + `group_by_state` | **1.3%** |

The last row is 1.3% because `group_by_state` is called **2 608 times in
total** across the whole corpus — the O(log A) term the bench makes look
dominant barely runs on real input.

**Correction, as implemented** (spec 0179 S2). `occurrences` becomes a
`SmallVec<[(u32, u32); 2]>` — inline capacity **2**, not the 4 proposed
here, and a `u32` count, not `u64`. Capacity 2 covers 98.15% of real frames;
capacity 4 covers 99.87% and measured *slower*, with higher peak RSS. Half
of all `ActiveEntry` record nothing at all and never allocated either way;
the other half were taking a 64-byte heap block (a `Vec`'s first capacity
for a 16-byte element) to hold one or two pairs. Result: allocations
4 761 300 → 1 035 352 (−78.3%), peak RSS −5.1%, and no score changes.

**Scratch buffers: declined.** Threading `child_pairs`, the partition and
`group_by_state`'s buffer through `WalkState` would target the 1.3% row.
That is real but small, and it trades the walk's current locality for
mutable shared state on its hottest path. Not worth it on this evidence; if
it is ever revisited it should be justified against a corpus measurement,
not against `benches/score`.

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

### P6. No pruning of hopeless candidates **[rejected 2026-07-26]**

Reported: entries whose score has fallen far enough that no remaining input
could recover a win are still carried through the walk, so a branch-and-bound
cut should be available.

**The premise is false.** The entry justifies the cut by "`score()` is
monotonically non-increasing in the penalty terms", but `score()`
(`walk.rs:82-88`) has a *positive* `matches` term, so a candidate's score
rises as well as falls and there is no monotone quantity to bound against.
Worse, "hopeless" is not decidable mid-walk: it means "cannot overtake the
eventual winner", and the eventual winner's score is itself still moving —
every rival's score can also fall. A sound cut would need a lower bound on
the final best score, which no single forward pass has.

**And it would not buy much.** Per-tag cost scales with *A*, the number of
distinct live **states** — one `find_transition` binary search each — not with
the number of entries. Dropping some entries from an `ActiveEntry` leaves the
per-tag work unchanged; only emptying a state entirely removes any. So the cut
would have to find every entry in a state simultaneously hopeless to pay at
all.

**And it would break the reports.** A pruned candidate stops accumulating, so
its counters become fiction. All four `score_all` consumers print those
counters — `score`, `--detailed-score`, `list-schemas`, and the decode
header — and two of them print them for candidates that did not win. Veto can
do this only because a vetoed candidate reports `vetoed: true` and no numbers
at all.

**Rejected**, not deferred: the mechanism is unsound, the payoff is bounded by
a quantity it does not reduce, and the cost is wrong output. The pruning the
walk actually has is veto, and its cost model is
[documented](#cross-cutting-observations) under the veto-demotion note.

### P7. **[closed as unrecoverable 2026-07-26]**

The entry has been an empty placeholder in every committed revision of this
file, back to `40c7bd8` where the report was first written — the finding was
lost before it was ever recorded, not since. There is nothing to re-derive.
**Closed.** If it mattered it will resurface as a fresh finding, which is a
better outcome than a permanent unanswerable placeholder.

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

**Veto is absorbing, and it was used too freely.** C5, C6, C7 and C12 are all
the same failure: a *soft* signal — an encoding choice, a forward-compatible
unknown value — is treated as *proof* the candidate is wrong. Because veto
cannot be recovered from, one such signal anywhere in a large blob eliminates
the correct answer. The scoring model already has graded penalties designed for
exactly this. The rule worth stating explicitly in the design doc: **veto only
for what the wire format makes impossible; score everything that is merely
unlikely.** (All four are fixed as of 2026-07-26: specs 0172, 0175, 0176 and
0178.)

**The real cost of a veto-to-penalty demotion is pruning, not precision
(established 2026-07-26 by spec 0178).** A veto empties the candidate's
`ActiveEntry`, and when the active set empties the byte scan returns early — so
veto is the only mechanism making scoring sublinear in blob size. Ranking power
is *not* at stake: the penalties are tens of times a single match, so a demoted
signal still dominates. Weigh a demotion against the prune it removes, and
check which consumers were relying on it — for C12 the answer was none, because
`ScoringOpts::default()` had the check off and that default is what `protolens`
reads.

**That principle bounds veto, not penalty (confirmed 2026-07-26).** The
scoring heuristic **deliberately penalizes suspicious serialization as much as
erroneous serialization** — that is a voluntary posture, and it is the entire
reason `non_canonical` and `out_of_range` exist. Legal-but-no-conformant-writer-
emits-it is exactly what they are for: a 5-byte negative `int32`, a varint with
overhang, a `bool` of `2`, a zero-length packed run. Do not "fix" such a
penalty by citing the principle above; only vetoes are in question. As of spec
0178 the principle indicts no surviving veto.

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
