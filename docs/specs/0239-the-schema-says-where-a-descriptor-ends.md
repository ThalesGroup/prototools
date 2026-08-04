<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0239 — the schema says where a descriptor ends

Status: draft
App: protoscan | fdp-scan-pyo3 | prototext
Refs: docs/specs/0238-….md (the `SCAN` policy, `EntryScore.termination`
        and the extension-range precondition this consumes);
        docs/protoscan/scan.md (the investigation: the boundary bug, the
        accept-rule measurements and the cost table)

## Background

`protoscan` finds `FileDescriptorProto` records in an unframed haystack.
It decides where one ends with a hand-rolled, schema-free wire walk
(`fdp-scan-pyo3/src/lib.rs`, `walk_protobuf_fields`). Two things are
wrong with it, both measured in `docs/protoscan/scan.md`:

**The boundary is wrong.** The stop rule wants to say "a second field 1
ends this record", but tests it by calling `looks_like_fdp_start`, which
also demands the length be `<= 200` and the payload decode as a plausible
`.proto` path. Those are the conditions for finding where a candidate
*starts*. An outer `FileDescriptorSet` record length is 291, 295, 352 —
neither. So on `googleapis.desc` the boundary never fires and the walk
skips each following file as one very long `name`: **1 candidate where
there should be 7 771**.

**The failure is silent.** `protoscan/src/protoscan/cli.py:39` accepts a
candidate if `ParseFromString` does not raise. Protobuf is permissive by
design: field 1 is singular, so last-wins overwrites `name` 7 770 times
and parsing the whole 25 MB blob *succeeds*. The tool prints one name and
exits 0.

Spec 0238 replaced the hand-rolled rule with a schema-derived one and
measured it against ground truth: **7 771 of 7 771** FDP payloads in
`googleapis.desc` report their own record's length under `Policy::Scan`,
given no length prefix, scored to the end of the whole buffer. This spec
spends that.

## Goals

- **G1.** The stop rule comes from the schema. `walk_protobuf_fields`
  and `looks_like_fdp_start` are deleted; the end of a candidate is
  `EntryScore.termination` from a `Policy::Scan` walk.

- **G2.** The accept rule is size-independent:
  `!vetoed && unknowns == 0 && mismatches == 0`. Never a threshold on
  `score()`.

- **G3.** Candidates are scored against `google.protobuf.FileDescriptorProto`
  and nothing else. One root.

- **G4.** `protoscan` stays a thin Python layer, and stays *dumb*: Rust
  scans, scores, decides, and returns only what it considers a genuine
  FDP. Python parses the accepted blobs to get a name, formats and
  writes. No policy lives in Python.

## Non-goals

- **N1.** Changing where a candidate *starts*. The `0x0A` anchor,
  `is_plausible_path` and `MAX_PROTO_NAME_LEN` stay exactly as they are.
  `SCAN` answers where a record ends; nothing in it nominates a start,
  and the schema cannot supply one — it says `name` is a `string` and no
  more. "Ends in `.proto`, not absolute, no `..` components, under 200
  bytes" is a claim about protoc's habits, and stays a heuristic.

- **N2.** Identifying what a blob *is*. protoscan finds
  `FileDescriptorProto` records; it does not classify its input. That is
  the general "what is this blob" profile, and it is `score_all` — 29 ms
  against `score_one`'s 10.8 µs, three orders of magnitude, proportional
  to root count. A `--db`-widened profile is separate work.

  In particular protoscan does **not** score against
  `FileDescriptorSet`; see Alternatives.

- **N3.** Compressed descriptors (`scan.md` §5). protodump does not
  handle them either; the phenomenon is real but orthogonal.

- **N4.** Recovering a boundary from a veto. 0238 N6: a veto fires inside
  a field already consumed, so it leaves no usable boundary. A vetoed
  candidate is rejected outright.

- **N5.** Sorting protoscan's output (`scan.md` §1.4), which that
  document calls a defect. It is not one. Discovery order *is*
  information — it is the layout of the records in the haystack, which is
  exactly what a caller scanning a binary may want to recover. Sorting
  destroys it and nothing gives it back; sorting downstream is a pipe
  away. If a `--sort` flag is ever added it must be opt-in.

## Specification

### What to embed

- **S1.** protoscan uses **`prototext`'s existing embedded WKT graph**.
  No new artifact, no new derivation. `google/protobuf/descriptor.proto`
  is in `prototext/wkt/SOURCES`, and reproto registers *every* message it
  compiles as a root — `phases.py:1964` appends unconditionally and
  recurses into nested types — so `FileDescriptorProto` is reachable by
  construction, not by luck.

  `scan.md` §4.1 proposed compiling `descriptor.proto` into its own
  embedded database. That was written before the WKT set included
  `descriptor.proto`; a second artifact would now be a second copy of the
  same graph, with a second thing to keep regenerated.

- **S2.** `default.nix`'s `wktRkyv` derivation gains
  `--emit-extension-ranges`, and `prototext/wkt/prebuilt/{wkt,wkt_index}.rkyv`
  are regenerated from it. Without this the graph carries
  `has_extension_ranges: false` and `Policy::Scan` trips 0238's S9 assert
  by design — an empty range set on every message would silently
  terminate on the first custom option of every descriptor.

  Note `wktRkyv` is a `let`-bound variable, not an exported attribute:
  `nix-build -A wktRkyv` fails. Regenerate per
  `prototext/wkt/prebuilt/README.md`.

### The scan

- **S3.** Per candidate start offset `s` nominated by the existing
  anchor: `score_one(&data[s..], "google.protobuf.FileDescriptorProto",
  graph, &opts)` with `policy: Scan`. The candidate's end is
  `s + entry.termination`.

  The walk is given the rest of the buffer, not a guessed length. That is
  the whole point: 0238's measurement is precisely that a `Scan` walk
  handed 25.6 MB reports the 291 bytes that belong to it.

- **S4.** The candidate is accepted if `!vetoed && unknowns == 0 &&
  mismatches == 0` (G2). Measured over all 7 771 genuine FDPs: 0 vetoed,
  0 unknowns, 0 mismatches, 0 non-canonical, 0 out-of-range — no false
  negatives on a real corpus.

  Accepting on the *defect counters* rather than on `score()` is what
  makes the rule size-independent. `score()` is `matches −
  10·unknowns − 15·out_of_range − 20·non_canonical − 30·mismatches`, a
  sum over matched fields: over those same 7 771 it ranges 8 … 171 309,
  four orders of magnitude purely as a function of file size. Any
  absolute cut-off would reject the small files or admit garbage, or
  both. The defect counters are zero for a genuine record at every size.

  With one root there is nothing to rank, so `score()` is not read at
  all.

  *The one false negative this admits*, stated so it is not rediscovered
  as a bug: a descriptor emitted by a **newer protoc**, carrying a
  declared field number our pinned `descriptor.proto` does not know,
  scores an unknown and is rejected. Custom options do not trigger this —
  0238's `Verdict::Extension` exempts an unknown inside a declared
  extension range, which is why S2 is a precondition — but a genuinely
  new field would. That is the price of a pinned schema, and it is
  visible as a version-skew failure rather than a silent wrong answer.

- **S5.** `offset` advances to the accepted candidate's end, as today. A
  rejected candidate advances `offset` by 1, as today.

### The boundary

- **S6.** `walk_protobuf_fields` and `looks_like_fdp_start` are deleted
  (G1). Their seven inline unit tests are re-pointed at the new path or
  deleted with them, case by case — the two garbage-name rejection tests
  belong to the *start* rule and survive; the boundary tests are replaced
  by the corpus check.

- **S7.** **`scan()`'s signature does not change**:
  `scan(buffer: bytes) -> list[tuple[int, int]]`, returning only accepted
  candidates. Rust scans, scores and decides; Python receives FDPs and
  nothing else (G4).

  This falls out of G3 and G4 together. With one root there is no winning
  FQDN to report, and with the accept decision in Rust there are no
  counters Python is entitled to act on — so the existing return type
  already carries everything a dumb layer needs. No `fdp_scan.pyi`
  churn, no `#[pyclass]`, no breaking change.

  `cli.py` keeps its `ParseFromString`, which stops being an accept gate
  and becomes what it should always have been: the way to read
  `fdp.name` for the output path.

  The cost, accepted deliberately: a rejected candidate is invisible, so
  a user facing a binary that yields nothing cannot ask why. If that ever
  needs answering it is a diagnostic surface — a separate entry point or
  a `--explain` — not a reason to widen the return type every caller
  uses.

## Alternatives considered

**Keep the hand-rolled walk and only fix its one condition** (0238 step
0). It is a smaller change and it produces the right 7 771 on this file.
Rejected as the *end state* because the rule it lands is 0238's S12 rule
2 restricted to field 1, hand-written: it generalizes to no other message
type, and it cannot see the two signals the schema gives for free —
undeclared field numbers, and repeats of the other five singular fields.
It was briefly considered as an interim fix to get a corrected
`protoscan` released ahead of this spec; that is not wanted, so it is
simply dropped.

**A threshold on `score()`.** Rejected by measurement, not argument: the
four-orders-of-magnitude range in S4 makes every threshold both too tight
and too loose.

**`score_all` per candidate instead of one named root.** Rejected on
cost (N2) and on precision: `score_all` on the 291-byte record leaves
21 444 non-vetoed survivors out of 49 255 roots. It ranks
`FileDescriptorProto` first, so it is not *wrong*, but veto alone is a
weak filter on a short blob and the accept rule of S4 is what does the
work regardless.

**Also scoring against `FileDescriptorSet`** (`scan.md` §4.1). Rejected,
and worth recording because the investigation recommends it.

Its stated purpose was to stop a whole descriptor set being mistaken for
one enormous FDP. `SCAN` against FDP alone already makes that impossible,
by the mechanism this spec is built on: `name` is singular, so the second
`file` entry's field-1 tag terminates the walk (0238 S12 rule 2). That is
exactly the 7 771/7 771 of 0238's step 6 — every record found its own
boundary with FDP as the only root. The second root buys nothing a
boundary needs.

What it would buy is *classification* of the input, which is not
protoscan's job (N2) and has no principled stopping point: if
`FileDescriptorSet`, then why not the other types a haystack might hold?
The line that admits exactly one exception is not a line. protoscan finds
`FileDescriptorProto` records; a tool that identifies blobs is the
`score_all` profile.

`scan.md` §4.1's table showing the scorer separates the two shapes
cleanly remains true — it just answers *can it* rather than *must
protoscan*.

**A separate embedded `descriptor.proto` database** (`scan.md` §4.1).
Rejected by S1 — it is now a duplicate of the WKT graph.

## Test plan

1. `test_scan_finds_every_record` — `googleapis.desc` yields 7 771
   candidates whose `(start, end)` pairs equal the length-prefix-derived
   boundaries exactly, in order. This is 0238 step 6's oracle: ground
   truth read off the `FileDescriptorSet` framing, independent of both
   the old and the new stop rule.
2. `test_a_record_never_swallows_its_successor` — a `Scan` walk started
   at the first record of `googleapis.desc` and handed the remaining
   25.6 MB terminates at 291 bytes, not at the buffer end. This pins the
   mechanism that makes the original bug unreintroducible with a single
   root, and fails loudly if `name`'s cardinality or S12 rule 2 ever
   stops being read.
3. `test_garbage_name_ending_in_proto_rejected`,
   `test_simple_garbage_name_rejected` — the surviving start-rule tests
   from `lib.rs`, unchanged (N1).
4. `test_accept_rule_is_size_independent` — a small FDP (score in the
   tens) and a large one (score in the six figures) are both accepted,
   pinning that no threshold crept back in.
5. `test_scan_needs_extension_ranges` — the S9 assert fires with a
   graph built without the flag, naming `--emit-extension-ranges` (S2).

## Measured outcome

Filled in at implementation.
