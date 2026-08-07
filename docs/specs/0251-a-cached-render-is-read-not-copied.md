<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0251 — a cached render is read, not copied

Status: partly implemented (S5-S8, 2026-08-07; S1-S4 withdrawn)
App: protolens
Refs: docs/specs/0116-tree-sitter-textproto-highlight-captures.md §8
        (the render cache, and its still-open issue 7: the byte budget
        was never tuned against data),
      docs/specs/0207-where-the-override-memory-work-stands.md (records
        the render clone as the one item outstanding, and why the
        obvious `Arc` fix fails — this spec is the one it says it
        needs),
      docs/specs/0222-the-text-lives-in-the-nodes.md (`node_text`,
        which is why the confirmed splice needs owned strings),
      docs/specs/0187-highlighting-is-a-property-of-the-viewport.md
        (S2/S4: highlighting is per frame over the on-screen window,
        which is what makes `window_styles` sensitive to *when* the
        text is patched),
      docs/specs/0193-the-fold-marker-lives-in-a-gutter.md (the
        `" ... }"` summary — the precedent for a display-time text edit
        and its span insertion),
      docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (its amended header block specifies the override emphasis: the
        per-drawn-row resolution of "does this node carry an override"
        that S1 reuses),
      docs/specs/0174-preview-interior-truncation-and-node-budget-removal.md
        (the preview byte budget, and the `...` marker S2 moves),
      docs/specs/0250-the-machine-works-on-what-the-user-waits-for.md
        (S9 here is the same rule as its S8, for the other cache),
      docs/specs/0249-a-large-document-answers-the-user-first.md (split
        from the same draft; S5's take-on-hit serves its splice)

## Background

### The cache is copied on every use

`RenderCache::get` clones the whole `(Vec<String>, Vec<NodeSpan>)` on a
hit (`render_cache.rs:79-85`) and `render_node_as` clones again on
insert (`override_apply.rs:1197`).

The copy is not gratuitous — the caller both mutates and consumes what
it gets:

- it patches the synthetic field name into `patch_rows` lines
  (`override_apply.rs:1239-1252`);
- a truncated preview gets a `...` marker spliced into lines *and*
  spans (`:1261-1263`);
- the confirmed splice then **moves** each node's lines into
  `node_text[slot]`, which must own its `Box<str>`.

This is why spec 0207 records the plain `Arc` fix as blocked: a shared
value forces `make_mut` and copies anyway. The mutations have to move
before the sharing means anything.

### The budget fails in a worse way than being small

`RENDER_CACHE_MAX_BYTES` is 1 MiB (`tui/mod.rs:137`), and `insert`
keeps the just-inserted entry even when it alone busts the budget
(`render_cache.rs:96-101`): `while total_bytes > max_bytes &&
entries.len() > 1`. So a confirmed retype of a large node evicts
*everything* and sits alone. The cache degenerates to one entry
precisely when hits were wanted.

### A display-time text layer already exists

`display_row_text` returns a `Cow` borrowed out of `node_text`, and
`row_text_of` (`render.rs:602-617`) strips annotations and splices spec
0193's `" ... }"` per drawn row, per frame. The two mutations above are
the odd ones out — they are the only edits made at splice time rather
than at draw time.

### What it costs, measured

From the root-override breakdown (2026-08-07, googleapis.desc 25.6 MB,
`flock -x … taskset -c 4-7`; full table in spec 0249):

| phase | time |
|---|---|
| `render_node_as` total | 2.09 s |
| — `prototext-core` render | 1.06 s |
| — split into `String`s | 0.43 s |
| — **`RenderCache::insert` clone** | **0.56 s** |
| whole override | 4.12 s |

**The clone is 13% of the override and it is pure waste.**
`render_node_as` builds the value, clones it to hand back, and inserts
the original into a cache that is about to be evicted anyway — the
entry is 250 MB against a 1 MiB budget, so `insert` keeps exactly this
one and nothing else.

## Goals

- **G1.** No copy of a render is made that nothing reads. *(Stated as
  "a hit costs a refcount bump" in the draft — the `Arc` was a means,
  and S5 reached the goal without it. See the S1-S4 withdrawal.)*
- **G2.** The cache's budget is derived from what it holds, and it
  holds only what a second lookup can reach.
- **G3.** No cache is written by a workload that scans the whole
  document.

## Non-goals

- **N1. The rendered grammar is not extended.** Nothing here adds a
  token; the `...` truncation marker is display-only and must continue
  never to reach the parser.
- **N2. No cache is merged with another.** `CandidateCache` was deleted
  because a shared MRU generic was not worth it at two call sites; a
  third would have to appear first.

## Specification

### S1-S4 are superseded by S5 — WITHDRAWN 2026-08-07

S1 moved the field-name patch to draw time and S2 moved the truncation
marker to the overlay, so that S3 could make `RenderCache::get` return
an `Arc` and S4 could let the preview share it. All four existed to
turn one clone into a refcount bump.

**S5 removed the clone they were aimed at.** Once the confirmed render
leaves the cache, the only remaining caller is the preview, whose
render is bounded by `override_preview_byte_budget`. The worst shape
that bound admits was measured at the default 4096 bytes: 2051 lines,
2049 spans, **104 512 B** (`measure_a_preview_renders_size`). Cloning
that is ~10 us, once per candidate keystroke.

Against that, S1 costs a text edit on the per-frame path for every
drawn row; a hard ordering constraint, since it must land above
`window_text` or every style right of the patch shifts by
`len(name) - 1`; and a borrow problem that was not visible when it was
written — choosing between the synthetic and the raw patch needs the
node's field type, which comes from `wrapper_target_for`, and that
takes `&mut self` (`decode.rs:1248`) while `display_row_text` is
`&self` and `render` holds `&self.tree` across the draw.

They are withdrawn as *superseded*, not declined: the reasoning was
sound until S5 changed which path uses the cache. If a future change
puts a large render back in the cache, they become live again — and
the ordering constraint above is the part to re-read first.

### The confirmed render leaves the cache

- **S5. The confirmed splice does not use the cache. — IMPLEMENTED
  2026-08-07.** Sharing would not have helped it: `node_text[slot]`
  must own its `Box<str>`, so copying out of a shared `Arc` copies
  anyway.

  The draft said "takes on a hit and never inserts". Implementation
  showed the two halves are one: `is_preview` is part of the key, so if
  nothing inserts a confirmed render nothing can ever hit one either.
  The confirmed path therefore skips the lookup as well as the insert.

  What it gives up is one re-render on revert-and-re-apply, which is
  user-paced. What it removes is 0.56 s of a 4.12 s root override —
  a clone to hand the value back plus a clone to store it, for a 250 MB
  entry that `insert` was going to reject anyway (S7).

- **S6. `RenderedAs`'s preview-only product is `bytes`, not `spans`.
  — IMPLEMENTED 2026-08-07.**

  Spec 0207 recorded that the preview renders a `Vec<NodeSpan>` and
  discards it. **That is no longer true and this spec repeated it
  without checking.** `PreviewOverlay.spans` is read by
  `preview_wire_row` (`wire.rs:439`), which feeds it to `preview_slice`
  to draw the wire row under a preview — spec 0225 S9, added after 0207
  was written. `RenderedAs`'s own doc comment already says so. Both
  products are live on both paths; there is nothing to split there.

  The field that *is* one-sided is `bytes`. `render_node_as` opens with
  `self.blob[raw_range].to_vec()` (`:1118`) — a full copy of the node's
  own bytes, 25 MB for a root override — and `splice_override`
  discards it (`:878-882`, `..`). Worse, the copy is made *before* the
  cache lookup, so a cache hit pays for it too.

  It exists for the preview alone, whose spans index into a truncated
  buffer that exists nowhere else. So: borrow the blob through a cloned
  `Arc` (O(1)) and keep a `Cow`, materializing an owned `Vec` only when
  the preview budget actually cut it or when the overlay actually needs
  it. `RenderedAs.bytes` becomes `Option<Vec<u8>>`, `Some` exactly when
  `is_preview`.

### The budget

- **S7. An entry that alone busts the budget is not kept. —
  IMPLEMENTED 2026-08-07.** The
  `entries.len() > 1` floor in `insert` is what turns one large render
  into a full eviction. Reject the oversized entry instead: it is
  cheaper to re-render one thing than to lose everything else.

- **S8. `RENDER_CACHE_MAX_BYTES` is re-derived after S5, not before.
  — IMPLEMENTED 2026-08-07.** Once confirmed renders leave the cache,
  the budget only has to hold previews.

  Measured worst case at the default 4096-byte input budget: 2051
  lines, 2049 spans, **104 512 B**, i.e. ~25x the input. A screenful of
  the ranked candidate list is ~50 entries, so one pass through it
  costs ~5 MB. **1 MiB -> 8 MiB**, derivation in the constant's doc
  comment (spec 0116's open issue 7, now closed).

  The doc comment also records an interaction nobody had written down:
  raise `--override-preview-byte-budget` far enough and one preview
  exceeds the whole cache, after which S7 rejects every entry and the
  cache quietly stops working. At the default there are ~80 worst-case
  entries of headroom.

- **S9. No document-scanning workload writes to the cache.**
  **IMPLEMENTED 2026-08-07 — by S5, with a test to hold it there.** A
  full document scan visits every node exactly once; under MRU eviction
  it gets a hit rate of approximately zero *and* leaves the cache
  holding the tail of the document for the interactive work that
  follows. A scan that needs a render obtains it, tests it, and
  discards it. This is spec 0250 S8 stated for the other cache, and the
  reason is the same.

  S5 makes this structural rather than a rule to be remembered: the
  cache's only writer is now the `is_preview == true` arm, and the only
  caller that passes `true` is `preview_override_highlight`, which
  renders the single node under the cursor. Every document-scanning
  path reaches the renderer through `resettle_node`
  (`override_apply.rs:100`), which passes `false`. There is no second
  place to police.

- **S10. Housekeeping.** `docs/protolens/design/caches.md` describes
  two caches; `CandidateCache` is gone and `render_cache.rs`'s module
  doc already says it is "the only one in the crate". Correct it here.

## Open questions

1. ~~**What is a preview render's real size?**~~ **Answered
   2026-08-07: 104 512 B** for the worst shape the default 4096-byte
   interior budget admits — 2051 lines, 2049 spans, ~25x the input.
   See S8.

2. ~~**Does anything else mutate a cache hit?**~~ Moot with S1-S4
   withdrawn: a cache hit is still a private copy, so mutating it is
   allowed and costs nothing shared.

## Alternatives considered

**Make `RenderValue` an `Arc` and change nothing else.** Fails, and
spec 0207 records why: the caller mutates the value, so a shared `Arc`
forces `make_mut` and copies anyway. Moving the mutations out (S1, S2)
is what makes the `Arc` real rather than decorative.

**Patch the field name in `row_text_of`, next to the fold summary.**
Reads as the obvious home and is wrong: the highlighter runs on
`display_row_text`, so it would parse `_` while the screen shows the
real name, and every style right of the patch would land in the wrong
column.

**Raise `RENDER_CACHE_MAX_BYTES` to ~50 MiB now.** Treats the symptom.
The observed failure is not a slightly-small budget but an entry that
evicts the entire cache and sits alone (S7); and after S5 the cache
stops holding the large entries at all.

**Let a search sweep populate the render cache, sized to hold the
document.** A cache big enough to hold every node's text *is*
`node_text` with an eviction path that never fires. The saving exists
only when the cache is smaller than the document — exactly when a
single-pass scan gets nothing from it and evicts what the user was
about to reuse.

## Test plan

Tests 1-6 of the draft belonged to S1-S4 and go with them.

1. `a_confirmed_splice_leaves_no_entry_behind` — a confirmed render
   leaves the cache empty; a preview of the same node fills it. S5.
   **Implemented 2026-08-07.**
2. `an_oversized_entry_evicts_nothing` — inserting an entry larger than
   the whole budget leaves the existing entries in place. S7.
   **Implemented 2026-08-07.**
3. `a_confirmed_render_copies_no_bytes` — the confirmed path allocates
   no copy of the node's own bytes, and the preview path still owns the
   truncated buffer its spans index into. S6. **Implemented
   2026-08-07.**
4. `measure_a_preview_renders_size` — `#[ignore]`d; reports the number
   S8's derivation rests on, so the derivation can be re-run rather
   than trusted. **Implemented 2026-08-07.**
5. `a_sweep_writes_to_no_cache` — cache contents identical before and
   after a workload that visits the whole document. S9. **Implemented
   2026-08-07.** Two previews are primed first, so the assertion is
   "unchanged" and not the weaker "still empty"; the sweep asserts it
   actually spliced, so the test cannot pass by sweeping nothing; and
   removing S5's guard was checked to make it fail (2 -> 3 entries).

## Measured outcome

Partly in (2026-08-07); the rest waits on a run against googleapis.

Measured so far:

| what | before | after |
|---|---|---|
| worst-case preview render | — | 104 512 B (2051 lines, 2049 spans) |
| `RENDER_CACHE_MAX_BYTES` | 1 MiB, asserted | 8 MiB, derived |

Still owed: the root-override phase table re-run, showing the 0.56 s
clone gone and the confirmed path's `to_vec` of the node with it; and
peak and at-rest memory on googleapis across a `t`/`Enter`/`o`/`d`
cycle, against spec 0207's 1.66 GiB / 0.94 GiB. State plainly anything
that did not improve.
