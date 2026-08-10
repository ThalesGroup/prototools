<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Making protolens fast

protolens is a terminal viewer for binary protobuf. Point it at a 25.6 MB
`.pb` blob with no schema and it infers the root type, decodes every byte,
and drops you at line 1 of a five-million-line document that you can scroll,
fold, search, retype and re-export — losslessly.

Two clocks run at once, and they want opposite things:

- **The frame.** Between a keystroke and the pixels there is a budget of
  about 16 ms. Everything in it is latency-critical.
- **The sweep.** Type inference, scoring, baking and read-ahead are bulk
  work over the whole blob. Nobody is watching a single unit of it, but
  everybody is waiting for the last one. That is throughput-critical.

Almost every optimization below is one of two moves: **do less before the
user sees something**, or **do the rest on cores nobody is waiting on**.

All numbers are measured on `googleapis.desc` — used as both blob and
schema — on an Intel Core Ultra 7 165U, release build, pinned with
`taskset`. Numbers that are simulated or unmeasured say so.

## The shape of the problem

| | |
|---|---|
| blob | 25.6 MB |
| candidate root types | 49 255 roots / 17 572 state groups |
| top-level records | 7 771 (a forest, not a tree) |
| decoded document | **5 281 124 lines**, 238 MB of text |
| wire-arena slots | 4 737 284 |
| rows a 50-line terminal shows | **48** |

The last two lines are the whole argument. The renderer was being asked to
materialize 110 000 times more document than the screen can hold.

---

# Part 1 — Latency: nothing between a keystroke and a frame

## 1. Render a screenful, not a document

Opening a file used to render all 5.28 M lines before the first frame.
Now `main` passes a row budget into the renderer, which stops as soon as it
has emitted enough and reports where it stopped. Those stops become folds.

| open, end to end (`taskset -c 4-11`) | before | after |
|---|---|---|
| inferred root type | 8.72 s | **1.10 s** |
| `--raw` | 4.65 s | **1.13 s** |
| lines materialized | 5 279 383 | **15 575** |

**7.9x**, and both paths land on the same number — what is left is the wire
arena, which the root type does not touch.

The renderer alone, benchmarked in isolation at a 51-row budget:

| one render | unbounded | budget 51 | |
|---|---|---|---|
| wall time | 2.0 s | **4.9 ms** | 410x |
| text emitted | 239 MB | 360 KB | 664x |
| style spans | 4 499 335 | 7 820 | 575x |
| rows | 5 278 322 | 15 599 | 338x |

The 15 599 rows against a budget of 51 are not a bug and are the whole
insight: **a bounded render costs its frontier, not its budget.** Stopping
inside a node means emitting every ancestor of the stop and every sibling
already begun, so the price is the width of the boundary you stop on. That is
also its stated limit — a wide, flat document cannot be bounded by folding,
because there is nothing to fold.

The off-by-one is worth knowing: the budget is `pane_rows + 1`, not
`pane_rows`. A stop's own header is one of the lines it emits.

## 2. Finish the document while nobody is waiting

A budget only moves work; it does not delete it. The rest is paid off in
`run_loop`'s idle arm — the branch taken when the event queue is empty —
one subtree per step, drawing no frame per step.

| root retype (`taskset -c 4-7`) | keystroke | deferred | worst idle step |
|---|---|---|---|
| render everything | **4.55 s** | — | — |
| render one screenful | **0.59 s** | 6.55 s | 22–25 ms |

The bake's row budget is 5000, chosen from a sweep of budgets: across ~70 800
steps the worst single step is 22–25 ms, two exceed 8 ms, and none exceeds 50.
Smaller budgets multiply the step count without improving the tail.

Four jobs share that idle arm, and the **order is load-bearing**, not merit:
search sweep, then discard, then bake, then read-ahead. Every bake step
changes the document's structure, and read-ahead restarts its wave whenever
the structure changes — so read-ahead runs last, in the only state it can
work in.

## 3. Stop paying for the document you replaced

Retyping the root vacates 2 864 189 boxed strings. Dropping them inline
looked cheap and was not: glibc parks small frees on fastbins and defers
consolidation to the next allocation that misses, so the bill lands on
somebody else's `malloc`. The boxes are now moved to a vector and freed
65 536 at a time in the same idle arm.

| confirm, one binary, A/B | before | after |
|---|---|---|
| keystroke | 0.428 / 0.445 / 0.456 s | **0.153 / 0.159 / 0.160 s** |
| deferred work absorbed | 5.38 s | 5.74 s (+5%) |
| exported bytes | 232 892 696 | `cmp`-identical |

**2.8x**, and the work is conserved on purpose.

## 4. Coalesce the burst, draw once

A mouse wheel delivers dozens of reports in a single `write`. The loop now
keeps `try_recv`-ing and dispatching after the first event and draws once at
the end, with an 8 ms budget and a stop condition on control transfers.
A pan that hit its bound forces no frame at all.

| 60 wheel reports in one write | before | after |
|---|---|---|
| still panning | 60 frames (58 monochrome) | **1** |
| against the bound | 60 frames (58 monochrome) | **0** |

The coalesced pass dispatches all 60 in 2.3 ms, so a real burst never
approaches the budget.

## 5. Drop the colors while the fingers are still moving

Tree-sitter highlighting is the largest single term in a frame. When input
is already queued, the frame is drawn monochrome and recolored when the
stream settles.

| keys 0.5 ms apart | colored (n=14) | monochrome (n=57) |
|---|---|---|
| `styles` µs | 320–480 | **0** |
| whole `draw` µs (mean) | 1194 | **632** |

Honest caveat: at ordinary key autorepeat (~30 ms) **nothing changes** — a
1.3 ms frame drains a 33 ms repeat and every frame stays colored. This buys
robustness for wheel inertia, ssh/tmux burst delivery and wide terminals,
not a faster PageDown on a fast box.

## 6. Do not compute an answer you can prove is zero

After every splice the code re-clamped the horizontal pan, which resolves
the whole visible window against a cache the splice had just emptied —
O(pane height) per splice, 70 894 times during a bake. But the clamp is
`0.min(x)` whenever the pan is already 0, and it almost always is.

| deferred drain | before | after |
|---|---|---|
| 50-row pane | 6.55 s | **5.42 s** |
| 500-row pane | 15.53 s | **5.51 s** |
| 5000-row pane | 102.24 s | **5.78 s** |

One `if self.pan_offset > 0` guard. Identical step counts, identical splices,
identical output bytes, three wall times — the cost was never in the work.

## 7. Put the text where the tree already is

The document used to be a `Vec<String>` of 5.28 M lines beside the tree.
Editing near the front of it meant a memmove of the rest. The text now lives
per arena slot: a bracketed node owns its header line, a flat node owns all
the lines it draws, and a closing brace is *derived* from the header's
indent rather than stored (780 110 footer lines, 5.85 MB of pure chrome).

| commit a retype (`key Enter`) | first record | last record |
|---|---|---|
| before | 102 ms | 61 ms |
| after | **0.66–1.06 ms** | 1.18–1.99 ms |
| the merge itself, before | 87.4 ms | 42.9 ms |
| the merge itself, after | **19–25 µs** | 30–96 µs |

The 2x first-versus-last spread that had gone unexplained for a whole spec
**inverted**, which explained it: it had been a memmove near the front of a
five-million-element vector.

The same layout discipline runs through the arena — a 44-byte node slot that
holds no links at all (parent, first child and the raw byte range live in one
flat `Vec<u32>` beside it), one slot per packed run rather than one per
element, and a 12-byte heat state. All three sizes are pinned by a
`const _: () = assert!(size_of::<..>() == ..)`, an equality rather than a
bound, because growth is the regression they exist to catch:

| memory | before | after |
|---|---|---|
| resident at rest | 1.96 GB | **875 MB** |
| peak during a root retype | 4.18 GiB | **1.66 GiB** |

## 8. Search that yields

A full-document miss is a 238 MB scan. It is a resumable sweep advanced from
the idle arm, 1000 candidates at a time, forcing a frame only when the
*answer* changes — 5 282 slices, worst slice 222–797 µs.

| full-document miss | before | after |
|---|---|---|
| case-folding pattern | 1.63–2.05 s | **183–272 ms** |

A `memchr2` prefilter over the two-case alphabet, worth **7x** — and worth
exactly zero on the case-sensitive arm, where `str::find` already does this.
Two guards it needs: the needle's first character must be ASCII *and* the
haystack must be entirely ASCII. `U+212A` KELVIN SIGN folds to `k`, and no
scan for `k`/`K` lands on it.

## Where a frame goes today

Medians, 200x50 pty, 48 rows, at the top of the document and after `G`:

| µs per frame | top | end |
|---|---|---|
| key dispatch | 10 | 24 |
| window (descent + walk) | 5 | 57 |
| **styles (tree-sitter)** | **276** | **431** |
| heat cues | 7 | 10 |
| override overlay | 11 | 14 |
| line spans + text | 75 | 93 |
| **whole `terminal.draw`** | **821** | **1359** |

Paging is O(page), not O(document): one `carry_caret` per page, ~40 µs at
the end of a 5 M-line file. The end of the document draws no slower than the
top.

And the conclusion we did *not* act on: tree-sitter is 340 µs of a
0.8–2.6 ms draw, perfectly linear at 247 ns/byte with no fixed cost, and a
hand-written line scanner would be 50–100x faster on that term. A custom
colorizer was **rejected on performance grounds** — the frame is not
styles-bound, and the rest is ratatui diffing plus the terminal write.

---

# Part 2 — Throughput: the sweep

## 9. One traversal serves every root that behaves alike

Before anything is split or scheduled, the work is deduplicated. The schema is
compiled once into a scoring DFA and put through textbook Hopcroft
minimization, so two message types that consume the wire identically become
one state. The walk then groups its live candidates by state and keeps one
`ActiveEntry` per distinct state, carrying the list of candidates riding on it.

| candidates | count |
|---|---|
| root types in `googleapis.desc` | 49 255 |
| distinct behaviors to actually walk | **17 572** |

That is a 2.8x reduction paid once at build time, and it is why the sweep can
score fifty thousand hypotheses against 25.6 MB at all. It also constrains
everything downstream: a state group is **indivisible**, because splitting one
across two parts would make both walk the same bytes rather than half each.
Deduplication and partitioning pull against each other, and deduplication wins.

## 10. Split the work far past the core count

Root-type inference scores 49 255 candidate roots against the blob. Split it
into parts and hand them out from one shared atomic cursor.

Per-part costs, measured one at a time on a single pinned core:

| parts | total serial work | slowest part | 8-worker makespan |
|---|---|---|---|
| 1 | 6.86 s | 6.855 s | 6.86 s |
| 8 | 6.84 s | 1.833 s | 1.83 s |
| 16 | **5.40 s** | 0.839 s | 1.08 s |
| 32 | 5.83 s | 0.737 s | 1.05 s |
| 256 | 7.96 s | 0.604 s | 1.32 s |

Three things fall out:

- **Splitting is cheaper even single-threaded** — 6.86 → 5.40 s at 16 parts
  with no parallelism at all, because smaller candidate sets make each step's
  multi-candidate handling cheaper. Partition even at `--jobs 1`.
- **Load imbalance, not convergence duplication, is the limit.** Total serial
  work grows ~4% from 1 to 8 parts. But at 8 equal-root-count parts the times
  ran 0.263 / 0.600 / 1.107 / 0.673 / 1.329 / **1.865** / 0.648 / 0.633 s —
  a 7x spread. A group's cost is how deep into the blob it stays alive, which
  is neither its root count nor knowable in advance.
- **There is a floor**: ~0.60 s from four indivisible fat state groups. Past
  32 parts, duplication costs a near-constant 11 ms per extra part.

| shipped | sweep | startup, pinned |
|---|---|---|
| 8 static shards | 1.89 s | 4.47 s |
| 24 parts, shared cursor | **0.89 s** | **3.55 s** |

`--jobs` is a ceiling, not a target: it is clamped to the CPUs the process
can actually reach, so `--jobs 12` under `taskset -c 4-11` legitimately
becomes 8.

## 11. Make the unit of work a part of a *query*

The same idea, moved into the interactive worker pool. Scoring queries for
on-screen rows used to be one task each, so one worker owned one query and
the other seven idled behind it. Now there is one partition per process and
workers pull `(query, part)` tasks from a single pool.

| one screenful of heat queries (7 jobs) | before | after |
|---|---|---|
| pane top 0 | 3.57 s | 3.29 s |
| pane top 5 000 | 412 ms | **91 ms** |
| pane top 200 000 | 408 ms | **93 ms** |
| pane top 2 000 000 | 259 ms | **59 ms** |

**4.4x on an ordinary screenful.** Pane top 0 is dominated by the document
root's own 25.6 MB range and does not shrink.

Two rules only implementation found:

- **Finer parts cannot replace preemption.** The partitioner never splits a
  state group, and the biggest group is 4 645 roots at *every* part count, so
  the worst part plateaus at ~574 ms. Going from 24 to 4096 parts makes the
  root walk 65% *slower*. Preemption is what bounds interactive latency.
- **An abort flag is not enough; it needs an epoch.** A part handed out,
  aborted, and then finding the flag low again would look complete and be
  cached — and a truncated score is *wrong*, not partial.

## 12. Throw away demand that is no longer true

Scroll fast and the queue fills with requests for rows that left the screen.
Each queued request is stamped with the window generation it was made under,
and a stale one is discarded at pop rather than served.

| 400 PageDowns at 20 ms | off | on |
|---|---|---|
| `Visible` sweeps | 1256 / 906 | 436 / 457 |
| `Prefetch` sweeps | 3090 / 2975 | **23819 / 7287** |
| stale requests discarded | 0 | 1557 / 1513 |
| unresolved cues on screen | 20 | **3** |

**Prefetch throughput 2.4–8.0x** — and the mechanism is not queue ordering,
which was already correct. A queued visible request holds an occupancy bit
that parks every speculative worker. The prefetcher was not outranked; it
was stopped.

## 13. Not all cores are the same core

The reference host has three tiers behind one `nproc`:

| CPUs | clock | topology |
|---|---|---|
| 0–3 | 4.9 GHz | 2 P cores, SMT paired |
| 4–11 | 3.8 GHz | 8 E cores, two shared L2s |
| 12–13 | 2.1 GHz | 2 low-power E cores, **no L3** |

Measured on the scoring walk, one part at a time:

| | relative cost |
|---|---|
| P core, alone | 1.00 |
| E core | 1.52 |
| LP-E core | 2.85 |
| two SMT threads of one P core, each | **1.92** |

SMT is worth almost nothing here: two threads on one core deliver
`2/1.92 = 1.04` cores of throughput for 1.92x the latency on each.

And the drawing thread cares far more about core choice than the workers do:

| pinned to one CPU | P core | E core |
|---|---|---|
| startup, `--jobs 1 --raw` | 1.163 s | 1.498 s (1.29x) |
| **median frame, 60 PageDowns** | **434 µs** | **1965 µs (4.5x)** |

So: if — and only if — the kernel *states* which CPUs are fast
(`/sys/devices/cpu_core/cpus`, `cpu_capacity`), protolens gives the main
thread one whole physical core of them and hands every worker
`inherited − that core` at spawn. No benchmark, no guess, no fallback,
silent on every failure. On a machine that declares nothing, protolens does
nothing.

A spinner on the drawing core's SMT sibling, versus on a different core:

| load on the neighbor | frame p90 | frame max |
|---|---|---|
| none | 2714 µs | 4168 µs |
| **SMT sibling** | 4961 µs | **6386 µs (1.8x)** |
| different core | 3089 µs | 4347 µs (1.15x) |

The last round of a sweep is the makespan, so the last part should finish on
the best core: a worker that finds the cursor empty donates its fast seat to
the longest-running worker on a slower one, forced by a mask that excludes
the donor's own CPU (the kernel will not move a running, never-sleeping
thread while its current CPU is still in the mask).

**Status: simulated, not measured.** Over 3000 random hand-out orders on the
measured per-part costs, makespan goes 2148 → **1598 ms** mean and
2692 → **1793 ms** p95 — about −22% of startup. The development VM exposes
no topology at all, so the feature correctly disables itself there and the
gain cannot be observed on it. It is quoted as a projection.

## 14. Sleep properly

An idle protolens should cost nothing. It now performs **zero** timed
wakeups: the reader thread blocks in an untimed `poll(2)` on three
descriptors — the tty, a SIGWINCH pipe, and a shutdown pipe — and the main
loop chooses between a blocking receive and a timed one at the single point
where it genuinely sleeps.

| idle, 10 s sample | before | after |
|---|---|---|
| voluntary context switches | 90 | **0** |
| involuntary | 0 | 0 |

90 was exactly 9/s: a 250 ms activity tick plus a 200 ms input poll. Nothing
was hiding behind them. Three things are mandatory rather than optional:
your own SIGWINCH pipe (crossterm synthesizes `Resize` from its own signal
pipe, and a reader watching only the tty sleeps through every resize), a
drain of already-parsed events before blocking (crossterm reads 1 KiB at a
time, so the fd goes unreadable while parsed events still sit in its queue),
and a backoff when the drain fails — otherwise the fd stays readable
*because* nothing collected it and the untimed wait spins forever.

## 15. The small ones that add up

None of these is a headline; together they are most of the constant factor.

| | |
|---|---|
| **Merge in the worker, not the collector.** The worker that walks a query's last part merges its runs. | 244 → **96 ms** (2.3x) |
| **Build the partition once per process, not once per query.** It is pure function of the graph. | −7.3 ms of serial setup on *every* query |
| **Size a `SmallVec` by inline capacity, not by `size_of`.** The walk's largest allocation site was one field; capacity 2 covers 98.15% of frames, capacity 4 covers 99.87% for twice the width. | that field was **81.6%** of walk allocations |
| **A veto is one bit.** `vetoed[e/64] & 1<<(e%64)`, checked before every candidate step. | — |
| **Interned range sets.** Extension ranges are deduplicated across the graph rather than stored per node. | — |
| **Lazy N-way merge.** The sharded ranking pulls from a heap of per-part run iterators instead of concatenating and sorting. | — |
| **Every cache is bounded and every bound is asserted.** The render cache is 8 MiB; the heat cache is 8192 entries, with a compile-time assertion that it exceeds the prefetch walk's 2048-row reach. | — |

## 16. What we did not do

Recorded because they were measured and rejected, not overlooked:

- **A hand-written colorizer.** 50–100x faster on the styles term, which is
  not the term that matters. The frame is not styles-bound.
- **Calibrating core speed by benchmarking at startup.** In a VM the
  vCPU→pCPU mapping need not be stable, and it burns CPU at exactly the
  moment the machine was made free.
- **Clock frequency as a proxy for core speed.** 3.8 GHz of E core is not
  78% of a P core. Different microarchitectures are not comparable by clock,
  even when — as here — it happens to give the right answer.
- **A per-frame arm/disarm of the drawing core's SMT sibling.** 22 syscalls
  per 434 µs frame, and it arrives late anyway: `sched_setaffinity` does not
  preempt a running thread.
- **Balancing sweep parts by root count.** It really is a defect — 3 parts of
  24 hold 23.7% of the roots and do 0.82% of the work — and it is worth 1–3%
  of makespan. Cost is depth into the blob, which is not knowable in advance.

---

# Part 3 — What the measuring taught us

The methodology mistakes cost more time than the optimizations did.

**Pin every measurement, always.** The identical binary, unpinned, gave
7.33 / 7.83 / **12.80 s** on one benchmark — a 1.7x spread that swamped the
1.7x effect being measured and, in one ordering, made the improved code look
2x slower. Pinned, three runs agree to within 4% and reproduce a table taken
a week earlier to two significant figures.

**A timer around `free` on glibc measures almost nothing.** The frees looked
like 47–56 ms; the real cost was 251 ms, 56% of the confirm, and 195 ms of it
was billed to two innocent `vec![]` calls in the next phase. Proven by
control, not inference: swapping the drop for `mem::forget` — identical work
otherwise — took the confirm 448 → 189 ms.

**Compare p90 and max, not medians, when the app can skip work.** Starved of
CPU, protolens drops its follow-up repaints, so the sample count fell 307 →
60 and the median compared two different *populations* of frame rather than
the same frame slowed. The raw median ratio read 7x. The honest number is
1.8x.

**Get a second corpus before naming a constant.** "About 512 state groups per
part" was a clean fit on googleapis and completely wrong: a second corpus 23x
smaller moved optimal groups-per-part by 22x while moving the optimal *part
count* by less than 2x. Part count is the near-invariant.

**The same question can have two right answers.** `HashSet::retain` is
O(capacity), which is correct for a confirm scrubbing 2.86 M descendants and
catastrophic for a bake doing 70 893 splices of a handful each — 0.153 s
either way for the confirm, 5.7 s versus **17.5 s** for the bake. The code
now walks whichever side is smaller.

**Measure before rewriting the thing that looks slow.** Tree-sitter is the
largest term in a frame and a hand-rolled scanner would be 50–100x faster on
it — and it still would not be worth doing, because the frame is not
styles-bound. Conversely, the biggest single latency win in the whole list
(102.24 s → 5.78 s) was one `if` guarding an arithmetic identity.

---

# Scoreboard

| | before | after | |
|---|---|---|---|
| open a 25.6 MB blob, inferred | 8.72 s | **1.10 s** | 7.9x |
| retype the document root | 4.55 s | **0.153 s** | 30x |
| type inference sweep | 6.86 s serial | **0.89 s** | 7.7x |
| a screenful of scoring queries | 412 ms | **91 ms** | 4.5x |
| speculative read-ahead under scroll | 3000 sweeps | **7 000–24 000** | 2.4–8x |
| full-document search miss | 1.63–2.05 s | **183–272 ms** | 7x |
| frames drawn for a 60-event burst | 60 | **1** | 60x |
| peak memory, root retype | 4.18 GiB | **1.66 GiB** | −60% |
| wakeups while idle, per 10 s | 90 | **0** | — |

And none of it changed a byte of output. Every change that touches what the
renderer produces was gated on exporting the whole document and comparing it
with `cmp` against the previous build — 249 734 534 bytes, identical, on both
sides of the bounded render, the deferred free and the incremental line
counts. That gate is the only reason any of the rest was safe to attempt.
