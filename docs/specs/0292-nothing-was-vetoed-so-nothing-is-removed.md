<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0292 — nothing was vetoed, so nothing is removed

Status: implemented
Implemented in: 2026-08-14
App: prototext-graph
Refs: docs/specs/0179-the-active-set-is-carried-inline.md
        (established `ActiveEntry` and the shape of the active set the
        scan below walks)

## Background

`propagate_vetoes` exists for one reason: a sub-message recursion may
veto a candidate, and the *parent* frame's active set must then stop
carrying it. It is called after every LEN recursion and every group
recursion, and it does this:

```rust
for ae in active.iter_mut() {
    ae.entries.retain(|e| !ws.is_vetoed(*e));
}
active.retain(|ae| !ae.entries.is_empty());
```

That is a scan of every entry of every `ActiveEntry` — near the top of a
walk, the whole part's surviving candidate set — once per LEN child.

Instrumented over a protolens startup on googleapis:

| | count |
| --- | ---: |
| `propagate_vetoes` calls | 1 072 120 |
| …that vetoed nothing | **1 071 701 (99.96%)** |
| entry visits | 107 873 196 |
| …in calls that vetoed nothing | **107 767 797 (99.90%)** |

So the function is essentially always a no-op that costs a full sweep of
the active set to discover. The 419 calls that do have something to
remove are lost in it.

## Goals

- **G1.** Skip the scan when the recursion vetoed nothing.
- **G2.** Score identically. This is a pure work elision, not a change of
  policy — the same entries are removed, in the same order.

## Non-goals

- **N1.** *No change to when a veto is raised.* The six veto sites of the
  walk are untouched.
- **N2.** *Not a per-entry dirty set.* Recording *which* entries a
  recursion vetoed would let the removal be targeted rather than
  skipped — but 99.90% of the visits are in calls with nothing to
  target, so a counter that answers "any at all?" captures the whole
  win at none of the bookkeeping.

## Specification

- **S1.** `WalkState` gains `veto_epoch: u64`, starting at 0.
- **S2.** `set_vetoed` increments it whenever a bit in `vetoed` goes from
  0 to 1. Re-vetoing an already-vetoed entry does not.
- **S3.** Each recursion site reads `ws.veto_epoch` immediately before
  recursing and passes it to `propagate_vetoes` as `since`.
- **S4.** When `ws.veto_epoch == since` the recursion raised no veto, and
  `propagate_vetoes` returns without scanning.
- **S5.** The LEN site reads the epoch **after** its
  `active.retain(|ae| !ae.entries.is_empty())`, not before. See
  Correctness.

## Correctness

The skip is sound only because of a standing invariant: **`active` never
holds an already-vetoed entry when a recursion begins.** Every
`set_vetoed` call site in the walk is immediately followed by
`ae.entries.clear()`, and the LEN arm then drops the emptied
`ActiveEntry`s — so a veto raised *before* the recursion has already been
taken out of `active` by the arm that raised it. Given that, an unchanged
epoch means no entry in `active` is vetoed, and the `retain` provably
removes nothing.

That invariant is asserted rather than assumed. `propagate_vetoes`
carries a `debug_assert` on the skip path that no entry of any
`ActiveEntry` is vetoed, which is exactly the condition the skip relies
on — so a future veto site that forgets its `clear()` fails loudly in
every debug build rather than silently scoring differently.

S5 is the one place the invariant is not free: the LEN arm's own
`retain` is what re-establishes it for that frame, so the epoch must be
sampled downstream of it.

## Test plan

1. The whole `prototext-graph` suite, unchanged — the vetoing paths are
   already covered, and G2 means every existing expectation still holds.
2. `RUSTFLAGS="-C debug-assertions=on" cargo build --release`, then a
   full googleapis startup. This runs the S4 `debug_assert` over the real
   corpus at full speed: **1 071 701 skips, no failure.**
3. A full `protolens … export /` over googleapis must be byte-identical
   to the previous binary.

## Measured outcome

`export /` over googleapis: **byte-identical across 5 278 322 lines**.

protolens startup on googleapis, `taskset -c 0-7 … -j 8`, 0291 and 0292
interleaved, medians of 5: **1.676 s → 1.617 s, −3.5%.** All five pairs
agree in sign.

Timestamping the progress lines (interleaved, 4 pairs) puts it in the
sweep, as expected: **1.245 s → 1.209 s, −2.9%**, with the serial phases
unmoved.

The gap between −3.5% of the total and −2.9% of the sweep is within this
machine's noise; the honest reading is that the whole saving is the
sweep's and it is worth about 3%.

That is a modest return for eliding 99.90% of 107.9 M entry visits, and
the reason is instructive: those visits are a linear scan over a small,
hot, contiguous `SmallVec`, which is close to the cheapest work a CPU
can do. The count was large but the cost per unit was near the floor.
