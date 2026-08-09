<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0266 — a payload with any flaw in it is not a message

Status: implemented
Implemented in: 2026-08-09
App: prototext-core
Refs: docs/specs/0097-raw-recursive-lendel.md (the three-step cascade
        whose Step 1 this governs), docs/specs/0110-render-sink-unification.md
        (`ProbeSink`, and open issue 1 — the roll-up this generalizes),
        docs/prototext/annotation-format.md (the invalid / non-canonical
        split this spec adopts as its rule)

## Background

An unknown LEN field is probed as a message before it is tried as a
string. The probe is wrong often enough that this is the third fix to
it, and each previous fix added one anomaly to a list:

| | what was added | what was left out |
|---|---|---|
| spec 0097 | an unterminated group counts | everything else |
| `b3c06aa` | the same, after the Sink rewrite silently dropped it | *"a mismatched `END_GROUP` still does not count: it closes, and the render annotates it"* |

The payload that exposes it this time is the string
`ANALYST_UPDATE_VERDICT`, reachable in three bytes of shell:

```console
$ printf '\x0a\x16ANALYST_UPDATE_VERDICT' > /tmp/verdict.pb
$ prototext decode --raw /tmp/verdict.pb
1 {  #@ message
 8: 0x555f5453594c414e  #@ fixed64
 10: 68  #@ varint
 8: 0x49445245565f4554  #@ fixed64
 8 {  #@ group; END_MISMATCH: 10
 }
}
```

Every "field" there is a letter. `A` (0x41) is a FIXED64 tag for field
8; `P` (0x50) a VARINT tag for field 10 with `D` as its value; and the
pair `C` `T` is 0x43 — `START_GROUP` field 8 — followed by 0x54 —
`END_GROUP` field **10**. The group closes against the wrong field
number, which is why the render says `END_MISMATCH: 10`, and the probe
does not count it.

Uppercase text is unusually good at producing that pair: wire type 3
(open) is any byte ending in `011` — `C K S [ c k s {` — and wire type 4
(close) any byte ending in `100` — `D L T \ d l t |`. A
`SCREAMING_SNAKE` enum name is close to a worked example.

**The defect is the method, not the omission.** The probe enumerates the
anomalies that disqualify a payload, so any anomaly nobody enumerated is
accepted by default, silently, and is found only when a user reads a
string rendered as nonsense. `b3c06aa`'s stated reason for excluding the
mismatched end — *"the render annotates it"* — inverts the test: an
annotation is the renderer saying the bytes are defective, which is
evidence against the payload being a message, not for it.

## Goals

- **G1.** The probe declines a payload whose rendering as a message
  would report anything **invalid**, by a definition that already
  exists and covers the whole vocabulary rather than a list of three.
- **G2.** A newly added annotation cannot default to "accepted". Adding
  one must fail to compile until somebody classifies it.
- **G3.** No information is lost when the probe declines.

## Non-goals

- **N1. No change to the rendered grammar.** No new `#@` token, no
  change to any existing one, and no change to the invalid /
  non-canonical classification of any token. This spec changes which of
  two *existing* renderings a payload gets — nested message, or
  string/bytes — and both already round-trip.
- **N2. The probe stays shallow.** A LEN field inside the payload
  remains opaque to it (spec 0110 §2). Its own contents get their own
  cascade when they are rendered; making the probe deep would make
  Step 1 quadratic and is not what any of the three defects needed.
- **N3. The schema-driven path is untouched.** A field a descriptor
  declares to be a message is never probed and must not start being.
  This governs unknown fields only.
- **N4. Steps 2 and 3 are untouched** — a declined payload is still a
  UTF-8 string if it is one, and raw bytes otherwise.

## Specification

- **S1. The rule: an invalid token disqualifies, a non-canonical one
  does not.** Step 1 accepts a payload if and only if parsing it as a
  message consumes every one of its bytes **and** would emit no token
  from the invalid class.

  The classification is not invented here. `annotation-format.md`
  already splits the vocabulary, and the split is exactly the right
  one for this question: an invalid token means "data integrity issue",
  while a non-canonical token is "losslessly recoverable — round-trips
  exactly". A message produced by an eccentric but working encoder is
  still a message. A record whose framing contradicts itself is not.

  **The case of the token is the verdict**: ALL CAPS disqualifies,
  lower case does not. That is already the documented convention, so
  the rule needs no second table to be kept in sync.

- **S2. The vocabulary, and which part of it the probe can see.** The
  probe runs with no schema, so every schema-dependent token is
  unreachable from it. Of 30 tokens, 12 can occur under a probe:

  | Token | Class | Reaches `ProbeSink` via | Counted today |
  |---|---|---|---|
  | `INVALID_TAG_TYPE` | invalid | `malformed(InvalidTagType)` | yes |
  | `INVALID_VARINT` | invalid | `malformed(InvalidVarint)` | yes |
  | `INVALID_FIXED64` | invalid | `malformed(InvalidFixed64)` | yes |
  | `INVALID_FIXED32` | invalid | `malformed(InvalidFixed32)` | yes |
  | `INVALID_LEN` | invalid | `malformed(InvalidLen)` | yes |
  | `TRUNCATED_BYTES` / `MISSING` | invalid | `malformed(TruncatedBytes)` | yes |
  | `INVALID_GROUP_END` | invalid | `malformed(InvalidGroupEnd)` | yes |
  | `OPEN_GROUP` | invalid | `end_nested(close_facts: None)` | yes |
  | **`END_MISMATCH`** | **invalid** | `GroupCloseFacts.mismatched_group_end` | **no — gap** |
  | **`TAG_OOR`** | **invalid** | `TagFacts.tag_oor` | **no — gap** |
  | **`ETAG_OOR`** | **invalid** | `GroupCloseFacts.end_tag_is_out_of_range` | **no — gap** |
  | `tag_ohb` | non-canonical | `TagFacts.tag_ohb` | no — correct |
  | `len_ohb` | non-canonical | `TagFacts.len_ohb` | no — correct |
  | `val_ohb` | non-canonical | `ScalarValue::Varint.val_ohb` | no — correct |
  | `etag_ohb` | non-canonical | `GroupCloseFacts.end_tag_overhang_count` | no — correct |

  The remaining 15 need a schema and so cannot arise here: the invalid
  `INVALID_PACKED_RECORDS`, `INVALID_STRING` and `TYPE_MISMATCH`; the
  non-canonical `truncated_neg`, `nan_bits`, `ohb`, `neg`; the
  informational `ENUM_UNKNOWN`; `pack_size`, which is bookkeeping and
  not an anomaly at all; and the five lower-case wire-type names.

  **So the whole behavioral change is three rows.** `END_MISMATCH` is
  today's bug; `TAG_OOR` and `ETAG_OOR` are the same bug not yet
  reported. `TAG_OOR` is worth its own sentence: a `0x00` byte is a tag
  for field 0, so every NUL in a string currently helps that string
  pass for a message.

- **S3. The classification is enforced by the compiler, not by
  vigilance.** Everything a renderer can say about a record reaches a
  sink through exactly four payload types: `TagFacts`, `ScalarValue`,
  `GroupCloseFacts` (and the `Option` around it, whose `None` is
  `OPEN_GROUP`), and `MalformedKind`. `ProbeSink` destructures each of
  them **exhaustively — no `..`, no `_` arm** — so a field added to any
  of them, or a variant added to either enum, stops the build at the
  probe until someone classifies it against S1.

  This is the whole of G2. The four data-carrying `Sink` methods have
  no default bodies either, so a new anomaly channel is also a compile
  error; the five methods that *do* default (`treat_len_as_opaque`,
  `unknown_len_is_message`, `row_budget_spent`, `note_undescended`,
  `tracks_level`) are policy hooks carrying no anomaly data.

- **S4. One statement of the verdict.** `probe.malformity_count() == 0
  && next_pos == data.len()` is currently written out at
  `len_field.rs:101` and again, deliberately "verbatim", in `arena.rs`'s
  test of the cached bit. It becomes a single function on `ProbeSink`
  that both call, so the cached verdict and the live one cannot drift
  for a third reason.

- **S5. Declining costs nothing the user cannot undo.** The maximal
  arena recurses regardless of the verdict (spec 0216 S14), so the child
  nodes still exist and `:override-as` still declares the payload a
  message. This is what makes strictness the cheap direction: a false
  negative is one keystroke away from repair, a false positive is a bug
  report.

## Alternatives considered

**Add the mismatched end to the list, as the last two fixes did.** Three
lines, ships today, and leaves `TAG_OOR` and `ETAG_OOR` still silently
accepted — two more instances of the same bug waiting for two more
reports. The list has been wrong at every point in its life.

**Disqualify on non-canonical tokens too.** This spec's first draft did,
on the argument that no conformant encoder emits an over-encoded varint,
so one is better evidence of misread text than of a message. It is
rejected because it contradicts a classification the project has already
made and documented: those encodings round-trip exactly and are legal
protobuf, and a rule that says "invalid" while meaning "invalid, plus
four things that are merely unusual" is the same unprincipled list this
spec exists to delete. If the corpus later shows overhangs are a
significant false-positive source, that is an argument for reclassifying
them in `annotation-format.md` — one place, affecting every consumer —
not for special-casing them here.

**Drive the probe from `TextSink`'s annotation builder**, so that it
declines exactly when the render would emit an ALL-CAPS token. One
implementation instead of two agreeing ones, and the right answer if a
fourth instance ever appears. Not taken now because that builder's
`mods` list mixes the two classes with ordinary type annotations, so the
refactor is to split them, which is a larger change to `sink.rs` than
the defect warrants. S3's compile error covers the same ground for the
cost of a destructuring pattern.

**Make the probe deep.** Would catch payloads whose nested LEN fields
are garbage. It also makes Step 1 quadratic in nesting depth, and none
of the three defects on record involved a nested LEN field.

## Test plan

1. `a_string_that_opens_a_group_is_not_a_message` — the reproduction
   above, asserted as a string.
2. `a_mismatched_group_end_fails_the_probe` — the reported defect.
3. `an_out_of_range_field_number_fails_the_probe` — `TAG_OOR`, the gap
   nobody has reported yet.
4. `a_mismatched_end_tag_out_of_range_fails_the_probe` — `ETAG_OOR`.
   Per the annotation fixture's notes this is not writable directly; it
   falls out of `END_MISMATCH: 536870912`.
5. `an_over_encoded_tag_still_probes_as_a_message` — the other half of
   S1, and the test that fails if someone quietly re-adopts the
   rejected alternative.
6. ~~`a_clean_nested_message_still_probes_as_one`~~ — the negative
   control, without which every test above passes on a probe that rejects
   everything. Not added: `probe_sink_recognizes_valid_nested_message`
   already is that test, and item 5 is a second one.
7. `the_cached_verdict_matches_a_live_probe` — `arena.rs`'s existing
   test, now sharing S4's single function.
8. `prototext-core/tests/anomaly_fixture.rs` must still pass unchanged:
   `grpconf/anomalies.pb` exercises all 30 tokens, so it is the
   standing check that this spec changed no rendering it should not.
9. **Corpus.** Render `googleapis.desc` with `--raw` before and after,
   and count the nodes that change from message to string/bytes. Then
   check the drained export round-trips byte-identically, since a
   payload moving between the two renderings must re-encode to the same
   bytes either way.

## Measured outcome

Implemented 2026-08-09. The reproduction now reads
`1: "ANALYST_UPDATE_VERDICT"  #@ string`.

**Corpus (test-plan item 9).** `googleapis.desc` rendered with `--raw`
before and after, 85 998 872 → 85 848 513 bytes of text:

| | before | after |
|---|---|---|
| nodes rendered as a message | 801 627 | 799 950 |
| `END_MISMATCH` | 1 760 | **0** |
| `TAG_OOR` | 405 | **0** |
| `ETAG_OOR` | 405 | **0** |

**1 677 nodes flip**, 0.21% of the messages: 1 363 to a string and 314 to
bytes. Not one invalid token survives anywhere in the render — the three
S2 gaps were the only way any of them could appear under a probe, so the
whole class is gone at once rather than three reports apart. `TAG_OOR`
and `ETAG_OOR` occur exactly 405 times each, which is the pairing S2
predicted: an out-of-range end tag cannot match its open tag either.

Both renders re-encode to `googleapis.desc` byte for byte (`cmp` clean,
25 585 977 B), before and after alike — a payload moving between the two
renderings must, and does.

**Did anything genuinely a message start rendering as bytes? No**, and
the 314 byte flips are the clearest evidence in the run. They are
elements of `repeated` fields whose every *sibling* already rendered as
bytes; the flipped element was the one that happened to parse, and it now
matches its neighbors:

```text
  1 { 1: "\004\023\002\005"  2: "\272\003\002\307\003\004"  #@ bytes  }
  1 { 1: "\004\023\002\006"  2: "\310\003\002\333\003\004"  #@ bytes  }   ← flipped
  1 { 1: "\004\023\002\007"  2: "\334\003\002\347\003\004"  #@ bytes  }
```

The 1 363 string flips are `SCREAMING_SNAKE` enum names and CamelCase
type names — `SecurityCategory`, `CLEARED`, `KEYWORD`, `StartupType`,
`CLUSTER`, `Subject` — exactly the shape the Background predicted.

One deviation: test-plan item 6 was not added, because
`probe_sink_recognizes_valid_nested_message` already is that control.

`prototext-core/tests/anomaly_fixture.rs` passes unchanged, as does the
rest of the workspace.
