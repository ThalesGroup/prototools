<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0251 — a cached render is read, not copied

Status: draft
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

- **G1.** A cached render is consumed read-only, so a hit costs a
  refcount bump rather than a copy of every line.
- **G2.** Every last-minute edit to a row's text happens at draw time,
  in one place, alongside the fold summary that already works that way.
- **G3.** No cache is written by a workload that scans the whole
  document.

## Non-goals

- **N1. The rendered grammar is not extended.** Nothing here adds a
  token; the `...` marker S2 moves is display-only and must continue
  never to reach the parser.
- **N2. No cache is merged with another.** `CandidateCache` was deleted
  because a shared MRU generic was not worth it at two call sites; a
  third would have to appear first.

## Specification

### Move the mutations to draw time

- **S1. The field-name patch happens in `display_row_text`, not at
  splice time.** A row already knows its owner (`display_row_source`),
  and the override-emphasis machinery (spec 0192) already resolves per
  drawn row whether that node carries an override and which. A patched
  row returns `Cow::Owned`; every other row keeps its borrow.

  **It must go above `window_text`, not in `row_text_of`.**
  `window_styles` is keyed on `display_row_text` offsets, so patching
  after the highlighter has parsed the placeholder `_` shifts every
  column right of the patch by `len(name) - 1`. Spec 0193's summary
  survives being applied later only because `spans_with_insertions`
  threads it through as an explicit insertion (`render.rs:743`, "the
  two **must** agree"); a rename *replaces* a token the grammar cares
  about, and belongs where the parser can see it.

  The packed-run case (`patch_rows`) is preserved: a run draws one row
  per element, each carrying its own placeholder.

- **S2. A truncated preview's `...` marker is reported by the overlay,
  not spliced into the render.** `PreviewOverlay` already knows it is
  truncated; it reports one extra row rather than mutating cached lines
  and spans. `window_text`'s blanking of the marker (`render.rs:527`)
  is unchanged — the marker is not prototext and must not reach the
  parser.

### Then the sharing is real

- **S3.** With S1 and S2 nothing mutates a cached render, so
  `RenderCache::get` returns `Arc<RenderValue>` and clones no strings.

- **S4. The preview path shares it.** `PreviewOverlay`'s lines become
  an `Arc` held jointly with the cache — the workload the cache was
  built for (spec 0116 §8).

- **S5. The confirmed splice takes on a hit and never inserts.**
  Sharing does not help it: `node_text[slot]` must own its `Box<str>`,
  so copying out of a shared `Arc` copies anyway. Taking gives zero
  copies. The cost is one re-render on revert-and-re-apply, which is
  user-paced.

- **S6. `RenderedAs` splits its two products.** The preview renders a
  `Vec<NodeSpan>` and discards it — `preview_override_highlight` takes
  only the lines (`override_select.rs:825`).

### The budget

- **S7. An entry that alone busts the budget is not kept.** The
  `entries.len() > 1` floor in `insert` is what turns one large render
  into a full eviction. Reject the oversized entry instead: it is
  cheaper to re-render one thing than to lose everything else.

- **S8. `RENDER_CACHE_MAX_BYTES` is re-derived after S5, not before.**
  Once confirmed renders leave the cache on use, the budget only has to
  hold previews. Derive it from a measured preview size times the
  candidates a user arrows through, and put the derivation in the
  constant's doc comment (spec 0116's open issue 7).

- **S9. No document-scanning workload writes to the cache.** A full
  document scan visits every node exactly once; under MRU eviction it
  gets a hit rate of approximately zero *and* leaves the cache holding
  the tail of the document for the interactive work that follows. A
  scan that needs a render obtains it, tests it, and discards it. This
  is spec 0250 S8 stated for the other cache, and the reason is the
  same.

- **S10. Housekeeping.** `docs/protolens/design/caches.md` describes
  two caches; `CandidateCache` is gone and `render_cache.rs`'s module
  doc already says it is "the only one in the crate". Correct it here.

## Open questions

1. **What is a preview render's real size?** S8 cannot be derived
   without it. Measure one at the sizes the override pane actually
   produces (spec 0174 bounds the interior at 4096 bytes by default),
   times a plausible number of candidates arrowed through.

2. **Does anything else mutate a cache hit?** S1 and S2 are the two
   found by reading `render_node_as`. The `Arc` in S3 makes any
   remaining one a compile error rather than a silent copy, so the
   answer arrives for free — but it should be looked for first, in case
   one of them is load-bearing and needs its own treatment.

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

1. `a_cache_hit_returns_the_same_allocation` — two lookups of one key
   yield `Arc`s that are `ptr_eq`. S3.
2. `a_cached_render_is_never_mutated` — the value seen through the
   cache after a splice equals the value inserted. S1, S2.
3. `a_renamed_field_shows_its_name` — the drawn row carries the display
   name although nothing stored does. S1.
4. `a_packed_run_names_every_row` — the `patch_rows` case survives the
   move to draw time. S1.
5. `the_highlight_lands_on_the_renamed_field` — a style hint's columns
   agree with the drawn text. Must drive a real render: `window_styles`
   is keyed on `display_row_text` offsets, **not** `row_content`.
6. `a_truncated_preview_shows_its_marker` — the overlay reports the
   marker row and the cached lines do not contain it. S2.
7. `a_confirmed_splice_leaves_no_entry_behind` — after a commit the key
   is absent, so a large render cannot evict the previews. S5.
8. `an_oversized_entry_evicts_nothing` — inserting an entry larger than
   the whole budget leaves the existing entries in place. S7.
9. `a_sweep_writes_to_no_cache` — cache contents identical before and
   after a full-document search. S9.

## Measured outcome

Filled in at implementation. It must include: the per-hit render cost
before and after; the root-override phase table re-run, showing the
0.56 s clone gone; peak and at-rest memory on googleapis across a
`t`/`Enter`/`o`/`d` cycle, against spec 0207's 1.66 GiB / 0.94 GiB; and
the re-derived budget with its derivation (S8, open question 1). State
plainly anything that did not improve.
