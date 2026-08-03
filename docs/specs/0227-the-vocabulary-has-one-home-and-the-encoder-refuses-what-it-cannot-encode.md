<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0227 — the vocabulary has one home, and the encoder refuses what it cannot encode

Status: draft
App: prototext-core, protolens, reproto
Refs: docs/specs/0226-a-fixture-shows-every-anomaly.md (the fixture that
        proved which tokens the renderer actually emits, and the six
        proposals this spec picks up); docs/specs/0225-the-wire-bytes-are-
        shown-under-each-line.md (the tier classifier and its drift test)

## Background

Spec 0226 built `grpconf/anomalies.pb`, drove every annotation token out
of the renderer, and pinned the result with a set-equality test. Doing so
surfaced five defects and one documentation drift that the fixture proves
are real but deliberately left alone (0226 N1):

1. **Two emitted tokens are unclassified.** The renderer emits
   `INVALID_PACKED_RECORDS` (`render_text/packed.rs:315`) and
   `INVALID_STRING` (`render_text/sink.rs:539`), but neither appears in
   `protolens/src/annotation.rs`'s `INVALID` nor in
   `reproto/tree-sitter-textproto/highlights.scm`. Both render comment-gray
   in protolens today, on both rows — an invalid blob that looks fine.
   Both are trivially reachable, and the fixture reaches them.

2. **Two classified tokens are never emitted.** `packed_ohb` and
   `packed_truncated_neg` are v1 residue. They are *list*-valued
   (`packed_ohb: [1, 2, 3]`); v2 spells the same facts one element line at
   a time as `ohb: N` and `neg`. Nothing in the renderer writes them.

3. **A trailing `#` comment on an unannotated line silently loses the
   field.** `seconds: 5  # note` is never split: `split_at_annotation`
   finds no `  #@ `, the value `5  # note` fails to parse, and the line is
   dropped. `prototext encode` then exits 0 having written a blob that is
   missing a field. Verified.

4. **An annotation runs to end of line, unbounded.** `#@ varint; int64 = 1
   # note; val_ohb: 4` encodes as `08 85 80 80 80 00`: the prose after the
   `#` was read as a modifier. This is a *feature* of the format — the
   annotation is the rest of the line — but it need not be, because
   `annotation_bounds` already computes where a following `#` sits, for
   free (see S8).

5. **The encoder cannot fail.** `encode_text_to_binary_into` returns `()`.
   Every line it does not understand is skipped. There is no signature on
   which to report which line, and callers therefore cannot tell an empty
   document from a wholly unparseable one — `protolens/src/blob.rs:100`
   says so in a comment.

6. **`docs/prototext/design.md`'s modifier table has drifted.** It spells
   `end_tag_ohb` for `etag_ohb`, and omits `ohb`, `neg`, `truncated_neg`
   and every `INVALID_*`. `docs/prototext/annotation-format.md` is
   accurate.

## Goals

- **G1.** One home for the emitted vocabulary. `prototext-core` publishes
  it; `protolens/src/annotation.rs` and `highlights.scm` are checked
  against that publication rather than maintaining private copies.
- **G2.** `INVALID_PACKED_RECORDS` and `INVALID_STRING` are colored as
  invalid on both rows.
- **G3.** `packed_ohb`, `packed_truncated_neg` and the v1 packed-array
  encode path they exist to serve are gone.
- **G4.** `name: value  # note` encodes `value`, on an unannotated line as
  well as an annotated one.
- **G5.** An annotation ends at the next `#`, not at end of line.
- **G6.** A line the encoder cannot encode is an error naming the line
  number, not a silent omission — including a value it cannot parse.
- **G7.** None of the above costs measurable encode throughput.

## Non-goals

- **N1.** No new rendered tokens, and no change to what the renderer
  emits. This spec moves a list and deletes dead code; the renderer's
  output is byte-identical throughout. The rule of
  `prototext_core_constraints` still stands: the rendered grammar may not
  grow a form the encoder has no arm for.
- **N2.** No change to the *annotated* trailing-comment semantics beyond
  bounding them at the next `#` (G5). `#@ int64 = 1; val_ohb: 4` keeps
  meaning what it means.
- **N3.** No new *validation*. The encoder reports the failures it already
  detects; it does not acquire checks it does not have today. It still
  does not verify that a value fits its declared type, that a field
  number is in range, or that braces balance.
- **N4.** No fuzz target. Tracked separately in `docs/scoring-flaws.md`.

## Specification

### The vocabulary moves into prototext-core (G1, G2)

- **S1.** A new module `prototext_core::annotation_vocabulary` publishes
  four `pub const [&str; N]` slices — `WIRE_TYPES`, `LANDMARK`,
  `NON_CANONICAL`, `INVALID` — holding exactly the tokens
  `serialize::render_text` can emit. After S5's deletions that is 5 + 1 +
  9 + 15 = 30 tokens, the same 30 spec 0226 S2 tabulated.

  It is a plain module of the crate proper, not behind a feature. It costs
  four static string tables; a feature gate would cost a build
  configuration that the two consumers would then both have to enable, and
  a `cfg` a future reader has to reason about.

  The lists are *not* threaded through the renderer's emit sites. There
  are 71 literal occurrences across six files, most inside `write!` format
  strings on the hot render path, and substituting a constant into a
  format string does not make the two agree — it only moves the copy. What
  keeps the published list honest is S7's test, which drives the real
  renderer and compares sets.

- **S2.** `prototext-core/tests/anomaly_fixture.rs` drops its private
  `VOCABULARY` table and asserts set *equality* against the concatenation
  of the four published slices. This is the mechanism of S1: the fixture
  exhibits every token, so a token the renderer emits but the module does
  not publish fails as `extra`, and one published but not emitted fails as
  `missing`.

- **S3.** `protolens/src/annotation.rs`'s `WIRE_TYPE_NAMES`, `LANDMARK`,
  `NON_CANONICAL` and `INVALID` become re-exports of the prototext-core
  constants. `tier_of` and `vocabulary()` are unchanged in behavior and
  keep their doc comments — including the paragraph explaining why
  `ENUM_UNKNOWN` is filed non-canonical, which is protolens's judgement
  about severity and does not belong in the crate that merely emits the
  token.

  Severity tiering stays protolens's, not prototext-core's: the crate
  publishes *what it emits*, grouped the way `annotation-format.md`
  already groups it, and protolens decides what a group looks like.

- **S4.** `highlights.scm`'s `@annotation.invalid` `#any-of?` list gains
  `INVALID_PACKED_RECORDS` and `INVALID_STRING`. Its
  `@annotation.non_canonical` list loses `packed_ohb` and
  `packed_truncated_neg` (S5). `every_keyword_is_colored_by_its_tier`
  (`protolens/src/colorize.rs:844`) already iterates
  `annotation::vocabulary()`, so after S3 it covers the new tokens without
  being touched.

### The v1 packed-array path is deleted (G3)

- **S5.** Delete, as one change — these are not isolated entries but a
  connected path:

  - `encode_annotation.rs:82` (`"packed_ohb"`) and `:87`
    (`"packed_truncated_neg"`) from `parse_annotation`'s modifier `match`;
  - the `Ann` fields `records_overhung_count`,
    `records_neg_int32_truncated` and `enum_packed_values`;
  - `fields.rs:428` `encode_packed_array_line` in full, and its only
    call site, the `value_str.starts_with('[')` guard at `fields.rs:227`;
  - the two names from `protolens/src/annotation.rs`'s `NON_CANONICAL`
    (11 → 9) and from `highlights.scm`.

  The guard at `fields.rs:227` is the v1 *value* form — a whole packed
  record on one line as `[1, 2, 3]`. v2 renders a packed run as one text
  line per element with `pack_size: N` on the first, handled by the
  per-line state machine at `mod.rs:344-382`, which is untouched. The
  three `Ann` fields are read only by `encode_packed_array_line`.

  Removing the guard also removes the one path by which a value
  legitimately *starting* with `[` — there is none in v2 output; an
  extension name is on the LHS — could be mis-encoded.

### A `#` terminates a value, and an annotation (G4, G5)

- **S6.** `annotation_bounds` (`encode_text/mod.rs:56`) returns a triple,
  `(value_end, ann_start, ann_end)`. `ann_end` is the loop's existing
  `end` local at the moment of a successful return: the position of the
  nearest `#` to the *right* of the marker that failed both tests, or
  `b.len()` if there was none.

  This is free. `end` is already maintained (`end = p` at the bottom of
  each failed iteration); no byte is re-examined and no branch is added.
  It refutes the earlier assumption that bounding an annotation would cost
  a forward scan of every annotation.

  A `#` inside a *string value* is to the left of the marker and is never
  visited by the right-to-left walk, so it cannot truncate an annotation.

- **S7.** `split_at_annotation` returns `&line[ann_start..ann_end]`
  instead of `&line[ann_start..]`.

- **S8.** When `split_at_annotation` returns an *empty* annotation, the
  value part is additionally scanned left-to-right for a `#` outside a
  double-quoted string, and truncated there. Only that arm: every line the
  renderer emits carries an annotation, so the scan is never entered on
  the hot path and G7 holds by construction.

  The scan must be quote-aware, and must honor backslash escapes, because
  `1: "a # b"` encodes correctly today and must continue to.

### The encoder reports what it could not encode (G6)

- **S9.** `encode_text_to_binary_into` returns `Result<(), CodecError>`
  and `encode_text_to_binary` returns `Result<Vec<u8>, CodecError>`. The
  variant is the already-declared but so far unconstructed
  `CodecError::TextDecodeFailed(String)`, whose message names the
  1-based line number of the offending line and quotes it.

- **S10.** Three conditions are errors:

  1. **Input that is not UTF-8.** Today `mod.rs:195` returns silently,
     which — as its own comment says — is indistinguishable from an empty
     document. protolens's `Blob::load` duplicates the check today purely
     to have somewhere to report it; after this change that duplicate
     goes.
  2. **A value line with no `:`.** A line that is neither a brace line,
     nor a comment, nor a comment-only annotation, and contains no colon,
     is the shape a dropped field takes. It is `mod.rs:334`'s `else {
     continue }`.
  3. **A value `parse_num` rejects.** Every one of these sites already
     holds the `Option` and already branches on it; reporting instead of
     dropping costs a discriminant the code has in hand. The sites:
     `fields.rs:336` (the general numeric arm), the `fixed64` and
     `fixed32` wire-type overrides (`:248`, `:260`), and every
     `if let Some(n) = parse_num(…)` in `encode_packed_elem`
     (`fields.rs:553`).

     A packed element is the strongest case for this. A dropped element
     does not shorten the *record* — the length prefix is computed from
     the payload afterwards — so the blob stays structurally valid and
     silently carries fewer elements than the `pack_size: N` that
     produced it. That is corruption, not omission, and nothing
     downstream can notice it.

  Nothing else. Two exclusions worth naming:

  - **An unmatched `}`** (`mod.rs:256`) stays ignored: `protolens export`
    of a subtree is a legitimate producer of brace-unbalanced text and
    turning that into an error would break it.
  - **The lenient `true`/`false` fallbacks** (`fields.rs:241` under the
    `varint` override, `fields.rs:607` under packed `bool`) are
    *tightened*, not merely reported. Today they read "anything that is
    not `true` is `false`", so `x: garbage  #@ varint` encodes `0` — a
    wrong value, not a missing one, which is the worst of the three
    outcomes. After this spec the fallback accepts exactly the two
    literals and anything else is condition 3.

- **S11.** `encode_scalar_line` and `encode_packed_elem` return
  `Result<(), &'static str>` — a static reason, not a formatted message,
  so that a failure costs no allocation and no `String` machinery is
  linked into the success path. The line loop in `encode_text/mod.rs` is
  the only place that knows the line number and its text, so it is the
  only place that builds the `CodecError::TextDecodeFailed` message.

  The ripple is mechanical: `encode_scalar_line` has one call site
  (`mod.rs:384`), `encode_packed_elem` has two (`mod.rs:345`, `:371`),
  and `encode_num` stays infallible because by the time it is reached the
  value has already parsed.

- **S12.** Call-site ripple. `render_as_bytes` and `render_as_text`
  already return `Result<_, CodecError>`, so the public API shape does not
  change — only the `?` inside them.

  - `prototext-core/src/lib.rs:128, 165` — propagate with `?`.
  - `protolens/src/blob.rs:119` — propagate into `Blob::load`'s existing
    error type; delete the UTF-8 pre-check and the comment at `:100`.
  - `prototext-pyo3/src/lib.rs:177, 302` — raise. A Python caller of a
    codec gets an exception on malformed input; returning a status would
    be silently ignorable, which is the defect being fixed.
  - `prototext-core/benches/codec.rs:116`, `render_text/mod.rs:1342,
    1402`, `protolens/tests/batch_export.rs`,
    `protolens/src/tui/tests/command_line.rs`, `blob.rs`'s tests —
    `.expect(…)` in test and bench code.
  - `prototext/src/run.rs` reaches the encoder only through
    `render_as_bytes`, so it needs no change beyond whatever error text it
    already prints.

### Documentation (item 6)

- **S13.** `docs/prototext/design.md`'s modifier table is replaced by a
  pointer to `docs/prototext/annotation-format.md`, which is accurate and
  is the reference the tools are written against. One table, one home.

- **S14.** `annotation-format.md` gains one sentence recording the settled
  terminator behavior: an annotation runs from `  #@ ` to the next `#` or
  end of line, and a value runs to the first `#` outside a string.

## Alternatives considered

**Thread the published constants through the renderer's emit sites.**
Rejected: 71 occurrences, most inside format strings on the hot path, and
it does not achieve the goal. `write!(f, "  #@ {TAG_OHB}: {n}")` is no
more coupled to the classifier than `"tag_ohb"` is — the coupling that
matters is *set membership*, which S2's test checks directly by running
the renderer.

**Put the severity tiers in prototext-core too.** Rejected: the tiers are
a display judgement, and one of them (`ENUM_UNKNOWN` as non-canonical) is
knowingly wrong for open enums and accepted anyway for reasons that are
about protolens's two rows. prototext-core has no opinion about color.

**Report only the two structural failures, and leave values alone.**
Rejected. The argument for it was that a per-value check would be a hot
path cost, and that is wrong: `parse_num`'s `Option` is already computed
and already branched on at every one of those sites, so the failure arm
is reached only when the encode is about to abort anyway. Detection is
not being added — only a return value where there was a `return`.

**Keep the `varint`/`bool` "anything not `true` is `false`" fallbacks.**
Rejected. They are the only places in the encoder that turn an
unrecognized value into a *plausible* one. A caller cannot distinguish
`x: false` from `x: nonsense` in the output, so the leniency buys nothing
and hides the case the rest of S10 exists to surface. This is the one
behavioral change in S10 rather than a reporting change, and test-plan
item 10 is what establishes no real producer relies on it.

**Keep `packed_ohb`/`packed_truncated_neg` as accepted-on-input aliases.**
Rejected: nothing emits them, so nothing can round-trip through them, and
an encoder arm with no matching renderer arm is exactly the asymmetry spec
0174 removed by deleting a token rather than by adding a renderer arm.
That is the precedent.

**Report the unmatched `}` too.** Rejected — see S10.

## Test plan

1. `the_fixture_covers_the_whole_vocabulary` (existing, retargeted per S2)
   — the published list and the renderer's real output are the same set.
2. `every_keyword_is_colored_by_its_tier` (existing, `colorize.rs`) — now
   covers `INVALID_PACKED_RECORDS` and `INVALID_STRING` by construction,
   and fails if `highlights.scm` is not updated.
3. `no_keyword_belongs_to_two_tiers` (existing) — unchanged, but now runs
   over the published lists.
4. `a_bracketed_value_is_no_longer_special` — a v2 rendering that
   round-trips unchanged after S5, proving the deleted guard was dead.
5. `a_trailing_comment_on_an_unannotated_line_is_dropped` —
   `seconds: 5  # note` encodes to `08 05`, not to nothing.
6. `a_hash_inside_a_string_value_is_not_a_comment` — `1: "a # b"` still
   encodes the full string. Both with and without an annotation.
7. `an_annotation_ends_at_the_next_hash` —
   `x: 5  #@ int64 = 1  # val_ohb: 4` encodes `08 05`, not the
   over-long form.
8. `a_line_with_no_colon_names_its_line_number` — the error message
   contains the 1-based line number and the line's text.
9. `non_utf8_input_is_an_error` — replaces `blob.rs`'s deleted pre-check
   test.
9a. `an_unparseable_value_is_an_error` — `seconds: abc  #@ int64 = 1`
    fails and names its line, rather than encoding nothing.
9b. `an_unparseable_varint_is_no_longer_encoded_as_false` — the
    tightened fallback: `x: garbage  #@ varint` fails, while
    `x: true` and `x: false` still encode `1` and `0`.
9c. `a_short_packed_element_is_an_error` — a packed run whose element
    fails to parse fails, rather than emitting a record with fewer
    elements than its `pack_size` announced.
10. **Corpus re-encode.** Re-encode all 375 `instances/**/*.pb` renderings
    from the googleapis DB and require byte-identical output and zero
    errors. This is the check that S5–S8 changed nothing for real input.
11. **`bin/bench -p prototext-core --bench codec`**, baseline against
    baseline first (`benchmark_noise_floors`: never trust a Criterion
    delta without the same-binary floor), then before/after on
    `B1_encode_text_to_binary`.

## Measured outcome

Filled in at implementation.
