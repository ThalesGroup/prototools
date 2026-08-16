<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0311 — a truncated field still has its declared type

Status: implemented
Implemented in: 2026-08-16
App: prototext-core, protolens
Refs: docs/specs/0302-….md (spec 0302 S1: `ArenaSink` already allocates
        child slots over a TRUNCATED_BYTES field's available bytes),
      docs/specs/0303-….md (spec 0303: the `TRUNCATED_MESSAGE; MISSING: N`
        annotation, and the encoder support that re-inflates the declared
        length from it — this spec supplies a second caller for both),
      docs/specs/0310-….md (spec 0310: a cut range scores what it has;
        the "bytes ran out" versus "a length prefix lied" distinction),
      docs/specs/0097-….md (the unknown-LEN cascade, deliberately
        untouched here — see N1),
      docs/specs/0312-….md (spec 0312: the same descend, reached from the
        probe instead of from the schema — it extends this spec's S1
        routing and reuses S2/S3/S4 whole)

## Background

A LEN field whose length prefix declares more bytes than the buffer holds
renders as `TRUNCATED_BYTES`, whatever the schema says it is. The check
that decides this is in `render_text/mod.rs:736`:

```rust
let Some(end) = payload_end(pos, length, buflen) else {
    let missing = bytes_missing(pos, length, buflen);
    sink.malformed(field_number, …, MalformedKind::TruncatedBytes { missing }, raw, …);
    return (buflen, None);
};
…
render_len_field(FieldCtx { field_number, field_schema, … }, …)
```

`render_len_field` (`helpers/len_field.rs:29`) is the only place
`field_schema` is consulted — at `:76` for the unknown-field cascade and
at `:206` for a declared message. The bounds check returns above it.
`field_schema` is already in scope at the check, computed at
`mod.rs:618`, and is simply not read.

So the schema is bypassed for a truncated field in every direction at
once: a declared sub-message is not descended into, a declared `string`
is not shown as a string, a declared packed field is not unpacked.

**The reproduction.** A `FileDescriptorSet` cut part-way through its
ninth `file` entry. protolens renders

```
9: "\n\014…"  #@ TRUNCATED_BYTES; MISSING: 1024
```

while `t` on that same node offers
`google.protobuf.FileDescriptorProto  (score: 105)`. The type is
established twice over and applied neither time: the schema declares
field 1 of `FileDescriptorSet` to be a `FileDescriptorProto`, and the
scoring walk independently walked those exact bytes and scored them 105.
The one fact acted on is that a varint claimed 1024 bytes the file does
not have.

The same file under `prototext decode -t google.protobuf.FileDescriptorSet`
shows the same opaque line, and there the bytes are unreachable
altogether — the CLI has no override mechanism.

Specs 0302 and 0303 already built everything needed to open such a
field. Nothing invokes it except a manual `message` override.

## Goals

- **G1.** A LEN field whose declared length overruns the buffer, and
  whose schema declares it a non-group message, renders as a nested
  message over the bytes that *are* present, with
  `TRUNCATED_MESSAGE; MISSING: N` on its header line.
- **G2.** That rendering re-encodes to the original bytes — the declared,
  pre-truncation length varint is restored, not the shorter actual one.
- **G3.** The missing count is per nesting site. A file cut inside a
  nested message truncates one frame at every level of the spine, and
  each carries its own `MISSING`.

## Non-goals

- **N1.** Unknown fields. A field with no schema keeps the spec 0097
  cascade and its `ProbeSink` verdict, unchanged. Whether a *cut* unknown
  payload should be admitted as a message is a genuinely harder question
  — forgiving the tail is what turns a short string into a fabricated
  message, because for a short payload an overrunning length prefix is
  the *usual* way the probe rejects it, not an edge case. Spec 0312 takes
  it up, with the corpus measurement behind its threshold. It reuses this
  spec's routing, `missing` parameter, and encoder path, but nothing here
  depends on it and the two land independently.
- **N2.** `string` and `bytes` declared kinds keep `TRUNCATED_BYTES`.
  Their content rendering (the available bytes, escaped) and their
  round-trip are already right; the only thing missing from the line is
  the field's declared *name*. Supplying it means giving `Sink::malformed`
  a `field_schema` parameter — a trait change across four implementations
  with broad expectation churn — in exchange for a label. Separable from
  G1 and not worth coupling to it.
- **N3.** Packed repeated fields. The trailing element is itself cut, so
  `MISSING: N` does not decompose into whole elements and the partial
  last element has no defined rendering. A different shape of truncation.
- **N4.** Groups. They have no declared length, so `MISSING` has no
  value (spec 0303 N6); `OPEN_GROUP` already reports a cut group.
- **N5.** `ProbeSink` and the scoring walk are untouched. The score of
  105 above already comes from spec 0310 and is unaffected either way.
- **N6.** A truncated `Any` or MessageSet field does not expand. Both
  expansions synthesize virtual fields whose round-trip is defined
  against complete bytes; a cut payload falls through to the plain
  nested-message rendering of S2.
- **N7.** No change to binary export (spec 0303 N1). `extract_binary`
  reads the original arena bytes and is already correct.

## Specification

### S1 — the bounds check consults the schema

`prototext-core/src/serialize/render_text/mod.rs`, the `WT_LEN` arm's
`payload_end` failure branch splits on the declared kind:

- Declared a non-group message — the same condition `render_len_field`
  itself uses at `len_field.rs:206-207` — call `render_len_field` with
  `data = &buf[pos..buflen]`, `raw_range = field_start..buflen`, and
  `missing = Some(bytes_missing(pos, length, buflen))`.
- Otherwise, the existing `sink.malformed(… TruncatedBytes { missing } …)`
  call, unchanged.

Both branches then `return (buflen, None)`. A truncated field is the last
thing in its frame either way, so parsing cannot continue past it.

### S2 — `render_len_field` carries the count

`helpers/len_field.rs`: `render_len_field` gains a `missing: Option<u64>`
parameter, used only by the plain nested-message `begin_nested` at
`:248`.

A `debug_assert` records the caller's guarantee that `Some` implies a
declared non-group message field. Under that guarantee the unknown-field
cascade (`:76-134`), the packed path, and the wire-type-mismatch path are
unreachable with it set, and the `Any` and MessageSet intercepts (`:211`,
`:230`) are skipped explicitly (N6).

### S3 — the count rides on the nesting event

`render_text/sink.rs`: `NestedKind::Message` gains
`missing: Option<u64>` beside `probed_as_message`. That is the right home
for the same reason `probed_as_message` has it — a fact about this
particular nested opening, reported once by the renderer rather than
re-derived by each sink.

`TextSink::begin_nested` (`:688`) currently takes the count from spec
0303's one-shot `self.missing_payload_bytes`. It now prefers the value on
`kind` and falls back to the one-shot. Both sources stay, because they
answer different questions:

- the `kind` value is *this field's own length prefix overran*,
  discovered by the renderer, at any depth, possibly several times per
  document (G3);
- the one-shot is *the caller reframed the root node it handed me and
  knows what it removed* (spec 0303 S3, `render_node_as`), which has no
  enclosing `render_len_field` to carry it.

The emitted text is spec 0303's, unchanged:
`TRUNCATED_MESSAGE; MISSING: N` after the field_decl token.

### S4 — the encoder needs nothing new

Spec 0303 S5/S6 already parse `TRUNCATED_MESSAGE` into
`Ann::truncated_message`, carry `MISSING: N` in `missing_bytes_count`,
store it as `Frame::Message::missing`, and add it to the compacted
content length through `fill_placeholder`'s `missing_extra`. G2 is a test
obligation here, not an implementation one.

### S5 — arena and render come into agreement

protolens: spec 0302 already has `ArenaSink` walk a `TRUNCATED_BYTES`
field's available bytes and allocate child slots, so that a later
`message` override would have somewhere to put its children. Under G1 the
render now produces those children too, so a case that previously had
arena slots and no rendered lines has both.

No protolens change is expected. The obligation is to *verify*: the two
structures are built by separate walks, and spec 0210's line accounting
asserts they agree.

### S6 — the descend path is refused for an unrepresentable length

A declared length whose *minimal* varint encoding needs more than five
bytes — that is, `declared >= 2^35`, about 34 G — must not take the
descend path. It renders as `TRUNCATED_BYTES` as it does today.

The reason is in `encode_text/placeholder.rs`. A message placeholder is
`BASE_OVERHEAD + ohb` bytes, laid out `waste(1) + next(5) +
varint_room_base(5) + ohb`, and `fill_placeholder` writes the length
varint *flush-right* into that region:

```rust
let varint_room_end = placeholder_start + BASE_OVERHEAD + ohb;
let varint_write_start = varint_room_end - k;
```

so `k > 5 + ohb` writes over the `next_placeholder` link at
`placeholder_start + 1 .. + 6` and silently corrupts the forward list the
compaction pass walks. There is no panic: `waste = BASE_OVERHEAD + ohb -
k` stays non-negative all the way to `k = 11 + ohb`, so the failure is a
wrong output, not a crash.

Ordinarily `k` cannot get there, because `child_len_compacted` is bounded
by the buffer actually written. `missing_extra` is the one term that is
*not* so bounded — it comes from a length prefix in the input, which in a
corrupt or cut file can say anything. So this hazard arrived with spec
0303 and is reachable today through a `message` override on a
`TRUNCATED_BYTES` field with an absurd declared length; G1 widens its
reach from "the user asked for it" to "any schema-typed truncated field".

The guard belongs in S1's branch condition rather than in
`fill_placeholder`, so that the renderer never emits text the encoder
cannot honor. Spec 0174 G1's round-trip promise is unconditional, and it
stays that way only if the two agree about what is emittable.

Hand-written text carrying an absurd `MISSING:` still reaches
`fill_placeholder` without passing through the renderer. That is not
fixed here — the guard is about what protolens and `prototext decode`
produce — but the invariant `k <= 5 + ohb` is stated at
`fill_placeholder` so the next caller finds it written down rather than
discovering it as corruption.

## Alternatives considered

### Leave it, and require a `message` override

The status quo. Ruled out twice over. It asks the user to re-supply a
type the schema already declares. And it exists only in protolens —
`prototext decode -t google.protobuf.FileDescriptorSet` has no override
mechanism, so for that caller the bytes stay unreachable.

### Rewrite the length varint before rendering

protolens' `reframe_to_actual_length` (`preview_truncate.rs:140`) does
exactly this on the override commit path, and it works there. Ruled out
for the renderer: it mutates the input, and the mutated buffer no longer
carries the declared length, so re-encode could not restore it without a
side channel — which is the very thing `missing` would have to be
anyway. Carrying `missing` beside unmodified bytes gets the same
rendering and keeps the original length recoverable.

### Extend spec 0303's `DecodeRenderOpts` one-shot to fields

It is a single value, consumed at the first `begin_nested` and cleared so
it cannot leak. G3 needs a count at every level of the spine. One value
cannot address more than one site.

### Put `missing` on `Span`

Rejected by spec 0303's own alternatives for reasons that still hold:
`Span` is public API, the field would be `None` for almost every span,
and it couples the codec's data model to one UI-level concern. Recorded
here so it is not re-proposed.

## Test plan

The property at risk is spec 0174 G1: *every* production the renderer can
emit re-encodes to the exact bytes it came from. This spec adds a
production (`TRUNCATED_MESSAGE` reached without a user override) whose
re-encode depends on a number — `MISSING: N` — that is carried across
four hand-offs and is not recoverable from the text's own bytes. Dropping
it anywhere is silent: the document still renders, still parses, and
re-encodes three bytes short. So the round trip gets tested first and
tested exhaustively, and the rendering assertions come after.

### A — the truncate-at-every-offset sweep

This is the load-bearing test, and the only one here that is not
example-based. prototext-core has no property or fuzz round-trip harness
today; three files mention `round_trip` and all three enumerate cases by
hand.

`truncating_anywhere_round_trips` — for a fixture blob of length `L`, for
every `k` in `1..=L`:

```
render(blob[..k], schema, annotations: true)  →  text
encode_text_to_binary(text)                   ==  blob[..k]
```

Every cut position is covered by construction: inside a tag byte, inside
a length varint, inside a payload at every nesting depth, and exactly on
a field boundary (where nothing is truncated at all and the sweep is
just re-asserting the ordinary round trip). That is precisely the space
this spec disturbs, and enumerating it by hand is how a case gets missed.

Three fixtures, each swept whole:

- **Nested and schema'd.** A message with a declared sub-message
  containing a declared sub-sub-message, so cuts land at three spine
  depths and G3's per-level counts are exercised at every `k` past the
  outer header. Run *with* the descriptor: this is G1's path.
- **The same bytes, schema-less.** Pins N1 across the whole sweep — no
  cut prefix may start rendering as a message — and this fixture becomes
  spec 0312's primary test when the carve-out lands, at which point the
  expected *rendering* changes but the round-trip assertion does not.
- **Mixed kinds.** One message carrying a declared sub-message, a
  declared `string`, a declared `bytes`, a packed `int32` field, a group,
  and an unknown field. Cuts through the string and packed fields pin N2
  and N3 for free, and a cut past the group's `START_GROUP` pins N4.

The fixtures are small (tens of bytes) so all three sweeps together are a
few hundred renders — cheap enough to be an ordinary `cargo test` case,
not a nightly one.

### B — round-trip cases the sweep cannot reach

The sweep only produces *tail* cuts, which by spec 0310's tail-cut
theorem means at most one overrunning frame per nesting level and a
`MISSING` derived from the real length. These four need bytes a
truncation cannot produce:

1. `a_lying_length_prefix_round_trips` — a length prefix that overruns by
   a large amount while the file continues after it is impossible from a
   tail cut but trivial to write. Assert the *declared* length varint
   comes back, not the actual one. This is the direct G2 assertion and
   the one a dropped `missing` fails outright.
2. `non_minimal_lengths_survive_truncation` — the truncated field's tag
   and length varint written with overhead bytes (`tag_ohb`, `len_ohb`).
   The re-encode must reproduce the non-minimal encoding *and* the
   inflated value; `fill_placeholder`'s flush-right write is where those
   two interact, and it is the one arithmetic in the chain that is not
   obviously right.
3. `a_truncated_message_with_no_available_bytes_round_trips` — declared
   length `N`, zero bytes present. Renders `N { }  #@ TRUNCATED_MESSAGE;
   MISSING: N`, an empty body whose entire content is the missing count.
   `child_len_compacted` is 0 here, so any implementation that treats
   `missing` as an adjustment to a non-zero length rather than an addend
   fails.
4. `truncated_bytes_inside_a_truncated_message_round_trips` — an outer
   declared sub-message that overruns, containing a declared `string`
   that also overruns. One document, both encoder arms: the placeholder
   path (S4, via `Frame::Message::missing`) and the direct-write path
   (`encode_text/fields.rs:201`, the `TRUNCATED_BYTES` arm). They compute
   the declared length by different routes and this is the only case that
   makes them agree in one buffer.

### C — the guard, and the hazard behind it

5. `an_unrepresentable_declared_length_does_not_descend` — a declared
   message field whose length prefix says `2^35` or more. Assert the
   render is `TRUNCATED_BYTES`, and that it round-trips. This is S6, and
   without it `fill_placeholder` writes over `next_placeholder` and
   returns a wrong buffer with no panic.
6. `the_placeholder_varint_room_is_respected` — a direct unit test on
   `fill_placeholder`, asserting `k <= 5 + ohb` for the largest value S6
   admits. The renderer-side guard is where the decision belongs, but the
   invariant it protects is the encoder's and should be stated there too,
   so that a future caller reaching `fill_placeholder` by another route
   is told what the limit is rather than discovering it as corruption.

### D — rendering, and the non-goal pins

7. `a_declared_message_field_that_overruns_descends` — the render
   brackets the field and shows the children that fit, rather than one
   bytes line. G1.
8. `the_truncated_header_carries_missing` — that header line contains
   `TRUNCATED_MESSAGE; MISSING: N` with `N` equal to
   `declared - available`.
9. `truncation_is_counted_at_every_spine_level` — two nested declared
   message fields, cut so both frames overrun; each header carries its
   own count. This is the test a one-shot implementation fails, and it
   pins G3.
10. `a_declared_string_field_that_overruns_stays_bytes` — pins N2.
11. `a_truncated_any_does_not_expand` — pins N6.
12. `an_unknown_truncated_field_is_unchanged` — no schema; still one
    `TRUNCATED_BYTES` line. Pins N1, and is the guard that stops this
    spec's change from drifting into the probe.
13. `every_malformity_marker_round_trips` — the existing test at
    `render_text/mod.rs:1678` is schema-less and must pass unchanged.
14. Spec 0303's own override path — `render_node_as` with a reframed root
    — must still round-trip now that `missing` has two sources and
    `TextSink::begin_nested` prefers the new one. Its existing tests
    cover this; they are named here because S3 is the edit that could
    break them.

### E — protolens

15. `arena_and_render_agree_on_a_truncated_declared_message` — open a
    blob with a cut declared-message field under a real schema; assert
    `assert_line_counts_are_exact` holds and the node folds and unfolds.
    This is S5's verification.
16. `export_binary_is_byte_identical_over_a_truncated_spine` — the
    protolens-side round trip, which goes through `extract_binary` and
    the arena rather than through the text encoder (N7). Both paths must
    return the input bytes for the same document.
17. End to end — truncate a `FileDescriptorSet` mid-`file`, open it with
    `-t google.protobuf.FileDescriptorSet`, and assert the cut entry
    renders as a `FileDescriptorProto` with its intact fields visible and
    no user action taken. The "before" state is the Background's
    reproduction.

## Measured outcome

**S4 and S5 held: neither the encoder nor protolens needed a line.** The
whole change is six files in `prototext-core/src/serialize` — the bounds
check and its two new helpers in `render_text/mod.rs`, the `missing`
parameter in `helpers/len_field.rs`, the field on `NestedKind::Message`
in `sink.rs`, and three call-site updates (`arena.rs`, `any_field.rs`,
`message_set_field.rs`). `encode_text` gained only the S6 invariant, as
a doc comment and a `debug_assert`; spec 0303's `missing_extra` already
did the arithmetic. protolens gained only tests.

**The sweep found nothing, which is the result.** Sixty prefixes across
the three fixtures — 17 bytes nested-and-schema'd, the same 17
schema-less, 26 mixed-kind — all round-trip. The four B cases and the
two C cases pass. `every_malformity_marker_round_trips` and
`capped_render_still_round_trips` pass unchanged, as does spec 0303's
override path.

**The fixture had to move.** `grpconf/anomalies.pb` §6.a put its
truncated payload on field 2 of `DescriptorProto` — `repeated
FieldDescriptorProto field = 2`, a declared message — so under G1 it
stopped being a `TRUNCATED_BYTES` case at all and
`the_fixture_covers_the_whole_vocabulary` failed on a marker that had
gone missing. It is now nested: an overrunning `field` containing an
overrunning `name`, which is test case B4 written as a fixture and shows
both markers in one document. `TRUNCATED_MESSAGE` joined spec 0226's
vocabulary list, having become reachable from a plain schema-backed
decode rather than only through a protolens override.

**Gates.** `cargo fmt --all --check`, `cargo clippy --no-default-features
--workspace --all-targets`, `cargo test --release --no-default-features
--workspace` (prototext-core 126 + 3, protolens 1082), and `reuse lint`
(922/922) all clean.

**Item 17 is folded into item 15.** The protolens fixture is the
Background's reproduction in miniature — a `Set` of three declared
`Entry` records with the third cut — rather than a real
`FileDescriptorSet`, which would have needed a descriptor fixture
protolens does not carry. Same path, same assertion: the cut record
opens under its declared type with `name: "c"` readable and no user
action taken.
