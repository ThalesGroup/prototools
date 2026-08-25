<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0355 — script navigation keybindings overhaul

Status: implemented
Implemented in: 2026-08-25
App: protolens
Refs: docs/specs/0271-a-script-walks-the-reader-through-the-blob.md (the
      script pane this spec revises)

## Background

The current script navigation keys — `,`/`;` to step and `?`/`.` to
scroll — were chosen (spec 0271 S7, 2026-08-14 amendment) to avoid the
arrows, which a presenter reaches for by reflex and must not accidentally
trigger a step change with. The result is correct but not comfortable:
the four punctuation keys are non-obvious and spread across the keyboard.
`space` is spent on toggling navigation on/off, which wastes the most
natural "advance" key in every pager and slide tool.

The separator legend ("`,/; step 3/23  ?/. scroll  space to quit script
navigation`") is long enough that it vanishes below 35 columns, and even
at full width it puts the step counter — the most useful thing — at the
left of a dense string that reads like line noise.

A presenter giving a live talk wants two things: **advance** and, rarely,
**go back**. Everything else is a distraction.

## Goals

- **G1.** `space` advances: scrolls the current step's text down if it
  is not yet at the bottom, then moves to the next step.
- **G2.** `Backspace` goes back: scrolls the current step's text up if
  it is not yet at the top, then moves to the previous step.
- **G3.** `Tab` toggles script navigation on/off, replacing `space`.
- **G4.** With script-pane focus: `PageDown` behaves as `space`;
  `PageUp` behaves as `Backspace`.
- **G5.** Clicking the script pane gives it key focus. While it has
  focus, `PageDown`/`PageUp` fire G4. Losing focus (any click elsewhere,
  or `Tab`) returns `PageDown`/`PageUp` to their previous document
  bindings.
- **G6.** Mouse wheel over the script pane scrolls its text, clamped to
  the step's own text the same way `?`/`.` were.
- **G7.** The separator legend is revised to reflect the new keys and
  shortened to keep the step counter visible at narrower widths.
- **G8.** `,`/`;`/`?`/`.` are retired as script-navigation keys.

## Non-goals

- **N1.** No modifier+space bindings. Terminals vary too widely in
  whether they deliver `Shift-Space`, `Ctrl-Space`, or `Alt-Space`
  distinctly from plain space; relying on any of them is fragile.
- **N2.** No double-click to advance. A double-click is awkward to
  perform reliably in a live demo, and a single-click already serves a
  distinct purpose (focus).
- **N3.** No change to the script file format or the `script` transcript
  subcommand.

## Specification

### S1 — `space`: advance

When a script is loaded and navigation is on, `space` (no modifiers):

1. If the step's text is not scrolled to its bottom, scroll down one
   pane-height of lines (same quantum as the old `.` key). Stop.
2. Otherwise, advance to the next step. At the last step, no-op with a
   status message.

When navigation is off, `space` is untouched — it remains `page-down` in
the document (`tui/key_dispatch.rs`) as before.

This matches the behaviour of every pager (`less`, `more`), PDF viewer,
and slide tool: `space` always means "give me more of what I'm looking
at, and when there is no more, move on."

### S2 — `Backspace`: go back

When a script is loaded and navigation is on, `Backspace` (no modifiers):

1. If the step's text is not scrolled to its top (`script_scroll > 0`),
   scroll up one pane-height. Stop.
2. Otherwise, go to the previous step. At the first step, no-op with a
   status message.

When navigation is off, `Backspace` is untouched.

### S3 — `Tab`: toggle navigation

`Tab` (no modifiers) replaces `space` as the toggle. Same semantics as
the old `space` toggle: turning navigation on re-applies the current
step, so the view snaps back to where the script left it.

`Tab` in the document pane was previously unbound. At the `:` command
prompt, `Tab` triggers completion (spec 0152) — the script block runs
in the tier above the command-buffer early-return, so Tab at the prompt
is still completion, not a toggle.

### S4 — Script-pane focus and `PageDown`/`PageUp`

**Focus.** The script pane gains a boolean `script_focus` in `App`.
Clicking anywhere in the script pane sets `script_focus = true` and
clears no other state. Any click outside the script pane, and `Tab`,
clear it.

**While `script_focus` is true and navigation is on:**
- `PageDown` → same as `space` (S1).
- `PageUp` → same as `Backspace` (S2).

While `script_focus` is false, `PageDown`/`PageUp` keep their existing
document bindings (`f`/`b` equivalents).

`script_focus` does not affect which pane receives ordinary key events
— the main pane, the manage pane and the override pane keep their own
focus model unchanged. It is a modifier on `PageDown`/`PageUp` only.

### S5 — Mouse wheel on the script pane

A scroll-up or scroll-down event whose column/row falls inside the script
pane's area (`App::script_area`) scrolls the step's text by one line,
clamped at both ends exactly as the old `?`/`.` keys were (spec 0271 S7,
2026-08-12 amendment). The document is not scrolled.

### S6 — Retired keys

`,`, `;`, `?`, `.` are no longer script-navigation keys. They revert to
their pre-0271 bindings:
- `,`/`;` were unbound; they remain unbound.
- `?` was backward search; it is again, unconditionally (the guard
  `!ctrl_or_alt(&key)` is no longer needed for the script block and can
  be removed from it).
- `.` was unbound; it remains unbound.

### S7 — Separator legend

The separator carries the step counter and a minimal hint. New text,
same right-aligned placement and same terminal-width cascade as before:

| legend | columns |
|---|---|
| `Tab to pause  space/Backspace step  step 3/23` | 49 |
| `space/Backspace step  step 3/23` | 34 |
| `step 3/23` | 11 |

Navigation-off legend:

| legend | columns |
|---|---|
| `Tab to resume script navigation` | 35 |
| `Tab to resume` | 15 |

The step counter is the last element to be dropped. "Tab to pause/resume"
names the toggle in the vocabulary the hint already uses; "step N/M" is
the counter the presenter reads out loud and the one thing a narrow
terminal must keep.

## Alternatives considered

### Keep `,`/`;` and add `space` as an alias for `;`

Rejected: two ways to advance with different scroll semantics (`;`
skips directly; `space` scrolls first) would confuse both the presenter
and the separator legend. One key, one behaviour.

### Use `Shift-Space` / `Ctrl-Space` / `Alt-Space` for go-back

Rejected by N1: terminal delivery of modifier+space is unreliable across
xterm, tmux, WezTerm, and kitty. `Backspace` is universally available
and carries strong "undo the last advance" semantics.

### Give the script pane full focus (absorb all keys while focused)

Rejected: a presenter clicking the script pane to scroll it should not
lose the ability to fold a node or move the caret. Focus here is only a
modifier on two keys; it does not reroute the whole keyboard.

### Double-click to advance

Rejected by N2: a double-click demands precise timing in a live setting.
A misfire advances two steps. `space` is faster and more reliable.

## Test plan

1. `space_scrolls_before_advancing` — with a step whose text overflows
   the pane, `space` scrolls down first and advances only once the
   bottom is reached.
2. `backspace_scrolls_before_retreating` — symmetric: `Backspace` scrolls
   up first and retreats only once the top is reached.
3. `tab_toggles_navigation` — `Tab` turns navigation on and off;
   turning it on re-applies the current step.
4. `pagedown_pageup_with_script_focus` — clicking the script pane then
   pressing `PageDown`/`PageUp` fires advance/retreat; clicking the main
   pane returns `PageDown`/`PageUp` to document scrolling.
5. `wheel_scrolls_script_text` — a scroll event inside `script_area`
   moves `script_scroll` and is clamped at both ends; one outside does
   not.
6. `retired_keys_do_not_navigate` — with navigation on, `,`, `;`, `.`
   do not change the step; `?` opens a backward search.
7. `space_is_page_down_when_navigation_is_off` — `space` with navigation
   off scrolls the document.
8. `reuse lint` passes.

## Measured outcome

Implemented 2026-08-25. All 1249 tests pass, `reuse lint` clean.

- `Tab` toggles navigation on/off; `space` scrolls then advances; `Backspace`
  scrolls then retreats.
- `PageDown`/`PageUp` fire advance/retreat while `script_focus` is true.
- Mouse wheel over the script pane scrolls its text by one line.
- Left click on the script pane sets `script_focus = true`; click elsewhere
  clears it; `Tab` also clears it.
- `,`, `;`, `?`, `.` reverted to their pre-0271 bindings.
- Separator legend updated to `Tab to pause  space/Backspace step  step N/M`
  with the three-rung cascade from S7.
