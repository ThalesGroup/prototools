<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens input review — keys and mouse, every pane and mode

*last verified: 2026-08-12*

A review of protolens's whole input surface: what its logic is, where it
is internally consistent, where it is not, and what could be changed.
Read the executive summary for the verdict; the sections after it carry
the evidence and the proposals.

Sources read in full: `protolens/src/tui/key_dispatch.rs`,
`mouse.rs`, `help_text.rs`, `command_line.rs`'s `handle_command_key`,
`manage_pane.rs`'s `handle_manage_key`/`handle_manage_mouse`,
`script_pane.rs`, `src/tui/tests/help_text.rs`.

## Executive summary

The input system is **better designed than most TUIs of its size**, and
its coherence is not accidental — it comes from four mechanisms that are
each written down once and then relied on everywhere:

1. a single **dispatch ladder** in `handle_key`, ordered from
   most-global to most-local, with early returns;
2. a per-pane **modifier gate** (`ctrl_or_alt`) that stops plain-`Char`
   arms from answering for chords;
3. a single **selection-clearing predicate** (`keeps_the_selection`)
   instead of a `clear_selection()` call in fifty arms;
4. a **shared motion vocabulary** (`j`/`k`, `Ctrl-n`/`Ctrl-p`, `f`/`b`,
   `PageUp`/`PageDown`, `Home`/`End`/`G`, `gg`, `/`/`?`/`n`/`N`,
   `F`/`B`) that every scrollable surface implements identically.

The system's weaknesses are not in the dispatcher. They are:

- **the help text has drifted** in four specific places, and the test
  that is supposed to prevent this can only prove a key is *mentioned*,
  not that what is written about it is true (§7);
- **`Tab` does not mean one thing** — it is completion, focus toggle,
  focus-lock complaint, and unbound, depending on where you are (§5.1);
- **`Esc` is overloaded four ways** and its meaning at a find prompt is
  "accept", which is the opposite of its meaning everywhere else (§5.2);
- **the modifier algebra has one genuine collision**: `Alt` means "pan
  the main pane" *and* "move by word", and both are live at once (§5.3);
- **four letters (`a`, `i`, `o`, `z`) mean different things in
  different panes** with no mnemonic linking the meanings (§5.4);
- there is **no way to quit that is a key** (§5.5);
- two arms in the override pane are **provably unreachable** (§8).

Fourteen proposals follow, ranked, in §9.

---

## 1. The logic of the system, stated

### 1.1 The dispatch ladder

`handle_key` (`key_dispatch.rs:456`) is a ladder of early returns, from
the most global concern to the most local. In order:

| # | Tier | Why it is at this height |
|---|------|--------------------------|
| 0 | dismiss splash; `message.clear()`; `search_echo = None` | side effects of *any* key, so they precede every branch |
| 1 | `Ctrl-Z` suspend | a process-level concern, not an app one |
| 2 | `F1` open help | must work from anywhere, including a locked pane |
| 3 | help overlay open → `handle_help_key` | a modal owns the keyboard |
| 4 | command buffer open → `handle_command_key` | ditto — and this is what keeps `space` a literal space at the `:` prompt |
| 5 | `:` open command line | reachable from every pane, so commands never need a focus dance |
| 6 | `v` jump to definition | same reason |
| 7 | script keys (`space`, and bare arrows while active) | above the side panes so the script drives regardless of focus |
| 8 | `override_focus` → `handle_override_key` | pane focus |
| 9 | `manage_open && manage_focus` → `handle_manage_key` | pane focus |
| 10 | empty-tree guard (`return`) | everything below indexes `self.tree` |
| 11 | `keeps_the_selection` → `clear_selection()` | one rule for the dozens of arms below |
| 12 | `gg` chord | must see the key before the plain-`g` world does |
| 13 | `Char(_) && ctrl_or_alt` → the whole Ctrl/Alt vocabulary | so the unmodified arms below cannot answer for it |
| 14 | `x` export chord | after 13, so `Ctrl-x` cannot arm an export |
| 15 | the main-pane match | the default |

**This ladder is the single best thing about the design.** Every "does
this key work here?" question has one answer, obtained by reading top to
bottom, and the tiers are commented with the reason each sits where it
does rather than what it does.

### 1.2 The two routing rules

There are exactly two, and they differ by device:

- **Keyboard routes by focus.** `override_focus`, then
  `manage_focus`, then the main pane. A tier-2-to-7 key ignores focus
  entirely; everything else obeys it.
- **Mouse routes by hover geometry.** `over_side` / `over_main` /
  `over_cmd` are hit tests, and the wheel pans *whatever the pointer is
  over* without moving focus (`mouse.rs`). A click *may* claim focus;
  a scroll never does.

Splitting these is right and is the idiomatic modern choice (it is what
every GUI does and what `less`, `htop`, and most vim setups do). It is
also undocumented as a principle anywhere — the help mentions
"whichever pane … the mouse is hovering" only under Shift-wheel.

### 1.3 The modifier algebra

Read across the whole app, the intended algebra is:

| Modifier | Meaning |
|----------|---------|
| none | move the caret / the highlight |
| `Shift` | **extend the selection** (main pane), or **escalate** an action (`A`, `Shift-Space` cascade; `Shift-Up/Down` move-and-activate) |
| `Ctrl` | **widen the scope** of a fold (`Ctrl-h`/`Ctrl-l` = all siblings), or **pan a side pane** |
| `Alt` | **pan the main pane**, or **move by word** |
| `Shift-Alt` | pan the main pane *horizontally* |

Three of these five rows are clean. The `Alt` row is the collision
discussed in §5.3, and the `Shift` row means two unrelated things.

### 1.4 The three chord machines

- `GChord` / `take_g_chord` — `gg` → home. Shared verbatim by the main
  pane, the override pane and the manage pane.
- `ExportChord::{None, Leader, Descriptor}` — `x` → `xb`/`xp`/`xd` →
  `xdb`/`xdp`. Main pane only.
- The find prompt (`F`/`B`) — not a chord but a *sticky* mode: the
  prompt stays open and `Enter` steps.

All three are correct in that a modified key cannot arm them and the
Ctrl/Alt tier explicitly disarms `pending_x`.

### 1.5 What the system is *not*

It is not modal in the vim sense. There is no normal/insert
distinction; there are **modal overlays** (help, command line, find
prompt) and **focusable panes** (override, manage), and those are
different things. Calling a pane a mode is the mistake the help text
makes in a couple of places.

---

## 2. Pane-by-pane inventory

### 2.1 Main pane

Motion `h`/`l`/`j`/`k` + arrows + `Ctrl-b`/`Ctrl-f`/`Ctrl-n`/`Ctrl-p`;
word motion `Alt-Left`/`Alt-Right`/`Alt-h`/`Alt-l`/`Alt-b`/`Alt-f`;
line ends `0`/`^`/`Ctrl-a` and `$`/`Ctrl-e`; `%` brace match;
`Home`/`gg`, `End`/`G`; paging `Space`/`f`/`PageDown` and
`Shift-Space`/`b`/`PageUp`; sibling motion `Ctrl-j`/`Ctrl-k`/
`Ctrl-Up`/`Ctrl-Down`; pan `Alt-Up`/`Alt-Down`/`Shift-Alt-Up`/
`Shift-Alt-Down`; selection `H`/`L`/`J`/`K` + `Shift`-arrows, copy
`Ctrl-c`; folds `z`/`Z`/`Ctrl-h`/`Ctrl-l`/`Ctrl-Left`/`Ctrl-Right`;
display `a` (annotations), `w` (wire span), `W` (wire subtree), `i`
(heat cues); history `Ctrl-o`/`Ctrl-i`; search `/`/`?`/`n`/`N`/`F`/`B`;
panes `t` (override), `o` (manage), `Tab` (focus manage, *only if
open*); `Enter` (smart proxy); `Esc` (three arms); export chord `x`.

### 2.2 Override pane (focus-locking)

`gg`; `Ctrl-n`/`Ctrl-p`; `Tab` → a message saying focus is locked; `Esc`
closes; `Ctrl-Left`/`Ctrl-Right` h-pan; `Ctrl-Up`/`Ctrl-Down` v-pan;
`Alt-Up`/`Alt-Down` (dead, §8); `j`/`k`/arrows; `f`/`b`/`PageUp`/
`PageDown`; `Home`/`End`/`G`; `i` sort; `/`/`?`/`n`/`N`/`F`/`B`; `o`
prefills `:override`; `Enter` applies and closes.

### 2.3 Manage pane (focus-toggling)

Everything the override pane has, minus `i`, plus: `Left`/`Right`
circulate the *main-pane* cursor through affected fields;
`a`/`Space` toggle; `A`/`Shift-Space` cascade; `z`/`Z` rotate origin
kind; `D` duplicate; `d`/`Delete`/`Backspace` remove; `s`/`r` prefill
`:save`/`:restore`; `Tab` unfocuses; `Enter` closes.

### 2.4 Command line (modal)

Readline: `Ctrl-b`/`Ctrl-f`, `Alt-b`/`Alt-f`, `Ctrl-a`/`Ctrl-e`,
`Ctrl-d`, `Ctrl-h`, `Ctrl-k`, `Ctrl-v`; `Tab`/`BackTab` completion
(command kind only); `Up`/`Down` search history (search kind only);
`Enter` commits or steps a find; `Esc` discards or *accepts* a find.

### 2.5 Help overlay (modal)

`Esc`/`F1` close; `j`/`k`/arrows; `f`/`b`/`PageUp`/`PageDown` (±10);
`Ctrl-n`/`Ctrl-p`.

### 2.6 Script pane (a fifth surface, not a pane you focus)

`Space` toggles navigation; while on, the **bare** `Left`/`Right` step
and `Up`/`Down` scroll the pane, from any focus. A modified arrow is
never the script's.

---

## 3. What is idiomatic

Genuinely good, and worth keeping as-is:

- **vim's motion core is intact and correct**: `h`/`j`/`k`/`l`, `gg`/`G`,
  `0`/`^`/`$`, `%`, `z` for folds, `/`/`?`/`n`/`N`, `Ctrl-o`/`Ctrl-i` for
  the jumplist. Someone who knows vim is productive immediately.
- **readline is spelled the way readline spells it**, and — this is the
  part most apps get wrong — *identically* in the main pane and the
  command line. `Ctrl-a` is line-start in both.
- **`less`'s paging** (`f`/`b`/`Space`/`PageUp`/`PageDown`) is present
  alongside vim's, which is the right call: this is a viewer.
- **emacs's `Ctrl-n`/`Ctrl-p`** as universal list motion, which is what
  makes every list in the app feel the same.
- **`Ctrl-click` documented as the alias for `Shift-click`** because
  terminals steal `Shift-click` for native selection. This is real
  terminal knowledge, written down where a user finds it.
- **`Shift-Space` documented as needing the Kitty protocol**, with `b`
  and `A` given as the always-works spellings. Also correct and rare.
- **Double-click as an alias** for `Shift-click` on the manage pane's
  marker — a mouse user never has to learn the escalation modifier.

## 4. What is internally consistent

- The **motion vocabulary** is implemented identically on four surfaces.
  There is no pane where `j` scrolls but `Ctrl-n` does not.
- The **search vocabulary** (`/`, `?`, `n`, `N`, `F`, `B`) is on all
  three list surfaces.
- Every pane's dispatcher opens with the **same three moves**: the `gg`
  chord, then the `ctrl_or_alt` gate, then the pane's own arms. The gate
  is the same function everywhere.
- **`Ctrl`-arrows pan a side pane** in both side panes, same directions.
- **`Esc` closes** in every pane that can close.
- Match arms are ordered **most-modified first** (`Shift-Alt`, then
  `Alt`, then `Shift`, then `Ctrl`, then bare), which is the only order
  that works given crossterm's `Char` reporting.
- **`q` is unbound app-wide** on purpose (spec 0236 S20) and is
  regression-tested in three places.

---

## 5. Where it is inconsistent

### 5.1 `Tab` has four meanings

| Where | What `Tab` does |
|-------|-----------------|
| command line, command kind | complete / cycle |
| command line, search kind | nothing |
| override pane | prints "focus is locked" |
| manage pane | unfocus, back to main |
| main pane, manage open | focus the manage pane |
| main pane, manage closed | **nothing at all** |

Rows 3 and 6 are the problem. A user who has learned "Tab moves focus
between panes" from the manage pane presses `Tab` in the override pane
and gets a *complaint*; presses `Tab` in the main pane with no side pane
open and gets silence. Neither is wrong in isolation; together they mean
`Tab` teaches nothing.

The help text asserts row 3 does the opposite of what it does — see §7.

### 5.2 `Esc` has four meanings, one of which is inverted

`Esc` is: close the help; discard the command line; **accept** a find;
close the override pane; close the manage pane; clear the selection;
and (main pane, nothing else pending) clear the message.

The find-prompt arm is the outlier. Everywhere else in the app and in
essentially every program ever written, `Esc` means *cancel*. At a find
prompt it commits the current match and lands the caret on it (spec
0278). It is a *good* interaction — it is how a find-as-you-type should
work — but it is the wrong key for it. There is no cancel at a find
prompt at all.

### 5.3 `Alt` means two things simultaneously

`Alt-Up`/`Alt-Down` pan the main pane. `Alt-Left`/`Alt-Right` move by
word. These are not variants of one idea; they are two ideas sharing a
modifier along one axis each. A user who learns "Alt pans" will press
`Alt-Left` expecting a horizontal pan and get word motion — and the
horizontal pan is on `Shift-Alt-Up`/`Shift-Alt-Down`, i.e. the
*vertical* arrows.

That last part is the sharpest edge in the whole system: **the
horizontal pan is bound to vertical arrow keys.** It reads as a
historical accident (the horizontal arrows were already taken by word
motion), not a decision.

### 5.4 Four letters are homonyms across panes

| Letter | Main pane | Override pane | Manage pane |
|--------|-----------|---------------|-------------|
| `a` | toggle annotations | — | toggle entry active |
| `i` | toggle heat cues | toggle candidate sort | — |
| `o` | open manage pane | prefill `:override` | prefill `:override` |
| `z` | toggle fold | — | rotate origin kind |

`o` is defensible (all three are "override-ish"). `a` is a stretch
("annotations" vs. "active"). `i` and `z` are pure collisions — `i` is
"inferred sort" in one pane and "heat cues" in another, and `z` is vim's
fold letter in one pane and a rotation in another.

This is only a problem because the panes look alike. Since a side pane
is on screen next to the main pane, the same physical key doing
different things depending on an invisible focus flag is a real
hazard — and unlike a mode line, protolens shows focus only through
highlight styling.

### 5.5 There is no quit key

`:quit` is the only way out (`:q` suffices). `q` is deliberately unbound
so a stray `q` cannot lose a session. Defensible, and it is documented
in the help — but `Ctrl-c` is *taken by copy*, so the two most common
reflexes a user has for leaving a program are one silently-nothing and
one silently-copies. The `Ctrl-Z`-then-`kill` escape hatch is the actual
fallback, which is not a good answer.

### 5.6 Smaller inconsistencies

- **`Enter` in the manage pane closes the pane**; `Enter` in the
  override pane *applies and closes*. Same key, one is a commit and one
  is a dismiss.
- **The manage pane's `Left`/`Right` move the main pane's cursor**, not
  anything in the manage pane. It is the only binding in the app where a
  focused pane's arrows drive a different pane.
- **`f`/`b` page in every pane but the help overlay pages by ±10 lines**
  rather than by a screenful, so the same key has a different stride.
- **`d`/`Delete`/`Backspace` all delete in the manage pane**, but
  `Backspace` is *caret motion* in the main pane. A user who has learned
  main-pane `Backspace` will delete an entry with it.
- **`i` in the main pane hides heat cues** — a *negative* toggle named
  with a letter that suggests "info". `heat_cues_hidden` is the field.

---

## 6. Mouse review

The mouse layer is the smaller and cleaner half. Its logic:

1. `Moved` events are discarded early (no hover state anywhere).
2. Splash / message / echo clearing, same as the keyboard's tier 0.
3. The help overlay eats the wheel while open.
4. Hit tests produce `over_side` / `over_main` / `over_cmd`.
5. `Shift`+wheel and native horizontal scroll pan by `WHEEL_PAN_STEP`.
6. The plain wheel scrolls the hovered pane, **without taking focus**.
7. `main_interactive = over_main && override_target.is_none()` — the
   override pane's focus lock is enforced for the mouse too.
8. `extend_click = shift || ctrl`.
9. `Down(Left)` → extend, or focus + `handle_click` + double-click
   detection + set anchor; or the lock-out message; or a side-pane focus
   claim; or `command_click`.
10. `Drag(Left)` extends. `Up(Left)` selects the line on a double-click,
    else clears if the drag never engaged.

**What is right**: hover-routed scrolling; the fold-marker hit test
adding `pan_offset` back so a panned pane still hits; a two-column
`FOLD_FIELD_WIDTH` target (a one-column glyph is too small a target);
the command row's 1:1 char↔column inversion; `Ctrl` as a click modifier
because terminals eat `Shift`.

**What is missing or asymmetric**:

- **No right-click anywhere.** A context menu is not a TUI idiom, but a
  right-click could very cheaply be "open the override pane for the node
  under the pointer", which is currently a two-step (`click`, then `t`).
- **No middle-click.** X11 users expect middle-click paste on the
  command line; `Ctrl-v` is the only paste.
- **The wheel does not pan the script pane.** Every other surface
  scrolls under the pointer; the script pane scrolls only via `Up`/`Down`
  with navigation on.
- **`Ctrl`+wheel is unbound.** In a viewer this is conventionally zoom;
  protolens has a natural analogue — fold depth — which is currently
  only reachable per-node.
- **No drag in the side panes.** Click selects an entry, but you cannot
  drag to reorder in the manage pane, where `Shift-Up`/`Shift-Down`
  already implement a move.
- **Double-click means two different things**: "select this line" in the
  main pane, "cascade this entry" in the manage pane.

---

## 7. Documentation drift, and the test that did not catch it

`src/tui/tests/help_text.rs` reads the dispatchers' *source text* at
compile time and asserts every `KeyCode::Char('x')` literal is mentioned
in `HELP_TEXT`. That is a genuinely clever check and it caught real gaps
when written. Its stated limitation is exactly the one that has since
bitten:

> This is a weaker check than generation — it says a key is *mentioned*,
> not that what is written about it is true.

Four live drifts, all invisible to the test:

1. **`w` is described as the old whole-document wire mode.**
   `help_text.rs:86` says "toggle wire mode — under every drawn line, a
   second row showing that line's own bytes in hex". Since spec 0268 `w`
   shows a **span**: the caret's line, or the selection. The help is
   describing behavior that no longer exists.
2. **`W` is entirely undocumented.** `key_dispatch.rs:880` binds it to
   `wire_subtree()`. The test passes because `mentioned_as_a_key` is
   **case-insensitive** — the documented `w` covers the undocumented
   `W`. This is a systematic hole: *no* capital-letter binding can ever
   be found missing if its lowercase twin is documented. `Z` (vs `z`),
   `A` (vs `a`), `D`, `J`/`K`, `H`/`L`, `N`, `B`, `G` are all in that
   shadow.
3. **`Tab` in the override pane is documented backwards.**
   `help_text.rs:148` says "Tab move focus between the main pane and the
   override pane (while it is open)". `key_dispatch.rs` answers `Tab`
   there with `OVERRIDE_FOCUS_LOCK_MESSAGE`. `Tab` is not a `Char`, so
   the test never looks at it — **no non-character key is checked at
   all** (`Esc`, `Tab`, `Enter`, arrows, `Home`, `End`, `PageUp`,
   `Delete`, `Backspace`, `F1`).
4. **The override pane's `Alt-Up`/`Alt-Down` rationale is dead.**
   `help_text.rs:162` explains them as the pan "for a session with a
   script loaded (where Ctrl-Up/Ctrl-Down belong to the script)". Since
   2026-08-12 the script takes the **bare** arrows, so `Ctrl-Up`/
   `Ctrl-Down` are not the script's, and these two arms are unreachable
   anyway (§8).

Also: **`i` in the main pane (heat cues) is not described anywhere.**
The help documents `i` only as the override pane's sort toggle; the test
passes on the mention.

---

## 8. Dead code in the dispatcher

`key_dispatch.rs:182-187` binds `Alt-Up`/`Alt-Down` in the override pane
to a vertical pan. Those arms are **unreachable**: the main pane's own
`Alt-Up`/`Alt-Down` arms at `:146`/`:147` match first. They were added
by spec 0271 for a reason that the 2026-08-12 bare-arrow amendment
retired. They should be deleted along with `help_text.rs:162-164`.

---

## 9. Proposals

Ranked by (value ÷ disruption). Nothing here is implemented.

### Tier A — correctness, no behavior change

**A1. Fix the four help drifts** (§7) and document main-pane `i`.
Cheap, and the help is the app's only discoverability surface.

**A2. Delete the unreachable `Alt-Up`/`Alt-Down` override arms** (§8).

**A3. Make `mentioned_as_a_key` case-*sensitive* for the bare-letter
scan.** The current case-insensitivity exists so `Ctrl-E` (reported as
`Char('e')`) matches the help's `Ctrl-E`. Split the scan: modified
literals keep the case-insensitive match, **unmodified** ones become
case-sensitive. That immediately catches `W`, and every future capital.

**A4. Extend the scan to non-character keys.** Parse
`KeyCode::{Tab, Enter, Esc, Home, End, PageUp, PageDown, Delete,
Backspace, Left, Right, Up, Down}` per *file*, and require each to be
named in the help section for that pane. This needs `HELP_TEXT` to be
sectioned rather than flat — see B1.

### Tier B — structure

**B1. Give `HELP_TEXT` sections as data.** Today it is a flat
`&[&str]`; there is no machine-readable link between "Override pane"
and `key_dispatch.rs`'s `handle_override_key`. A
`&[(Pane, &[&str])]` costs nothing at runtime, keeps the prose
hand-written, and makes A4 and a per-pane help possible.

**B2. Show which pane has focus, in text.** Focus is currently
communicated only by highlight styling, and §5.4's homonyms are only
dangerous because of that. One word in the status/command row —
`[main]` / `[override]` / `[manage]` — removes the whole class of
problem for a one-cell cost.

**B3. Make the help context-aware.** With B1, `F1` could open at the
section for the focused pane (still scrollable to the rest). This is the
single largest discoverability win available.

### Tier C — binding changes, ordered least to most disruptive

**C4. Give the find prompt a cancel.** Keep `Esc` = accept (it is good),
but add `Ctrl-g` (readline's abort, which the app does not use) or
`Ctrl-c` = cancel the find and restore the pre-find caret. Currently
there is no way to back out of a find without moving the caret.

**C5. Make `Tab` mean one thing: "cycle focus".** Concretely: in the
main pane with no side pane open, `Tab` does nothing *visibly* today —
make it open nothing but say nothing either (fine); in the override
pane, replace the focus-lock complaint with an actual message that names
the two keys that *do* work (`Esc` to cancel, `Enter` to apply). The
complaint should teach the way out, not merely refuse.

**C6. Move the horizontal pan onto horizontal keys.** `Shift-Alt-Up` /
`Shift-Alt-Down` for *left* / *right* (§5.3) is the least defensible
binding in the app. `Shift-Alt-Left` / `Shift-Alt-Right` are free and
obvious. Keep the old pair as aliases for one release if muscle memory
matters.

**C7. Alias `W` and document it as "wire, whole subtree".** No change
needed — just the doc (A1) — but consider also `Ctrl-w` as a "clear all
wire spans", which currently has no spelling at all.

**C8. Add `Ctrl`+wheel = fold depth.** A viewer's zoom, mapped onto the
one thing protolens can zoom. It composes with the existing hover
routing for free and needs no new state.

**C9. Add right-click = "override the node under the pointer"**, i.e.
`handle_click` followed by `t`. Zero new concepts, removes a two-step.

**C10. Give the script pane the wheel** when the pointer is over it.
Every other surface scrolls under the pointer; this is the exception,
and it is the surface a *presenter* uses under time pressure.

**C11. Reconsider `i`.** As a main-pane binding it names a *hiding*
toggle with an "info" letter, and it collides with the override pane's
sort. If a rename is acceptable: `i` = "inferred sort" everywhere
(it already is, in the override pane), and heat cues move to something
mnemonic — `c` is free in the main pane and reads as "cues".

**C12. Reconsider `Backspace` in the manage pane.** It deletes an entry
there and moves the caret in the main pane (§5.6). `d` and `Delete`
already cover deletion; dropping `Backspace` from the manage pane
removes a genuine footgun at the cost of nothing.

### Tier D — alternates worth considering, not recommended outright

**D13. A leader key.** protolens already has one chord machine (`x`) and
one focus-locking pane. A `,`- or `<space>`-style leader would let
pane-specific actions live under a namespace (`,o` for override, `,m`
for manage) and would dissolve §5.4's homonyms entirely. It is also a
large break with the vim-plus-less idiom the app has committed to, and
`space` is already the script toggle. **Recommend against**, but it is
the principled answer to the homonym problem if the binding count keeps
growing.

**D14. A user-configurable keymap.** The natural next request after this
review, and the wrong one to satisfy: the help text is hand-written
prose, so a remappable keymap would make the one discoverability surface
unmaintainable. If it is ever wanted, B1 is the prerequisite.

---

## 10. Summary table of proposals

| # | Proposal | Cost | Value |
|---|----------|------|-------|
| A1 | Fix four help drifts, document main-pane `i` | trivial | high |
| A2 | Delete unreachable override `Alt-Up`/`Alt-Down` | trivial | low |
| A3 | Case-sensitive scan for unmodified letters | small | high |
| A4 | Scan non-character keys too | medium | high |
| B1 | Section `HELP_TEXT` as data | small | enabling |
| B2 | Name the focused pane on screen | small | high |
| B3 | Context-aware `F1` | medium | high |
| C4 | A cancel at the find prompt | small | medium |
| C5 | Make the focus-lock message teach the way out | trivial | medium |
| C6 | Horizontal pan onto horizontal keys | small | medium |
| C7 | Document `W`; consider `Ctrl-w` clear-all | trivial | medium |
| C8 | `Ctrl`+wheel = fold depth | small | medium |
| C9 | Right-click = override here | small | medium |
| C10 | Wheel over the script pane | small | medium |
| C11 | Rename the heat-cue toggle off `i` | small | low |
| C12 | Drop `Backspace` from the manage pane | trivial | low |
| D13 | A leader key | large | — (not recommended) |
| D14 | Configurable keymap | large | — (not recommended) |
