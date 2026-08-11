<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0273 — a pattern is a path or a regex

Status: implemented
Implemented in: 2026-08-11
App: protolens
Refs:
- docs/specs/0235-the-search-answers-before-it-has-finished.md (the
  resumable sweep, the two haystacks, smartcase)
- docs/specs/0246-a-search-stops-at-every-match.md (a stop is a match,
  `RowBound`, the origin row's two halves)
- docs/specs/0272-the-prompt-answers-while-you-are-still-typing.md (the
  sweep is rebuilt on every keystroke)
- docs/specs/0195-one-pattern-for-every-pane.md (`SearchPattern`,
  smartcase, the allocation-free fold)

## Background

`/` is a substring search. Two things it cannot do:

- **No regular expressions.** `name.*deadline`, `^\s*id:`,
  `"(foo|bar)"` are searched for literally and find nothing.
- **The path haystack is a coincidence, not a mode.** Spec 0235 S19
  hands *every* pattern to both haystacks, rendered text first. So
  `/1/2` scans 5.28 M rows of text it can never match, and — worse for
  the reader — `2` silently searches paths as well as text and stops on
  rows whose visible content has nothing to do with it.

Two defects fall out of that second point and are fixed here:

- The path prefix is a raw `str::starts_with`, so `/1` matches `/12`.
- Every *line* of a node tests that node's path, so a node with three
  own lines is three consecutive stops on the same answer.

## Goals

- **G1.** A pattern that looks like a positional path searches paths,
  and only paths, rotating among the nodes whose path has the pattern
  as a prefix.
- **G2.** Every other pattern is a regular expression in the `regex`
  crate's syntax (RE2: leftmost-first alternation, no backreferences,
  no lookaround, linear time), compiled when the pattern text changes.
- **G3.** Smartcase survives, with an escape hatch.
- **G4.** No regression in the cost of the common case — a short
  literal swept over a large document.

## Non-goals

- **N1.** **No match crosses a row.** The haystack stays one rendered
  row, so `\n` is unmatchable and a multi-line pattern is refused (S11)
  rather than silently finding nothing.

  This is the whole of what this spec defers, and it is deferred
  because it is the whole of what is expensive. A match that may span
  rows cannot be found by a budgeted slice without a bound on its
  length; removing that bound means the search stops running on the
  drawing thread, which means a worker, which means reading `node_text`
  concurrently with the bake that fills it, the splice that vacates it
  and spec 0256's discard — and spec 0249 already rejected "search
  reading stale text", so a snapshot is not available either. That is a
  spec of its own, and everything here is a prerequisite for it rather
  than throwaway work.
- **N2.** No PCRE. Backreferences and lookaround stay unavailable; the
  linear-time guarantee is what makes it safe to run a user-supplied
  pattern over 5.28 M rows inside a frame budget.
- **N3.** No replace, no multi-cursor, no "select all matches". `n`/`N`
  rotation stays the only way to walk matches.
- **N4.** No syntax for making path mode explicit (a `path:` prefix, a
  leading `\`). The shape test in S2 is the whole rule, and it is
  decidable by eye.
- **N5.** No worker thread, no sliding buffer, no `regex-cursor`. All
  three belong to the successor and none of them is needed to match
  within a row.

## Specification

### Dispatch

- **S1.** `SearchPattern` becomes an enum, built once by
  `SearchPattern::new` from the pattern text:

  ```
  Path(Vec<u32>)                       // segments, root-first
  Literal { needle, case_sensitive }   // today's fields, unchanged
  Regex(regex::Regex)
  ```

- **S2.** A pattern is a **path pattern** iff it consists of `/` and
  ASCII digits, starts with `/`, and has no empty segment other than a
  single trailing `/`. So `/`, `/1`, `/1/2`, `/1/2/` are paths;
  `/1/a`, `1/2`, `//2`, `/1 ` are not.

- **S3.** A path pattern's segments are compared **segment-wise**. `/1`
  matches `/1` and `/1/23`; it matches neither `/12` nor `/2/1`.

  The comparison is against `PathScratch::segments`, not against the
  rendered string: `write_positional_path` already fills that `Vec<u32>`
  (leaf-first, wrapper leg popped) on its way to building the text, so
  a path match needs no string at all. Note the direction — the
  pattern's segments are root-first and the scratch's are leaf-first.

  A bare `/` has no segments and is therefore a prefix of every path,
  so `/` rotates through every node in the document. Useless and
  consistent; consistency wins, and the shape test needs no special
  case.

- **S4.** For a path pattern only the row with `line_in_node == 0` of
  each node is a candidate. A node's other own lines carry the same
  path and are indistinguishable stops on the same answer.

- **S5.** A path pattern is never matched against row text, and a
  non-path pattern is never matched against a path. The two haystacks
  stop being tried together, which is what deletes spec 0246 S9's
  "the path is the row's stop only when the row's text has no match at
  all" guard and its extra scan.

### The regex

- **S6.** Compiled with `RegexBuilder`: `multi_line(true)`,
  `dot_matches_new_line(false)`, `case_insensitive` per S9, and
  `size_limit` bounded so a pathological pattern fails to compile
  rather than allocating without limit.

  `multi_line` has no observable effect on a single-row haystack, where
  `^`/`$` are the row's ends either way. It is set now so that the
  successor does not change the meaning of a pattern the reader has
  already learned.

- **S7.** The **literal tier** takes patterns whose parsed HIR is a
  literal (`Properties::is_literal()`) containing no `\n`, and runs
  today's matching code unchanged — the pre-folded needle, the
  `memchr2` prefilter and its two ASCII guards.

  The needle is the HIR's literal, not the raw pattern text, so `a\.b`
  searches for `a.b`. Escapes work, which they do not today.

  **This tier exists on a premise that must be measured, and deleted if
  the measurement does not support it** (see Test plan 14). A `find_at`
  over a ~47-byte row is on the order of a couple hundred nanoseconds
  against `memchr2`'s few, but the sweep's slice is 1000 rows, so even
  the pessimistic figure is ~200 µs a slice and nowhere near a frame.
  Two implementations of "does this row match" is two behaviors to keep
  in agreement; if the regex path is within noise, keep one.

- **S8.** A pattern that does not compile is **not an error and not a
  search**: the sweep is not started, the view does not move, nothing
  is highlighted. `foo(` is what `foo(bar)` looks like halfway through
  typing it, and spec 0272 rebuilds the pattern on every keystroke. The
  compile error goes to the message line only on `Enter`.

- **S9.** Smartcase is decided from the parsed HIR's **literals**, not
  from the raw pattern text: case-insensitive iff no literal character
  is uppercase. Reading the raw text would make `\D`, `\W` and `\S`
  case-sensitive patterns, which is not what the reader typing them
  meant.

  A reader who wants an all-lowercase pattern matched
  **case-sensitively** writes `(?-i)` — vim spells the same thing `\C`.
  This escape hatch is what makes smartcase a default rather than a
  restriction, and it comes free with the syntax the pattern is already
  in.

  The hatch is detected by parsing the pattern a second time with the
  parser's own `case_insensitive` flag on and comparing the two trees.
  An unchanged tree means the fold had nothing to do: either the pattern
  carries no case-foldable literal (`\d+`, `\{`), or it says `(?-i)` and
  has taken the decision itself. Either way smartcase must keep its
  hands off. Reading the raw text for `(?-i)` would instead find it
  inside `\Q(?-i)\E` and inside a character class; and the builder's
  flag alone is not enough, because the literal tier of S7 does not go
  through the builder at all.

- **S10.** The compiled pattern is **cached against the text it was
  compiled from**, one entry on `App`. `search_highlight_pattern` is
  called from `render` on every frame and today returns a freshly built
  `SearchPattern` (`search.rs:838`, `:845`); building one is currently
  a `String` clone, and after this spec it is a regex compile. A
  compile per frame is not affordable and the call site is not
  obviously a hot path, so the cache is not an optimization to defer.

- **S11.** A pattern that **must** match `\n` is refused, with a message
  saying multi-line patterns are not supported. The predicate is whether
  every string in the pattern's language contains `\n` — true for
  `id\nvalue`, `\n+` and `a(\n|\r\n)b`.

  Refusing is a usability decision, not a correctness one: the haystack
  is one row, so such a pattern would simply never match. Silently
  finding nothing is the worst available outcome.

  The predicate is what the pattern *requires*, not what it admits.
  `\s`, `[^…]` and `(?s).` all admit `\n` and all match plenty of rows;
  refusing them would refuse most of the regex vocabulary in the name of
  a message about a match they were never going to need. `a\n?b` is the
  same case one step further in: `\n` appears in the pattern and still
  does not have to appear in the match.

  The recursion is sound rather than exact — a literal or a `\n`-only
  class needs one, a repetition needs one when its minimum is at least
  one and its body needs one, a concatenation when any part does, an
  alternation when every branch does, and a look-around never does.
  Anything it declines to prove is a pattern that matches.

- **S12.** `\A` and `\z` are **rejected at compile time**. On a
  single-row haystack they are merely synonyms for `^`/`$`, so this
  costs the reader nothing today — but under the successor the haystack
  becomes a window and they would come to mean its arbitrary edges.
  Rejecting them now means the successor changes no pattern's meaning.

- **S13.** In the override and manage panes the regex applies
  unchanged; those panes' entries are single rows already. Path
  patterns find nothing there, as they find nothing there today.

### What does not change

- **S14.** `SweepHit` keeps its shape — a match lies within one row, so
  a start column and a width still describe it. `RowBound`, the origin
  row's split at the caret, the backward last-match scan, the slice
  budget, `next_candidate`'s bijection and the wrap are all untouched:
  `pick_match` calls `find_range_from`, and only that method's body
  changes.

- **S15.** The render highlight pass keeps its per-row loop. Only the
  matcher behind it changes.

### Dependency

- **S16.** protolens takes a direct dependency on `regex` and
  `regex-syntax`. Both are already in `Cargo.lock` — `tree-sitter` and
  `tree-sitter-highlight`, both direct protolens dependencies, pull
  `regex` 1.12.3 — so this costs no new compilation.

## Alternatives considered

**Keep both haystacks and add a shape test only for ordering.** A
smaller diff that leaves the reported complaint — `2` stopping on rows
for reasons the reader cannot see — in place. G1 is about it being an
either/or.

**Do A and C together.** Rejected as sequencing, not as design: C's
cost is dominated by a question that has nothing to do with regular
expressions (can a worker read `node_text` while the bake fills it, the
splice vacates it and spec 0256 discards it), and holding G1–G4 hostage
to it buys nothing. B — multi-line with a generous cap, on a sliding
buffer on the drawing thread — was considered as an intermediate and
skipped deliberately: the buffer would be thrown away by C.

**`regex-cursor`** (pascalkuthe, written for Helix) — a `Cursor` trait
yielding chunks, so a rope is searched without materializing. It solves
non-contiguity, which is the successor's problem, and not
interruptibility: it states that "backtracking is required by this
crate. That makes it unsuitable for searching fully unbuffered
streams", it has no pause/resume and no budget, and its streaming
PikeVM cannot use prefilters longer than one byte. 0.1.5 (July 2026),
self-described prototype. Revisit for the successor, not here.

**`reggy`** — the one crate with a resumable shape: `Search::next()` fed
chunk by chunk, spans relative to the stream start, results invariant to
chunking. Disqualified on everything else: its own "friendly dialect"
rather than RE2 syntax, ~50% API coverage at 0.0.6, and a
`regex_automata::dense::DFA` that is "memory-intensive and slow to
construct" — and spec 0272 recompiles on every keystroke.

**Write the resumable engine.** `rust-lang/regex#425` is open and
BurntSushi declined it: engines cannot "pause execution of a match at an
arbitrary point, and then pick it back up again", supporting it is "a
rewrite of every single regex engine", and start recovery needs the
reverse DFA over the buffered match. The choice is end-indices only, or
start and end via the NFA engines at "approximately an order of
magnitude slower", or buffer everything. His own suggested workaround —
fork the crate, keep only the PikeVM — he priced at months and did not
spend.

**Note for the successor:** `rust-lang/regex#934` — sharing one `Regex`
across threads costs real time on mutex contention over its internal
cache pool. A search worker needs its own.

## Test plan

1. `a_pattern_of_digits_and_slashes_searches_paths_only` — `/1/2` stops
   on the node at that path and not on a row whose text contains
   `/1/2` (S2, S5).
2. `a_path_prefix_is_compared_by_segment` — `/1` finds `/1` and
   `/1/23`, and finds neither `/12` nor `/2/1` (S3).
3. `a_bare_slash_walks_every_node` (S3).
4. `a_path_stops_once_per_node` — a node with three own lines is one
   stop, not three (S4).
5. `a_word_pattern_never_stops_on_a_path` — the converse of 1 (S5).
6. `a_regex_pattern_matches_by_its_syntax` — an alternation finds either
   branch, `(id|name):` (S6).
7. …and `^\s*id` anchors to the row, including when the search resumes
   from a later column (S6).
8. `an_escaped_metacharacter_is_a_literal` — `a\.b` matches `a.b` and
   not `axb`, on the literal tier (S7).
9. `an_uncompilable_pattern_leaves_the_view_alone` — typing `foo(`
   moves nothing, highlights nothing and reports nothing until `Enter`
   (S8).
10. `smartcase_reads_literals_and_yields_to_an_inline_flag` — a class
    escape does not make a pattern case-sensitive, `\bid` matches `ID`
    (S9).
11. …and an inline flag takes case sensitivity back: `id` matches `ID`,
    `(?-i)id` does not (S9).
12. `the_compiled_pattern_survives_a_frame` — two consecutive renders
    with an unchanged pattern compile once (S10).
13. `a_multi_line_or_haystack_anchored_pattern_is_refused` — `id\nvalue`
    says so and `(?s)a.b` does not; `\Aid` says so too (S11, S12).
14. **The literal tier's premise.** A full-document miss timed on the
    reference corpus with the tier and with it forced off. If the
    difference is within noise, delete the tier and this test with it
    (S7).

## Measured outcome

`googleapis.desc` opened against itself — 5 278 324 rows, 4 737 283 arena
slots — under a pty at 50×200, pinned to `taskset -c 4-7`, with the bake
run to completion before the pattern is typed. The number is the trace's
`key Enter us=`: the `Enter` handler runs the sweep to the end
synchronously, so it *is* the whole search.

**Test 14 — the literal tier's premise.** The same pattern `zzqqxx`, a
full-document miss, with the tier and with it forced off:

| | run 1 | run 2 | run 3 |
|---|---:|---:|---:|
| literal tier | 700.6 ms | 708.0 ms | 710.8 ms |
| regex engine | 773.0 ms | 775.0 ms | 772.1 ms |

The spread within a tier is 1.5% and the gap is 9% — about 12 ns a row.
That is above noise, so **the tier stays**, and S7's premise is the one
it was written on rather than a guess.

It is worth saying what the number is not. It is not the 6× S7 quotes
from spec 0235: that factor is `memchr2` against the every-position fold
*within a row*, and the sweep spends most of its time getting to the row
rather than in it. Nine percent is what the 6× is worth once it is paid
for at document scale.

The full-document miss itself is **~0.70 s**, against the 1.63-2.05 s
recorded for the same shape of search after spec 0222. The improvement
is not this spec's — specs 0257 and 0272 stand between the two
measurements — but it does mean this spec's regex arm, at 0.77 s, is
still comfortably under the figure the search used to cost before any
pattern could say `(id|name):` at all.
