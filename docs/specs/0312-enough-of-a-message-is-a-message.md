<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0312 — enough of a message is a message

Status: implemented
Implemented in: 2026-08-16
App: prototext-core, protolens
Refs: docs/specs/0266-….md (the probe's verdict: any invalid token
        disqualifies; this spec carves out exactly one of them),
      docs/specs/0097-….md (the unknown-LEN cascade, whose step 1 is the
        probe and whose steps 2/3 are its `else`),
      docs/specs/0310-….md (spec 0310: "the bytes ran out" versus "a
        length prefix lied", and the `end_undeclared` predicate that
        separates them),
      docs/specs/0311-….md (spec 0311: the same descend, reached from the
        schema instead of from the probe — the two share S1's routing,
        `render_len_field`'s `missing` parameter, and the whole encoder
        path),
      docs/specs/0302-….md (`ArenaSink` already walks a cut payload's
        available bytes),
      docs/specs/0216-….md (`unknown_len_is_message`: the maximal-tree
        sink recurses regardless of the verdict)

## Background

Spec 0311 gives a truncated field its declared type. This spec is about
the field that has no declared type — where the question is not "apply
what the schema said" but "is this a message at all", and the answer
today is no, categorically, whenever the payload is cut.

The mechanism is spec 0266. `ProbeSink::says_message` is

```rust
self.invalid_count == 0 && next_pos == data.len()
```

and `ProbeSink::malformed` counts every `MalformedKind` as invalid,
`TruncatedBytes` among them, one variant at a time and on purpose. So a
payload with one cut field at its end is not a message, however many
complete fields precede it, and the spec 0097 cascade falls through to
its steps 2/3 and renders the whole thing as one escaped bytes line.

That is the right default and it earns its keep constantly: for a short
payload, an overrunning length prefix is the *usual* way the probe
rejects a string, not an edge case. `"hello world"` read as a message is
field 13 LEN declaring 108 bytes; the cut is what stops it.

**Where it costs.** `grpconf/stage/boblog` — 20 198 bytes, three intact
log entries and a fourth cut short by 1 024 bytes — collapses to a single
20 KB line. Every field of the three complete entries is present and
well-formed; the file is disqualified by the tail. protolens has an
answer (`message`, spec 0299) but it is the reader's to apply, and
`prototext decode --raw` has none at all.

**Two facts make the carve-out possible now that were not available
before.** Spec 0310 supplies the predicate that separates a cut file from
a lying length prefix — the range's end is the buffer's end — and shows
that the distinction is decidable rather than a matter of taste. Specs
0302/0303/0311 supply the machinery to *render* a forgiven payload
losslessly: available bytes descended into, `TRUNCATED_MESSAGE;
MISSING: N` on the header, and the declared length restored on re-encode.

**What this is not.** It is not a veto and it does not become one. Both
outcomes here are successful renderings — a nested message with a cut
tail, or an escaped bytes line — and the reader gets a legible document
either way. That matters because the obvious way to express "not a
message" in the scoring walk is `veto_all`, which flushes pending
charges, sets the veto bit on every entry of every `ActiveEntry`, and
clears `active`; a veto propagates to the enclosing candidate and from
there to its parent, so a deep cut would take the whole document with
it. The probe has no such reach, which is why the decision stays there.

## Goals

- **G1.** An unknown LEN payload whose only flaw is a cut tail, and which
  showed at least `P` complete fields before the cut, renders as a nested
  message over the bytes that are present, with `TRUNCATED_MESSAGE;
  MISSING: N`.
- **G2.** The forgiveness is conditional on the cut being a cut. The
  payload's end must be the end of the buffer — spec 0310's predicate,
  unchanged and now stated in a third place. A length prefix that
  overruns while bytes continue after it stays disqualifying.
- **G3.** One flaw. A payload with a cut tail *and* anything else invalid
  in it is not a message, exactly as today.
- **G4.** Below `P`, and in every other respect, spec 0266 is unchanged:
  a short cut payload still reads as a string or as bytes, and no
  rendering it produced before this spec changes.
- **G5.** Both outcomes round-trip. In particular the declined case must
  still re-encode the *declared* length, which is not automatic — see S3.
- **G6.** `prototext decode --raw` gets the change too, not only
  protolens. It is the caller with no override mechanism and therefore
  the one for which this is the only route.

## Non-goals

- **N1.** The scoring walk is untouched, including `EntryScore::truncated`,
  which stays a `bool`. Spec 0310's rationale still holds — the sites that
  set it return immediately, so it can be set at most once per walk — and
  the carve-out does not create a second site. Making it a counter is
  recorded under Alternatives so that it is proposed with evidence or not
  at all.
- **N2.** No veto, anywhere, for any reason in this spec. Stated as a
  non-goal rather than left implicit because it is the design that
  suggests itself and the one that cascades.
- **N3.** Groups. A cut group is `OPEN_GROUP`, which stays invalid: a
  group has no length prefix, so `MISSING` has no value (spec 0303 N6)
  and there is nothing to restore on re-encode. `ProbeSink::end_nested`'s
  `None`-`close_facts` arm is unchanged, and its comment about a trailing
  `START_GROUP` byte rescuing a string is precisely the reason.
- **N4.** Malformities other than `TruncatedBytes`. `InvalidVarint` and
  `InvalidFixed64`/`InvalidFixed32` are also "the bytes ran out", and are
  also always at the tail — but they are indistinguishable from garbage
  in a way a length prefix is not (a length prefix at least parsed, and
  said a number), they carry no `missing` count, and their encoder arms
  write verbatim rather than restoring a declared length. A separate
  question with a separate round-trip story.
- **N5.** The top-level buffer of `decode_and_render`. It is rendered by
  `render_message` directly, with no enclosing LEN field and no probe, so
  a cut in its last field already renders as `TRUNCATED_BYTES` at the
  document's own level and nothing here reaches it. protolens is
  unaffected by that gap because `Blob` prepends a real field-1 tag and
  length, so its root *is* a LEN field.
- **N6.** Packed repeated fields (spec 0311 N3) and truncated
  `Any`/MessageSet expansion (spec 0311 N6), for the same reasons.

## Specification

### S1 — the routing, shared with spec 0311

`render_text/mod.rs`, the `WT_LEN` arm's `payload_end` failure branch.
Spec 0311 S1 splits it two ways; this spec adds the third arm:

- the sink treats LEN as opaque (`ProbeSink` itself) — `sink.malformed`,
  unchanged, and this is how the probe sees the cut at all;
- the schema declares a non-group message — `render_len_field` with
  `missing: Some(…)` (spec 0311);
- **no schema for this field number** — `render_len_field` with
  `missing: Some(…)`, and the cascade decides;
- otherwise — `sink.malformed`, unchanged.

`data` is `&buf[pos..buflen]`, the bytes that are present, in all cases.

### S2 — the probe is told where the buffer ends

`ProbeSink` sees a slice and cannot tell whether its end is the file's
end or an enclosing length prefix's boundary. G2 needs that distinction,
so it is threaded rather than guessed: `render_message` and
`render_len_field` gain a `frame_ends_at_eof: bool`, true for the
outermost call and propagated to a nested payload as
`parent_ends_at_eof && payload_end == parent_buf.len()`.

A thread-local was considered and does not work. The obvious one would
hold EOF as an offset, and offsets are not comparable across frames: a
nested message resets the coordinate frame (`render_len_field` passes
`raw_range.end - data.len()` as `payload_start` for exactly that
reason). The bool is frame-relative by construction, which is what the
question is.

The same value serves protolens' `ends_where_the_bytes_end`
(`override_pane.rs:92`) and the scoring walk's `ScoringOpts::end_undeclared`.
Three callers, one rule, and this spec should not be the one that lets
them drift — the rule is quoted in each.

### S3 — the cascade, and the declined path's round trip

`helpers/len_field.rs`, the unknown-field branch (`:76-134`):

- Step 1 runs `ProbeSink` over the available bytes as it does today,
  now with `frame_ends_at_eof` passed through. This remains the only
  place the verdict is computed.
- On `probed_as_message || sink.unknown_len_is_message()`, descend as
  today, with `missing` on `NestedKind::Message` (spec 0311 S3) so the
  header carries the count.
- **On decline, when `missing.is_some()`, emit
  `sink.malformed(… TruncatedBytes { missing } …)` — not
  `scalar_field(… ScalarValue::Bytes …)`.**

That last point is the round-trip obligation and it is not cosmetic. The
`ScalarValue::Bytes` fallback re-encodes as tag + `len(available)` +
bytes, which is three bytes short of the input and silently so. The
`TRUNCATED_BYTES` arm (`encode_text/fields.rs:201`) is the one that adds
`missing_bytes_count` back. Routing the declined case through
`render_len_field` at all is what creates this hazard, and this is its
whole fix: the declined output is byte-for-byte what the pre-0311 code
emitted.

### S4 — `says_message` gains one clause

`ProbeSink` gains two fields beside `invalid_count`:

- `complete_fields: u64`, incremented once per field the probe saw
  rendered without objection;
- `forgiven_tail_cut: bool`, set by the `malformed` arm when the kind is
  `TruncatedBytes` *and* `frame_ends_at_eof`, in which case that arm does
  not call `invalid()`.

```
says_message = invalid_count == 0
            && next_pos == data.len()
            && (!forgiven_tail_cut || complete_fields >= P)
```

G3 falls out of the existing `invalid_count == 0`: anything else wrong
still counts, so one forgiven cut plus one real flaw is still a
rejection. The exhaustive destructuring of spec 0266 S3 stays exhaustive
— the `TruncatedBytes` arm now has a body instead of joining the
or-pattern, which is the same compile-time guarantee with one more case
spelled out.

`next_pos == data.len()` continues to hold on the cut path: the
malformity branch returns `(buflen, None)`.

### S5 — `P`, and how it is chosen

`P` is the number of complete fields a payload must show before its cut
tail is forgiven. It is the whole safety margin of this spec and it is
**not** to be picked by taste.

The failure mode it guards is concrete. At `P = 1`, the five bytes
`08 01 0A 09 78` — field 1 varint 1, then field 2 LEN declaring 9 with
one byte present — pass, and that is an ordinary-looking scrap of binary
being reported as a message. At `P = 8` the boblog case still passes with
room to spare. Somewhere between, the curve turns.

The measurement, before implementation is finished:

- **Negative controls** — PNG, ELF, gzip, UTF-8 prose, and JSON, each
  truncated at every offset, plus their untruncated forms. Every one of
  these must decline. This is the rate that must not move.
- **Positive cases** — the googleapis corpus (see
  `corpus_googleapis_db.md`), each message truncated at every offset.
  The fraction admitted, by `P`.
- Report both as a function of `P` from 1 to 16, and record the chosen
  value with its two rates in Measured outcome. A value with no numbers
  beside it is not a chosen value.

`P` is a constant, not an option. A knob here would be a knob on what
the document *is*, and there is already a user-facing answer for
disagreeing with the verdict — the `message` override (spec 0299), which
continues to overrule it in both directions.

### S6 — the unrepresentable length

Spec 0311 S6's guard applies unchanged and for the same reason: a
declared length at or above `2^35` must not take the descend path,
because `fill_placeholder` writes the length varint flush-right into
five bytes plus `ohb` and a sixth byte overwrites the `next_placeholder`
link with no panic. The guard is in S1's routing, so it covers this
spec's arm as well as spec 0311's.

## Alternatives considered

### Penalize truncation in the scoring walk instead

The user-facing symptom is that boblog reads as bytes, and the walk has
a `truncated` charge, so making that charge heavier looks like the fix.
It is not, for a reason that is structural rather than a matter of
tuning: `len_field.rs:88-134` is a **cascade with an early `return`**,
not a comparison. When the probe says message, the string reading is
never formed, and when it says bytes, no score is consulted. There is no
point at which two candidate readings of the same payload are ranked
against each other, so no penalty of any magnitude can change the
outcome. The walk is also never invoked for an untyped render at all.

Separately, there is no score floor anywhere: `pick_winner` takes rank 1
unless the top two tie, and `sweep::rank` filters on `!vetoed` and sorts.
A penalty reorders; it never rejects.

### Veto a truncated field with no valid subfield, and propagate

Proposed and worked through. It reaches the right verdict for a single
field and the wrong one for a document: `veto_all` sets the veto bit on
every entry of every `ActiveEntry` and clears `active`, and a vetoed
child makes a vetoed parent, so a cut four levels down vetoes the root.
The distinction that makes the carve-out safe is that "not a message" is
a *local rendering fallback* while "vetoed" *propagates*, and the probe
already keeps them apart. See N2.

### Make `EntryScore::truncated` a counter

`u32` with `-5 × n`, so a document cut at several spine levels is
charged per level. Rejected here for want of a driver: the walk's cut
sites return immediately, so the flag is set at most once per walk, and
spec 0310 chose a `bool` on exactly that ground and wrote down that a
count only ever 0 or 1 invites a reader to sum it. If a walk that can be
cut at two levels appears, this is the change to make, together with the
popup line at `tui/popup.rs:240` joining the other five counted terms
with an `n ×` prefix. Not before.

### Forgive the cut only at the root

The narrowest fix for boblog, and the one drafted before spec 0299. It
special-cases the root, which spec 0299 already declined to do and for
the same reason: the root is a LEN field like any other, and a rule that
holds only there is a rule nobody can predict from the outside.

### Rewrite the length prefix to the actual length before probing

protolens' `reframe_to_actual_length` does this on the override commit
path. Rejected for the renderer by spec 0311's alternatives: it mutates
the input and destroys the declared length, which is the one number the
re-encode needs.

## Test plan

Spec 0311's test plan is the base; this spec inherits it and changes one
expectation in it. The round-trip assertions are unchanged throughout,
which is the point of having written them there.

### A — the sweep, reused

1. **The schema-less fixture of spec 0311's truncate-at-every-offset
   sweep becomes this spec's primary test.** Its round-trip assertion —
   `encode_text_to_binary(render(blob[..k])) == blob[..k]` for every `k`
   — must hold unchanged, while the *rendering* it produces changes for
   those `k` where enough complete fields precede the cut. That is the
   cleanest statement of what this spec is allowed to do: change what the
   document says, never what it is.
2. `declining_still_restores_the_declared_length` — the same sweep,
   restricted to the prefixes below `P`, asserting the rendered text is
   byte-identical to what the pre-0312 renderer produced. S3's hazard
   fails exactly here and nowhere else, and it fails silently three bytes
   short if only the accepted path is tested.

### B — the verdict

3. `a_cut_tail_after_enough_fields_is_a_message` — G1.
4. `a_cut_tail_after_too_few_fields_is_bytes` — G4, at `P - 1`.
5. `a_lying_length_prefix_is_still_not_a_message` — the same payload with
   trailing bytes after the overrunning field, so the frame does not end
   at the buffer's end. Declines. G2, and the test that fails if
   `frame_ends_at_eof` is dropped or hardcoded true.
6. `a_cut_tail_plus_one_more_flaw_is_not_a_message` — a `TAG_OOR` earlier
   in the payload. G3.
7. `a_cut_group_is_still_not_a_message` — N3, and the specific case of a
   string whose last byte is a `START_GROUP` tag.
8. `frame_ends_at_eof_is_false_below_a_satisfied_prefix` — a unit test on
   the threading itself: a complete outer LEN field, containing a cut
   inner one, inside a file with more content after. Two levels, and the
   inner frame must not inherit the outer's answer.

### C — the negative controls, as tests

9. `binary_files_do_not_become_messages` — PNG, ELF, gzip, prose, JSON,
   each rendered untyped whole and at a handful of cut offsets, each
   asserted to produce one bytes line. These are S5's measurement frozen
   into a regression guard, and they are the tests that fail if `P` is
   later lowered without redoing the measurement.

### D — protolens and end to end

10. `arena_and_render_agree_on_a_forgiven_cut` — spec 0302 already has
    `ArenaSink` allocate child slots over a cut payload; the render now
    produces the matching lines. Assert `assert_line_counts_are_exact`.
11. `export_binary_is_byte_identical_for_a_forgiven_cut` — the
    arena-side round trip, which does not go through the text encoder.
    boblog: 20 198 bytes in, 20 198 out. Today, untyped, it is 20 202 —
    a spurious tag and a 3-byte length.
12. End to end — open boblog with no type at all and assert `/1`
    resolves, the three complete entries are navigable, and the fourth
    carries `TRUNCATED_MESSAGE; MISSING: 1024`. No override, no `-t`.
13. `prototext decode --raw` on the same file produces the same
    structure. G6.

## Measured outcome

**`P = 1`.**

The sweep read `complete_fields` and `forgiven_tail_cut` off the probe
directly instead of recompiling per value, so every `P` in `0..=16` was
measured in one pass over each payload. `P = 0` was included although S5
asked for `1..16`, because without it there is no way to see how much of
the admission is the threshold's doing and how much is spec 0266's.

### The negative controls

PNG, ELF, gzip, UTF-8 prose and JSON, cut at each of **72 742** offsets.
**One** false positive, and it is the same one at every `P` from 0 to 16:
a ten-byte PNG prefix — the signature `\x89PNG\r\n\x1a\n` and two zero
bytes — that parses clean as a two-byte tag and one `fixed64`, reaching
the end of the buffer with nothing cut. It forgives nothing; it is spec
0266's own verdict and it predates this spec. **The rate this spec was
not allowed to move did not move.**

Only JSON reached a forgivable cut at all (413 of its offsets); prose,
ELF, gzip and PNG reached zero between them, which is worth stating
because it means for four of the five files `P` was never consulted.

### Where `P` actually separates

The file sample cannot rank one `P` against another — spec 0266's
invalid-token rule rejects those payloads before the threshold is
reached. So the question was asked where it can be answered: over
**every** byte string of a given length.

| width | strings | `P = 0` | `P = 1` | `P ≥ 2` |
|---|---|---|---|---|
| 1 | 256 | 0 % | 0 % | 0 % |
| 2 | 65 536 | 5.88 % | 2.98 % | 2.98 % |
| 3 | 16 777 216 | 8.96 % | 3.06 % | 3.06 % |
| 4 | 4 294 967 296 | 10.66 % | 2.51 % | 2.43 % |

`P ≥ 2` is spec 0266's baseline exactly: at four bytes, 104 158 576
strings, which is the number admitted when no cut is ever forgiven. So
the whole cost of this spec, over every four-byte string in existence, is
**3 714 750 strings — 0.086 percentage points**. The cost of `P = 0` is
+8.24 points, which is the threshold earning its existence: without it a
lone cut field with nothing in front of it counts as evidence of itself.

Every `P` above 1 is indistinguishable from `P = 1` at these widths.

### What each step up costs

googleapis, 375 blobs, **21 782** cut offsets of which 18 399 reached a
forgivable cut; and `fixtures/descriptor.pb`, **18 752** cut offsets.
Percentage of cuts recovered:

| `P` | googleapis | descriptor.pb |
|---|---|---|
| 0 | 89.22 % | 99.88 % |
| 1 | 68.74 % | 98.67 % |
| 2 | 54.05 % | 97.33 % |
| 3 | 40.67 % | 87.21 % |
| 4 | 27.33 % | 81.98 % |
| 8 | 12.19 % | 8.11 % |
| 16 | 5.66 % | 0.05 % |

So `P = 1` buys nothing over `P = 2` on the false-positive side and gives
up 14.7 points of recall to reach it. Nothing above 1 is paid for.

### The prediction S5 made, priced

S5 named `08 01 0A 09 78` as the failure mode of `P = 1`: an
ordinary-looking scrap of binary reported as a message. It is real —
`a_cut_tail_after_enough_fields_is_a_message` is that fixture, and it
descends. The measurement's answer is that the whole family it belongs to
is 0.086 points of four-byte strings, against a 14.7-point recall loss to
refuse it. Recorded rather than argued away.

S5 also expected boblog to want a large `P`: it forgives at `P ≤ 3` and
declines at `P ≥ 4`, having shown exactly three whole entries before its
cut. `P = 8` would not in fact have worked.

### The document this spec exists for

`grpconf/fixtures/boblog`, 20 198 bytes, untyped and with no override:

- `protoc --decode_raw` — exit 1, `Failed to parse input.`, no output.
- `prototext decode --raw` — exit 0, 1 215 lines, with
  `1 {  #@ message; TRUNCATED_MESSAGE; MISSING: 1024` on the fourth
  entry's header and `TRUNCATED_BYTES; MISSING: 1024` on the `response`
  field inside it.
- Binary export: 20 198 bytes in, **20 198** out. Before this spec it was
  20 202 — a spurious tag and a 3-byte length, which is S3's hazard
  measured rather than reasoned about.

### Where the implementation departed from the test plan

- **A2 folded into A1.** `truncating_anywhere_round_trips`'s schema-less
  sweep already asserts the round trip at every cut offset, forgiven and
  declined alike. A declined cut that re-encoded three bytes short fails
  there, which is what A2 was for; a second test asserting text-identity
  with the pre-0312 renderer would pin the rendering, not the claim.
- **C9 recast.** "These files produce one bytes line" is false and was
  false before this spec — see the PNG prefix above. The assertion is
  instead that no offset of any of the five descends *by being forgiven*,
  which is the claim the measurement actually supports, at every offset
  rather than a handful.
- **D11/D12 use boblog's shape, not boblog.** `grpconf/fixtures/` is
  subtracted from the nix `workspaceSrc` as demo-only, so a unit test
  cannot `include_bytes!` it. `THREE_THEN_CUT` is three whole records and
  a fourth cut, which is what the assertions turn on; the file's own
  numbers are above.

### One consequence the specification did not name

A schema-less cut field with **zero** available bytes now descends, as an
empty `TRUNCATED_MESSAGE`. `P` does not stop it and should not: nothing
was forgiven. The payload has no cut *in* it — it has no bytes at all —
so `says_message`'s third clause is not reached, exactly as its own doc
comment says ("a payload with no cut is judged as it was before, however
few fields it has").

It is consistent rather than surprising: `0a 00`, a *complete* empty LEN
field with no schema, has always rendered `1 {}`. Spec 0266's verdict on
an empty payload is and was "message"; all this spec changed is that a
cut field now reaches the cascade that asks. `prototext`'s
`a_malformed_field_gets_its_own_one_line_span` was the only test resting
on the old routing, and its fixture gained one garbage byte so that it is
still testing a malformity.

### Two bugs the arena work surfaced

Both in `ArenaSink`, both found by `assert_cached_verdicts_are_real`:

- The spec-0302 descent passed `frame_ends_at_eof: true` unconditionally,
  which would forgive a cut in a frame in the middle of the blob. Fixed
  by giving `ArenaSink` a `blob_len` and re-deriving the flag — this
  spec's rule holds in the arena too or it holds nowhere.
- `well_framed_len_payload`, the test-side oracle, only recognized an
  exactly-fitting LEN field, so it expected `false` for cut fields — which
  since this spec *are* cascade nodes. The cached `true` it flagged at
  `descriptor.pb` offset 2884 was correct: the 14 available bytes of
  `request_type_url` parse as a `fixed64` and a `fixed32`, consuming all
  14.
