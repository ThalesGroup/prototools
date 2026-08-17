<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0313 — a record ends at its last clean boundary

Status: implemented
Implemented in: 2026-08-17
App: prototext-graph | fdp-scan-pyo3 | protoscan | reproto
Refs: docs/specs/0239-the-schema-says-where-a-descriptor-ends.md (the
        scanner this fixes: `Policy::Scan`, one root, the accept rule on
        defect counters),
      docs/specs/0238-an-extension-range-is-what-makes-an-unknown-field-innocent.md
        (S12-S13, the termination-offset contract; N6, the per-boundary
        snapshot deferred there and built here),
      docs/specs/0310-a-cut-file-is-not-a-wrong-file.md (N2, which
        refused to change `Policy::Scan` — narrowed, not overturned; its
        overrun veto stands)

## Background

**The last `FileDescriptorProto` of an embedded `FileDescriptorSet` is
lost whenever arbitrary bytes follow the set.** Measured:

```
protoscan bobapp.desc                     41 files, incl. bobapp/v1/log.proto
protoscan bobapp.desc + 4 KB of urandom   40 files, log.proto gone
protoscan <the bobapp executable>         40 files, log.proto gone
protoscan <the prototext executable>      10 of 11, wrappers.proto gone
```

A single appended byte is enough.

`score_candidate` hands `score_one` the rest of the buffer and relies on
the *next* member's field-1 tag to stop the walk — `FDP.name` is
singular, so `scan_terminates` fires and reports a clean boundary. The
final member has no next tag. The walk runs on into whatever the linker
placed after the set, trips a veto, and a veto yields no boundary at
all, so a flawless 272-byte record is discarded whole.

Which veto fires is decided by one adjacent byte, and that is why the
obvious narrow fix is not one:

```
bobapp/v1/log.proto     next byte 0x77 -> field 14, wire type 7, illegal
                                       -> "garbage wire tag"
wrappers.proto          next byte 0x73 -> field 14, wire type 3, legal
                                       -> "wire-type mismatch on field 14"
```

Same defect, same cause, two different veto sites. Any rule keyed on the
veto's kind is fitted to whichever junk byte the linker happened to
supply.

protoc emits dependencies first and the files named on its command line
last, so the descriptor lost is always the one that motivated the scan.
This is what leaves `grpconf/stage/bobapp.desc` with zero `bobapp.*`
types and breaks the demo beat that overrides the log envelope to
`bobapp.v1.Entry`.

The counters are not the problem. At its true end each of these records
is perfect — the walk simply never stops there.

## Goals

- **G1.** A `FileDescriptorProto` that ends where the buffer's structure
  says it ends is recovered, whatever follows it, and whatever the first
  byte of that following data happens to encode.
- **G2.** The boundary reported is the record's true end, exact — not a
  usable prefix of it.
- **G3.** No new false positive. The scanner's precision on a corpus of
  known descriptors is unchanged.

## Non-goals

- **N1.** A rule of its own for truncation. There is nothing to write.
  `ScoringOpts::end_undeclared` stays refused under `Policy::Scan`
  (spec 0310 N2), so all seven `cut_or_veto` sites take their `veto_all`
  arm and `EntryScore::truncated` can never be set by a scan. An overrun
  is already the strongest anomaly there is, and the rule below treats
  it exactly like every other one.

  The consequence, accepted deliberately: a genuinely cut descriptor is
  reported as its last clean boundary rather than refused outright. The
  walk cannot do better, because it cannot tell the two cases apart — an
  intact record followed by rubbish and a record whose own bytes ran out
  are both "a clean prefix, then trouble", and which veto fires is
  settled by the same accident of one adjacent byte that Alternatives
  rejects a rule on. What is reported is never itself a truncated
  record: every field in it was read normally and it parses whole. What
  is not offered is a marker saying the source was cut.

- **N2.** Teaching the scanner about `FileDescriptorSet`. Spec 0239's
  reasoning stands — one root, and a root list admitting exactly one
  exception is not a root list. The member length prefix is *not* read;
  see Alternatives.

- **N3.** Changing `score_all`, `pick_winner`, or any policy other than
  `Scan`. S1's snapshots are taken under `Policy::Scan` only, which has
  exactly one production caller (`fdp-scan-pyo3/src/lib.rs`), so the
  scoring walk protolens and prototext drive is untouched — no extra
  state, no extra branch in its hot loop.

## Specification

- **S1.** Under `Policy::Scan`, at each depth-0 field boundary — the
  point where the walk has finished a top-level field and not yet read
  the next tag — each active entry whose counters are still *clean*
  (S2) records a snapshot: the offset, and the counters as they stand.

  Per entry, not per walk: roots terminate at different offsets because
  each has its own singular fields and its own extension ranges (spec
  0238 S13), and that stays true of where each was last clean.

- **S2.** *Clean* means the entry has accumulated no anomaly of any
  kind: `unknowns`, `mismatches`, `non_canonical` and `out_of_range` all
  zero and `vetoed` unset. `EntryScore::truncated` is deliberately not
  named — under `Policy::Scan` it cannot be set (N1), and a condition
  that can never fire does not belong in a definition.

  Stricter than the accept rule of spec 0239, which ignores the two
  sloppiness counters. It costs nothing: across all 7 771 members of
  `googleapis.desc`, scored each to its own true end, **zero** are not
  clean under this definition. A legitimate descriptor is emitted by
  protoc and is canonical in every respect, so anything less than
  perfect is a sign the walk has left the record.

  This is what makes backtracking safe rather than permissive. A
  snapshot is only ever taken at an offset where nothing has yet gone
  wrong, so the record handed back is complete as far as it goes and
  every byte of it was read normally.

- **S3.** Under `Policy::Scan` the first anomaly of any kind finalizes
  the entry, which then reports its last snapshot — offset and counters
  — instead of reporting nothing. With no snapshot it reports nothing,
  as today.

  One rule, not two. Anomalies that veto and anomalies that merely count
  are treated alike because cleanliness is monotone: counters only rise
  and `vetoed` only gets set, so once anything has gone wrong no later
  boundary can ever be clean and nothing beyond the first anomaly can
  improve the answer. Stopping there is therefore free of consequence
  and saves the rest of the walk.

  The rule rewrites an entry the walk **abandons**, and nothing else. An
  entry that *finished* — by a spec 0238 S12 termination, or by reaching
  the end of the buffer — has already reported its own true boundary,
  and its counters describe the record that ended rather than a walk
  that overran it. The distinction is load-bearing rather than tidy:
  `apply_cardinality_multi` runs at a clean termination and can charge
  `mismatches` for a `required` field the record genuinely lacks (spec
  0238 S13), and that verdict must survive. A sweep over the counters at
  the end of the walk cannot tell it apart from the wreckage of an
  overrun, so there is no such sweep — the snapshot is restored at the
  two places an entry is abandoned, and there are only two: the veto
  funnel, and the boundary pass of S1.

  The existing clean-termination path (spec 0238 S12-S13,
  `scan_terminates`) is unchanged, and still handles every candidate
  followed by a recognizable next record without a snapshot ever being
  consulted. It is worth noting that this rule *subsumes* it rather than
  sitting beside it: `scan_terminates` has exactly three rules, and each
  is a lookahead for one anomaly —

  | `scan_terminates` rule | the anomaly it foresees |
  |---|---|
  | `tag.out_of_range` | `out_of_range` |
  | undeclared and not in an extension range | `unknowns` |
  | repeated singular | `mismatches` |

  Because it fires on the tag *before* consuming it, the boundary it
  reports is precisely the last clean boundary, so the two paths agree
  on both offset and counters wherever both apply. It is kept because it
  is the common case and reaches its verdict without recording anything;
  this rule extends the same judgment to anomalies lookahead cannot see,
  below depth 0 or inside a payload.

- **S4.** `fdp-scan-pyo3` refuses a candidate whose recovered record
  contains no depth-0 field other than `name`.

  The scanner's judgment, not the walk's: `prototext-graph` supplies the
  boundary and the counters and stays generic, while "is this worth
  reporting as a `.proto` file" is knowledge about
  `FileDescriptorProto`. The record's bytes and its end are both in
  hand, so the check is a walk of its top-level tags.

  Structural, deliberately, rather than a threshold on `matches` or
  `score`. It states the thing that is actually wrong with the candidate
  — it declares no package, no message, no service, not even a syntax —
  instead of a number that has to be re-justified against a corpus every
  time the corpus changes. The measurements that show nothing real is
  near the line are in Test plan, not in the rule.

  It is needed because cleanliness cannot reject these. Every false
  positive the scanner faces today is a `.proto`-suffixed Java package
  name — `option java_package = "com.google.cloud.pubsublite.proto"` is
  field 1 of `FileOptions`, tag `0x0a`, indistinguishable from a file
  name at the anchor. Its first boundary is a flawless one-field
  descriptor and would be snapshotted; the veto arrives at the *next*
  boundary. Without this rule the stub is handed back.

  One field, though, and not two or three. Real descriptors that small
  exist, and were compiled to check: `name` + `package` is 19 bytes,
  `name` + `syntax` 24, `name` + `dependency` 31 — the last being a file
  whose only statement is an `import`. A floor of two depth-0 fields
  would refuse all three. Only the totally empty `.proto` file, 16
  bytes, yields a descriptor carrying nothing but its own name, and
  nothing real is lost by refusing that.

## Alternatives considered

**Demote only the garbage-wire-tag veto, at depth 0, to a termination.**
Cheap — nothing is consumed when a tag fails to parse, so `tag_start` is
already the exact boundary and no snapshot is needed. Built as far as
measurement and rejected: it recovers `bobapp/v1/log.proto`, whose next
byte is `0x77`, and still loses `wrappers.proto`, whose next byte is
`0x73`. The rule was fitted to one byte of one binary.

**Backtrack on any veto, at any depth, without S2's cleanliness test.**
Rejected on measurement. Scoring every depth-0 prefix of the nine
anchors the scanner currently rejects, all nine have an accepted prefix,
and in every case it is the name-only stub. Cleanliness is what keeps
the snapshot honest; without it the rule is "find some prefix that
parses", which any string ending in `.proto` satisfies.

**Bound the walk with the enclosing `FileDescriptorSet` member length.**
The three bytes before the record say 272, and reading them would fix
both cases. Rejected: it works only when the record is inside a set, it
is a second stop rule bolted beside the schema-derived one spec 0239
exists to establish, and it edges toward N2. The snapshot needs no
framing and works on a bare record.

**A numeric floor on `matches` or `score` instead of S4.** Would work —
the minimum `matches` across the 7 771 real descriptors is 5, and every
stub scores 1. Rejected as S4 explains: the number is a proxy for a
structural fact, and a proxy has to be re-measured whenever the corpus
or the scoring changes.

## Test plan

1. `the_last_record_of_an_embedded_set_is_recovered` — `bobapp.desc`
   with 4 KB of trailing bytes scans to 41 files including
   `bobapp/v1/log.proto`. The pure-file case (no trailing bytes) already
   yields 41 and must keep doing so.

2. `the_recovered_boundary_is_the_records_true_end` — the boundary
   equals the length the enclosing member header declares: 272 for
   `bobapp/v1/log.proto` in the bobapp executable, 518 for
   `google/protobuf/wrappers.proto` in the prototext executable. A
   usable prefix is not enough (G2): `log.proto` also scores clean at
   +217 and +264.

3. `a_java_package_name_is_not_a_descriptor` — the eight
   `com.google.cloud.pubsublite.proto` and
   `com.google.storagetransfer.v1.proto` anchors in `googleapis.desc`
   stay refused, and `protoscan googleapis.desc` still reports exactly
   7 771 names.

4. `every_real_descriptor_is_clean_at_its_own_end` — all 7 771 members
   of `googleapis.desc`, scored individually to their declared lengths,
   are clean under S2. This is what licenses including `non_canonical`
   and `out_of_range` in the definition; it must be re-run if either
   counter gains a new site.

5. `a_cut_record_reports_its_clean_prefix` — a real descriptor cut
   mid-field reports the last depth-0 boundary before the cut, and one
   cut before its second depth-0 field is refused by the structural
   floor rather than reported as a name-only stub. Pins N1's accepted
   consequence so that a later change cannot make it accidental.

6. `a_scan_never_sets_truncated` — `EntryScore::truncated` is false on
   every entry of every scan in the suite, pinning the reasoning that
   removed it from the cleanliness definition. It must be revisited if
   spec 0310 N2 is ever relaxed.

7. `scan_snapshots_do_not_reach_the_default_policy` — a walk under the
   default policy takes no snapshots, pinning N3.

8. `an_anomaly_stops_the_walk` — under `Policy::Scan` a candidate whose
   first anomaly is an unknown field at depth 0 reports the boundary
   before it, and the bytes beyond are never read. Pins the single-rule
   form of S3, which is otherwise indistinguishable from the two-rule
   form by its output alone.

## Measured outcome

Every case from Background is recovered, and the boundary is exact.

```
                                          before      after
bobapp.desc (51 111 B, pristine)          41          41
  + one 0x77 byte                         40          41
  + 4 KB of urandom                       40          41
the bobapp executable                     40          41
the prototext executable                  10 of 11    11 of 11
googleapis.desc                           7 771       7 771
```

`bobapp/v1/log.proto` comes back at `start=50839 end=51111`, length
**272** — the same 272 in all four bobapp columns, and the same 272 the
enclosing member header declares. In the bobapp executable it is at
`start=4150997 end=4151269`, again 272.
`google/protobuf/wrappers.proto` comes back at
`start=3519061 end=3519579`, length **518**. So G1 and G2 both hold, and
the clean prefixes at +217 and +264 that a weaker rule would have
accepted are not what is reported.

G3 holds unchanged: `googleapis.desc` still scans to exactly 7 771
names, in 0.561 s, and `test_scan_finds_every_record` confirms every one
of those 7 771 boundaries agrees with the `FileDescriptorSet` framing.
`every_real_descriptor_is_clean_at_its_own_end` adds that all 7 771,
scored on their own declared extent, are clean under S2's strict
definition — zero `non_canonical`, zero `out_of_range` — which is the
measurement S2 rests on.

Two notes for whoever reads this next.

`grpconf/stage/bobapp.desc` is **itself an output of the bug**: demo
beat 6 rebuilds it through `reproto -I <the binary>`, so the staged copy
genuinely holds 40 members, its last one ending exactly at EOF. It looks
like a regression under the fixed scanner and is not one. It has to be
re-staged.

The design took one correction during implementation, and
`test_scan_cardinality_applied_at_termination` is what caught it. The
first build restored snapshots in a sweep at the end of the walk, which
also rewrote entries that had terminated *cleanly* — erasing the
`mismatches` that `apply_cardinality_multi` legitimately charges for an
absent `required` field. S3's abandoned-versus-finished paragraph is
that correction; the sweep is gone and the restore happens only at the
two abandonment sites.
