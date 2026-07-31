<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the render and candidate caches

*last verified: 2026-07-31*

## Executive summary

protolens keeps two independent, byte-bounded MRU caches — one for scored
type-candidate lists, one for fully rendered subtrees — that exist purely
to make interactive browsing feel instant. Both are session-scoped (never
persisted), sized generously against a rough "session, not archive"
budget, and share the same eviction discipline. Neither is required for
correctness: either cache being empty just means the next lookup falls
through to recomputation.

## Technical detail

### Why two caches, not one

The two caches memoize genuinely different, independently expensive
operations:

- The **candidate cache** (`override_pane::CandidateCache`) memoizes the
  *ranked list of plausible types* for a byte range — the output of
  `prototext_graph::score_all` against the scoring graph, keyed by range.
  This is what makes reopening the override-select pane on a
  previously-visited range instant instead of re-scoring against every
  known type.
- The **render cache** (`render_cache::RenderCache`) memoizes the
  *rendered output* — the lines and their `NodeSpan`s — for a
  `(range, type, is_preview)` triple: the expensive output of decoding
  and formatting a subtree under a specific target type. This is what
  makes arrowing through the select pane's candidate list cheap even
  though each highlighted row is, in principle, a full re-decode.

  Two things are deliberately *not* in that key. The field name is not,
  because the render always uses the fixed placeholder `_` and the real
  name is patched in afterwards as a substring replacement, leaving the
  cached render field-name-invariant. Highlighting is not, because since
  spec 0187 there is nothing render-scoped left to highlight — styles
  are recomputed per frame over the on-screen window only. What *is* in
  the key, and must stay, is `is_preview`: a preview renders at most
  `override_preview_byte_budget` interior bytes, so it is literally not
  the same input as the confirmed render, and confirming an override
  must not silently reuse a truncated one.

A candidate-cache hit and a render-cache hit answer different questions
("what types are plausible here" vs. "what does this range look like
rendered as this specific type") and are consulted at different points in
the override workflow, so collapsing them into one cache keyed on
everything would force every candidate-list lookup to also pay for (or
awkwardly special-case around) render output it doesn't need.

### Shared eviction discipline

Both caches are simple maps with a running byte-size estimate, evicting
least-recently-used entries once that estimate exceeds a fixed budget —
with one shared invariant: the entry that was *just* inserted is never
itself evicted to make room for itself, even if it alone would exceed the
budget. This guarantees a cache lookup immediately followed by a cache
insert of the same key always leaves that key retrievable, which is what
every call site actually relies on (insert-then-immediately-reuse is the
common pattern, not insert-and-forget).

### Capped previews vs. complete lists

The candidate cache stores only a *capped* preview (as many rows as the
pane last had room to show) for ranges other than the one currently
active in the override-select pane — the full ranked list for the active
range lives uncached, directly in `App` state, and is only demoted into
the capped cache when the pane closes. This two-tier arrangement (full
list for "here now," capped preview for "recently visited elsewhere")
keeps the cache's total footprint bounded by the number of distinct
ranges visited, not by the number of candidates per range, while still
making a full re-score on returning to a previously-viewed range
unnecessary in the common case — the pane transparently upgrades a capped
preview back to a complete list the moment the user tries to scroll or
navigate past what's cached.
