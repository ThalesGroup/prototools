<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Algorithms and structures used across prototools

A catalogue of the algorithmic and mathematical ideas that the workspace
relies on, with the crate each lives in and the reason it was chosen over
the obvious alternative. It is a reference list, not a narrative — the
narratives are in `docs/protolens/techniques.md` (accessible) and
`docs/protolens/performance.md` (detailed).

Everything below is implemented unless the entry says otherwise. A short
section at the end lists ideas that are designed and argued but not built.

---

## 1. Automata, equivalence and quotients

### Bisimulation equivalence on the schema graph
`prototext-graph`, `docs/schema-match.md` Part 2.

The whole descriptor database is one directed graph: nodes are message
types, edges are fields, and an edge's label is the wire tag
`(field_number, wire_type)`. The graph is deterministic — for a given node
and tag there is at most one outgoing edge — and recursive message types
make it cyclic.

Two nodes are equivalent when they generate the same set of tag-path
strings, defined coinductively so that cycles are handled by the fixpoint
rather than by a special case: `u ~ v` iff for every tag `a`, an edge
`a → u'` on one side implies an edge `a → v'` on the other with `u' ~ v'`.

Two mutually recursive schemas `a → b → a` and `c → d → c` with matching
tags merge to `{a,c} → {b,d} → {a,c}`.

The equivalence is deliberately coarser than the schema's own type system:
`string` and `bytes` at the same tag are indistinguishable on the wire, so
they merge. That is correct for this purpose — the graph exists to explain
bytes, not to preserve names.

### Hopcroft partition refinement
`prototext-graph`.

The quotient `G/~` is computed by partition refinement: initialize blocks
by outgoing-tag signature (leaves pre-partitioned by wire type), then
repeatedly split any block whose members' `a`-successors fall in different
blocks, until stable. `O(|Σ| · |E| log |V|)` in the number of distinct
tags, edges and nodes.

Measured on `googleapis.desc`: 49 255 root types collapse to **17 572
distinct behaviors** (`protolens/src/sweep.rs:77`; the same table records
1 900 / 1 166 for a smaller corpus). The nodes of the quotient are exactly
the walker's `StateID`s.

The honest limit is recorded with it: this is a RAM optimization and a
traversal-count optimization, not a scoring optimization. `K` schemas
sharing one state replace `K` transition-table lookups with one, but still
pay `K` score increments. Deduplication compresses the tail of the schema
tree; the shallow levels, where schemas are intentionally distinct,
dominate walk cost and are unaffected.

### Static versus active back-references
`prototext-graph`, `docs/schema-match.md` §Scoring with Back-References.

A `StateID` carries, at compile time, the set of `(schema, message_type)`
nodes Hopcroft merged into it. The walk must *not* score through that set:
at runtime only some of those schemas are actually routing through the
state at this depth, and some may already be vetoed. The walk therefore
carries a second, per-frame list — which initial roots reached this state
*here* — and scores through that.

The distinction is the difference between a compile-time property of a
node and a runtime property of a path. Using the first for the second
over-counts.

---

## 2. Layout as index

### Level-order arena built by counting sort
`prototext-core/src/serialize/render_text/arena.rs`, spec 0216.

The observation the arena rests on is that **the child decomposition of a
byte range is schema-free**. A schema supplies names, types and
presentation; it never moves a boundary. So one maximal tree is built from
the bytes alone, and every schema interpretation is a *pruning* of it.

Building it breadth-first requires reading it depth-first: the bytes only
admit a recursive descent, but the layout wanted is level order. The
resolution is a **counting sort keyed on depth** — depth is a small integer
bounded by the recursion cap, so the pass buckets rather than compares and
runs in O(n + D). Depth alone is a sufficient key: within a depth, document
order is already the order wanted.

### Navigation by arithmetic instead of stored pointers
`protolens/src/tui/structure.rs`.

Once the arena is in level order with siblings contiguous, every link is
arithmetic on two arrays:

| link | expression |
|---|---|
| children of `i` | `first_child[i] .. first_child[i + 1]` |
| child count | one subtraction |
| k-th child | one addition |
| next sibling | `i + 1` |
| sibling ordinal | `i - first_child[parent[i]]` |
| is a root | `parent[i] == i` |

A root being its own parent terminates the upward climb without a sentinel
value and without a branch that means anything else.

Its soundness condition is stated and tested rather than assumed: a
rendered node consumes either the whole of its slot's child block or none
of it, never a subset (`decode::the_arena_covers_a_real_corpus`).

### One allocation, sliced
`prototext-core/.../arena.rs:334`.

`Arena { cells: Vec<u32>, len }` — a single vector partitioned into
`first_child` (n + 1 entries), then `parent`, `raw_start`, `raw_end` (n
each), then a `probes_as_message` bitset of `n.div_ceil(32)` words. One
allocation, one cache-friendly region, and the bitset costs a thirty-second
of a byte per node rather than a byte.

### Nothing about it is dynamic
Spec 0216.

The tree is a function of the bytes, so it is built once and never edited:
no free list, no tombstones, no refcounts, no generation tags on slot
indices. A slot index is a `u32` that stays valid for the session. This is
what makes the arithmetic above safe to inline everywhere.

### Reverse linear pass over a level order
`protolens/src/tui/node_status.rs`.

Level order guarantees a parent's index is below every child's index.
Walking the array backwards therefore visits every child before its
parent — a bottom-up fold with no recursion, no explicit stack and no
queue, in O(n).

### Compile-time size equalities
`protolens/src/decode.rs:643`, `prototext-core/.../sink.rs:1271`,
`protolens/src/tui/heat_cue.rs:111`.

`const _: () = assert!(size_of::<TreeNode>() == 44);` and the same for
`NodeSpan == 32` and `HeatState == 12`. Equalities, not bounds: a field
that *shrinks* the struct is as much a change to the memory model as one
that grows it, and both should have to be acknowledged.

### Leaf sentinels descending from `u32::MAX`
`prototext-graph`.

Canonical leaf states (one per wire type) are numbered downward from the
top of the index space, so they cannot collide with compiled state IDs
numbered upward from zero, and no separate tag bit or enum wrapper is
needed to tell a leaf from a state.

---

## 3. Monotonicity as a lever

Several places turn a guess into a search by establishing that a predicate
is monotone, so a boundary can be found in one pass instead of by trial.

### The sticky, prefix-monotone veto
`prototext-graph/src/score/walk.rs`, exploited in `docs/protoscan/scan.md`
§3.3.

`set_vetoed` is followed by `ae.entries.clear()`: a veto triggered by a
byte in a prefix cannot be undone by extending the buffer. That makes
"where does this candidate end?" a well-posed question —

> the candidate ends at the **maximal prefix that does not veto** —

findable in a single pass, instead of Protod3's descending brute force of
up to ~1 024 trial parses per candidate.

Measured: the bogus 25 MB whole-file candidate is vetoed against
`FileDescriptorProto` at score 0, while the correct 291-byte first record
is not. The veto separates them with no length heuristic at all.

What shipped uses the monotonicity as a *rejection* test rather than as a
boundary search. A veto fires inside a field the walk has already consumed,
so the offset it leaves may sit past the record's true end; the boundary
protoscan uses is the walk's own termination point (§7), and a vetoed
candidate is discarded rather than trimmed.

### A monotone watermark, and why an omission was permanent
`protolens`, spec 0258.

`descend` is a monotone watermark rather than a per-visit flag. The
consequence is worth stating because it inverted a diagnosis: a subtree
missed by the watermark is not rendered *late*, it is never rendered — the
mark is the gate, so an omission is permanent, not delayed.

### An epoch counter, not an abort flag
`protolens`, spec 0262.

Cancellation uses a monotonically increasing epoch, compared against the
epoch a task was issued under. An `AtomicBool` cannot express this: it can
be raised and lowered again while a long walk is in flight, and the walk
has no way to tell "never aborted" from "aborted and re-armed". A counter
that only goes up cannot be misread.

The same reasoning stamps queued render requests with the window they were
issued for (spec 0252), so a request that outlived its viewport is
discarded on arrival rather than serviced.

---

## 4. Writing a stream whose lengths are not yet known

### The in-place text → wire encoder
`prototext-core/src/serialize/encode_text/placeholder.rs`, driven by
`encode_text/mod.rs`.

A LEN message on the wire is *tag, length, body*, but the length is not
known until the body has been written. The encoder writes one output
`Vec<u8>` strictly left to right and never revisits it.

- On `{`, `write_placeholder` reserves `BASE_OVERHEAD + ohb` = 11 + ohb
  bytes: a `waste` byte, a 5-byte `next_placeholder` offset, and
  `varint_room` of 5 + ohb. The body follows immediately.
- On `}`, `fill_placeholder` writes the length varint **flush-right**
  inside `varint_room`, so it abuts the body; the unused prefix becomes
  the recorded `waste`.
- `compact` then makes one left-to-right pass, walking a linked list and
  using `copy_within` to slide real data over the wasted prefixes, then
  truncates.

Three properties carry it:

1. **The bookkeeping lives in the bytes that are about to be deleted.**
   The placeholder holds its own waste count and a 5-byte offset to the
   next placeholder — a linked list threaded through the waste itself, so
   there is no side table and no vector of offsets. `NO_NEXT` is
   `0xFF_FFFF_FFFF`.
2. **List order is consumption order.** Placeholders are linked in opening
   order, which is exactly the left-to-right order compaction needs. No
   sort, no recursion.
3. **The length written is the post-compaction length.**
   `child_len_compacted = child_len_raw - frame_acw`, where `acw` is the
   accumulated compaction waste propagated up the frame stack. Without
   this, every enclosing message declares a length that counts bytes about
   to vanish.

Each byte is written once and moved at most once. The alternatives it
replaces are a per-message temp buffer plus splice (one allocation per
message, and a byte at depth *d* copied *d* times), and measure-then-write
(the encoder runs twice, and the two passes must agree).

`ohb` — *overhanging bytes* — is non-minimal varint padding, preserved so
that a re-export is byte-identical rather than canonicalized. That is why
`varint_room` is `5 + ohb` and not simply 5.

### The same instinct at the scalar level
`prototext-core`, `docs/prototext/performance.md` §P1–P4, §earlier
analysis.

`encode_varint_bytes(value) -> Vec<u8>` immediately `extend_from_slice`d
into a parent buffer was 21% `memmove` plus 11% allocator; replacing it
with `write_varint_ohb(value, ohb, &mut out)` writes in place. Measured:
1.7 ns into a reused buffer against 15 ns into a fresh one, a 10x gap that
is entirely allocation.

---

## 5. Partitioning, scheduling and merging

### Greedy largest-first bin packing over indivisible groups
`prototext-graph/src/score/walk.rs:436`, `partition_roots`.

Roots that share a Hopcroft state cannot be split across parts: if two
parts both hold roots of the same state, both parts walk that state and the
work is duplicated rather than divided. So the *group* is the unit, and the
groups are disjoint by construction.

Balancing is largest-group-first onto the currently-smallest part — the
standard greedy heuristic, `O(G log G)` on a few thousand groups against
seconds of walking. The comment says plainly that it does not need to be
cleverer.

Its known weakness is recorded rather than hidden: it balances on root
*count*, not on walk cost, so on a real corpus three parts hold 24% of the
roots and do 0.8% of the work
(`docs/protolens/performance.md`, sweep scheduling notes).

### N-way merge over pre-sorted runs
`protolens/src/sweep.rs:768`, `Merged`.

Each shard returns its candidates already sorted, so the combination is a
`BinaryHeap` N-way merge exposed as an `Iterator`, with `collect()` as the
eager special case.

The reason is not the linear scan a `sort_by` would pay to rediscover run
boundaries — that is microseconds against a multi-second sweep. The reason
is that **a merge can stop early and a sort cannot**, and almost no caller
wants the whole ranking: the winner is the first element plus one more for
the tie check, the statistics want the best score and how many share it,
and the pane's `top_n` is capped at a screenful.

`Head::cmp` is written as `candidate_order` with its arguments swapped
rather than as a hand-spelled inverse, because a hand-spelled inverse would
compile, agree, and then quietly stop agreeing the first time the tie-break
moved — and every run is sorted under precisely that relation.

`remaining` is known exactly up front (the shards partition the roots, so
the total is the sum of run lengths), which makes the iterator an
`ExactSizeIterator`: a caller that takes only the top few can still report
how many there were.

### Work stealing off an atomic cursor
`protolens`, specs 0217 / 0218 / 0262.

The unit of work is a *part of a query* rather than a whole query, handed
out by `fetch_add(1, Relaxed)` on a shared cursor. `SWEEP_PARTS` is 24. The
limiting factor is measured and named: per-part load imbalance, not
duplicated convergence.

### Seat donation on a heterogeneous machine
`protolens`, specs 0269 / 0270.

One worker per physical core, each pinned to a single CPU. When a member
idles it donates its seat to the longest-running straggler. The relevant
measured facts: SMT gives 1.04x throughput for 1.92x latency on this
workload, and `sched_setaffinity` migrates a running thread within about a
millisecond *only* when the new mask excludes its current CPU.

The projected effect (3 000 random hand-out orders replayed over measured
per-part costs) is a makespan of 2148/2692 ms falling to 1598/1793 ms, and
it is quoted as a projection because the dev VM exposes no CPU topology.

---

## 6. Zero-copy artifacts and reproducibility

### rkyv archives read by pointer cast
`prototext-graph/src/fds_index.rs`, `docs/prototext/lazy_fds_design.md`.

Both the descriptor index and the compiled graph are rkyv archives, mmapped
and accessed through `access_unchecked` — two `mmap` calls and a pointer
cast, no deserialization. The startup cost of a large descriptor set drops
from about 3 s of symbol-table construction to under a millisecond, and the
OS pages in only the fraction of the file actually touched.

The price is that the archived layout is now an ABI: `access_unchecked`
forbids a silent layout change, so the format carries a version tag.

### Making a hash map's serialized bytes a function of its key set
`prototext-graph/src/fds_index.rs:16-51`, spec 0177.

Archiving a `HashMap` is not deterministic by default, and fixing the seed
is only half the fix.

- `ArchivedHashTable::serialize_from_iter` assigns each key the *first
  empty* slot in its probe sequence and writes the data region in iteration
  order, so the bytes depend on source iteration order. `RandomState` is
  seeded per process, so every build differed. Fixed by
  `FxBuildHasher = BuildHasherDefault<FxHasher64>`.
- hashbrown resolves a probe-sequence collision in favor of whichever
  colliding key was inserted *first*. So a randomly ordered source — a
  `HashMap` at the pyo3 boundary, or a Python `set` — still leaks into the
  bytes even with a fixed seed. Fixed by `canonical_map`, which sorts the
  entries before inserting.

The result is a layout that is a function of the key set alone. It costs
nothing at read time: the source hasher never reaches the archive, and
archived lookups use `FxHasher64` regardless.

### Lazy pool with DFS dependency closure
`prototext-schema::lazy_pool`, `docs/prototext/lazy_fds_design.md`.

The index maps type → file, file → byte span in the raw `.pb`, and file →
its imports. Resolving a type does a depth-first traversal of the
dependency graph, decoding each file's descriptor only after all of its
dependencies are in the pool — which is a hard requirement of the
reflection library, and which DFS post-order satisfies for free.

Measured shape: startup under 1 ms, first use of a type 1–10 ms, later uses
under 1 µs.

---

## 7. Search, scanning and anchors

### Anchor choice is a candidate-count decision
`docs/protoscan/scan.md` §2.4 — measured. protoscan uses the `0x0A` anchor;
the profile split below is not built.

Both candidate anchors for finding descriptors in a binary are cheap to
evaluate (`memchr` for one byte, `memmem` for six). What differs by two
orders of magnitude is how many candidates survive to the expensive stage:

| haystack | size | `0x0A` anchor | `.proto` anchor | ratio |
|---|---|---|---|---|
| `gh` binary | 54 976 608 B | 242 293 | 2 205 | 110x |
| `googleapis.desc` | 25 660 332 B | 1 040 408 | 44 100 | 24x |

They are not equivalent in coverage — the `.proto` anchor structurally
cannot find a descriptor whose name does not end in `.proto`, and the
`0x0A` anchor plus a path filter rejects garbage the other would hand to
the parser. So the resolution is a profile split rather than a winner.

### The end of a record is a question the schema already answers
Specs 0238 and 0239, implemented 2026-08-04;
`fdp-scan-pyo3/src/lib.rs:113-181`.

Finding descriptors in an unframed haystack needs two answers — where a
record starts and where it ends — and only one of them is a schema
question. The start is a heuristic and stays one: a `0x0A` byte, a length
under 200, and a payload that looks like a `.proto` path. Nothing in the
schema nominates a start; it says `name` is a `string` and no more.

The end used to be a hand-rolled wire walk whose stop rule was "a second
field 1". The replacement gives the scorer the **entire rest of the
buffer** — no guessed length — and takes the boundary from where the walk
stops. Under the scan policy a root terminates at the first tag it cannot
carry, and `FileDescriptorProto.name` is singular, so the next record's
field-1 tag is the boundary. The hand-rolled rule was that same fact
written out by hand for one field; the schema supplies it for the other
five singular fields and for every undeclared field number as well, at no
extra cost, and generalizes to message types nobody has thought about.

The old rule found **1** candidate in a 25.6 MB `FileDescriptorSet` where
there are **7 771**, and the failure was silent — protobuf is permissive,
so `name` was simply overwritten 7 770 times and parsing the whole blob
succeeded. After the change all 7 771 are found, with boundaries matching
the framing exactly, at 0.57 s over the corpus.

Two consequences are worth separating out. The accept rule is on the
**defect counters**, never on `score()` — see §9. And a *veto* yields no
boundary at all rather than a shorter one: a veto fires inside a field
already consumed, so the offset it leaves may lie past the true end of the
record. A vetoed candidate is rejected outright instead of being trimmed.

The rejected alternative is instructive. The investigation had proposed
embedding two roots, `FileDescriptorProto` and `FileDescriptorSet`, so that
a whole descriptor set could not be mistaken for one enormous descriptor;
it measured that the scorer separates the two shapes cleanly in both
directions (the 291-byte record is vetoed as a set and scores 35 as a
descriptor; the 25 MB file is the reverse). The measurement holds, but the
second root became unnecessary: a correct boundary rule makes the confusion
structurally impossible with one root, because the second `file` entry's
tag is exactly what terminates the walk. Classifying the input is a
different question from finding records in it, and it has no principled
stopping point — if `FileDescriptorSet`, why not every other type a
haystack might hold?

### A case-folding prefilter, and the two guards it needs
`protolens`, spec 0235.

Case-insensitive substring search is prefiltered with `memchr2` on the two
cases of the needle's first character. That is only valid under two
conditions, both checked: the needle's first character must be ASCII, and
the haystack must be entirely ASCII. Otherwise U+212A KELVIN SIGN case-folds
to `k` and the prefilter skips a real match.

### A resumable sweep instead of a blocking one
`protolens`, specs 0235 / 0246.

A full-document search miss used to cost a fixed ~1.8 s. The search is now
a resumable state machine driven from the idle arm, so it advances between
frames and stops at every match rather than at the end.

---

## 8. Bounds, depth and adversarial input

### Overflow-safe bounds tests
`prototext-core`.

`len > buflen - pos` rather than `pos + len > buflen`. The length is an
attacker-chosen `u64` off the wire, and the natural form wraps.

### Depth as a counter, not as a call stack
`prototext-core`, spec 0171.

A START_GROUP tag costs one byte, so a 1 MB input can demand a million
recursion frames. Group extent scanning is therefore iterative with a
`usize` depth counter. Where recursion does remain, `MAX_WIRE_DEPTH` is
justified by a measured per-frame stack cost for each walker and a
per-(walker, thread) margin table, rather than by a round number.

### A varint that overflows is a truncated varint
`docs/schema-match.md` §Note on too-big varints.

When the accumulated value reaches 2⁶⁴ the parser treats the varint exactly
as it treats one that never terminates. This collapses what would otherwise
be a separate proto-type range check into an existing wire-level invalid,
and means no proto-type table needs an "unreachable" row.

---

## 9. Scoring as a measurement, not a verdict

### The formula and the order of its terms
`prototext-graph`, `docs/scoring-flaws.md`.

```
score = matches·1 − unknowns·10 − out_of_range·15 − non_canonical·20 − mismatches·30
```

The ordering encodes evidence strength. A match is weak positive evidence —
many schemas declare field 1. An unknown field is moderate negative
evidence but **never a veto**, because version skew and unknown extensions
are legitimate. A wire-type/proto-type conflict is definitive: the schema
was already credited with knowing the field, and the bytes contradict its
declared type.

There are six veto sites, all enumerated. The asymmetry between
"termination" and "consumption" is deliberate and documented; demoting a
veto to a penalty costs pruning, not precision.

### Depth is scored implicitly
`docs/schema-match.md`.

A schema that declines to recurse into a length-delimited field is
suspended for that subtree: it pays exactly one outcome at the boundary and
receives nothing inside. So a deep match accumulates more evidence than a
shallow one without any explicit depth weighting.

### Size-proportional scores rank; they do not gate
`docs/protoscan/scan.md` §4.2, implemented as spec 0239 S4
(`fdp-scan-pyo3/src/lib.rs:177`).

Across all 7 771 genuine descriptors in `googleapis.desc`, `score()` ranges
from **8 to 171 309** — four orders of magnitude, purely as a function of
file size. Any absolute cut-off would reject the small files, admit
garbage, or both.

The size-independent accept rule is

```
!vetoed && unknowns == 0 && mismatches == 0
```

which on the same 7 771 descriptors gives 7 771/7 771 with zero vetoes,
unknowns, mismatches, non-canonicals or out-of-range values. `score()`
keeps one job: ranking, when more than one root survives the accept rule.

The complement is recorded too: `score_all` on a 291-byte record leaves
21 444 non-vetoed survivors out of 49 255 roots, ranking the right answer
first. Veto alone is a weak filter on a short blob; the accept rule is what
does the work.

### Geometric decay of the candidate set
`docs/schema-match.md` §Complexity Analysis.

`A(d, k) ≈ N · p^k · q^d`, with `p` the per-field survival rate at a given
depth and `q` the fraction of active schemas that recurse into a given
sub-message. Because `q < 1`, cost attenuates geometrically with depth and
is dominated by depth 0 — which is also, and not coincidentally, the region
Hopcroft cannot compress, since schemas are intentionally distinct near
their roots.

---

## 10. Correctness oracles

### Lossless round-trip
`prototext`, `prototext/tests/roundtrip.rs`.

`binary → text → binary` must be byte-identical for *any* input, including
malformed and non-canonical input. This is why the codec carries `ohb`
(overhanging varint bytes) and records non-canonical encodings rather than
normalizing them: canonicalizing would be a smaller, simpler codec with no
oracle. The corpus check is stated as an equality on the whole document —
249 734 534 bytes, identical on both sides.

### An exhaustive match as the verdict
`prototext-core/.../sink.rs`, spec 0266.

`ProbeSink` decides whether a length-delimited payload is a nested message.
The mechanism is an exhaustive destructuring: any invalid token disqualifies
the payload, and the *case* of the token is the verdict. Exhaustiveness is
not decoration here — it is what makes "we considered every token kind" a
compiler-checked claim rather than a comment.

### The measurement discipline itself
`docs/protolens/performance.md` Part 3.

Two rules that changed conclusions:

- Establish a **same-binary noise floor** per benchmark target before
  believing a delta. One bench target reproduces a +15.9% swing with no code
  change at all; a single virtualised core produced 287 / 420 / 449 µs on
  three consecutive runs of an unchanged binary.
- Never difference wall-clock totals to attribute per-part cost. Pin
  part → CPU explicitly and measure the parts.

### Static dispatch, checked structurally
`prototext-core`, spec 0110.

The `Sink` abstraction (`TextSink`, `ProbeSink`, `IndexingTextSink`,
`ArenaSink`) is consumed exclusively through static generics. When timing
was too noisy to prove the refactor cost nothing, the question was settled
structurally instead — there is no `dyn Sink` anywhere, so every call site
monomorphizes and there is no vtable indirection for a regression to hide
in.

---

## 11. Rendering under a budget

### A bounded render costs its frontier, not its budget
`protolens`, specs 0255 / 0257 / 0261.

Opening a document renders one screenful and defers the rest to an idle-arm
bake. The measured contrast on the same input:

| | unbounded | bounded to a screenful |
|---|---|---|
| time | 2.0 s | 4.9 ms |
| peak heap | 239 MB | 360 KB |
| spans built | 4 499 335 | 7 820 |
| rows produced | 5 278 322 | 15 599 |

The bake runs in roughly 70 800 steps, worst step 22–25 ms and none over
50 — the step count is chosen so that no single step can miss a frame.

### Counts, not positions
`protolens`, specs 0210 / 0254.

Nodes store line *counts*, never line positions. Editing a node's count
therefore touches its ancestors only, by a signed-delta climb, instead of
shifting every position after it. `assert_line_counts_are_exact` carries
the correctness of the whole scheme and is deliberately not weakened.

### Inline capacity chosen by distribution
`protolens`, `prototext-graph`, spec 0179.

`SmallVec` inline capacities are set from measured distributions, not from
`size_of`: `entries: SmallVec<[u32; 4]>` covers 93.4% of states inline, and
`occurrences: SmallVec<[(u32, u32); 2]>` covers 98.15% of frames — which is
81.6% of all allocations the walk would otherwise make.

The trap that came with it is recorded: the synthetic benchmark inverts the
walk's real allocation profile, so the distribution has to come from the
corpus.

---

## 12. reproto: closure, marking, and inverting a lossy encoding

One note on sources first: reproto's algorithm is written up in the module
docstring of `reproto/src/reproto/reproto.py`, not under `docs/reproto/`.
The entries below are read from there and from the phase implementations;
`docs/reproto/` holds empirical protobuf findings rather than the design.

### The problem, stated as a graph problem

Reproto is given a pile of compiled descriptors and a request — *these
types, please* — and must emit the smallest set of `.proto` files that
compiles, contains what was asked for, and contains nothing gratuitous.
"Compiles" is the hard half: protobuf source has no forward declarations,
so every type a written type mentions must also be written, and every type
that *encloses* a written type must be written too, even if nobody
references the enclosure.

Those two obligations pull in opposite directions along two different edge
sets, and that observation is the whole architecture. Everything reproto
does is a closure over one of them:

- **reference edges** point from a user to what it uses: a field to its
  message type, an RPC method to its input and output. Following them
  forward answers *what else do I need?*
- **containment edges** point from a child to its enclosure: a nested
  message to its parent message, a message to its file. Following them
  backward answers *what has to exist on the page before I can write this
  down?*

The graph is cyclic in both (mutually recursive types, mutually importing
files), so every traversal below is a closure with a visited set rather
than a recursion, and nothing anywhere assumes a tree.

The seven phases are: load files, build the descriptor pool in dependency
order, build the name graph, prune, propagate reachability, propagate
summoning, render. Phases 4–6 are the three closures; phases 1–2 are two
more traversals of the same kind on the file graph.

### Two relations on one node set, closed in opposite directions
Phases 4–6.

The FQDN graph carries two distinct edge sets over the same nodes:
*references* (`targets` — a field's type, a method's input and output) and
*containment* (`contains` / `parent` — a nested message inside its parent, a
message inside its file). Three marks are three closures, each over one
relation in one direction:

| mark | relation | direction | meaning |
|---|---|---|---|
| `is_pruned` | `contains` | forward from the pruned node | excluded, with everything inside it |
| `is_reachable` | `targets` | forward from the seeds | transitively needed by something asked for |
| `is_summoned` | `parent` | backward from reachable nodes | contains something needed |

A node is written out iff `is_summoned && !is_pruned`.

Using both directions is what makes the output minimal and well-formed at
the same time. Forward on `targets` answers "what else do I need?";
backward on `parent` answers "what has to be written down before I can say
it?" A reachable nested message drags its enclosing message and its file
into the output without either being reachable itself.

Precedence between the marks is explicit rather than emergent: pruning a
file also clears its `is_seed`, so prune beats seed.

### Why each closure runs the direction it does

**Pruning** (phase 4) runs forward on containment. Excluding a message must
exclude everything defined inside it, because those definitions have no
existence apart from their enclosure — there is nowhere to put them once the
enclosure is gone. Prunings are given as patterns, so the phase first
resolves each pattern to a node set and then takes the closure; a pattern
that resolves to nothing is where the edit-distance suggestion below fires.

**Reachability** (phase 5) runs forward on references, from the seeds. This
is ordinary transitive need: a seed message needs its field types, which
need theirs. It is the phase that makes the output *sufficient*, and the
phase that makes it *small* — anything the seeds cannot reach is never
considered again. Each node also records the first node that reached it,
which turns "why is this file in my output?" from guesswork into a lookup.

**Summoning** (phase 6) runs backward on containment, from every reachable
node. This is the phase that makes the output *syntactically writable*. A
reachable nested type is not something you can print on its own: you must
print `message Outer { ... }` around it, and put that in a file, and emit
that file. So summoning walks parent links upward and marks the enclosures,
none of which are reachable in their own right. A file is emitted only if
something inside it was summoned, which is what keeps untouched files out.

The two phases are not interchangeable and neither subsumes the other.
Reachability without summoning yields a set of types with nowhere to live;
summoning without reachability yields whole files. Running them in that
order over the two relations yields exactly the enclosures of exactly the
needed types.

**Pruning interacts with both, and the interaction is where the awkward
cases live.** A pruned type can still be referenced by a type that survives,
and the pool will reject a file that references something absent. That is
what the strip-and-record mechanism below exists for: the dangling reference
is removed so the file is well-formed, and the removal is kept as data so it
can be rendered as a comment instead of vanishing.

### Reconciling what the semantics needs with what an import can carry
Spec 0046; `_phase6_summoning` sub-pass 2 and `_shortest_lex_path` in
`phases.py`.

Reachability works on *type* references — a field in file A names a message
in file C — but `.proto` has no way to say "give me that type". It only has
`import "<path>";`, a file-level mechanism, and the set of paths A may
import is not reproto's to choose: it is `A.dependency[]`, fixed by the
input descriptor. Reproto re-derives the import lines from the summoning
state, but it cannot invent one that was not there.

So the two obligations are stated over different objects and can disagree.
Suppose A imports B, B imports C, and a field of A's has a type defined in
C. Reachability marks the type in C, summoning marks C — and A's rendered
output still has no route to it, because A's only import is B, and if B is
not summoned that import line is dropped. The output is minimal and does
not compile.

The invariant that has to hold is: for every summoned file A and every
foreign type it references in a summoned file C, some file B is summoned
such that A directly imports B and B leads to C (possibly `B == C`).

The fix that satisfies it is a shortest-path closure on the import graph:

1. Collect A's **type-level** targets by walking A's whole containment
   subtree, not just A's own edges — a field's type reference is recorded on
   the field node, so a file's semantic needs are the union over everything
   inside it (`_all_type_targets`).
2. For each such target, find its host file C; skip it unless C is already
   summoned.
3. BFS from A to C over the import graph and summon every *intermediate*
   file on the path. Fewest hops means fewest files added.
4. Repeat to a fixpoint: a bridge file summoned in step 3 has type-level
   targets of its own, which may need bridges of their own.

Two details are worth stating. **Ties are broken lexicographically**, and
the BFS gets that for free rather than by enumerating paths and sorting
them: expand each node's imports in sorted-by-name order, and the first
arrival at a node is already along the lexicographically smallest shortest
path, so recording one predecessor per node suffices. Determinism here is
what makes the output diffable across runs. And **pruned and
reference-only files are excluded from the graph entirely**, not filtered
out of the result, so a path is never routed through a file that will not
be written.

What this replaced is the measure of the gain: the previous rule summoned
any file that imports a summoned file. That never misses a bridge, but
seeding on `google.protobuf.Duration` produced 75 files of which one was
needed — the other 74 were corpus files that happened to import
`duration.proto`. Reverse-reachability is sound and useless; the shortest
path is what "minimal" actually means here.

The syntactic side is then rendered from the same marks. An import line
whose target is summoned is emitted as code; one whose target is not is
emitted as a commented-out vestige, alongside the imports that were
stripped before pool insertion because their target was pruned or was never
supplied (`re_file.py:356-397`). The file records what it used to depend on
without depending on it.

### A level-synchronous frontier, used twice
Both closures are breadth-first with an explicit frontier
(`seeding_files` / `fresh_files` / `reachable_files`), where membership in
the visited set is the deduplication test.

Phase 2's topological sort is the same shape on the dependency graph:
repeatedly collect the files all of whose targets have already left the
working set — the debug output calls each round a *rank* — insert them,
remove them, repeat. Kahn's algorithm in level form. Cycle detection comes
free and needs no second pass: if files remain but no leaf does, what
remains is exactly a cycle.

### Circular imports handled by deferring resolution, not by breaking cycles
A genuine import cycle has no topological order at all. Insertion works
anyway because the descriptor pool accepts a file whose references are not
yet resolvable and resolves them lazily on lookup. The ordering is a
well-formedness and performance aid, not a correctness requirement — which
is why the cycle case degrades to a report rather than a failure.

### Nodes interned by name, with declare-then-define
`topology.py:70`.

`ReFile.__new__` returns the existing instance when the name is already
known, so each name maps to exactly one object. A node therefore has three
lifecycle states, and the third is not an error path:

- **reference** — it exists only because something pointed at it
- **defined** — the file containing it was loaded and filled it in
- **orphan** — it stayed a reference to the end, because the input
  descriptor set was incomplete

This is how a cyclic reference graph gets built in one pass: a forward
reference materializes as a stub, and the definition patches the same
object later. An orphan is rendered as a comment, so an incomplete input
produces an annotated output rather than a crash.

### Removal reified as data
Spec 0087.

A field whose `type_name` cannot be resolved has to be stripped before the
descriptor is added to the pool, or the pool rejects the file. The strip is
not silent: the removed record is accumulated on the file, keyed by owner
FQDN, and phase 3 dispatches it back to the matching node for rendering.

The predicate is worth quoting because the obvious version is wrong. A type
name is unresolvable iff the pool cannot find it **and** it is not defined
within the file currently being added — the second half is what stops a
file's own forward references from being stripped.

### The rendering order is a canonical section of a many-to-one map
`docs/reproto/INSIGHTS.md`.

protoc normalizes element order: all fields, then all nested types, then all
enums, whatever order the source used. That was established by compiling two
sources differing only in element order and observing identical descriptors —
a proof by construction rather than an assumption.

So `source → descriptor` is many-to-one and no `descriptor → source` can
invert it. Reproto picks one canonical representative per fiber: a fixed
rendering order. The two round trips are then different statements, and the
distinction is the whole design:

- `descriptor → source → descriptor` is the **identity** — the property
  that can be tested, and is.
- `source → descriptor → source` is a **normal form**, not the identity.
  Original element order is not recovered, and recovering it is not
  attempted.

A useful corollary falls out: `source_code_info` is not needed for
correctness, which matters because it only exists if the input was compiled
with `--include_source_info`. Comments are purely additive when present.

### Recovering an override by recomputing the default
Several distinctions the source made are simply not recorded as presence
bits. The recurring technique is to recompute what the default would have
been and compare:

- **`json_name`** — `HasField("json_name")` is *always* true, so presence
  carries no information. A custom override is detected as
  `json_name != camel_case(name)`. A source-level
  `[json_name = "sameAsAuto"]` that equals the derived name is genuinely
  indistinguishable from no annotation, and is correctly dropped.
- **`packed`** — ambiguous in the other direction:
  `HasField("packed") == false` means *unpacked* in proto2 and *packed* in
  proto3. So `FileDescriptorProto.syntax` is not source-reconstruction
  metadata — it is semantically required to decode wire bytes at all.
- **`default_value`** — `HasField` is the only discriminator, because an
  unset default and an explicit empty-string default both read back as `''`.

### Synthetic constructs detected by conjunction, with a named authority
A proto3 `optional` field compiles into a synthetic oneof. Detection needs
three conditions together: the oneof's name starts with `_`, it holds
exactly one field, and that field has `proto3_optional`. Which of the three
is *authoritative* is written down — `proto3_optional`, not the underscore —
so a real user oneof that happens to be named `_foo` is never suppressed.

### Edition features: a step function, then an override chain
`feature_resolution.py`.

Resolving one feature for one element is two lookups:

1. **The edition default** — a predecessor query. Each feature carries a
   list of `(edition, value)` pairs sorted ascending, and the default is the
   value at the greatest edition ≤ the file's edition: a step function over
   the edition axis, evaluated by scanning the sorted list in reverse and
   stopping at the first entry that applies.
2. **The override chain** — the sparse `FeatureSet` messages from coarsest
   to finest (file → message → field / enum / oneof), last one that has the
   field set wins. Sparsity is the point: protoc stores a feature only where
   it differs from the resolved default, so an absent entry means *inherit*
   and never *zero*.

Two properties of the table itself:

- It is **derived from the input's own `descriptor.proto`**, by reading
  `FieldOptions.edition_defaults` off each field of the `FeatureSet`
  message, rather than hardcoded. Reproto learns a new edition by being
  handed a newer descriptor. A variant with no `FeatureSet` message yields
  an empty table, which is how "this variant predates editions" is
  represented instead of being special-cased.
- `RETENTION_SOURCE` features are excluded by construction, because they
  never reach a runtime descriptor. The same reasoning is why extension
  range options need no rendering at all.

`descriptor.proto` is required to have no dependencies of its own — it is
the root of the meta-hierarchy, and reproto raises rather than proceed if
it does.

### Down-conversion targets wire equivalence, not semantic equivalence
Spec 0072; `docs/reproto/force-proto2-output.md`; `syntax.py`,
`re_descriptor.py`, `re_field.py`.

The problem is a tooling gap, not a protobuf one. Prototools is split
across two runtimes: reproto is Python on the upstream protobuf library,
which understands editions; everything else is Rust on prost and
prost-reflect, which as of the observed versions (prost 0.13,
prost-reflect 0.16.3) do not — an `edition = "2023"` file fails to parse
(`docs/prototext/PROST-ISSUES.md` §2). An editions descriptor is therefore
readable by one half of the workspace and opaque to the other.

Rewriting a protobuf front end is not the proportionate response.
Translating the schema is: `--force-proto2-output` emits proto2 source for
an editions input, which recompiles to a proto2 descriptor that the Rust
side can read. The question then becomes *which* equivalence the
translation has to preserve, and the answer chosen is the weakest one that
still makes the two descriptors interchangeable in practice: **wire
equivalence** — a message encoded against the original decodes against the
output and back, with the same field numbers, the same wire types, and the
same type assignments. Not semantic equivalence, not source fidelity.

Picking that equivalence is what makes the translation total rather than
partial, and it decides every individual mapping:

- **`field_presence`**: `EXPLICIT` and `IMPLICIT` both map to `optional`,
  `LEGACY_REQUIRED` to `required`. Collapsing two distinct source concepts
  onto one keyword is sound precisely because the difference between them
  is not on the wire: an unset field is absent either way, and only the
  presence of a `has_field()` accessor differs.
- **`repeated_field_encoding`**: `PACKED` must be spelled `[packed = true]`
  and `EXPANDED` must be spelled with nothing at all. Same intent, opposite
  spellings, because the two dialects have opposite defaults — editions
  2023 packs by default and proto2 does not. What is preserved is the
  effective value, not the annotation.
- **`message_encoding = DELIMITED`** is the case where preserving the bytes
  requires *changing* the construct. `DELIMITED` is what proto2 groups
  became; it encodes with SGROUP/EGROUP tags (wire types 3 and 4), not the
  length-prefixed wire type 2 a plain message field would get. Rendering it
  as `optional Inner field = 5` would be the faithful-looking translation
  and the wire-wrong one, so it is rendered as a proto2 `group` block.
- Features that provably do not reach the bytes — `utf8_validation`,
  `json_format`, `enum_type` — are dropped, and dropping them needs no
  case-by-case argument once the target is stated. The criterion *is* the
  target.

The `DELIMITED` case is also worth reading for how it is implemented. A
native proto2 group's field points at an implicit nested type of its own,
distinct from any standalone message; an editions `DELIMITED` field points
straight at the real message type. Rather than add a second rendering path,
reproto manufactures the shape the existing path already expects: it
registers a fresh descriptor node under a synthesized FQDN
(`{enclosing}.{CamelCasedFieldName}`, with a numeric suffix on collision
with an existing nested name), re-initializes it from the *same* underlying
`DescriptorProto`, marks it as a group, and repoints the field. The result
is two independent wrapper trees over one read-only descriptor — safe
because every mutable mark (`is_reachable`, `is_summoned`, `targets`, …)
lives on the wrapper and never on the shared protobuf object. After that
the group renderer needs no changes at all.

Where no wire-compatible translation exists, the field is **omitted** and
replaced by a warning comment, and the process still exits 0. The reasoning
is asymmetric on purpose: a missing field is passed through untouched by
every runtime as an unknown field, whereas a field emitted with the wrong
wire type corrupts data silently.

### What cannot be represented is written into the output
`anomalies.py`.

Every case reproto cannot render faithfully has a registry entry carrying
two independent format strings — one for stderr, one for an `// OMITTED` or
`// WARNING` comment in the emitted `.proto`. A helper drops the keys a
given template does not reference, so one context bag feeds both without
coupling them. The output documents its own lossiness rather than being
quietly wrong.

### Linear-time matching, and edit distance for the miss case
`phases.py` imports `re2 as re`: RE2 matches in linear time with no
catastrophic backtracking, on patterns that arrive from user `--seed` and
`--prune` arguments. When a pattern resolves to no node,
`rapidfuzz.fuzz.ratio` scores it against every known FQDN with a floor of
85, and the suggestion is re-normalized (the leading dot stripped) so it can
be pasted straight back into `--seed` or `--prune`.

---

## 13. Designed and argued, not implemented

Listed separately so the catalogue is not read as a feature list.

- **Branch-and-bound pruning of the candidate set**
  (`docs/schema-match.md`, "Optional pruning"). Drop a schema when
  `matches − unknowns + remaining_fields < current_best`. Proposed, not
  built.
- **Inverted tag → candidate index** (same document, Open Questions), to
  prune the depth-0 active set before the walk begins — aimed exactly at
  the region deduplication cannot help.
- **Wire-compatibility bisimulation for `--force-proto2-output`**
  (spec 0073, status *idea*). Reduce each schema to a *skeleton* — the set
  of `(field number, wire type, child skeleton)` triples, recursively, with
  the visited set as the cycle sentinel — and declare two descriptors
  wire-compatible iff their skeletons are equal. It is the equivalence of
  §1 applied to a different question: two schemas are the same if no
  sequence of bytes can tell them apart. The spec also records why it is
  harder than it looks — the `DELIMITED` → group translation changes the
  wire type deliberately, so the relation cannot be a plain equality across
  the two inputs.
- **Gzip pre-pass for compressed descriptors** (`docs/protoscan/scan.md`
  §5). Go's APIv1 registered descriptors gzipped, so a byte-level scan
  cannot see them. The design consequence is the interesting part: offsets
  would be in the decompressed coordinate space, so the API would have to
  return a container reference rather than a plain `(start, end)`.

---

## Where to read more

- `docs/protolens/techniques.md` — the accessible version, no spec
  vocabulary.
- `docs/protolens/performance.md` — the detailed narrative, with the
  measurement lessons.
- `docs/schema-match.md` — the matching and deduplication design in full.
- `docs/protoscan/scan.md` — scanning, prior art, and the veto-monotone
  boundary.
- `docs/prototext/lazy_fds_design.md` — the lazy descriptor pool.
- `docs/prototext/performance.md` — the codec's optimization history.
- `docs/specs/` — each spec carries its own Measured outcome section, and
  is the authority for any figure quoted here.
