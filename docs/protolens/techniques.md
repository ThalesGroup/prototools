<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# How protolens opens a 25 MB protobuf blob in two seconds

protolens is a terminal viewer for binary protobuf. You point it at a file
of raw wire bytes, with no schema and no idea what type it holds, and it
tells you what it is and shows it to you.

The reference workload is the Google APIs descriptor set used as its own
input — 25.6 MB of wire bytes, matched against the 49 255 message types
defined inside it, and displayed as a 5.28 million-line document:

```
$ time protolens --descriptor-set googleapis.desc googleapis.desc
protolens: inferring root type (24 MB) on 12 threads...
protolens: rendering root node as google.protobuf.FileDescriptorSet (24 MB)...
protolens: indexing 15595 lines...

real    0m2.285s
user    0m18.624s
```

Two seconds of wall clock, eighteen seconds of CPU. The first honest
attempt at this did not finish at all — it was still working when we gave
up on it and killed it.

Startup is only the headline. A viewer is a thing you *use*, and the same
document has to stay responsive while you reinterpret it, scroll it and
search it:

| | |
|---|---|
| reassign the type of the outermost message | 0.15 s |
| reassign the type of any inner field | 1–2 ms |
| draw a frame | 0.4–1.4 ms, anywhere in the document |
| page down | ~40 µs, at line 5 000 000 as at line 1 |
| search the whole document and find nothing | 0.2 s |

This document is the list of techniques that produce those numbers. It is
not a history. Everything in it rests on **one observation about protobuf**
and **three ideas about work**:

0. **The bytes have a structure that the schema does not change.**
1. **Do not do work nobody will look at.**
2. **Do not do the same work twice.**
3. **Do the rest somewhere nobody is waiting.**

Observation 0 comes first because the other three are built on it.

---

## The shape of the problem

| | |
|---|---|
| input | 25.6 MB of schemaless wire bytes |
| candidate types for the outermost message | 49 255 |
| the document, fully decoded | 5 281 124 lines / 238 MB of text |
| rows the terminal can show | 48 |

The last two lines are the entire argument. A naive viewer decodes the blob,
formats every line, and hands 238 MB of text to a screen that can display
about four kilobytes of it. It is doing roughly a hundred thousand times too
much work, and it is doing it before showing you anything.

---

## Observation 0 — the structure is not the interpretation

A protobuf message on the wire is a sequence of records, each a tag
followed by a payload whose length the tag either states or implies. To cut
a byte range into its child ranges you need to read tags — but **only to
find where each record ends.** You do not need to know what any of them
means.

That is the whole observation, and it is worth stating in its strongest
form: *a schema supplies names, types and presentation; it never moves a
boundary.* Whether field 3 is a nested `FileDescriptorProto`, an opaque
`bytes`, or a field the schema has never heard of, it occupies exactly the
same bytes and ends in exactly the same place.

So there is a **single tree implied by the bytes alone**, and every
interpretation of those bytes — every assignment of types to fields — is a
*pruning* of that one tree, never a different tree. protolens builds it
once, at load, by descending into every length-delimited payload that
plausibly parses, whether or not the current interpretation would show it.
That makes it the *maximal* tree: 4.74 M nodes, against the 4.5 M any one
interpretation actually displays.

Because the blob does not change, this tree does not change. It is built
once and never mutated for the rest of the session. That is a much stronger
property than it sounds, and the rest of this section is what it buys.

### A child's identity is its position

Not its field number. Field numbers repeat, are absent from the schema, and
do not exist at all for a malformed tail — but a position in an ordered list
always exists and is always unique. So a node's address is a path of
positions: "third child of the first child of the root."

### The layout can be the index

A frozen tree needs no index, because the *order you store it in* can be the
index.

The nodes are laid out **breadth-first**: all the roots, then everything at
depth 1, then everything at depth 2. Within a level, siblings are stored
contiguously, and sibling blocks appear in the order of their parents. That
single choice turns every navigation question into arithmetic:

| question | answer |
|---|---|
| where are node *i*'s children? | slots `first_child[i] .. first_child[i+1]` |
| how many children has it? | subtract those two |
| its *k*-th child? | `first_child[i] + k` |
| next sibling? | `i + 1` |
| which sibling am I? | `i - first_child[parent[i]]` |
| my parent? | `parent[i]` |
| am I a root? | `parent[i] == i` |

Several things are worth pointing out about that table.

There is no stored child count and no stored child list: the end of *i*'s
block is the start of the next node's, so one array of `n+1` entries encodes
every block boundary in the tree. There is no end-of-list sentinel either — a
root is defined to be its own parent, which terminates an upward climb
without a special value to test for. And resolving a whole path costs one
addition and one bounds check per level, with no hashing and no allocation
at any point.

Because it is arithmetic on flat arrays, it is also cache-friendly in a way
that a tree of heap nodes is not. Scanning a node's children is a linear
walk across adjacent memory rather than a pointer chase through five million
scattered allocations.

The whole structure is in fact **one** allocation. Four parallel arrays —
first-child, parent, and the two ends of each node's byte range — plus a
bitset, all carved out of a single flat vector of 32-bit integers, because
they are built together, live together and die together. One `malloc`, one
first touch, instead of four.

### Building it breadth-first requires reading it depth-first

You cannot walk wire bytes level by level, because you cannot find level
*d+1* without having parsed level *d*. So the tree is built in two passes:
a recursive descent that emits nodes in document order, then a sort into
level order.

That sort is the one part that could plausibly have been expensive, and
isn't. It is a **counting sort keyed on depth** — depth is a small integer
with a hard cap, so nodes are bucketed rather than compared, and the pass is
linear. Depth alone is a sufficient key: within a level, document order
*already is* the order that parent order induces, since an earlier parent's
entire subtree precedes a later parent's, and nothing can fall between two
children of the same parent without being a descendant of one of them.
The desired stability isn't engineered; it falls out of visiting in document
order with bucket cursors that only ever advance.

### Nothing about it is dynamic

Once those two passes are done, the structure never allocates again. Not on
a retype, not on a scroll, not on a search, not on an export. There is no
growth to bound and therefore:

- no free list, no compaction pass, no slot reuse scheme
- no generation counters or tombstones, because nothing is ever freed
- no reference counting, no interior mutability, no `Option<Box<Node>>`

And — the practical payoff — **a slot index is just a `u32` that stays valid
for the entire session.** It can be stored in the cursor, in the fold state,
in the undo history, in every cache, and handed to a background thread,
without any of them needing to be told when the document is reinterpreted.
Invalidation is usually where this kind of design gets expensive; here the
question does not arise.

One soundness condition makes the arithmetic legitimate, and it is checked
against a real corpus rather than assumed: a displayed node must consume
either the whole of its slot's child block or none of it, never a subset.
If that failed, a positional path would count *displayed* children while the
arithmetic counted *stored* children, and the two would drift apart
silently.

### Reinterpreting is relabeling

Assigning a different type to a field
does not build a tree, allocate a node or move a boundary. It changes which
existing nodes are labeled with which types, and which of them are
displayed. This is why retyping a nested field costs a millisecond or two,
and why nothing accumulates: an earlier interpretation leaves no superseded
copy of a subtree behind, because there was never a second copy. A viewer
that rebuilds on every reinterpretation grows without bound across a
session; this one does not grow at all.

It also means every override is *guaranteed* to be expressible. If the blob
loaded, the node you want to retype already exists. No reinterpretation can
fail for want of somewhere to put it.

And it is what makes caching sound. Anything derived purely from the
structure — where a node begins and ends, what its children are, how deep
it is — is computed once and stays valid forever, no matter how many times
you change your mind about the types. Only the *labels* are invalidated by
a retype, and only under the subtree you retyped.

Everything below is downstream of this.

---

## Idea 1 — Do not do work nobody will look at

### Stop rendering when the screen is full

The formatter takes a row budget and stops when it has produced enough rows
to fill the window. It reports where it stopped, and each stopping point
becomes a collapsed section — which the interface already knows how to draw,
because collapsing sections is something users do by hand.

Formatting the whole document takes about two seconds and produces 239 MB.
Formatting one screenful takes **five milliseconds** and produces 360 KB.

There is a subtlety worth stating plainly, because it is the technique's
real cost model and its real limit: **a bounded render costs the width of
the boundary, not the size of the budget.** Stopping part-way through a
deeply nested structure still means emitting every enclosing level and every
sibling already begun, so a 51-row budget yields about 15 600 rows, not 51.
It follows that this technique cannot help a document that is wide and flat
rather than deep — there is nothing to collapse.

### Finish the rest when the user stops typing

A budget postpones work; it does not remove it. The remainder is completed
in the event loop's idle branch — the code path taken when there is nothing
in the input queue — in small increments, without drawing a frame per
increment. The user gets a complete document without ever having waited for
one.

The unit size matters more than it looks. Increments are sized so that no
single one exceeds about 25 milliseconds, and virtually all are far under
that; the point is not throughput but that a keystroke arriving mid-increment
is never visibly delayed.

Four background jobs share this branch, and their **order is a correctness
constraint, not a priority ranking.** Completing the document changes its
structure, and speculative read-ahead has to restart whenever the structure
changes — so read-ahead is scheduled last, because it is the only position
in which it can make progress at all.

### Search that gives the terminal back

Searching a 238 MB document for a string that is not there is a long scan.
It runs in the same idle branch, a thousand candidates at a time, redrawing
only when the *answer* changes rather than when the scan advances. The
window stays live throughout: you can keep scrolling while it runs.

### Discard requests that are no longer true

While you scroll, the viewer queues background questions about the rows on
screen — chiefly "how well does each of these bytes match the type we
assigned it?". Scroll quickly and most of those rows are gone before their
answers arrive. Each request is therefore stamped with the scroll position
it was made under, and one that no longer describes the visible window is
thrown away when it reaches the front of the queue instead of being answered.

The gain here is not the CPU saved on obsolete questions. It is that a
pending question about visible rows blocks speculative work, so a queue full
of stale requests does not merely waste effort — it **stops** the read-ahead
that would have made the next screen instant.

---

## Idea 2 — Do not do the same work twice

### Types that consume bytes identically become one type

Identifying the outermost message means testing 49 255 hypotheses against
the same 25.6 MB. But most message types are not distinguishable by their
effect on a byte stream: two types with the same field numbers and the same
wire types accept and reject exactly the same input.

So the schema is compiled, once, into a state machine, and that state
machine is minimized with the classic Hopcroft algorithm — the same
procedure used to minimize a DFA. Types that behave identically collapse
into one state:

| | |
|---|---|
| candidate types | 49 255 |
| distinct behaviors that must actually be tested | **17 572** |

Every subsequent decision is downstream of this one. In particular, a
behavior class is **indivisible**: two workers given half a class each would
both walk the same bytes rather than half the bytes each, so the classes are
a hard floor on how finely the work can be divided. Deduplication and
parallelism pull against one another here, and deduplication wins.

### Store nothing the bytes already say

The structural tree of Observation 0 is a table of five million entries, so
every byte per entry is five megabytes. Since it is immutable, it can afford
to be laid out for reading rather than for editing — and since it is a
function of the bytes, anything the bytes already say need not be in it at
all:

- A node records how many lines its subtree occupies — a *size*, not a
  position. Absolute line numbers are computed by addition when needed. This
  is what makes editing the document cost time proportional to nesting depth
  rather than to the number of lines after the edit.
- Closing brackets are not stored. They are re-derived from the indentation
  of the line that opened them.
- A repeated packed field occupies one entry, not one per element. Element
  boundaries are recovered by re-reading the bytes, which is cheaper than
  having stored them.
- Nodes hold no pointers to each other. Parent and child links live in one
  flat integer array beside the table.

The text itself lives with the node that produced it rather than in one
enormous list of lines. That is the difference between replacing a section
near the top of the document costing a hundred milliseconds and costing one.

Each structure's size is frozen by a compile-time assertion — an *equality*,
not an upper bound, because silent growth is exactly the regression these
assertions exist to catch. The result is 875 MB resident for a document
whose text alone is 238 MB.

### Never copy a byte twice, even when you don't know the length yet

Going the other way — turning the edited text back into wire bytes — runs
into protobuf's one awkward property. A nested message is written as *tag,
length, body*, but you cannot know the length until you have written the
body. There are two obvious ways to deal with this and both are bad:

- **Encode each message into its own buffer**, then write its length and
  append it to the parent's buffer. That is one allocation per message, and
  a byte nested *d* levels deep gets copied *d* times on its way out. On a
  descriptor set, which nests deeply, the copying dominates.
- **Walk everything twice**, once to measure and once to write. Now the
  whole encoder runs twice, and the two passes must agree exactly.

protolens does neither. It writes into **one** buffer, strictly left to
right, and never goes back:

1. On `{`, reserve a small fixed-size gap — enough for the largest length
   varint that could be needed — and start writing the body immediately
   after it.
2. On `}`, the length is finally known. Write the varint **flush-right**
   inside the gap, so that it ends exactly where the body begins, and record
   how many bytes at the front of the gap went unused.
3. When the whole document is encoded, make one left-to-right pass that
   slides the real data leftwards over the unused gap fronts and truncates.

Each byte is written once and moved at most once. The whole encode is one
allocation and two linear passes over it.

The part worth appreciating is step 3's bookkeeping, because it has none. To
close the gaps you need to know where they all are — which sounds like a
list of offsets, i.e. exactly the side allocation the design was avoiding.
Instead, **each gap stores its own waste count and the offset of the next
gap**, threading a linked list through precisely the bytes that are about to
be deleted. The bookkeeping therefore occupies no space of its own; it sits
in bytes that are going to be discarded either way. And because gaps are
opened in document order, the list is already
in the order the final pass consumes it: no sorting, no recursion, no stack.

One subtlety is easy to get wrong and worth stating: the length written into
a gap must be the length the body will have *after* compaction, not the
length it has at the time of writing. So each frame accumulates the waste of
every gap nested inside it and subtracts that before encoding its own
length. Get this wrong and every enclosing message declares a length that
counts bytes which are about to vanish.

The gap is also sized a little larger than strictly necessary, on purpose.
Protobuf permits non-minimal varints — a length may carry padding bytes — and
a viewer that promises to re-export the original byte-for-byte has to be able
to reproduce that padding rather than silently canonicalize it.

---

## Idea 3 — Do the rest where nobody is waiting

### Divide work far more finely than the core count

The obvious way to test 17 572 behaviors on eight cores is eight groups of
about 2200. This is a poor plan, for two reasons that are easy to miss.

The first is that the cost of a group is not proportional to its size. A
behavior costs whatever it takes to disqualify it, which for most candidates
is a few bytes and for a few candidates is the entire file — and which is
not knowable in advance. Eight equal-sized groups had running times spread
over a factor of seven, so seven cores spent most of their time waiting for
one.

The second is more surprising: **dividing the work makes it cheaper even on
a single core.** Testing many hypotheses at once requires bookkeeping at
every step that is superlinear in how many are still alive; smaller groups
mean cheaper steps. Splitting the job sixteen ways reduced the *total* work
by about 20% with no parallelism involved at all.

So the work is cut into far more chunks than there are cores — 24 — and
workers take the next available chunk from a shared counter as they finish.
Small chunks make the imbalance self-correcting: a worker that draws a cheap
chunk simply comes back sooner.

There is a limit, and it is the one from Idea 2. Chunks cannot be made
arbitrarily small, because a behavior class cannot be split, and the largest
class contains 4 645 candidates at every chunk count. Past a certain
fineness the chunks stop getting smaller and the bookkeeping starts getting
more expensive. Finer division is not a substitute for being able to
interrupt work already in progress.

### Make the unit of work a piece of a question, not a question

The same principle applies to the interactive background pool. When a
screenful of rows each need a question answered, assigning one question per
worker means one worker is busy and the rest are idle behind it. Instead the
questions are cut into the same chunks and the workers pull
(question, chunk) pairs from one pool. A screenful of answers arrives about
four times sooner.

Interrupting a chunk needs one piece of care: an abort must be identified,
not just signalled. A chunk that is handed out, cancelled, and later
encounters a cleared cancellation flag would look finished — and a
half-computed answer here is not an approximate answer, it is a wrong one.

### Not all cores are the same core

A modern laptop CPU reports a single core count and delivers at least three
different machines behind it. On the reference host:

| | | measured cost of the same work |
|---|---|---|
| performance cores | 4.9 GHz | 1.00 |
| efficiency cores | 3.8 GHz | 1.52 |
| low-power cores, no shared cache | 2.1 GHz | 2.85 |
| two threads sharing one performance core | | 1.92 each |

Two conclusions follow.

First, hyper-threading is nearly worthless for this workload: two threads on
one core deliver 1.04 cores' worth of throughput while each runs at half
speed. Second — and much more important — **the thread that draws the screen
cares about this far more than the workers do.** The same background job
costs 1.3x more on an efficiency core; the same *frame* costs 4.5x more.
Redrawing is latency, and latency is what the user feels.

So, when the operating system explicitly reports which cores are fast,
protolens reserves one whole physical core of them for drawing and gives the
background workers everything else. Note the three qualifications, all of
which are the point:

- **When the OS reports it.** Not when protolens measures it. Benchmarking
  cores at startup burns the very CPU that was just freed up, and inside a
  virtual machine the mapping between virtual and physical cores need not
  even be stable.
- **Explicitly.** Clock speed is not a proxy: different core designs are not
  comparable by frequency even when the comparison happens to come out right.
- **One whole physical core**, so that nothing is scheduled onto the drawing
  core's hyper-thread sibling. Background load on that sibling costs the
  drawing thread 1.8x on its worst frames.

On a machine that reports nothing, protolens does nothing. There is no
fallback and no guess.

### An idle program should cost nothing

A terminal application that polls is a laptop that does not sleep. protolens
performs **zero** timed wakeups when idle: the input reader blocks
indefinitely on the terminal, a window-resize pipe, and a shutdown pipe,
and the main loop blocks on its queue. Ten seconds of sitting still produces
no context switches at all.

Three details are load-bearing rather than optional, and all three are ways
of getting an untimed wait wrong:

- You need your own resize-signal pipe. Terminal libraries typically
  synthesize resize events from an internal one, so a reader watching only
  the terminal sleeps through every resize.
- You must drain already-parsed input before blocking. Input is read in
  kilobyte chunks, so the file descriptor can be unreadable while parsed
  events still sit in a queue.
- You must back off if that drain finds nothing. Otherwise the descriptor
  stays readable *because* nobody collected it, and an untimed wait spins
  forever.

---

## Things that were measured and deliberately not done

- **Replacing the syntax highlighter.** It is the single largest term in a
  frame, and a hand-written scanner would be 50–100x faster on that term.
  It is still not worth doing, because the frame is not limited by
  highlighting; the remainder is screen diffing and the terminal write.
- **Balancing chunks by candidate count.** The imbalance is real and
  measurable, and correcting it is worth 1–3%. Cost is unpredictable from
  size, so the fix does not fix anything.
- **Adjusting core reservations per frame.** Twenty-two system calls per
  frame to arrange something that arrives after the frame it was meant to
  protect.

---

## What it cost to be sure

Every change that could alter what the viewer displays was gated on the same
check: export the entire document to text and compare it byte-for-byte
against the previous build. 249 734 534 bytes, identical, every time.

Three measurement habits mattered as much as the optimizations themselves:

- **Pin every measurement to fixed cores.** Unpinned, the same binary
  produced timings spread over 1.7x — enough to swamp the effect being
  measured, and in one ordering enough to make the faster build look twice
  as slow.
- **Compare tails, not medians, whenever the program is allowed to skip
  work.** Starved of CPU, protolens drops redundant repaints, so the median
  ends up comparing two different populations of frame rather than the same
  frame slowed down. One such comparison read as 7x. The true figure was
  1.8x.
- **Do not name a constant from one input.** A rule fitted cleanly to this
  25.6 MB corpus was completely wrong on a corpus 23x smaller. What held
  across both was the number of chunks, not the size of them.

---

## Summary

| technique | what it exploits |
|---|---|
| **build the structural tree once** | **the schema never moves a boundary** |
| **lay it out breadth-first** | **a frozen tree can be its own index** |
| **navigate by arithmetic, store no pointers** | **contiguous siblings make every link an add** |
| **never allocate after load** | **an index that cannot dangle needs no invalidation** |
| **retype by relabeling, never rebuilding** | **every interpretation prunes the same tree** |
| stop formatting at the screen edge | the user cannot read 5 million lines |
| finish the document while idle | the user is not typing most of the time |
| resumable search | an answer that changes matters, a scan that advances does not |
| drop stale requests | scrolling invalidates its own questions |
| minimize the type automaton | most types are indistinguishable on the wire |
| store sizes, not positions | edits become local |
| store nothing the bytes say | five million entries make every byte expensive |
| encode in place, compact once | a byte copied per nesting level is a byte copied too often |
| thread the bookkeeping through the waste | the bytes you are about to delete are free storage |
| many small chunks, shared counter | cost per chunk is unpredictable |
| chunk the question, not the queue | one screenful is many questions |
| reserve a fast core for drawing | cores differ by 4.5x on latency |
| untimed waits everywhere | idle should mean idle |

The first five are really one technique, and it is the one that matters
most, because it is the only one that pays out on every subsequent
keystroke rather than once at startup.
