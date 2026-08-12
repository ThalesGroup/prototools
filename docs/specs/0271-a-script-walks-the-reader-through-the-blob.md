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
      docs/specs/0199-the-arrow-keys-fold-before-they-leave-the-node.md
      (the bare-arrow bindings this displaces while navigation is on),
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

**Amended 2026-08-10: the pane is zero rows while navigation is off.**
Only the commentary collapses; the separator stays at one row (S5).
Commentary the reader has stepped out of is a quarter of the screen
spent on a paragraph about wherever the script last was, which is not
where the reader now is — and stepping out is precisely the gesture for
going somewhere else. The two heights are therefore independent, and
the layout asks for them separately: deriving the separator's from the
pane's, as it did until this amendment, deleted the one row that has to
survive. `space` brings the text back.

A consequence, and the reason S9's anchoring matters here: toggling
moves the content pane's top edge by 3 to 12 rows mid-session. Which
line stays put is spec 0259's answer — the node owning the pane's top
row — not a special case for this pane.

### S5 — The separator line

One row, full width, filled with a horizontal rule character, carrying
a micro-help legend **flushed to the right edge**, with two rule
characters of run-out past it so it reads as sitting on the rule rather
than ending it. Two states:

- **navigation off:** `space to enter script navigation`, falling back
  to `space to enter`.
- **navigation on:** `^←/^→ step 3/23`, then `^↑/^↓ scroll`, then
  `space to quit script navigation`, falling back to `space to quit`,
  then dropped from the right as the terminal narrows. The step counter
  is the last thing to go — on a narrow terminal it is the only thing
  worth the space.

Amended 2026-08-10. Both details are corrections. The legend was first
written left-aligned and it began in the same column the document's
first line begins in one row below, in a green close to the document's
own palette; at a glance it read as a line of the blob, which is the one
confusion the separator exists to prevent. And a bare `press space`
names a key without naming what pressing it does, so the legend is where
that gesture gets its name — in the word this spec uses for it,
*navigation*, rather than "mode", which protolens spends elsewhere.

The toggle has two spellings rather than being dropped whole, which is
what the other two parts do. It is the part with no other source: the
step counter and the scroll keys describe a pane the reader can see,
while nothing else on screen says that `space` is the way in or out. On
one ladder of whole parts the hint was gone from 67 columns down to 35 —
an ordinary half-screen terminal — and shortening it before dropping it
costs one rung. Widths include the six columns of rule `fits` charges
around a legend:

| legend | columns |
|---|---|
| `^←/^→ step 3/23  ^↑/^↓ scroll  space to quit script navigation` | 68 |
| `^←/^→ step 3/23  ^↑/^↓ scroll  space to quit` | 50 |
| `^←/^→ step 3/23  ^↑/^↓ scroll` | 35 |
| `^←/^→ step 3/23` | 21 |
| `space to enter script navigation` | 38 |
| `space to enter` | 20 |

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

**Amended 2026-08-12 by spec 0279 S5: step 3 places the node, it does
not merely bring it on screen.** "Scroll it into view" was
`clamp_scroll_to_cursor`, the *reader's* rule — it moves only far
enough to make a row visible, so it lands the node on the pane's last
row whenever the previous step was above it, and the subtree the step
is about ends up entirely off the bottom. A step declares a view (N1),
so it now scrolls to the top of the highest ancestor of the node whose
subtree still fits the pane, and falls back to the node's own row when
none does. Both halves of a split step — the anomalous line and its
canonical twin, which share a parent — therefore get the same view, and
the comparison the split exists for reads without a keypress.

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
- **while navigation is on:** `Left` previous step, `Right` next step,
  `Up`/`Down` scroll the script pane by one line.
- **while navigation is off:** the arrows are not touched.

`Left`/`Right` at the ends of the script are a no-op with a message;
they do not wrap.

**Amended 2026-08-12: `Up`/`Down` stop at both ends of the step's own
text.** A step is a paragraph, not a document — scrolling past either
end shows blank rows and loses the only thing the pane is for, and the
ends of a step are exactly where `Left`/`Right` take over. The floor is
0 and the ceiling is the step's wrapped height less the pane's, which is
0 whenever the step already fits.

That height comes from `Paragraph::line_count` on the paragraph the pane
will actually draw, which is why the pane and the bound now share one
`script_paragraph` constructor and why `protolens` enables ratatui's
`unstable-rendered-line-info`. A word-wrapper of our own would be a
second answer to a question the widget already answers, and the two
would disagree the moment either changed. The width to wrap at is the
pane's, so `render_script_pane` records its `Rect` in `App::script_area`
before its early return — the same way `help_area`, `cmd_area` and
`main_area` are recorded — and the clamp is applied on the keypress
rather than a frame later.

**Amended 2026-08-12: the bare arrows, not the Ctrl-arrows.** Handing
navigation to the pane should hand it *navigation* — the keys a reader
already reaches for — and a walkthrough is read with the same four keys
as a slide deck. A modifier on an arrow is never the script's, on or
off, so the selection (`Shift`), the pan and the word motion (`Alt`) and
the sibling-level fold (`Ctrl`) all still reach the main pane while a
step is on screen. The table below is therefore a table of what a *bare*
arrow displaces, and every entry in it is displaced only while
navigation is on.

What this costs, and what covers it:

| Displaced | Where | Survives as |
| --- | --- | --- |
| page down | `space`, `:733` | `f`, `PageDown` |
| page up | `Shift-Space`, `:730` | `b`, `PageUp` (`Shift-Space` also still works) |
| caret motion | main `Left`/`Right`/`Up`/`Down` | `h`/`l`/`k`/`j` |
| override-pane highlight | `Up`/`Down`, `:184`/`:185` | `j`/`k` |
| manage-pane highlight | `Up`/`Down` | `j`/`k` |

Every displaced gesture has a letter spelling that is not displaced, so
nothing is lost. The 2026-08-12 amendment above also *returns* the four
Ctrl-arrow gestures the first draft had taken — the sibling skip, the
sibling-level fold and the override pane's two pans — which is why this
spec's `Alt-Up`/`Alt-Down` addition to the override pane is no longer
load-bearing.

### S8 — On load

The pane is visible immediately, showing step 1, with navigation **on**.
Step 1 is applied unconditionally at startup — which is cheap, because a
well-written step 1 sets the scene in prose and touches little else.

Amended 2026-08-10: navigation used to start off, on the grounds that
the pane arrives unasked for and a session should not silently rebind
four keys. That was written when a script could plausibly be picked up
from beside the blob; it is not how this shipped. The pane is on screen
only because `--script` named a file, and a reader who asked for a
walkthrough should not have to find a key before the walkthrough answers
to anything. Off remains one keystroke away, and is now also visible in
the layout — S4's zero-row pane.

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

The background is the rule's **own hue and saturation**, taken to
lightness 0.22 (dark) or 0.88 (light): the pane and the rule that
borders it have to read as one region. It began at half that lightness,
below the document row's own, on the argument that the blob must
dominate. On a projector it did not read as green at all, and a
commentary pane whose edges nobody can see is not a pane. The document
is a whole screen and the pane is at most twelve rows, so what dominates
here is decided by area, not by tint.

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
forward/toggle is the pager idiom, and the bare arrows for step/scroll
are what a slide deck trains people to expect — and, as of the
2026-08-12 amendment to S7, what "hand navigation to the pane" ought to
mean literally. The displaced bindings all have letter spellings
already (S7).

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
8. `scrolling_the_pane_stops_at_the_steps_own_text` — the 2026-08-12
   amendment: `Up` at the top is a no-op, `Down` stops one paneful short
   of the step's end, and halving the pane's width moves that stop,
   since the bound is the *wrapped* height and not the line count.
9. `reuse lint` passes.

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
