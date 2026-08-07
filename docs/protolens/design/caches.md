<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the render cache

*last verified: 2026-08-07*

## Executive summary

protolens keeps one byte-bounded MRU cache — `render_cache::RenderCache`,
holding fully rendered subtrees — that exists purely to make arrowing
through the override-select pane's candidate list feel instant. It is
session-scoped (never persisted) and not required for correctness: an
empty cache just means the next lookup falls through to a re-render.

This document described two caches until 2026-08-07. The second, the
candidate cache, no longer exists; and after spec 0251 the surviving one
serves the live preview alone. The heat cue's `tiered::TieredBounded` is
not a second one: it is bounded by entry count rather than by bytes, has
four tiers, and does not promote on a low-tier read.

## Technical detail

### What is cached, and what is not in the key

The render cache memoizes the *rendered output* — the lines and their
`NodeSpan`s — for a `(range, type, is_preview)` triple: the expensive
output of decoding and formatting a subtree under a specific target
type.

Two things are deliberately *not* in that key. The field name is not,
because the render always uses the fixed placeholder `_` and the real
name is patched in afterwards as a substring replacement, leaving the
cached render field-name-invariant. Highlighting is not, because since
spec 0187 there is nothing render-scoped left to highlight — styles are
recomputed per frame over the on-screen window only.

What *is* in the key, and must stay, is `is_preview`: a preview renders
at most `override_preview_byte_budget` interior bytes, so it is
literally not the same input as the confirmed render, and confirming an
override must not silently reuse a truncated one.

### Only the preview is cached

Spec 0251 S5: the cache is consulted and written only under
`is_preview`. The confirmed splice neither reads nor writes it.

This started as a performance fix and ended as a scoping rule. Caching
a confirmed render cost two full clones of it — one to insert, one for
the caller — for an entry no second lookup could reach, since the
override is applied exactly once. On a root override of a 25 MB
document that clone alone was 0.56 s of a 4.12 s freeze.

The rule it leaves behind is the useful part. Because the preview
renders the single node under the cursor and is bounded by the byte
budget, every entry in the cache is small and interactive by
construction: the worst shape the default 4096-byte budget admits is
104 512 B (2051 lines, 2049 spans, ~25x the input). And because the
only caller that passes `is_preview: true` is
`preview_override_highlight`, no document-scanning workload can write
to the cache at all — a scan visits every node once, so it would get a
hit rate of approximately zero while evicting exactly what the user was
about to look at. That is structural now rather than a rule to
remember: every sweep reaches the renderer through `resettle_node`,
which passes `false`.

### Eviction, and the entry that is too big

The cache is a vector of entries with a running byte-size estimate,
evicting least-recently-used entries once that estimate exceeds
`RENDER_CACHE_MAX_BYTES`. The estimate counts each line's string bytes
plus `spans.len() * size_of::<NodeSpan>()`; it ignores spare capacity
and headers, which is safe because undercounting costs memory and never
correctness.

An entry that alone exceeds the whole budget is **rejected** (spec 0251
S7). The cache used to keep it instead, under an "never evict the entry
just inserted" floor on the eviction loop — so one large render evicted
every other entry and then sat alone, degenerating the cache to a
single entry precisely when hits were wanted. Re-rendering one thing is
cheaper than losing everything else.

The budget is 8 MiB, derived rather than asserted (spec 0251 S8,
closing spec 0116's open issue 7): a screenful of the ranked candidate
list is ~50 entries, so holding one pass through it costs ~5 MB of
worst-case previews, leaving ~80 entries of headroom. The doc comment on the constant
records the interaction this implies — raise
`--override-preview-byte-budget` far enough and a single preview
exceeds the whole cache, after which S7 rejects every entry and the
cache quietly stops working.
