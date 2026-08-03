<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0237 — the origin is the argument

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0236-….md (`:override-as`, the pre-fill, the option
        registry — this respells its argument grammar and replaces its
        two value completers)
      docs/specs/0235-….md (`SearchSweep`, the live prompt and its
        unmatched tint — S12 splits that tint in two)

## Background

Spec 0236 collapsed three mechanisms into `:override-as [<type>]
[--origin <origin>] [--field-name <name>]`. Using it exposes three
problems, all in the arguments rather than in the idea.

**The positional is the wrong argument.** An override is *about* an
origin — that is its identity, that is what the manage pane groups by,
and that is the one argument every invocation must think about. The
type is what the origin is being rendered as, and is frequently absent
(a bare command means raw). Putting the optional thing in the
positional slot and the mandatory subject behind a flag inverts the
sentence.

**Both value completers answer the wrong question.** `--origin` offers
a *sorted* list of the ≤3 buildable shapes, so the user reads a list to
find the widening they want; the shapes have a natural order —
narrowest to widest — and rotating through it in that order is the
whole interaction. `<type>` prefix-matches the full lexicographic FQDN
list, which on googleapis is 58 777 names; the pane beside it has
ranked those names by inference score, and completion ignores that
ranking entirely.

**`--field-name` has no completion at all**, so the pre-filled name is
the only one the user gets without typing. The four ways protolens can
name a field are already all implemented; they are simply not reachable
from the command line.

Separately, two small things:

`Ctrl-d` does nothing in the command line, although `Ctrl-a`, `Ctrl-e`,
`Ctrl-b` and `Ctrl-f` are all bound to their readline meanings there
(spec 0235 S17/S18). `Ctrl-d` is readline's forward-delete and its
absence is the one hole in that set.

The live search prompt (spec 0235 S10) tints its pattern red both while
the sweep is still running and after it has finished with nothing —
deliberately, on the reasoning that "from the user's seat those are one
fact". They are not: on a large document the sweep takes long enough to
read, and a red pattern that is about to turn out fine trains the user
to ignore red.

## Goals

- **G1.** `<origin>` is `:override`'s positional argument; the type
  moves to `--as <fqdn>`.
- **G2.** Tab on `<origin>` rotates through the buildable origin
  shapes, narrowest first.
- **G3.** Tab on `--as` offers inferred types by decreasing score,
  falling back to the lexicographic list.
- **G4.** Tab on `--field-name` rotates through the four derivations of
  a field's display name.
- **G5.** `Ctrl-d` forward-deletes in the command line, as `Delete`
  does.
- **G6.** The search prompt distinguishes "still looking" from "not
  there".

## Non-goals

- **N1. No compatibility alias for `:override-as`.** Same reasoning as
  spec 0236 N2: an alias keeps two spellings of one operation in
  `COMMANDS`, in the help text and in the completion list. The command
  is one release old and has no saved artifacts referring to it — the
  override YAML stores origins and types, never command lines.
- **N2. Completion does not wait for a cold inference cache.** A miss
  in `heat_lookup` queues background scoring and returns nothing; Tab
  answers from the lexicographic list instead of doing nothing visible
  (S8). A completer that sometimes ignores a keystroke is worse than
  one whose order is sometimes alphabetical.
- **N3. `--as` does not rotate.** Unlike the other two arguments it has
  no small closed set to rotate through, so it stays a prefix-matched
  completer.

## Specification

### The command

- **S1.** The command is renamed `:override`. `COMMANDS` loses
  `override-as` and gains `override`; `:o` still resolves to it (no
  other command begins with `o`). `command_flags` gains `--as` beside
  `--field-name`, and loses `--origin`.

- **S2.** The grammar becomes:

  ```
  :override <origin> [--as <fqdn>] [--field-name <name>]
  ```

  `<origin>` is **required** — `:override` with no positional is an
  error naming the three shapes. This is the one argument that has no
  sensible default now that it is what the command is about; spec 0236
  S4's "absent means default" continues to hold for the two flags,
  which is what makes deleting an argument from the pre-filled line a
  way to ask for its default.

  `<origin>` parses exactly as spec 0236 S5 defined it: split on the
  last `:`, and the container is an FQDN whenever it does not begin
  with `/`.

- **S3.** `--as` carries what the positional carried: the type FQDN or
  primitive keyword, absent meaning raw. Validation, application,
  merging and the affected-node message are unchanged (spec 0236
  S9/S10/S11).

### The pre-fill

- **S4.** `o` pre-fills `<origin>` with the subject's *current* origin
  — the highlighted entry's own origin in the manage pane, and in the
  selection pane the applicable active entry's origin if the target has
  one, else the target's bare `Path` origin.

  The applicable entry's origin, not always the bare path: `o` on a
  node covered by an `fqdn:field` override must pre-fill that override,
  or `Enter` on the unedited line would silently narrow it to the one
  node — the opposite of spec 0236 G2.

- **S5.** `--field-name` pre-fills with the first available of S7's
  four derivations, which is what it already did, with S7's (3)/(4)
  split replacing spec 0236 S8's single `f<P>` fallback.

  Spec 0236 S8's normalization is unchanged: a `--field-name` equal to
  the schema-derived name is stored as `None`.

### Completion

- **S6.** `<origin>`: an unfiltered rotation through
  `OverrideKind::{Path, PathField, FqdnField}` **in that order** —
  narrowest to widest, which is the order in which a user widens a
  scope and the order the manage pane's own `z` already rotates.
  Shapes `origin_for_kind` cannot build for the subject are skipped, so
  the rotation only ever offers what works.

  Unfiltered: the token being completed is almost always the
  pre-filled origin, and prefix-matching it against the other two
  shapes would match nothing, leaving Tab dead exactly where it is most
  wanted. The three shapes are alternatives to each other, not entries
  in a namespace to search.

- **S7.** `--field-name`: an unfiltered rotation through the four
  derivations of the subject node's display name, in this order:

  | # | Derivation | Available when |
  |---|---|---|
  | 1 | the applicable entry's stored `name` | an entry names it |
  | 2 | the parent schema's field name | the parent's type resolves and declares the field |
  | 3 | `f<field-number>` | `span.field_number != 0` (0 is the virtual-wrapper/root sentinel) |
  | 4 | `p<position>` | always — `sibling_position` is 1-based |

  Duplicates are dropped, keeping the first occurrence. (1) and (2)
  coincide whenever the stored name came from the schema, which is
  exactly the case spec 0236 S8 normalizes to `None`; without the
  dedup, Tab there would appear to do nothing.

  (3) and (4) were one candidate in spec 0236 (`f<P>`, position under
  an `f` prefix), which was a straightforward defect: `f` reads as
  "field" and a reader would take `f5` for field 5. They are now two
  candidates under two prefixes, and the fixture that tells them apart
  (`group_type_fixture`: field 5, position 1) is already in the suite.

- **S8.** `--as`: prefix-match the **inferred** candidates for the
  subject node, in decreasing score order; if that yields nothing,
  prefix-match the lexicographic list (primitive keywords for the
  node's wire type, then `all_type_fqdns`) as today.

  The inferred list is `heat_lookup`'s cached `top_n` for the subject's
  scored range, read at `Tier::User` exactly as the selection pane
  reads it. A cache miss queues the request and yields an empty
  inferred list, so the fallback runs — silently (N2).

  The two lists are tried in sequence rather than concatenated: a
  prefix that matches an inferred type must not also drag in the
  hundreds of unranked FQDNs sharing that prefix, which is the whole
  value of ranking them.

### The command line

- **S9.** `Ctrl-d` joins `handle_command_key`'s `Control`/`Alt`
  character gate, doing what the `Delete` arm does. It must live in
  that gate rather than beside `Delete`: the gate is what stops the
  plain `Char(c)` arm from typing a literal `d` (spec 0235 S18).

- **S10.** `Ctrl-d` is *not* readline's "delete-or-EOF" — an empty
  buffer is not a quit, here or anywhere (spec 0236 G8). On an empty
  buffer it does nothing, exactly as `Delete` does.

### The search prompt

- **S11.** The prompt's pattern takes one of three styles:

  | Sweep state | Style |
  |---|---|
  | running, no hit yet | `tier_non_canonical` (orange) |
  | hit found | default |
  | finished, no hit | `tier_invalid` (red) |

  These are `SearchSweep::is_finished()` and `found`, both already
  public to the module. A pattern with no sweep at all (an empty pane,
  where `begin_search_sweep` returns `None`) is red: there is nothing
  left to look in.

- **S12.** The orange is the palette's existing `Tier::NonCanonical`
  hue, not a new color: "in progress, no verdict yet" is the same
  register as "suspicious but not wrong", and adding a fourth severity
  hue for it would put two oranges on screen at once.

## Alternatives considered

**Keep the type positional and add `--origin` as today.** Rejected by
G1's reasoning: the origin is now mandatory and the type is not, so the
optional argument would hold the positional slot. It also leaves the
command reading as a statement about a type when it is a statement
about an origin.

**Concatenate the inferred and lexicographic lists for `--as`.**
Simpler to write, and wrong: on googleapis a two-character prefix
matches thousands of FQDNs, so the handful of ranked ones would be
buried in position 1-5 of a list nobody scrolls. Sequencing the lists
is what makes the ranking reachable.

**Block Tab on `--as` until the scores arrive.** Rejected as N2. It is
what the selection pane does, but the pane has a placeholder row to
show meanwhile; the command line's only feedback is the message row,
which the prompt itself is sharing.

**Prefix-filter the origin and field-name rotations.** Rejected as S6.
Both tokens are pre-filled with one of the very candidates being
rotated, so filtering leaves Tab dead in the common case.

**A separate `--position` flag instead of the `p<N>` derivation.**
Considered for S7 (4) and rejected: the four derivations are one
question — "what should this field be called" — with four answers, and
splitting one of them into a flag would make it the only answer that
cannot be reached by the same keystroke as the others.

**One tint for "searching" and "not found", as spec 0235 S10 had it.**
That spec argued the two are one fact from the user's seat. On a small
document they are; on googleapis a full-document miss takes long enough
to read the red and act on it. Splitting them costs one style lookup.

## Test plan

1. `override_requires_an_origin` — a bare `:override` errors naming the
   three shapes; `:override /1` succeeds. S2.
2. `override_takes_its_type_from_the_as_flag` — `:override <origin>
   --as pkg.Msg` sets the type; omitting `--as` is raw. S3.
3. `o_prefills_the_applicable_entry_origin` — with an `fqdn:field`
   override in force, `o` on a covered node pre-fills *that* origin,
   and `Enter` leaves the entry's blast radius unchanged. S4.
4. `origin_completion_rotates_narrowest_first` — Tab from the
   pre-filled origin yields `path`, then `path:field`, then
   `fqdn:field`, unfiltered; a subject whose parent is raw skips
   `fqdn:field`. S6.
5. `field_name_completion_rotates_four_derivations` — on a node with a
   stored name differing from the schema's, Tab yields all four; on the
   group fixture the third is `f5` and the fourth `p1`, which is what
   pins the two apart. S7.
6. `field_name_completion_drops_duplicate_derivations` — a stored name
   equal to the schema name yields a 3-way rotation. S7.
7. `as_completion_prefers_inferred_order` — with a warm `by_range`
   entry, Tab on `--as` offers the inferred FQDNs in decreasing score
   order; a prefix matching none of them falls back to the
   lexicographic list, and the two are never mixed. S8.
8. `as_completion_falls_back_on_a_cold_cache` — no cached scores, Tab
   still completes, from the lexicographic list, with no message. S8/N2.
9. `ctrl_d_forward_deletes_in_the_command_line` — beside the existing
   `Delete` coverage, and on an empty buffer it is a no-op that does
   not type a `d`. S9/S10.
10. `only_the_bound_ctrl_and_alt_chords_do_anything_in_the_command_line`
    — the existing gate test, extended with `Ctrl-d`. S9.
11. `search_prompt_is_orange_while_sweeping_and_red_when_finished` —
    the three states of S11, driven through `search_sweep_step`.
12. `resolve_command_no_longer_knows_override_as` — the old name
    errors. S1/N1.

## Measured outcome

All twelve test-plan items are in the suite and green, in
`tui/tests/override_cmd.rs` (1-8, 12), `tui/tests/search.rs` (9-11) and
the `command_line`/`manage_pane`/`render`/`key_dispatch` test modules
whose `override-as` call sites were respelled.

Two things the implementation added that the spec did not name:

**A second completion primitive.** The existing `apply_completion`
stores `CompletionState { index: None }` — the first Tab only extends
the token to the candidates' longest common prefix and primes the
cycle, selecting nothing. On a rotation (S6, S7) that reads as Tab
doing nothing at all, since the token is already one of the candidates
and is therefore itself the common prefix. `apply_rotation` is the
sibling: it replaces the token immediately and starts at the candidate
*after* whichever one the token currently spells. Both live in
`command_line.rs`; which one an argument gets is the whole difference
between a namespace and a closed set.

**The completer has to be told which node it is talking about.** For
`<origin>` the token *is* the origin, so `parse_origin(prefix)` names
the subject; for the two flags the subject comes from scanning the rest
of the line for an already-typed origin (`line_origin`, which steps
over flag/value pairs), falling back to the command's own target node.
Without this, Tab on `--as` after widening the origin to `fqdn:field`
would still rank types for the narrow node.

S8's `heat_lookup` is called with a bounded `end` (the selection pane's
own list height), not `usize::MAX`: only the `complete` cache slot can
satisfy an unbounded request, and it fills only after the selection
pane's upgrade pass, so the unbounded form would miss on exactly the
cache the spec means to read.
