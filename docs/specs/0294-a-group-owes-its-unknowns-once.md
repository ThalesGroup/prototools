<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0294 — a group owes its unknowns once

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0291-a-name-is-looked-up-not-searched-for.md (the rule
        of measuring the amortization ratio before implementing),
      docs/specs/0292-nothing-was-vetoed-so-nothing-is-removed.md (the
        veto epoch, whose 99.96% skip rate is what makes the flush in
        `propagate_vetoes` nearly free),
      docs/specs/0293-a-state-carries-its-own-fields.md (the baseline
        this is measured against)

## Background

After 0293 the single hottest *line* in a googleapis startup was

```rust
ws.scores[e as usize].unknowns += 1;
```

at **2.515 G instructions, 9.23%** of the whole run, with its loop
header adding 1.87% and the `match ae.verdict` above it 1.06%. The other
27 sites that write a counter in `ws.scores` came to 1.67% between them:
this one site was ~85% of all counter traffic.

It is a *scatter*. `ws.scores` is indexed by candidate-root id and
`EntryScore` is 72 bytes, so on googleapis it is a 3.5 MB array and each
increment is a random read-modify-write into it. Unlike 0292's
contiguous `entries` scan, the addresses have no locality of their own.

The work was also redundant. Every entry of an `ActiveEntry` shares its
state, so an unknown field is charged to all of them identically — the
increment is a property of the *group*, and only its destination is
per-entry.

## Goals

- **G1.** One unknown field costs one local increment, not one scattered
  read-modify-write per entry.
- **G2.** Scoring output is unchanged, byte for byte.

## Non-goals

- **N1.** No deferral of the other counters. `matches`,
  `non_canonical`, `out_of_range` and `mismatches` are written from
  arms that are not group-uniform (`apply_value_verdict` carries a
  per-value amount) or that are simply rare. Together they are 1.67% of
  the run, and each would need its own accumulator and its own place in
  the flush.

- **N2.** No narrowing of `EntryScore`. Splitting the cold `fqdn` and
  `termination` out would take the scattered array from 72 to 48 bytes,
  but that is a change to what the scatter *costs*, not to how much of
  it there is, and it is orthogonal to this.

- **N3.** No flush at the end of each tag. That is once per group per
  tag, which is the count this exists to reduce.

## Specification

- **S1.** `ActiveEntry` gains `pending_unknowns: u64`. The four
  `Verdict::Unknown` arms of the body loops (VARINT, I64, LEN, I32)
  become `ae.pending_unknowns += 1`.

  The fifth `unknowns` site, the `stay_out_entries` loop in the
  START_GROUP arm, is untouched: it walks a flat list of entry indices,
  not a group, so there is no group to charge.

- **S2.** `flush_pending(ae, scores)` adds the accumulator to every
  entry and zeroes it. It must run before `ae.entries` can shrink or the
  group can die — the charge is owed to the entries that were present
  when it was accrued.

- **S3.** The flush sites are exactly the places where an entry leaves a
  group: the seven `ae.entries.clear()` sites, `veto_all`, and
  `propagate_vetoes`' retain path. `propagate_vetoes` therefore takes
  `&mut WalkState`; by 0292's measurement it reaches that path on 0.04%
  of calls, so the added flush is nearly free.

- **S4.** `score_message_multi` becomes a wrapper: it calls an inner
  function taking `active: &mut Vec<ActiveEntry>` and flushes the whole
  set on the way out. The body returns from some twenty places, and a
  frame owns its `ActiveEntry`s outright — every recursion is handed a
  fresh set from `group_by_state` — so "the frame returned" is exactly
  "these groups are over". This makes the flush structural rather than a
  checklist.

- **S5.** Deferral is invisible because no counter in `EntryScore` is
  read during the walk. The walk reads `vetoed` (through `is_vetoed`),
  writes `termination`, and reads `fqdn` on a debug path; every counter
  is read only by `EntryScore::score` after the walk is over. Addition
  is commutative, so reordering the charges changes nothing.

## Alternatives considered

### Charging the group and dividing at the end

Keeping one counter per *state* and multiplying out once would avoid the
accumulator entirely — but a group's membership changes over its
lifetime, so there is no single multiplier. That is precisely why the
flush is tied to entry removal rather than to the group's end.

### A `Drop` impl on `ActiveEntry`

`Drop` cannot reach `ws.scores`, and threading it in would mean an
`ActiveEntry` holding a pointer into the walk state.

## Test plan

1. The existing `prototext-graph` suite (95 + 10). It asserts
   `unknowns` directly at 20-odd sites, across vetoed, terminated and
   surviving entries, which is the whole space of flush paths.
2. `protolens … export /` over googleapis is byte-identical to 0293's.

## Measured outcome

Dev VM (8 E-cores, two L2 clusters), googleapis (25.6 MB descriptor
set, 49 255 roots), `--descriptor-set $SET $SET quit`.

| | 0293 | 0294 |
|---|---|---|
| wall clock `-j 1`, `taskset -c 4`, median of 5 | 4.06 s | 3.00 s |
| wall clock `-j 8`, `taskset -c 0-7`, median of 11 | 1.77 s | 1.70 s |
| instructions (`-j 1`) | 27.37 G | 22.92 G |
| `score_message_multi` Ir (both frames) | 19.79 G | 15.46 G |
| its share of the run | 72.3% | 67.5% |

**−16.3% instructions for −26.2% single-threaded time** — for once the
campaign's lesson in the *favorable* direction, and the first time in it
that a change returned more time than instructions. The removed
read-modify-writes were worth well above the average instruction, which
is what the 3.5 MB scatter predicted.

The `-j 8` figure is only −4.3% because at eight workers the scoring
walk is no longer most of a startup: the serial phases around it —
reading and mapping the descriptor set, building the graph — are.

The amortization was measured before the change was written, per 0291's
rule, with throwaway `AtomicUsize` counters:

```
group=41726817 entry=228597154 ae=2571866 ae_entries=13249300
```

mean 5.48 entries per group update. Deferring replaces **228 597 154**
scattered read-modify-writes with 41 726 817 local ones plus
**13 249 300** scattered flush writes — **17.3x less scattered
traffic**, 4.2x fewer updates in total.

`export /` over the whole corpus is byte-identical to 0293's output,
5 278 322 lines.
