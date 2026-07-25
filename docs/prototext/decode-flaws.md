<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# prototext-core decode and sinks — flaws report

*last verified: 2026-07-25*

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

By impact: **C3** is the worst — it is the only one whose failure is a
process-level `SIGSEGV` with no unwinding and no message, and it fires on
the 24.5 MB input class that is an explicit target. **C1** and **C2** are
crashes and hangs on hostile input. **C5** is the subtlest: it produces
plausible-looking *wrong output* with no error at all.

### C1. The LEN bounds check overflows before it can reject

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

### P2. `natural_annotation` is computed for every container node and read by nobody

**Where:** produced at
`prototext-core/src/serialize/render_text/sink.rs:1070`, `:1310`,
`:1320`; declared on `NodeSpan` at `sink.rs:957-1034`.

**What happens.** A repo-wide grep for `natural_annotation` finds:

- three producer sites in `sink.rs`,
- `: None` initializers (`extract.rs:370`, `:413`, `:459`, plus test
  fixtures),
- prototext-core's own tests (`mod.rs:859-1063`),
- a stale doc comment at `protolens/src/tui/tests/override_apply.rs:199`
  referring to an `.expect()` that no longer exists.

**Zero production readers.** The field is computed, stored, and never
consulted. It costs an `Option<String>` — 24 bytes of `NodeSpan`, plus a
heap allocation whenever it is `Some`.

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

Before deleting, confirm the three producer sites do not have a side
effect worth keeping, and remove the stale doc comment at
`override_apply.rs:199` in the same change so the next reader is not
misled again.

### P4. `display_name()` allocates a `String` per output line, under a doc comment promising it does not

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

**Every confirmed correctness bug here is triggered by malformed
input.** C1, C2, and C3 are crashes on bytes that are not valid protobuf;
C5 is wrong output when a resource limit fires. This library's stated
purpose is to render bytes of unknown provenance, so "the input was
malformed" is its normal operating condition. It is not fuzzed. A
`cargo-fuzz` target taking arbitrary bytes into `render_message` with a
`TextSink` and a fixed small descriptor pool would have found C1 and C2
in minutes, and asserting "the same bytes render identically at two
budget levels" would have found C5.

**Two doc comments in this report state the opposite of what the code
does** — `output.rs:75`/`:95` ("without String allocation", P4) and
`sink.rs:865-868` ("never mutates any shared render-mode thread-local
state", C5). In a codebase whose house style is dense *why*-comments,
that style is load-bearing: readers trust the comment instead of
re-deriving the behavior. Both should be fixed *as part of* the code
fixes, not separately, so the comment and the guarantee land together.

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
  them.
- **The thread-local implicit-parameter set is coherent** — `LEVEL`,
  `NODE_BUDGET`, `NODE_COUNT`, `EXPAND_ANY`, `EXPAND_MESSAGE_SET`,
  `ANNOTATIONS`, `HIDE_UNKNOWN`, `CBL_START`, `ANY_LOADER` are
  consistently scoped and restored. `NODE_COUNT` under `ProbeSink` (C5)
  is the one exception found.
