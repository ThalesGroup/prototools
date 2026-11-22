<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Seriation: the greedy search

Phases 3 and 4 ([design.md](design.md)) raise a score by moving one
snapshot at a time. The search is one loop for both (`seriation/src/
greedy.rs`); only the score differs. Which move it makes decides the
order, so every rule below is part of the result, down to the ties.

## What a score provides

| Operation | Does |
|---|---|
| `score` | the score, computed from scratch |
| `rank` | per position, how promising moving its snapshot is |
| `begin(from)` | prepare to probe moves of the snapshot at `from` |
| `probe(from, to)` | the change of the score if it moved to `to` |
| `end(from)` | undo what `begin` changed |
| `commit(from, to)` | make the move: the snapshot goes to `to`, those between shift by one |

For the edge score ([scores.md](scores.md)), `rank` counts each edge at
both its ends, +1 if it disagrees with the order and −1 if it agrees;
`begin` and `end` do nothing; `probe` reads the moved snapshot's edges. For
the interleaving score, `rank` is the change of removing the snapshot;
`begin` removes it from its files' subsequences and `end` puts it back;
`probe` adds the change of reinserting it at `to`.

## The loop

```
s = score()
loop:
    rank = rank()
    candidates = positions by rank, highest first, ties by position
    best = none
    for (tried, from) in candidates:
        if tried >= max_tries: stop trying
        if tried >= nom_tries and best is some: stop trying
        begin(from)
        for to in from−1 down to 0, then from+1 up to n−1:
            if a barrier is at to: stop this direction
            c = s + probe(from, to)
            if c > s and (best is none or c > best): best = (from, to, c)
        end(from)
        if the first improving move was found with this candidate
           and tried + 1 >= nom_tries: stop trying
    if best is none: done
    commit(best); s = best's score
```

- **Ties.** Among equal improvements, the first found wins: the leftward
  scan from `from − 1` before the rightward one, candidates in rank order.
- **Trying candidates.** With `nom_tries` at 1 (the default), the search
  takes the first candidate with an improving move. Otherwise it tries at
  least `nom_tries` candidates once one has an improving move, and gives up
  after `max_tries` (default 20) without one.
- **Barriers.** With order hints on (phase 2), a snapshot the sort is
  shown a hint for stops scanning a direction at the first snapshot whose
  shown hint differs from its own. A snapshot without one moves anywhere.
- **Termination.** Every move strictly raises `s`, an integer bounded by 0,
  so the loop makes at most `−s₀` moves.
- **Checking.** In debug builds every iteration recomputes the score and
  asserts it equals `s`; release builds trust the increments.

## Cost

Each move probes every destination of up to `max_tries` candidates. A
probe costs `O(deg)` for the edge score, `O(F_x · (K + L))` for the
interleaving score ([scores.md](scores.md)). Many snapshots of one build
are where this shows; a probing window limited by time and by edge
partners is the known remedy (spec 0450 N2), an option for later, since it
changes which moves are found.
