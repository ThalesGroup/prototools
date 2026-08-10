<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Making protolens fast — slides

Companion to [performance.md](performance.md). One `##` per slide.
Every number is measured on `googleapis.desc`, release, `taskset`-pinned,
unless the slide says otherwise.

## The problem

- A terminal viewer for **binary protobuf**, no schema required
- Input: a 25.6 MB blob — 49 255 candidate root types
- Output: a **5 281 124-line**, 238 MB document, losslessly re-exportable
- Screen: **48 rows**
- We were materializing ~110 000 screens to show one

## Two clocks, opposite goals

| | budget | measure |
|---|---|---|
| **the frame** | ~16 ms | tail latency |
| **the sweep** | seconds | makespan of the *last* part |

Two moves, over and over:

- **do less before the user sees something**
- **do the rest on cores nobody is waiting on**

## Latency 1 — render a screenful, not a document

- Pass a row budget into the renderer; it stops and reports where
- Every stop becomes a fold; folds are already in the UI vocabulary

| | before | after |
|---|---|---|
| open, inferred root | 8.72 s | **1.10 s** |
| open, `--raw` | 4.65 s | **1.13 s** |
| lines materialized | 5 279 383 | **15 575** |

- Renderer in isolation: 2.0 s → **4.9 ms** (410x), 239 MB → 360 KB

## Latency 1b — a bounded render costs its *frontier*

- Budget 51 → **15 599 rows**, not 51
- Stopping inside a node emits every ancestor and every begun sibling
- The stated limit: a **wide, flat** document cannot be bounded by folding
  — there is nothing to fold

## Latency 2 — finish it while nobody is waiting

- The rest is paid in the event loop's **idle arm**, one subtree per step

| root retype | keystroke | deferred | worst step |
|---|---|---|---|
| render everything | 4.55 s | — | — |
| render a screenful | **0.59 s** | 6.55 s | 22–25 ms |

- Four idle jobs, and the **order is load-bearing**:
  search → discard → bake → **read-ahead last**
- Every bake splice changes the structure; read-ahead restarts on structure
  change, so it must run in the only state it can survive

## Latency 3 — stop paying for the document you replaced

- A retype vacates 2 864 189 boxed strings
- Freeing them inline: glibc parks small frees on fastbins, bills the next
  `malloc` that misses
- Now moved to a vector, freed 65 536 at a time in the idle arm

| confirm | before | after |
|---|---|---|
| keystroke | 0.428 s | **0.153 s** (2.8x) |
| deferred work | 5.38 s | 5.74 s (+5%, on purpose) |

## Latency 4 — coalesce the burst, draw once

- A mouse wheel arrives as dozens of reports in **one** `write`
- Dispatch the whole drain, draw once, 8 ms budget

| 60 wheel reports | before | after |
|---|---|---|
| still panning | 60 frames | **1** |
| already at the bound | 60 frames | **0** |

- All 60 dispatch in 2.3 ms — a real burst never nears the budget

## Latency 5 — drop the colors while fingers are moving

- Tree-sitter is the largest single term in a frame
- Input already queued → draw monochrome, recolor when the stream settles

| keys 0.5 ms apart | colored | monochrome |
|---|---|---|
| styles µs | 320–480 | **0** |
| whole draw µs | 1194 | **632** |

- **Honest caveat:** at ordinary autorepeat (~30 ms) this changes *nothing*.
  It buys robustness for wheel inertia, ssh/tmux, wide terminals.

## Latency 6 — do not compute an answer you can prove is zero

- After each splice, re-clamp the horizontal pan: O(pane height), ~70 800x
- But the clamp is `0.min(x)` when the pan is 0 — and it always is

| deferred drain | before | after |
|---|---|---|
| 50-row pane | 6.55 s | **5.42 s** |
| 500-row pane | 15.53 s | **5.51 s** |
| 5000-row pane | **102.24 s** | **5.78 s** |

- One `if`. Identical steps, identical splices, identical bytes.

## Latency 7 — put the text where the tree already is

- Was: a `Vec<String>` of 5.28 M lines beside the tree → editing near the
  front is a memmove of the rest
- Now: text lives per arena slot; closing braces are *derived*, not stored

| commit a retype | first record | last record |
|---|---|---|
| before | 102 ms | 61 ms |
| after | **0.66 ms** | 1.18 ms |

- The unexplained 2x first-vs-last spread **inverted** — which explained it

## Latency 7b — layout is a performance feature

- 44-byte node slot holding **no links** (links in one flat `Vec<u32>`)
- One slot per *packed run*, not per element; 12-byte heat state
- Each size pinned by `const _: () = assert!(size_of::<..>() == ..)` —
  an **equality**, because growth is the regression

| memory | before | after |
|---|---|---|
| resident at rest | 1.96 GB | **875 MB** |
| peak, root retype | 4.18 GiB | **1.66 GiB** |

## Latency 8 — search that yields

- A full-document miss is a 238 MB scan → resumable sweep in the idle arm,
  1000 candidates a slice, frame only when the *answer* changes

| full-document miss | before | after |
|---|---|---|
| case-folding | 1.63–2.05 s | **183–272 ms** (7x) |

- `memchr2` over the two-case alphabet; worth **zero** case-sensitively
- Two guards: needle's first char ASCII **and** haystack all-ASCII
  (U+212A KELVIN SIGN folds to `k`)

## Where a frame actually goes

Medians, 200x50 pty, 48 rows:

| µs | top | after `G` |
|---|---|---|
| key dispatch | 10 | 24 |
| window descent + walk | 5 | 57 |
| **tree-sitter styles** | **276** | **431** |
| line spans + text | 75 | 93 |
| **whole draw** | **821** | **1359** |

- Paging is O(page), not O(document) — the end draws as fast as the top

## Throughput 1 — deduplicate before you parallelize

- Compile the schema to a scoring DFA, **Hopcroft-minimize** it
- Types that consume the wire identically become one state

| | |
|---|---|
| root types | 49 255 |
| distinct behaviors to walk | **17 572** |

- A state group is therefore **indivisible** — splitting one duplicates work
  instead of dividing it. Deduplication constrains all scheduling downstream.

## Throughput 2 — split far past the core count

| parts | serial work | slowest part | 8-worker makespan |
|---|---|---|---|
| 1 | 6.86 s | 6.855 s | 6.86 s |
| 8 | 6.84 s | 1.833 s | 1.83 s |
| **16** | **5.40 s** | 0.839 s | 1.08 s |
| 32 | 5.83 s | 0.737 s | 1.05 s |
| 256 | 7.96 s | 0.604 s | 1.32 s |

- **Splitting is cheaper even single-threaded** (6.86 → 5.40 s at `--jobs 1`)
- The limit is **load imbalance**, not duplication: 8 equal-root-count parts
  ran 0.263 … **1.865 s**, a 7x spread
- Shipped: 24 parts off one atomic cursor — sweep 1.89 → **0.89 s**

## Throughput 3 — the unit of work is a part of a *query*

- Was: one query = one task = one worker, seven idle behind it
- Now: one partition per process, workers pull `(query, part)`

| screenful of heat queries | before | after |
|---|---|---|
| pane top 5 000 | 412 ms | **91 ms** |
| pane top 200 000 | 408 ms | **93 ms** |
| pane top 2 000 000 | 259 ms | **59 ms** |

- **Finer parts cannot replace preemption**: the biggest group is 4 645 roots
  at *every* part count; 24 → 4096 parts makes the root walk 65% **slower**
- An abort flag needs an **epoch** — a truncated score is wrong, not partial

## Throughput 4 — throw away demand that is no longer true

- Scroll fast → the queue fills with rows that already left the screen
- Stamp each request with its window generation; drop stale ones at pop

| 400 PageDowns @ 20 ms | off | on |
|---|---|---|
| prefetch sweeps | 3090 | **23 819** |
| stale discarded | 0 | 1557 |
| unresolved cues on screen | 20 | **3** |

- The mechanism is **not** queue ordering — that was already right. A queued
  visible request holds an occupancy bit that parks every speculative worker.
  The prefetcher was not outranked; it was **stopped**.

## Throughput 5 — not all cores are the same core

Three tiers behind one `nproc`:

| CPUs | clock | notes |
|---|---|---|
| 0–3 | 4.9 GHz | 2 P cores, SMT paired |
| 4–11 | 3.8 GHz | 8 E cores, two shared L2s |
| 12–13 | 2.1 GHz | low-power, **no L3** |

| scoring walk | relative cost |
|---|---|
| P core alone | 1.00 |
| E core | 1.52 |
| LP-E core | 2.85 |
| 2 SMT threads of one P core, each | **1.92** |

- SMT yields `2/1.92 = 1.04` cores of throughput for 1.92x the latency

## Throughput 5b — the drawing thread cares most

| pinned to one CPU | P core | E core |
|---|---|---|
| startup `--jobs 1 --raw` | 1.163 s | 1.498 s (1.29x) |
| **median frame** | **434 µs** | **1965 µs (4.5x)** |

- So: **if and only if the kernel states which CPUs are fast**, give the main
  thread one whole physical core and hand workers `inherited − that core`
- No benchmark, no guess, no fallback, silent on failure
- On a machine that declares nothing, protolens does nothing

## Throughput 5c — the last part decides the makespan

- A worker that finds the cursor empty **donates its fast seat** to the
  longest-running worker on a slower one
- Forced with a mask that excludes the donor's own CPU — the kernel will not
  move a running, never-sleeping thread whose CPU is still in the mask
- **Status: simulated, not measured.** 3000 random hand-out orders over the
  measured per-part costs: makespan 2148 → **1598 ms** mean, 2692 →
  **1793 ms** p95. The dev VM exposes no topology, so the feature correctly
  disables itself and the gain cannot be observed there.

## Throughput 6 — sleep properly

| idle, 10 s | before | after |
|---|---|---|
| voluntary context switches | 90 | **0** |

- 90 was exactly 9/s: a 250 ms activity tick + a 200 ms input poll
- Untimed `poll(2)` on tty + SIGWINCH pipe + shutdown pipe
- Three things are mandatory, not optional:
  - **your own SIGWINCH pipe** (crossterm synthesizes `Resize` from its own)
  - **drain parsed events before blocking** (crossterm reads 1 KiB at a time)
  - **back off when the drain fails**, or the untimed wait spins forever

## Lessons 1 — pin every measurement

- Same binary, unpinned: 7.33 / 7.83 / **12.80 s**
- A 1.7x spread swamping the 1.7x effect — and in one ordering it made the
  *improved* code look 2x slower
- Pinned: three runs within 4%, reproducing a week-old table to 2 s.f.

## Lessons 2 — a timer around `free` measures nothing

- glibc fastbins defer consolidation; the bill lands on someone else's
  `malloc`
- Frees looked like 47–56 ms. Real cost **251 ms** — 56% of the confirm —
  and 195 ms of it was billed to two innocent `vec![]` calls
- Proven by **control, not inference**: swap the drop for `mem::forget`,
  identical work otherwise → confirm 448 → **189 ms**

## Lessons 3 — p90, not medians, when the app can skip work

- Starved of CPU, protolens drops follow-up repaints
- Sample count fell 307 → 60: the median compared two different
  **populations** of frame, not the same frame slowed
- Raw median ratio read **7x**. The honest number is **1.8x**.

## Lessons 4 — get a second corpus before naming a constant

- "About 512 state groups per part" fit googleapis cleanly
- A corpus 23x smaller moved optimal groups-per-part by **22x** — and the
  optimal *part count* by **less than 2x**
- Part count is the near-invariant. Name that instead.

## Lessons 5 — the same question can have two right answers

- `HashSet::retain` is **O(capacity)**
- Right for a confirm scrubbing 2.86 M descendants; catastrophic for a bake
  doing ~70 800 splices of a handful each

| | confirm | bake |
|---|---|---|
| retain unconditionally | 0.153 s | **17.5 s** |
| walk the smaller side | 0.153 s | **5.7 s** |

## Lessons 6 — measure before rewriting the thing that looks slow

- Tree-sitter: largest term in a frame, and a hand-rolled scanner would be
  **50–100x** faster on it. **Rejected** — the frame is not styles-bound.
- The biggest latency win in the whole project (**102.24 s → 5.78 s**) was
  one `if` guarding an arithmetic identity.
- Intuition picked the wrong target both times.

## Scoreboard

| | before | after | |
|---|---|---|---|
| open a 25.6 MB blob | 8.72 s | **1.10 s** | 7.9x |
| retype the root | 4.55 s | **0.153 s** | 30x |
| type-inference sweep | 6.86 s | **0.89 s** | 7.7x |
| a screenful of scoring | 412 ms | **91 ms** | 4.5x |
| read-ahead under scroll | 3090 | **23 819** | 2.4–8x |
| full-document search miss | 1.63 s | **183 ms** | 7x |
| frames for a 60-event burst | 60 | **1** | 60x |
| peak memory | 4.18 GiB | **1.66 GiB** | −60% |
| idle wakeups per 10 s | 90 | **0** | — |

## The closing line

- **249 734 534 bytes, `cmp`-identical**, on both sides of the bounded
  render, the deferred free and the incremental line counts
- That gate is the only reason the rest was safe to attempt
