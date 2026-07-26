<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# prototext-core decode and sinks — flaws report

*last verified: 2026-07-26*

Findings from a fresh-eyes review of `prototext-core`'s decode path —
`render_message` and its helpers, the `TextSink` / `IndexingTextSink` /
`ProbeSink` family, and the thread-local implicit-parameter mechanism
they share. The pipeline's role inside protolens is described in
[../protolens/design/rendering.md](../protolens/design/rendering.md);
this document is about the library, not the TUI.

This code is on protolens's critical path for every frame, every override
commit, and every heat-cue sweep, and it is the component that must
honor the project's core promise: **any byte range is always renderable,
and schema knowledge only ever improves the rendering.** That promise
means malformed input is the *normal* operating condition, not an edge
case.

Ranked in three bands: **correctness bug** (wrong output, panic, hang,
or crash), **perf cliff** (correct but degrades superlinearly or blocks
the UI), and **minor / doc drift**.

Numbering follows the audit that produced these findings, so gaps are
intentional — items not re-derived against source in this pass are listed
under [Pending re-derivation](#pending-re-derivation) rather than
renumbered away.

---

## Correctness bugs

**All four are now resolved.** Re-derived against source on 2026-07-26:

| | |
|---|---|
| **C1** | fixed — `payload_end` (spec 0171), regression-tested |
| **C2** | fixed — the recursion is gone entirely (spec 0171 §S3), and all three of its sub-defects with it |
| **C3** | fixed — depth cap (0171) + budget deleted rather than enabled (0174) |
| **C5** | dissolved — the shared counter it raced no longer exists (0174) |

Auditing them turned up one new, minor finding — **M1**, the depth cap
degrading LEN and `START_GROUP` inconsistently — which is written up under
[Minor / doc drift](#minor--doc-drift).

The ranking below is preserved as written for the record. Worth noting how
they closed: **three of the four were retired by one spec** (0171), and the
fourth by deleting the mechanism rather than fixing it (0174). Two of the
four were closed by *removing* code — the node budget and the recursive
group skip — which is the pattern to expect when a defect is in a mechanism
nobody needed.

By impact: **C3** was the worst — it is the only one whose failure is a
process-level `SIGSEGV` with no unwinding and no message, and it fires on
the 24.5 MB input class that is an explicit target. **C1** and **C2** are
crashes and hangs on hostile input. **C5** is the subtlest: it produces
plausible-looking *wrong output* with no error at all.

### C1. The LEN bounds check overflows before it can reject

> **Resolved by spec 0171.** The site is now
> `render_text/mod.rs:592-593`, and the guard is the subtraction this entry
> asked for — except it lives in a shared helper rather than being inlined:
> `payload_end(pos, length, buflen)` with `bytes_missing(pos, length, buflen)`
> supplying the `MISSING:` count. The cross-cutting recommendation at the
> foot of this document was taken; see the note there.
>
> Every LEN/I64/I32 length check on the decode path went through the same
> conversion, not just the one this entry names — `mod.rs:540`, `:592`,
> `:696`, plus `group_scan.rs:65/73/90`, `message_set_field.rs:104/140/148/151`
> and `any_field.rs:59/67/97`. The fixed-width `8`/`4` cases cannot actually
> wrap; they were converted anyway, because leaving the wrapping idiom in the
> file is how it comes back.
>
> Guarded by `len_prefix_near_u64_max_does_not_panic`
> (`render_text::tests`, `mod.rs:1102`), which asserts the output contains
> `TRUNCATED_BYTES` — i.e. that the *correct* rendering is produced, not
> merely that nothing panics.

**Where:** `prototext-core/src/serialize/render_text/mod.rs:538-555`.

**What happens.** The main decode loop reads a LEN prefix, casts it to
`usize`, and guards with an addition:

```rust
let length = lr.varint.unwrap() as usize;
if pos + length > buflen {
    let missing = (length - (buflen - pos)) as u64;
    let raw = &buf[pos..];
    sink.malformed(field_number, TagFacts { tag_ohb, tag_oor, len_ohb },
                   MalformedKind::TruncatedBytes { missing }, raw);
    return (buflen, None);
}
let data = &buf[pos..pos + length];
```

A varint may legitimately encode any `u64` up to `u64::MAX`. This project
builds release exclusively, where `pos + length` **wraps** rather than
panicking. With `length` near `u64::MAX`, `pos + length` wraps to a small
value that is `<= buflen`, so the guard passes — and execution reaches
`&buf[pos..pos + length]`, whose `start > end` panics with
`slice index starts at N but ends at M`.

The guard is not merely ineffective, it is inverted: the one input it was
written to catch is the one input it lets through. And the failure is a
panic in a library whose documented contract is that *no* byte sequence
is unrenderable — a `MalformedKind::TruncatedBytes` line is precisely the
right output here, and the code even knows how to produce it.

**Proposed correction.** Subtract instead of adding. `pos <= buflen` is a
loop invariant, so the subtraction cannot underflow:

```rust
// `length` is attacker-controlled up to u64::MAX. The natural
// `pos + length > buflen` wraps in release builds and *passes*, so the
// check must be phrased as a subtraction on the known-good side.
if length > buflen - pos {
```

See also [the same defect in `prototext-graph`](../scoring-flaws.md)
(C3 there, four sites) — two crates, same mistake, independently. The
cross-cutting note at the end of this document argues for a shared
helper.

### C2. `skip_group` recurses without a depth cap

> **Resolved by spec 0171 §S3, and more thoroughly than proposed below.**
> `skip_group` no longer exists. It was replaced by `scan_group_extent`
> (`render_text/helpers/group_scan.rs:38`), which is **iterative** — group
> nesting is matched with a `depth: usize` counter instead of the call stack.
> That is better than threading a depth and capping it, for a reason the
> proposal could not have known: because it cannot overflow, it is usable *as*
> the recovery path for `render_message`'s own depth cap, rather than being
> subject to that cap itself.
>
> All three defects in this entry are closed by the rewrite:
> - the recursion is gone (guarded by `deep_nesting_does_not_overflow_the_stack`,
>   200 000 nested `START_GROUP` tags);
> - the `pos + len > buflen` overflow at the old `:150` is now
>   `payload_end(...)?` (`group_scan.rs:73`);
> - the `pos == buflen` guard at the old `:119` survives verbatim
>   (`group_scan.rs:46`) and is now correct, exactly as this entry predicted —
>   it was only dangerous in combination with the overflow, and `payload_end`
>   restores the `pos <= buflen` invariant it depends on.
>
> One check was traded away, and the trade is documented at
> `group_scan.rs:31-37`: only the *outermost* closing field number is
> validated, because per-level validation would need a `Vec<u64>` of open
> field numbers — an allocation in a routine whose point is to have none.
> Sound here because the extent is only ever used to bound a span reproduced
> verbatim, so a tolerated inner mismatch changes no output byte.

**Where:** `prototext-core/src/serialize/render_text/helpers/any_field.rs:116-161`,
self-recursing at `:156`.

**What happens.**

```rust
fn skip_group(buf: &[u8], mut pos: usize, expected_field: u64) -> Option<usize> {
    let buflen = buf.len();
    loop {
        if pos == buflen { return None; }        // :119
        ...
            if pos + len > buflen { return None; }   // :150-153
            pos += len;
        ...
            pos = skip_group(buf, pos, field_number)?;   // :156 — no depth
```

There is no depth parameter to thread. A blob of *n* consecutive
START_GROUP tags costs *n* stack frames; since a group tag can be a
single byte, a 1 MB blob buys ~1 M frames. Stack overflow is a `SIGSEGV`:
no unwinding, no panic hook, no message.

Two secondary defects in the same function:

- `:150` has the identical overflow as C1 (`pos + len > buflen`), so
  `pos += len` can wrap `pos` backward.
- `:119` guards with `pos == buflen`, not `>=`. That is exactly correct
  *given* the invariant `pos <= buflen` — but the overflow above breaks
  that invariant, at which point `==` misses and the loop runs away.
  Under a correct bounds check `==` is fine; the two defects are only
  dangerous together, which is why fixing the overflow is the priority.

**Proposed correction.** Thread `depth: usize`, return `None` past a
shared cap, and fix the overflow at `:150`. `None` already means
"structural error, stop", so every call site handles the new rejection
without change. Use the same constant as the main decode's depth cap
(see C3) so there is one number to reason about.

### C3. There is no recursion depth cap on the main decode path, and production disables the node budget

> **Resolved (2026-07-25).** The depth cap landed in spec 0171. The
> node-budget half is resolved the other way round: spec 0174 established
> that a *caller* budget cannot live in this crate at all (its marker had
> no `encode_text` arm, so it broke the round-trip promise) and deleted
> it. Turning it on in production was never the fix; bounding the input
> is. The depth cap is now the crate's only brake, and it is
> unconditional.
>
> **Re-derived 2026-07-26, and the cap does *not* do what step 1 below asked
> for** — it does something better, and the difference is worth recording:
>
> - The over-deep node is rendered as an **ordinary opaque scalar**, not a
>   `MalformedKind` line. Emitting a malformity for a *well-formed* deep nest
>   would be a lie, and only the byte form round-trips. No new grammar was
>   added, which is the constraint a `MalformedKind` would have violated
>   (`deeply_nested_len_degrades_to_bytes_at_the_cap`, `mod.rs:1157`).
>
>   Verified end to end through the CLI on a 1005-deep LEN nest wrapping
>   `field 1 (varint) = 42`. Line 1001 of the `--raw` rendering — the innermost
>   one, at the cap — is:
>
>   ```
>   1: "\n\n\n\010\n\006\n\004\n\002\010*"  #@ string
>   ```
>
>   The 999 enclosing levels render normally as `1 {  #@ message`
>   (`MAX_WIRE_DEPTH - 1`, since the root frame costs one), and the remaining 6
>   levels of framing appear verbatim as the escaped payload of that one opaque
>   scalar — `0A 0A | 0A 08 | 0A 06 | 0A 04 | 0A 02 | 08 2A`. `prototext encode`
>   on that output reproduces the input **byte-identically**, which is the
>   property a `MalformedKind` line could not have had.
> - `at_depth_cap()` (`mod.rs:250`) is consulted at each recursion site
>   *before* anything is written, so the deep node degrades **in place** while
>   its siblings and every enclosing level render normally. `DepthGuard::enter`
>   returning `None` (`:228`) is a deliberate backstop for a recursion site
>   added later without a check, not the mechanism.
> - `DEPTH` (`:160`) is a separate thread-local from `LEVEL`, precisely because
>   `LEVEL` is the *indentation* counter and is not maintained by every sink.
>   `DEPTH` is, including by `ProbeSink` — and the doc comment at `:215-220`
>   argues why that does not breach the invariant C5 was about: `DEPTH` counts
>   real stack frames, a probe's frames sit on top of the outer render's, so
>   the outer value is the right starting point and the RAII guard restores it.
>   That is the one shared thread-local the probe legitimately touches, and the
>   reasoning is written down where the next reader of C5 will look.
> - `tripping_the_depth_cap_does_not_leak_the_counter` (`:1215`) pins the
>   consequence that a thread-local makes possible and a parameter would not:
>   protolens reuses render threads, so a guard that failed to unwind would
>   silently cap every later render on that thread.

**Where:** `protolens/src/decode.rs:769-787` (the production options), and
the *absence* of any depth limit throughout
`prototext-core/src/serialize/render_text/`.

**What happens.** protolens constructs its decode options as:

```rust
DecodeRenderOpts {
    annotations: true,
    indent_size,
    expand_any: false,
    expand_message_set: false,
    ..Default::default()
}
```

`..Default::default()` supplies `node_budget: None`. So the node budget —
the mechanism spec 0163 added precisely to bound decode work — is
**off in production**.

Separately, grepping `render_text/` for `DEPTH|depth` finds only `LEVEL`,
the thread-local that drives *indentation*. It is never compared against
a limit. Nesting depth is therefore unbounded on the main path, not just
in `skip_group` (C2).

The two facts compound. A LEN field whose payload is itself a LEN field,
repeated, recurses once per level; with a minimum of two bytes per level
a 24.5 MB blob affords ~12 M levels. Nothing stops it: not the budget
(disabled), not a depth cap (absent). The result is a `SIGSEGV` on the
exact input class that is an explicit target — `googleapis.desc` —
and the crash is uncatchable, so protolens cannot even degrade to
"showing this region as opaque bytes", which is the behavior the design
calls for.

**Proposed correction.** Both halves, and treat this as a **Phase 1**
item in the protolens worklist rather than a scaling concern — it is a
crash, not a slowdown:

1. Add a depth cap to `render_message`, threaded like `LEVEL` already is,
   emitting a `MalformedKind` line at the limit rather than recursing.
   The malformed line is the correct output: "this nests deeper than we
   will follow" is information, and it keeps the renderability promise.
2. Set an explicit `node_budget: Some(n)` in protolens's production
   options. Leaving it at the `Default` was almost certainly an
   oversight — spec 0163 built the machinery and nothing turned it on.

Note the interaction with C5 before choosing *n*: a tripped budget
currently has a side effect on rendering fidelity, which must be fixed
first or the budget will do harm when it finally fires.

### C5. `ProbeSink` shares the node-count thread-local with the render in progress

> **Dissolved (2026-07-25).** Spec 0174 deleted `NODE_BUDGET`/
> `NODE_COUNT` outright, so there is no longer a shared counter for the
> probe to disturb and no `NODE_BUDGET_EXCEEDED` for it to miscount as a
> malformity. `ProbeSink`'s documented contract now holds as written.
> Kept below because the *shape* of the defect — a read-only helper
> silently mutating a render-mode thread-local — is the thing to watch
> for whenever a new one is added.

**Where:** `prototext-core/src/serialize/render_text/helpers/len_field.rs:64-83`,
violating the invariant documented at
`prototext-core/src/serialize/render_text/sink.rs:865-868`.

**What happens.** The spec-0097 cascade probe decides whether an unknown
LEN payload is a nested message or opaque bytes by trial-decoding it:

```rust
let mut probe = ProbeSink::default();
let (next_pos, _) = render_message(data, 0, None, None, false, &mut probe);
if probe.malformity_count() == 0 && next_pos == data.len() { ... }
```

`ProbeSink`'s own doc comment states the contract it must honor:

> Never mutates any shared render-mode thread-local state
> (`tracks_level` returns `false`): it is a read-only helper that may be
> invoked from the middle of an in-progress outer render (typically a
> `TextSink` pass), and must not disturb that render's own state.

But `render_message` increments the `NODE_COUNT` thread-local, and
`NODE_COUNT` is shared with the outer render. The probe therefore *does*
mutate shared render-mode state, and does so proportionally to the size
of the payload it is speculatively decoding — including payloads it then
concludes are *not* messages and renders as opaque bytes. Every probe,
successful or not, charges the outer render's budget.

The consequence, once C3's fix turns the budget on, is worse than a
miscount. `ProbeSink::malformed` is:

```rust
fn malformed(&mut self, ...) { self.malformity_count += 1; }   // sink.rs:940-942
```

A `NODE_BUDGET_EXCEEDED` inside the probe is a malformity, so
`malformity_count != 0`, so the probe reports "not a message" — and a
**well-formed nested message is silently reclassified as opaque bytes.**
The output is plausible, contains no error marker, and is wrong. Which
regions get downgraded depends on how much budget earlier siblings
consumed, so the same blob renders differently depending on where in the
document you are.

This directly violates the core promise that schema knowledge only ever
*improves* rendering: here, exhausting a resource *degrades* structure
that was correctly recoverable.

**Proposed correction.** The probe must run in its own accounting scope.
Save, zero, and restore `NODE_COUNT` around the probe call — or better,
give `render_message` an explicit budget parameter for the probe's own
sub-decode so the probe is bounded (it must be — an unbounded probe is
its own DoS) without touching the outer counter:

```rust
// The probe speculatively decodes a payload that may not be a message
// at all. Charging that work to the outer render's budget lets an
// exhausted budget reclassify well-formed nested messages as opaque
// bytes — silently, and dependent on document position. Give the probe
// its own scope.
let saved = NODE_COUNT.replace(0);
let (next_pos, _) = render_message(data, 0, None, None, false, &mut probe);
NODE_COUNT.set(saved);
```

Add a test that decodes the same nested message at two different budget
pressures and asserts the rendering is identical. That is the invariant
worth pinning, and nothing currently pins it.

---

## Perf cliffs

### P2. `natural_annotation` is computed for every container node and read by nobody **[verified open 2026-07-26]**

**Where:** declared at
`prototext-core/src/serialize/render_text/sink.rs:1018`, computed by
`natural_annotation_from` (`:1055`) from the single call site at `:1272`.
The line numbers below the fold are stale by roughly −120; these are current.

**What happens.** A repo-wide grep for `natural_annotation` finds 55 hits in
17 files, and **not one is a production read**:

- one computing site, `sink.rs:1272` (in `end_nested`);
- two `: None` literals in the same file (`:1189`, `:1208` — the scalar and
  packed-element paths);
- `: None` initializers in `protolens/src/extract.rs:370`, `:413`, `:459`;
- five tests in `prototext-core` that assert the field's own value
  (`render_text/mod.rs:917-1087`) and eight `: None` lines in protolens's
  test fixtures;
- a stale doc comment at `protolens/src/tui/tests/override_apply.rs:199`
  referring to an `.expect()` that no longer exists.

**One correction to the original write-up:** "three producer sites" overstates
it. Two of the three are `None` literals; there is exactly **one** site that
computes anything, and it runs once per *container* node, not per node. So the
`Option<String>` costs 24 bytes on every `NodeSpan` but a heap allocation only
per container.

**The reason it is still here is a specific, documented, false claim.** The
field was added by spec 0122 for override header patching, and spec 0135
deleted that patching — the ~70-line `patched_annotation` token-splicing
block — thereby removing the only reader. Spec 0135 §Non-goals justifies
leaving the field itself in place:

> `NodeSpan::natural_annotation` (`prototext-core`) itself is
> `pub`/general-purpose and used elsewhere in `prototext-core` (unrelated to
> this override-specific patching) — left untouched; only its *use* in
> `splice_override` is removed.

There is no such use elsewhere. The only other references in `prototext-core`
are the five tests that exist to test the field. Spec 0135 correctly identified
that it was deleting the last consumer and then declined to delete the producer
on the strength of an unchecked assumption — which is why this survived a
review that was looking straight at it.

That 24 bytes matters more than it looks. `NodeSpan` is 120 B and
`TreeNode` is 280 B, and at the measured density of 0.566 nodes/byte a
24.5 MB descriptor set produces ~13.9 M nodes — so the arena is ~3.9 GB
fresh and ~16.9 GB after two override commits. `natural_annotation` is
~330 MB of that, before counting the allocations.

**Proposed correction.** Delete the field. This is a straight deletion,
not an optimization — there is no reader to preserve. It also **changes
step 5 of the `TreeNode` shrink (S12 / W25) from "intern
`natural_annotation`" to "remove it"**, which is strictly less work and
strictly more saving.

The side-effect check the entry asked for is done: `natural_annotation_from`
(`sink.rs:1055`) is a pure forward scan over `self.inner.out` for a `#@`
marker — it reads the already-written output buffer and mutates nothing. So
the deletion takes `natural_annotation_from`, the field, the one computing
site, the two `None` literals, the five tests in
`render_text/mod.rs:917-1087`, and the `header_start` member of the
in-progress-node marker (`sink.rs:1036-1040`), which exists only to feed it.
Remove the stale doc comment at `override_apply.rs:199` in the same change so
the next reader is not misled again, and record in the spec that this reverses
spec 0135's non-goal on the basis that its stated reason was wrong.

### P4. `display_name()` allocates a `String` per output line, under a doc comment promising it does not

*Fixed by spec 0173 S4, 2026-07-26 — replaced by `write_display_name(&self, out: &mut Vec<u8>)`, making the doc comments true.*

**Where:** `prototext-core/src/serialize/render_text/mod.rs:96-101` and
`helpers/output.rs:75-113`.

**What happens.**

```rust
pub(super) fn display_name(&self) -> String {
    match self {
        FieldOrExt::Field(f) => f.name().to_owned(),
        FieldOrExt::Ext(e) => format!("[{}]", e.full_name()),
    }
}
```

Both arms allocate. The `Field` arm allocates to return an owned copy of
a string that is already resident in the descriptor pool and outlives the
call. Meanwhile the call sites are:

```rust
/// Write field-line prefix without String allocation          // output.rs:75
...
    Some(fi) => out.extend_from_slice(fi.display_name().as_bytes()),   // :89
/// Write open-brace prefix without String allocation           // output.rs:95
...
    Some(fi) => out.extend_from_slice(fi.display_name().as_bytes()),   // :109
```

The functions written specifically to avoid allocation allocate, once per
line, and say in their own doc comments that they do not. On a 24.5 MB
descriptor set at ~193 K lines per 1.1 MB — roughly 4.3 M lines — that is
4.3 M allocate-copy-free cycles whose entire purpose is to satisfy a
signature.

**Proposed correction.** Return a borrow for the common arm and defer the
formatting for the rare one. The `Ext` case needs brackets, so a `Cow` is
the minimal change:

```rust
pub(super) fn display_name(&self) -> Cow<'_, str> {
    match self {
        // The descriptor pool owns this string and outlives the call —
        // `to_owned()` here allocated once per rendered line.
        FieldOrExt::Field(f) => Cow::Borrowed(f.name()),
        FieldOrExt::Ext(e) => Cow::Owned(format!("[{}]", e.full_name())),
    }
}
```

Cleaner still, since both call sites immediately push bytes into an
existing buffer: give `FieldOrExt` a `write_display_name(&self, out: &mut Vec<u8>)`
that writes the brackets and the name directly, so even the `Ext` arm
stops allocating and the doc comments at `:75` and `:95` become true.

---

## Minor / doc drift

### M1. The depth cap degrades LEN and `START_GROUP` differently, and the group form mislabels valid bytes **[new, 2026-07-26]**

**Where:** `render_text/mod.rs:643-652` (the `WT_START_GROUP if at_depth_cap()`
arm) versus the LEN arm's opaque fallback described in C3.

**What happens.** The same condition — one level past `MAX_WIRE_DEPTH` — gets
two unrelated treatments. Measured through the CLI on 1005-deep nests:

| | rendering at the cap |
|---|---|
| LEN | `1: "\n\n\n\010\n\006\n\004\n\002\010*"  #@ string` |
| `START_GROUP` | `0: "\013\013\013\013\013\013\010*\014\014\014\014\014\014"  #@ INVALID_TAG_TYPE` |

Three differences, and the third is the defect:

1. The group form reports field number **0**, not the real field number. It has
   to: the raw span it hands the sink starts at `field_start`, so the opening
   tag is *inside* the quoted payload rather than being rendered as the field.
2. The group form goes through `sink.malformed`, so it is a malformity line;
   the LEN form is an ordinary scalar.
3. It claims `INVALID_TAG_TYPE`. **The tag is a perfectly valid
   `START_GROUP`.** The renderer is telling the reader that well-formed bytes
   are malformed, and specifically that a valid wire type is invalid.

**Severity: minor, and bounded by measurement.** Both forms round-trip
byte-identically (`prototext encode` reproduces the input exactly in both
cases), so no data is lost and the core promise is intact. What is wrong is the
*claim*: for a tool whose annotations a human reads to decide whether a blob is
damaged, a false `INVALID_TAG_TYPE` is worse than no annotation. It also
contradicts the reasoning C3's fix is built on — that a `MalformedKind` for a
well-formed deep nest would be a lie — by doing exactly that on the group path.

**Why it is defensible today, and where the line is.** `scan_group_extent` can
genuinely fail (`unwrap_or(buflen)`), and when it does the remainder really is
structurally broken, so a malformity is right *in that case*. The bug is
treating the success case the same way.

**Proposed correction.** Split the two outcomes: on a successful extent scan,
render the group opaquely under its own field number, matching the LEN path;
keep the malformity only for `scan_group_extent` returning `None`, and give it a
kind that is true — `InvalidGroupEnd` already exists and is what an unclosed
group actually is. Whether a dedicated kind is warranted is a separate question
and probably answered "no" for the same reason C3 answered it "no".

---

## Pending re-derivation

The audit that produced this report also raised **C4, C6, C7, C8, C9,
P3, and P5–P14**. Those were not re-checked against source in this pass
and are deliberately not written up here — an unverified finding stated
with the same confidence as a verified one devalues both.

One is worth recording because the audit itself flagged it as untested:

- **C8 — descriptor names may be written unescaped.** If a field or
  message name contains characters that are significant in the text
  format, emitting it verbatim would produce output that does not
  round-trip. Whether protoc's own validation makes such names
  unreachable in a `FileDescriptorProto` was not established.

Re-derive the remaining items before any of them is turned into a
worklist entry.

---

## Cross-cutting observations

**The same bounds-check bug was written twice, in two crates.** C1 here
and C3 in [../scoring-flaws.md](../scoring-flaws.md) (four sites) are
the identical defect: `pos + len > buflen` on an attacker-controlled
`len`, which wraps in release and passes. Both crates *also* independently
wrote an uncapped blind group-skipping recursion (C2 here, C1 there).
These are not coincidences. They are what happens when the wire-format
bounds-checking idiom lives in a reviewer's head rather than in a
function.

**The single highest-leverage change across both reports is a shared,
tested, documented helper module for wire-format bounds arithmetic** — a
handful of functions (`len_fits`, a depth-capped group skip, a
checked advance) used by `prototext-core` and `prototext-graph` alike.
It is a small module and it retires six confirmed defects and the class
that produced them.

> **Done (spec 0171).** The module is
> `prototext-core/src/helpers/bounds.rs`: `payload_end`, `bytes_missing`, and
> `MAX_WIRE_DEPTH`, with `prototext-graph` depending on it. Both crates' LEN
> checks and both crates' blind group skips went through it (C1 and C2 here,
> C3 and C1 in the scoring report), so all six sites are retired and the two
> walkers now refuse the same inputs by construction.
>
> Two things the prediction got right that are worth naming, because they are
> the reason the shared-helper argument was correct rather than merely tidy.
> The helper's doc comment is where the measured stack figures for *both*
> walkers ended up living — a per-crate fix would have had no such place, and
> the calibration question (scoring C2) would have had nowhere to be answered.
> And `payload_end` returns `None` for `pos > buflen` instead of asserting,
> which is what let C2's `pos == buflen` guard stay correct without every
> caller carrying an invariant check.
>
> The helper is *not* what the prediction described in one respect: there is
> no "depth-capped group skip" in it. The group skip became iterative and
> therefore needs no cap, and it stayed in each crate because the two walkers
> want different return values. The shared thing was the arithmetic and the
> constant, not the traversal.

**Every confirmed correctness bug here is triggered by malformed
input.** C1, C2, and C3 are crashes on bytes that are not valid protobuf;
C5 is wrong output when a resource limit fires. This library's stated
purpose is to render bytes of unknown provenance, so "the input was
malformed" is its normal operating condition. It is not fuzzed. A
`cargo-fuzz` target taking arbitrary bytes into `render_message` with a
`TextSink` and a fixed small descriptor pool would have found C1 and C2
in minutes, and asserting "the same bytes render identically at two
budget levels" would have found C5.

> **"It is not fuzzed" was wrong, and the correction sharpens the
> recommendation rather than retiring it** (checked 2026-07-26).
>
> There is no `fuzz/` directory, but there *is* a randomized round-trip stress
> test: `selftest_roundtrip` (`prototext/tests/roundtrip.rs:2497`). Its
> scaffolding is better than this observation assumed — a deterministic
> xorshift64, a real schema (`knife_schema`), env-tunable `PROTOTEXT_SELFTEST_N`
> / `_SEED`, and exactly the right oracle: `binary → text → binary` must be
> byte-identical. A `cargo-fuzz` target would add nightly and out-of-CI
> infrastructure to reach an oracle this test already has.
>
> **What is weak is the generator, and it is weak in a way that provably
> excludes all four bugs.** `random_bytes` (`:2486`) returns uniformly random
> bytes of length **0–63**. Therefore:
>
> - **C1 unreachable.** Its input needs a ten-byte varint of `0x80`-continued
>   bytes to encode a length near `u64::MAX`. From uniform bytes that is a
>   ~2⁻⁷⁰ event.
> - **C2 and C3 unreachable at any N.** A LEN level costs two bytes and a group
>   level one, so 63 bytes cannot express more than 63 levels — the cap is 1000.
>   No seed reaches it.
> - **C5 unreachable.** It needed budget pressure across *sibling* subtrees,
>   which needs a payload large enough to have siblings.
> - Most iterations die on the first byte as `INVALID_TAG_TYPE` and consume the
>   whole buffer, so the bulk of the run re-tests one path.
>
> So the million iterations are real but shallow: the generator samples the
> byte space uniformly, while the decoder's interesting behavior lives in a
> vanishingly small, highly structured subset of it. **Sharpening the generator
> is worth more than adding a fuzzer**, and it stays inside `cargo test` and
> `nix-build -A ci`. Concretely, in rough order of yield:
>
> 1. **Generate records, not bytes.** Emit a sequence of `(field number, wire
>    type, payload)` triples, drawing the field number from {schema-known,
>    unknown, 0, `u32::MAX`-ish} and the wire type from all eight values
>    including the two invalid ones. This lands the generator inside the space
>    where the decoder does work.
> 2. **Bias the length prefixes adversarially.** Sample LEN from a fixed
>    interesting set — `0`, `1`, exactly-remaining, remaining+1, `2^31`,
>    `2^32`, `u64::MAX` — plus overlong (non-canonical) encodings of small
>    values. That set contains C1 by construction, and non-canonical varints
>    are already a concept the renderer annotates.
> 3. **Mutate valid messages instead of only generating.** Build a well-formed
>    message, then apply one damaging edit: truncate at a random offset, flip a
>    wire type, delete a length byte, duplicate a tag. Truncation is the
>    single highest-yield mutation here because every `payload_end` rejection
>    path is a truncation.
> 4. **A nesting-depth mode.** Wrap a leaf in `k` levels of LEN or
>    `START_GROUP` with `k` drawn from around the cap — 0, 1, 999, 1000, 1001,
>    5000. This requires lifting the 63-byte ceiling; make the length
>    log-uniform from 1 byte to a few KB so both the tiny-edge and the deep
>    cases get sampled.
> 5. **Add two more oracles — both free, and each catches a class the
>    round-trip cannot.** *Idempotence:* rendering the same bytes twice on the
>    same thread must give identical output — that is exactly C5's class and
>    the `DEPTH`-guard leak that `tripping_the_depth_cap_does_not_leak_the_counter`
>    currently pins with one hand-written case. *Schema monotonicity:*
>    rendering with and without a schema must re-encode to the same bytes,
>    which is the direct statement of the core promise and is currently
>    asserted nowhere at random.
> 6. **Report a coverage proxy.** Count how many iterations produced at least
>    one nested message, one malformity of each kind, one depth-capped node.
>    Printing that is what would have made the current generator's weakness
>    visible instead of leaving it to be inferred.
>
> Also a small doc drift in the same test: the comment at `:2495` says the
> default `N` is 10 000; the code at `:2502` says `1_000_000`.
>
> This remains the highest-value item in this report — above P2, which is a
> memory saving rather than a correctness one.

**Two doc comments in this report state the opposite of what the code
does** — `output.rs:75`/`:95` ("without String allocation", P4) and
`sink.rs:865-868` ("never mutates any shared render-mode thread-local
state", C5). In a codebase whose house style is dense *why*-comments,
that style is load-bearing: readers trust the comment instead of
re-deriving the behavior. Both should be fixed *as part of* the code
fixes, not separately, so the comment and the guarantee land together.

> **Both are true now, and each got there a different way.** P4's comments
> became true by changing the code to match them (spec 0173 S4:
> `write_display_name(&self, out: &mut Vec<u8>)`). C5's became true by deleting
> the state that falsified it (spec 0174 removed `NODE_COUNT`). The second
> route is the one to prefer where it is available: a guarantee about state
> that does not exist cannot rot.

---

## Checked and clean

- **`ProbeSink`'s design is right, its plumbing is not.** The
  `treat_len_as_opaque() == true` / `tracks_level() == false`
  configuration is exactly what a speculative sub-decode needs, and the
  contract is stated explicitly in the doc comment. C5 is a leak through
  one thread-local the contract did not enumerate, not a flawed design.
- **The cascade probe's accept condition is correct** —
  `probe.malformity_count() == 0 && next_pos == data.len()`
  (`len_field.rs:64-83`) requires both no malformities *and* exact
  consumption, so a payload that happens to parse as a prefix is
  correctly rejected.
- **`MalformedKind` covers the cases it needs to** — `INVALID_VARINT`,
  `TRUNCATED_BYTES`, `NODE_BUDGET_EXCEEDED`. The renderability promise is
  expressible; C1 and C3 are failures to *reach* these paths, not gaps in
  them. *(2026-07-26: the variant set is now `InvalidTagType`,
  `InvalidVarint`, `InvalidLen`, `TruncatedBytes`, `InvalidGroupEnd` —
  `NodeBudgetExceeded` went with spec 0174, and no depth-cap variant was
  added because the cap degrades to opaque bytes instead. The conclusion is
  unchanged and now stronger: every variant round-trips.)*
- **The thread-local implicit-parameter set is coherent** — `LEVEL`,
  `NODE_BUDGET`, `NODE_COUNT`, `EXPAND_ANY`, `EXPAND_MESSAGE_SET`,
  `ANNOTATIONS`, `HIDE_UNKNOWN`, `CBL_START`, `ANY_LOADER` are
  consistently scoped and restored. `NODE_COUNT` under `ProbeSink` (C5)
  is the one exception found. *(2026-07-26: `NODE_BUDGET`/`NODE_COUNT` are
  gone (spec 0174) and `DEPTH` was added (spec 0171), so the exception no
  longer exists. `DEPTH` is RAII-guarded and deliberately shared with
  `ProbeSink`; see the C3 note.)*
