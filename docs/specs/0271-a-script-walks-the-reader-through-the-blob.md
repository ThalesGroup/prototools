<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0271 — a script walks the reader through the blob

Status: implemented
Implemented in: 2026-08-10
App: protolens
Refs: docs/specs/0123-protolens-batch-mode.md (the batch subcommand this
      adds a transcript mode to), docs/specs/0126-protolens-focus-independent-keys.md
      (the `handle_key` tier the script keys join),
      docs/specs/0147-protolens-status-message-command-line-split.md (the
      command/message row a step prefills),
      docs/specs/0242-the-selection-is-a-span-of-characters.md (the
      Ctrl-arrow bindings this displaces),
      docs/specs/0257-the-first-pane-does-not-wait-for-the-last-line.md
      (why a step may name a node whose lines do not exist yet),
      docs/specs/0261-an-export-waits-for-the-lines-it-names.md (the
      bake-then-act precedent a step follows),
      docs/specs/0268-the-bytes-are-shown-where-the-reader-asked.md (the
      wire span a step declares)

## Background

protolens is used to explain protobuf encodings to people watching:
`grpconf/anomalies.pb` exists for exactly that, and `grpconf/README.md`
carries a seven-section narrative — "what it shows, in order" — written
to be read top to bottom during a talk. Today that narrative lives only
in the README. Delivering it means the presenter remembers a sequence of
navigation moves, performs them live, and speaks the commentary from
memory or from a second screen. Nothing in the session tells the
audience what they are looking at.

The moves themselves are not hard — set the cursor to a node, fold what
is noise, show the bytes of one line. They are just numerous, ordered,
and easy to fumble in front of a room.

## Goals

- **G1.** A YAML file lists ordered steps. Each step declares, in one
  place, a piece of commentary and the view that goes with it: which
  node is current, what is folded, which rows show their wire bytes.
- **G2.** The script is found without being asked for: `--script <path>`
  names it explicitly, and otherwise a `<blob-stem>.script` file sitting
  next to the blob is used.
- **G3.** The script guides; it never constrains. After a step is
  applied, every ordinary key still does what it always did, and the
  session is free to wander off the script and come back.
- **G4.** Stepping backward lands on exactly the view that stepping
  forward produced, without an undo stack.
- **G5.** A script runs headlessly and prints a transcript, so a script
  is testable as a golden file rather than by watching it.
- **G6.** `grpconf/anomalies.script` delivers the README's narrative.

## Non-goals

- **N1. No undo stack, and no capture/restore of session state.** G4 is
  met by making a step's view a function of the script and the document
  alone (S6), not by recording what the view was before it.
- **N2. No control flow.** A script is a list. No conditionals, loops,
  variables, expressions or includes. If a script needs to branch, the
  presenter branches.
- **N3. A step never applies an override itself.** It may prefill the
  `:override` command line and stop (S11). Driving the override pane is
  the demonstration, not the plumbing.
- **N4. No authoring UI.** Scripts are written in a text editor.
- **N5. No timers.** No auto-advance, no per-step duration, no
  animation. Every transition is a keypress.
- **N6. The fold vocabulary is deliberately small** (S9) — three forms,
  no more — and is expected to grow once `anomalies.script` shows what
  is actually missing. Specifying a rich fold language before writing a
  real script would be guessing.

## Specification

### S1 — Discovery and CLI

- `--script <path>` loads that file. A missing or malformed file is a
  hard error at startup.
- With no `--script`, protolens looks for the blob path with its
  extension replaced by `.script` (`grpconf/anomalies.pb` →
  `grpconf/anomalies.script`). Absent, the session is an ordinary one;
  present, it is loaded. A malformed implicit script is still a hard
  error — silently ignoring it would be indistinguishable from not
  finding it.
- `--no-script` suppresses implicit discovery. It conflicts with
  `--script`.
- `--script-height <rows>` overrides the computed pane height (S4).
- Nothing is printed to announce a script. The pane's presence is the
  announcement.

### S2 — File format

YAML, parsed with `serde_norway` (already protolens's YAML parser, for
override collections):

```yaml
title: A field is bytes, and bytes are a guess
steps:
  - text: |
      Every field on the wire is a tag and a payload. The tag says
      which field and what shape; it does not say what the payload
      means.
    node: /1
    wire-line: /1

  - text: |
      Field 4 is a nested message — or a string that happens to parse.
      Nothing in the bytes distinguishes the two.
    fold: all
    unfold: [/4]
    node: /4/2
    wire-lines:
      from: /4/2/1
      to: /4/2/3

  - text: |
      Read it as the schema says, and the same bytes say something
      else. Press Enter to apply.
    node: /7
    override: "override /7 --as google.protobuf.Any"
```

- `title` is optional and unused for now beyond being carried in the
  transcript.
- `steps` is required and non-empty.
- An unknown key is an error, not a warning: a typo in a directive that
  silently does nothing is the failure mode this format most needs to
  avoid.

### S3 — Position

Every directive that names a place in the document takes a *position*,
always a plain scalar. **If it looks like a positional path, it is
one; otherwise it is a search string.**

- A **positional path** is `/`, or `/` followed by one or more decimal
  1-based child numbers separated by `/` — the same notation
  `resolve_path` (`tui/command_line.rs:948`) already accepts for
  `:override` and batch `export`. `/4/2` is the root's 4th child's 2nd
  child.
- Anything else is a **search string**, matched from the top of the
  document by the same matcher `/` uses, first match wins. This
  includes a scalar that begins with `/` but is not a well-formed path
  (`/name`, `/4/x`, `/4/`), so no escape syntax is needed and there is
  no `{search: ...}` wrapper.

The classification is syntactic and total, so it is decided once at
load. A search position is *resolved* when the step is applied, not when
the script is loaded, so it follows the document through an override.

### S4 — The pane

A new pane, terminal width, at the very top, above everything the
session has today. It joins the top-level vertical split in
`tui/render.rs:1142`, ahead of the existing content and command rows:

```
[ Length(script_rows), Length(1), Min(0), Length(1) ]
   script pane          separator   content  command row
```

Height is `25%` of the terminal's rows, clamped to `3 ..= 12`, and the
separator is one row on top of that. Below 3 rows there is no room for
a sentence; above 12 the blob — the thing being explained — stops
dominating the screen. `--script-height` overrides the computation and
is clamped only by the terminal.

The pane shows the current step's `text`, word-wrapped to the pane
width, scrolled by `script_scroll` lines. `script_scroll` resets to 0
on every step change.

### S5 — The separator line

One row, full width, filled with a horizontal rule character, carrying
a micro-help legend. Two states:

- **navigation off:** `press space`.
- **navigation on:** `^←/^→ step 3/23`, then `^↑/^↓ scroll`, then
  `space off`, each dropped from the right as the terminal narrows.
  The step counter is the last thing to go — on a narrow terminal it is
  the only thing worth the space.

### S6 — A step declares a view, not a change

Applying a step performs, in order:

1. **Reset.** Clear the user fold set (`App::folded`), clear the wire
   span, clear the selection, and dismiss any command line the previous
   step prefilled.
2. **Fold** (S9), then **unfold** — unfolding a position also unfolds
   its ancestors, so a named node is always reachable.
3. **Cursor.** Resolve `node`, unfold its ancestors, set the cursor,
   scroll it into view.
4. **Wire** (S10).
5. **Override prefill** (S11).
6. **Text.** Render `text`, scrolled to the top.

Every one of those is derived from the step and the current document.
Nothing is remembered from the step before. That is what makes G4 true
without an undo stack: `Ctrl-Left` re-applies step *n−1* from scratch
and gets the same view it got the first time, whatever the presenter
did in between.

The consequence is deliberate and worth stating: a fold the presenter
opened by hand between steps is discarded at the next step. Wandering
is free; the next step snaps back.

### S7 — Keys

The script keys are checked in `handle_key` (`tui/key_dispatch.rs:437`)
in the tier that already holds `F1`, `:` and `v` — after the `help_open`
and `command_buffer` early returns, before the `override_focus` and
`manage_focus` dispatches. That placement is what makes them
focus-independent while still leaving `space` as a literal space
character at the `:` prompt.

- **`space`, no modifiers, whenever a script is loaded:** toggles script
  navigation. Unconditional — it is the toggle, so it cannot be
  conditional on the state it toggles. Turning it *on* re-applies the
  current step, so the gesture that puts the session back under the
  script also puts the view back where the script left it: wandering off
  between steps is free (G3), and coming back is one key.
- **while navigation is on:** `Ctrl-Left` previous step, `Ctrl-Right`
  next step, `Ctrl-Up`/`Ctrl-Down` scroll the script pane by one line.
- **while navigation is off:** the Ctrl-arrows are not touched.

`Ctrl-Left`/`Ctrl-Right` at the ends of the script are a no-op with a
message; they do not wrap.

What this costs, and what covers it:

| Displaced | Where | Survives as |
| --- | --- | --- |
| page down | `space`, `:733` | `f`, `PageDown` |
| page up | `Shift-Space`, `:730` | `b`, `PageUp` (`Shift-Space` also still works) |
| sibling skip | main `Ctrl-Up`/`Down`, `:693`/`:696` | `Ctrl-k`/`Ctrl-j`, `:583`/`:584` |
| fold/unfold all siblings | main `Ctrl-Left`/`Right`, `:740`/`:743` | `Ctrl-h`/`Ctrl-l`, `:581`/`:582` |
| override-pane horizontal pan | `Ctrl-Left`/`Right`, `:156`/`:159` | `Alt-Left`/`Alt-Right`, `:148`/`:149` |
| override-pane vertical pan | `Ctrl-Up`/`Down`, `:168`/`:171` | **nothing** |

The last row is the only real loss, so this spec adds `Alt-Up`/
`Alt-Down` as a second spelling of the override pane's vertical pan,
mirroring the horizontal pair that already has one.

### S8 — On load

The pane is visible immediately, showing step 1, with navigation off and
the separator reading `press space`. Step 1 is applied unconditionally
at startup — which is cheap, because a well-written step 1 sets the
scene in prose and touches little else.

### S9 — Fold directives

- `fold: all` — fold every node that has children.
- `fold: none` — the default; equivalent to omitting the key.
- `fold: [<position>, ...]` — fold exactly these.
- `unfold: [<position>, ...]` — applied after `fold`, each unfolding its
  ancestors too.

Nothing else, for now (N6).

### S10 — Wire directives

Three forms, mapping onto spec 0268's two gestures:

- `wire-line: <position>` — the `w` gesture: the node's own line shows
  its bytes.
- `wire-lines: { from: <position>, to: <position> }` — the `w` gesture
  over a range. The nested form exists so a search position containing
  a dash is not mistaken for a range separator.
- `wire-node: <position>` — the `W` gesture: the whole subtree.

At most one per step; two is an error at load.

These go through `App::set_wire_span` (`tui/navigation.rs:330`), which
is a *toggle* — it clears the span when the probe row already shows its
bytes. A step must not call it blindly. Since S6 step 1 has already
cleared the wire span, the toggle is always in its off state by the
time the wire directive runs, but the applier states that dependency
rather than relying on it silently.

### S11 — Override prefill

`override: "<command text>"` opens the command line with that text
already typed and the cursor at the end, and stops. Enter runs it; Esc
abandons it. The text is not validated at load — it is command-line
text, and the command line is what reports its errors.

This is the whole of a script's interaction with overrides (N3).

### S12 — A step waits for the lines it names

Under spec 0257 a session opens with a screenful rendered and the rest
baked afterwards, so a step naming a deep node can resolve to a node
whose lines do not exist yet. Before applying a step, the nodes it names
are baked first — the same order `:export` uses under spec 0261: bake,
then act. A step never races the bake.

The arena itself is complete at open (spec 0216), so position resolution
never has to wait; only the line-bearing directives do.

### S13 — A broken directive does not stop the talk

An unresolvable position, or a `to` that precedes its `from`, is
reported on the command/message row and the rest of the step is applied
anyway — the `text` in particular always appears. A script that has
drifted out of sync with its blob degrades into a slide deck, which is
recoverable in front of an audience; a hard stop is not.

Load-time errors (S2, S10) remain hard, because they are fixable before
anyone is watching.

### S14 — Transcript

A new batch subcommand (spec 0123):

```
protolens --descriptor-set … --type … [--script <path>] <blob> script [-o <file>]
```

Which script it walks is S1's, so the subcommand takes no script
argument of its own. It applies every step in order and writes, per step: the step index, the
resolved cursor path, the number of folded nodes, the wire rows as a row
range, the prefilled command if any, any S13 diagnostics, and the first
line of `text`. No terminal is required.

This is the test vehicle for G5, and it is what keeps a script honest as
the blob and the renderer change.

### S15 — Colors

Green, dim for the pane background, brighter for the separator rule and
its legend. The entries are added to the existing theme struct with dark
and light variants and resolved the way every other color already is —
no literal color anywhere outside `theme.rs`.

## Alternatives considered

### Record the session state before each step and restore it

An undo stack. Rejected: the state that would have to be captured is
the fold set, the wire span, the selection, the viewport anchor, the
cursor, and the override collection — and it grows every time protolens
grows a mode. A declared step is a function of the script and the
document, which is both smaller and exactly reproducible.

### A step drives the override pane

The pane could be opened, a candidate highlighted and confirmed, all
from the script. Rejected: the override pane *is* the demonstration.
Automating it hides the part worth showing. Prefilling `:override` and
leaving the Enter to the presenter keeps the gesture visible and keeps
the script out of the pane's internals.

### Leave the existing keys alone and pick free chords

The keys this spec takes are all bound. But the chords still free are
the ones nobody can find under stage lighting. `space` for
forward/toggle is the pager idiom, and Ctrl-arrows for step/scroll are
what a slide deck trains people to expect. The displaced bindings all
have letter spellings already (S7), except one, which this spec gives
one.

### Record a keystroke macro instead of a declarative script

Smaller to implement, and it needs no position language. Rejected: a
macro is a path, not a destination. It breaks the moment a fold default,
a line count or an auto-override changes, and it cannot be read or
edited by the person giving the talk.

### Put the script inside the blob file

`anomalies.pb` is already `#@ prototext` text, so a comment block could
carry the narrative. Rejected: it would only work for text-form blobs,
and the point is to script a session over any blob, including binary
ones.

## Test plan

1. `script_is_found_next_to_the_blob` — `<stem>.script` is picked up;
   `--no-script` suppresses it; `--script` overrides it; a missing
   explicit script is an error.
2. `an_unknown_directive_is_a_load_error` — a misspelled key fails at
   load rather than being ignored.
3. `a_step_is_a_function_of_the_script` — apply step 1, 2, 3, then hand
   the session an arbitrary sequence of ordinary keys, then step back to
   2: the view equals the view step 2 produced the first time. This is
   G4, and it is the test that fails if a directive is ever made to
   inherit from the previous step.
4. `space_toggles_and_ctrl_arrows_are_conditional` — with navigation
   off, `Ctrl-Up` still skips siblings; with it on, it scrolls the
   pane; `space` toggles in both states, and `f` pages down in both.
5. `a_step_waits_for_its_lines` — with the bake artificially stalled, a
   step naming a deep node produces its wire rows rather than an empty
   span (S12).
6. `a_broken_position_still_shows_its_text` — S13.
7. `anomalies_script_walks_without_a_broken_position` — S14's transcript
   over `grpconf/anomalies.script`, asserting that it reports no S13
   diagnostic and reaches its last step. This is the smoke test the
   script exists to be. Deliberately not a golden file of the whole
   transcript: the row ranges in it move with any change to how a line is
   rendered, which would make it a test of the renderer rather than of
   the script.
8. `reuse lint` passes.

## Measured outcome

Implemented 2026-08-10.

`protolens/src/script.rs` is the format and `protolens/src/tui/script_pane.rs`
the applier, split along the line the spec draws: nothing in the former
mentions `App`.

`grpconf/anomalies.script` walks the README's seven sections in 20 steps,
and every position in it resolves. Two things the walk needed that the
spec did not anticipate:

- `App::set_wire_span` is a *gesture* handler, and a gesture aimed at a
  row that already shows its bytes means "off" — so a step, which
  declares a state, cannot use it. `show_wire_span` clears and then sets,
  which makes the outcome a function of the span alone.
- `App::resolve_path` cannot reach into unbaked territory at all:
  `child_slots` reports no children for a node whose first child slot was
  never rendered. S12 is therefore not a wait but a descent — each level
  is expanded before its children are counted, which is sound because a
  splice renumbers no slot (spec 0216 S12).

`default.nix`'s `fixtureFilter` gained `.script`, without which the
transcript test cannot see the walk it runs.
