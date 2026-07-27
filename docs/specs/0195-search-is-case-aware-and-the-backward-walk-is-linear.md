<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0195 — search is case-aware, and the backward walk is linear

Status: implemented
Implemented in: 2026-07-27
App: protolens
Refs: docs/specs/0114-protolens-override-pane.md (§4, the wrap-around
        walk this spec extends),
      docs/specs/0117-protolens-override-management-pane.md (§3, the
        manage pane's own `/`/`?`/`n`),
      docs/specs/0137-protolens-override-pane-refinements.md (G4),
      docs/specs/0194-the-cursor-is-a-caret.md (S8, which will want a
        match *offset* out of the same function)

## Background

A user report of 2026-07-27: "`p` after `/` stalls at 99% CPU on
googleapis.desc; search is case-blind; a leading space seems dropped."
Three claims, triaged separately. Two are real and are what this spec
fixes; the third is not reproducible as stated.

### 1. The backward walk is quadratic

`jump_to_match` (`override_select.rs:873-898`) walks the document-order
node chain and wraps at its ends:

```rust
cur = match dir {
    SearchDir::Forward => self.tree[cur].doc_next.unwrap_or(self.first_node),
    SearchDir::Backward => self.tree[cur].doc_prev.unwrap_or(self.last_node()),
};
```

`unwrap_or` takes its argument **by value**, so it is evaluated on every
iteration whether or not the `Option` is `None`. The forward arm gets
away with it because `self.first_node` is a plain field. The backward arm
does not: `last_node()` (`navigation.rs:394-401`) walks the entire
`doc_next` chain from the root. So a backward search costs
O(steps x nodes) where it should cost O(steps).

Measured before the fix — release build, `wide_sibling_scalars_app`, a
full no-match wrap:

| nodes  | backward | forward |
|--------|----------|---------|
| 5 000  | 142 ms   | 0.59 ms |
| 10 000 | 429 ms   | 0.63 ms |
| 20 000 | 1 564 ms | 1.30 ms |

Roughly 4x per doubling against 2x — quadratic against linear. On
googleapis.desc's 49 255 roots this is the reported stall.

The report blames `p`, which is nearly right and worth stating precisely
because it changes what to test: `p` searches `dir.reverse()`
(`key_dispatch.rs:603-607`). After `/` it is backward and slow; after
`?` it is *forward* and fast, and what is slow in that direction is `?`
itself and `n`. The invariant is **any backward walk**, not "any `p`".

### 2. Search is case-blind, in all three panes

Every one of the three searches lowercases both the pattern and the
candidate:

| pane     | site |
|----------|------|
| main     | `override_select.rs:877` + `:885` |
| override | `override_select.rs:834` + `:843` |
| manage   | `manage_pane.rs:237` + `:245` |

So `Foo` and `foo` are indistinguishable — in a tool whose subject
matter is protobuf FQDNs, where `google.protobuf.Any` and a field named
`any` are different things and the case is the only thing that
distinguishes a message name from a field name.

The three sites are three copies of the same two lines, which is why
they were all wrong in the same way and why the fix is one function.

Each also allocates two `String`s per candidate, on the walk that
Background 1 is about.

### 3. What vim does

For reference, since "case aware" has three plausible meanings:

- `'ignorecase'` is **off** by default, so stock vim search is
  case-*sensitive*.
- `'smartcase'` (also off by default, and only meaningful with
  `'ignorecase'` on) makes an all-lowercase pattern match
  case-insensitively and any pattern containing an uppercase letter
  match exactly.
- `\c` and `\C` anywhere in the pattern override both options for that
  search.

vim does not trim the pattern.

`n` repeats the last search in the same direction and **`N`** repeats it
in the opposite one. `p` is *put* — paste. protolens binds the
reverse-repeat to `p` at all three panes, which is an invention, and `N`
is unbound.

Of protolens's four search keys, then, `/`, `?` and `n` are faithful and
one is not.

### 4. The leading space

Not reproducible as reported. A headless probe (`/`, space, `i`, `d`,
`Enter`) yields `command_buffer == Some(" id")` and
`last_search == Some((Forward, " id"))`; nothing in `command_line.rs`
trims a pattern.

The likely explanation is a property of *what* is searched rather than
of the pattern: `jump_to_match` tests only a node's own header line
(`self.lines[span.text_range.start]`), which still carries its full
indentation. So ` id` and `id` match the same lines except where a token
has no preceding space, which is only at indent 0 or after a `:`. It
reads as though the space were ignored. Out of scope until there is an
exact repro — see N4.

## Goals

- **G1.** A backward search costs the same as a forward one.
- **G2.** All three panes' searches are **smartcase**: an all-lowercase
  pattern matches case-insensitively, a pattern containing any uppercase
  character matches exactly.
- **G3.** One matcher, used by all three panes, so the next change to
  search semantics is made once.
- **G4.** Matching allocates nothing per candidate.
- **G5.** The key that repeats a search the other way is `N`, as it is
  in vim.

## Non-goals

- **N1.** Regular expressions. The patterns stay plain substrings.
- **N2.** vim's `\c`/`\C` in-pattern overrides. They are an escape hatch
  for an option this spec does not offer.
- **N3.** Making case sensitivity configurable. protolens has no user
  configuration surface, and inventing one for this is disproportionate.
- **N4.** The leading-space report (Background 4). Nothing is trimming
  anything; there is no defect to fix until there is a repro.
- **N5.** Widening *what* the main pane searches. It tests a node's own
  header line only, so a match on a footer line, or on a line the
  annotation-hiding transform removed, stays invisible to it. That is a
  real limitation and a separate spec.
- **N6.** Caching `last_node`. See A1.

## Specification

### S1. The wrap becomes lazy

```rust
SearchDir::Backward => self.tree[cur].doc_prev.unwrap_or_else(|| self.last_node()),
```

That is the entire fix for G1. `last_node()` then runs once per search
rather than once per step, and a search that wraps pays one extra O(N)
walk on top of the O(N) it was already going to pay — a constant factor,
not a complexity class.

### S2. One smartcase matcher

A small type beside `search_wrap` (`mod.rs:240`), which is already where
the shared search primitives live:

```rust
pub(super) struct SearchPattern {
    needle: String,
    case_sensitive: bool,
}
```

`SearchPattern::new` decides once, at construction, by vim's rule: any
uppercase character in the pattern makes the search case-sensitive.
Deciding once is the point — the alternative is re-deriving it per
candidate, which is how the current code ends up allocating.

Matching is case-folded on the fly rather than by lowercasing the
haystack (G4): the candidate's characters are lowercased as they are
compared, which allocates nothing and handles the multi-character
lowercase mappings (`İ`) that a byte-wise fold would get wrong.

An empty pattern is never constructed: all three call sites return early
on one, exactly as they do today.

### S3. The three call sites use it

`jump_to_match`, `jump_to_override_match` and `jump_to_manage_match` each
build one `SearchPattern` before their walk and test candidates with it.
The two `to_lowercase()` calls disappear from each.

Their surrounding behavior is unchanged: the same wrap order, the same
`pattern not found: {pattern}` message reporting the pattern **as the
user typed it**, the same jumplist entry and ancestor unfolding in the
main pane, the same live preview in the override pane.

### S4. The reverse-repeat key becomes `N`

At all three panes, and `p` is unbound rather than kept as an alias.
There is no compatibility argument for keeping it: `p` was never right,
`N` was never taken, and leaving both would teach the wrong one to
anybody reading the key list.

`p` survives only where it never meant "previous": as a chord sub-key in
`xp` and `xdp` (export as prototext), which is dispatched before the
top-level key match and is untouched.

## Alternatives considered

### A1. Cache `last_node` in a field

The obvious companion to S1: `first_node` is a plain field, so why is the
tail a walk?

Because the two are not symmetric. `first_node` may be a field only
because spec 0188 G1 guarantees the root node keeps its identity across a
root respice. Nothing guarantees that for the tail: `splice_override`
re-renders whole subtrees, so a cached `last_node` would need
invalidation at every splice site, and a stale one would send a backward
search into the wrong end of the document.

And it buys almost nothing. After S1 the wrap happens once per search, so
the cache saves one O(N) walk out of a search that is O(N) anyway in the
not-found case — a factor of two on the slowest path, in exchange for an
invariant somebody has to maintain forever. Rejected.

### A2. Plain case-sensitive search, as vim ships

Closest to the reference in Background 3, and the simplest thing that
answers the report. Rejected: with `'ignorecase'` off, vim also ships
`/` alongside a config file that most users edit. protolens has no
config (N3), so the default is the only behavior there will ever be, and
it should be the one that is right most often. Typing a lowercase
pattern is a natural "I don't care about case" signal, and typing
`Timestamp` is a natural "I mean the message, not the field" one.
Smartcase reads both correctly without asking.

### A3. Keep lowercasing both sides, and just fix the walk

Half the work, and it leaves search case-blind — which is one of the two
defects reported. Rejected on the report.

### A4. Lowercase the haystack per candidate, but the pattern only once

The minimal shape of S2: one allocation per candidate instead of two.
Rejected because the on-the-fly fold is no harder to write, is correct
for the multi-character mappings that lowercasing a `str` handles and
lowercasing byte-wise does not, and removes the allocation from a walk
this same spec is making linear. Halving an allocation on a hot path is
a strange place to stop.

## Test plan

1. A backward search over a large document scales linearly: the ratio of
   the times at 10 000 and 20 000 nodes is near 2, not near 4.
2. Forward and backward no-match searches over the same document cost
   the same order of magnitude.
3. Backward search still finds the same node it found before, and still
   wraps at the start of the document to the last node.
4. An all-lowercase pattern matches candidates of any case, in each of
   the three panes.
5. A pattern containing an uppercase character matches only exact-case
   candidates, in each of the three panes.
6. A pattern that is entirely uppercase is case-sensitive too — the rule
   is "contains an uppercase character", not "is mixed case".
7. `pattern not found` reports the pattern as typed, not folded.
8. A pattern with a leading space still searches for the leading space
   (Background 4): it matches a line where the token is preceded by a
   space and not one where the token starts the line.
9. `N` reverse-repeats in each of the three panes, and `p` no longer
   does anything there — while `xp` still exports as prototext.
10. Matching a candidate allocates nothing — asserted by a counting
   allocator over a no-match wrap, or failing that by the linearity of
   test 1.

## Measured outcome

Same harness as Background 1 — release, `wide_sibling_scalars_app`, a
full no-match wrap — now kept as `backward_search_scales_linearly` in
`tui::tests::profiling` (`#[ignore]`d, run with `--ignored`):

| nodes  | backward before | backward after | forward after |
|--------|-----------------|----------------|---------------|
| 5 000  | 142 ms          | 1.47 ms        | 1.39 ms       |
| 10 000 | 429 ms          | 3.12 ms        | 2.28 ms       |
| 20 000 | 1 564 ms        | 5.45 ms        | 4.61 ms       |

The doubling ratio falls from ~3.6x to 1.75x, and backward now costs
what forward costs — G1. At 20 000 nodes the search is 287x faster, and
the gap widens with the document, which is why the report was about
googleapis.desc's 49 255 roots and not about the fixtures.

The three `to_lowercase()` pairs are gone, so a no-match wrap allocates
nothing per candidate where it previously allocated two `String`s (G4).
That is by construction rather than by a counting allocator — test plan
item 9's fallback — and it is part of why even the *forward* figures
improved.

445 tests, up from 439: the smartcase rule at the matcher, the
multi-character lowercase mapping, the leading space, and one smartcase
test per pane. The three `..._repeat_with_p_reverses_direction` tests
became `..._repeat_with_capital_n_...` and press `N` (S4).
