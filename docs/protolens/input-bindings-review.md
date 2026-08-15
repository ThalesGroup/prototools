<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens input review — keys and mouse, every pane and mode

*last verified: 2026-08-14*

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
- **there is no cancel at a find prompt.** `Esc` is consistent (it
  leaves the innermost open thing, and the find prompt is one), but
  every keystroke of a pattern has already moved the caret and nothing
  puts it back (§5.2);
- **the modifier algebra has one genuine collision**: `Alt` means "pan
  the main pane" *and* "move by word", and both are live at once. The
  word motion is the correct one, so the pan is what has to move (§5.3);
- **`z` means different things in different panes** with nothing linking
  them; `a`, `i` and `o` are milder cases of the same (§5.4);
- there is **no way to quit that is a key**, and the help buries the
  answer (§5.5);
- two arms in the override pane are **provably unreachable** (§8) —
  worth reviving rather than deleting.

Twenty proposals follow, ranked, in §9.

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
| 7 | script keys (`space`, and `,`/`;`/`?`/`.` while active) | above the side panes so the script drives regardless of focus |
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

`Space` toggles navigation; while on, `,`/`;` step and `?`/`.` scroll
the pane, from any focus. **No arrow key is ever the script's** — as of
2026-08-14 they were handed back, because a presenter reaches for one by
reflex and a reflex must not change step (0271 S7). `?` is the one
binding really taken, and only while navigation is on.

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
C5 removes row 3 by giving `Tab` a job in the override pane.

### 5.2 `Esc` has four meanings, and no find prompt has a cancel

`Esc` is: close the help; discard the command line; leave the `F`/`B`
find prompt; close the override pane; close the manage pane; clear the
selection; and (main pane, nothing else pending) clear the message.

Read as "leave this mode", the find arm is **not** an outlier: every one
of those seven is a dismissal of the innermost thing that is open. The
find prompt is a sticky mode (§1.4), so leaving it is exactly what `Esc`
should do, and the caret staying on the previewed match (spec 0278) is
the find-as-you-type behavior a reader wants.

What is genuinely missing is the other direction. Because `Esc` leaves
the mode *keeping* what it previewed, and `Enter` steps to the next
match, **there is no way to back out of a find without moving the
caret.** Every keystroke of the pattern has already moved it. That is a
gap in the vocabulary, not an inversion of `Esc` — so the fix is to
*add* a cancel rather than to reassign the commit (C4).

### 5.3 `Alt` means two things simultaneously

`Alt-Up`/`Alt-Down` pan the main pane. `Alt-Left`/`Alt-Right` move by
word. These are not variants of one idea; they are two ideas sharing a
modifier along one axis each.

Of the two, **the word motion is the one that is right**:
`Alt-Left`/`Alt-Right` and `Alt-b`/`Alt-f` are readline's own
`backward-word`/`forward-word`, and the app spells readline correctly
everywhere else (§3). So the collision has to be resolved by moving
*the pan*, not the word motion.

That also explains the shape of the current bindings, and one of them is
better than it looks. `Shift-Alt-Up`/`Shift-Alt-Down` pan
**horizontally**, on the vertical arrows, which reads at first as pure
accident — the horizontal arrows were taken. But it is the same
convention as `Shift`+wheel: across essentially every GUI and terminal,
adding `Shift` to a vertical gesture turns it horizontal, and the app
already documents exactly that for the mouse. On that reading the
binding is principled rather than accidental — it is a keyboard spelling
of `Shift`+wheel.

It survives only as long as the horizontal arrows are unavailable,
though. The moment `Shift-Alt-Left`/`Shift-Alt-Right` are free, "the
arrow points where it pans" beats any analogy, and the analogy stops
being needed.

The residual problem is that `Alt` is doing two jobs, and that only the
main pane has a pan on it at all — the side panes pan on `Ctrl`-arrows
(§4), and the override pane's `Alt` arms are dead (§8). There is no
single modifier that means "pan" app-wide. The proposal is to make one:
`Shift-Alt`+arrows, all four directions, every pane (C6). That leaves
`Alt` to readline alone, keeps `Ctrl` for scope-widening, and gives §8's
dead arms a reason to exist.

### 5.4 Four letters are homonyms across panes

| Letter | Main pane | Override pane | Manage pane |
|--------|-----------|---------------|-------------|
| `a` | toggle annotations | — | toggle entry active |
| `i` | toggle heat cues | toggle candidate sort | — |
| `o` | open manage pane | prefill `:override` | prefill `:override` |
| `z` | toggle fold | — | rotate origin kind |

`o` is defensible (all three are "override-ish"). `a` is a stretch
("annotations" vs. "active").

`i` is more defensible than it first looks: the heat cues *are* derived
from the same inferred-type ranking the override pane sorts by, so one
letter for "the inference" is a real link, not a coincidence. It is
still an indirect one, and `c` — "cues" — is free in the main pane and
says the thing directly (C11).

`z` is the one genuine collision. It is vim's fold letter in the main
pane and a rotation through origin kinds in the manage pane, and there
is no reading under which those are the same idea. The letter to move is
the rotation, since the fold binding is the one with an external
convention behind it. **`t`/`T` is the proposal**: the manage entry's
origin kind is what the override *targets*, `t` is already the main
pane's "open the override pane" key so the letter is associated with the
same subject, and `t`/`T` are both unbound in the manage pane today —
so the rotation keeps its forward/backward pair (C13).

This class of problem exists only because the panes look alike, and it
is bounded by how legible focus is. **Focus is not invisible**: every
pane carries its own status line (spec 0147 G2), and the focused one is
drawn white + bold + reversed against gray + reversed for the others
(`pane_focus_style`, `theme::focus_style`). The complaint is that this
is *too subtle* — the difference between the two is brightness alone, on
a one-row bar, and brightness is exactly the channel a low-contrast
terminal theme flattens. B2 proposes adding hue to it.

### 5.5 There is no quit key

`:quit` is the only way out (`:q` suffices). `q` is deliberately unbound
so a stray `q` cannot lose a session. Defensible, and it is documented
in the help — but `Ctrl-c` is *taken by copy*, so the two most common
reflexes a user has for leaving a program are one silently-nothing and
one silently-copies. The `Ctrl-Z`-then-`kill` escape hatch is the actual
fallback, which is not a good answer.

The right answer is not a new key. It is that `F1` works from anywhere,
including a locked pane (tier 2 of the ladder), so a lost user always
has one reachable surface — and that surface should answer the question
they actually have. **`:quit` should be the first line of `HELP_TEXT`**
(A5). Today it is far down, under the command section, which is where a
reader looks last.

### 5.6 Smaller inconsistencies

- **`Enter` in the manage pane closes the pane**; `Enter` in the
  override pane *applies and closes*. Same key, one is a commit and one
  is a dismiss. The two panes are consecutive steps of one workflow —
  choose a type, then refine what it applies to — so a single rule can
  cover both: `Enter` commits, `Tab` advances to the next step, `Esc`
  leaves. See C5, which is a redesign rather than a rename.
- **The manage pane's `Left`/`Right` move the main pane's cursor.** It
  is the only binding in the app where a focused pane's arrows drive a
  different pane — but the thing they drive is *this entry's* affected
  fields, circulated one at a time. That is a property of the selected
  manage row, displayed where it is legible, so the pane is not really
  reaching outside itself. Withdrawn as an inconsistency: it is a
  deliberate coupling and it is the only way to answer "what does this
  entry actually touch?" without leaving the pane.
- **`f`/`b` page in every pane but the help overlay pages by ±10 lines**
  rather than by a screenful, so the same key has a different stride.
  The overlay should page by a screenful like everything else (C14).
- **`d`/`Delete`/`Backspace` all delete in the manage pane**, but
  `Backspace` is *caret motion* in the main pane. A user who has learned
  main-pane `Backspace` will delete an entry with it. Drop it from the
  manage pane (C12).
- **`i` in the main pane is documented nowhere** (§7), and the field
  behind it is named negatively (`heat_cues_hidden`) even though the
  binding itself is a plain toggle. The binding is fine; the field name
  and the missing help line are what to fix.

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

- **No right-click anywhere.** This is the one real gap, and the answer
  is a small **context menu** rather than a shortcut: a right-click puts
  a short list next to the pointer, and the entries are the things the
  reader would otherwise have to know a letter for — over a main-pane
  node, "override this node", "fold / unfold", "show wire bytes",
  "go to definition"; over a manage row, "activate", "activate with
  children", "duplicate", "delete". It is the only discoverability
  surface that appears *at* the thing it describes, and every entry it
  lists is already a bound key — so it documents the keyboard instead of
  competing with it (C9).
- **The wheel does not pan the script pane.** Every other surface
  scrolls under the pointer; the script pane scrolls only via `?`/`.`
  with navigation on (C10). The 2026-08-14 rebinding sharpens this: the
  scroll keys are now punctuation nobody guesses, so the wheel is the
  only gesture a presenter would find by instinct.
- **`Ctrl`+wheel is unbound.** In a viewer this is conventionally zoom,
  and protolens has an exact analogue in fold depth. Concretely:
  `Ctrl`+wheel-down folds one level *under the node the pointer is over*
  — the deepest currently-unfolded generation of its subtree closes —
  and `Ctrl`+wheel-up reopens one generation. That is `Z`'s
  whole-subtree semantics applied one level at a time, so no new state
  is needed beyond the depth the node is already at, and it composes
  with the existing hover routing for free (C8).
- **Double-click means two different things**: "select this line" in the
  main pane, "cascade this entry" in the manage pane. The cascade is the
  weaker claim on the gesture — nothing about a double-click says
  "include the children" — and with a context menu in place it has a
  better home as a menu entry.

Two asymmetries that are *not* worth closing:

- **Middle-click paste.** X11 users expect it on the command line, but
  many current mice have no usable middle button and `Ctrl-v` already
  works. Not proposed.
- **No drag in the side panes.** The manage pane's `Shift-Up`/
  `Shift-Down` look like a move-the-entry pair, which would want a drag
  analogue, but they are not: they navigate to the neighboring entry and
  activate it. There is no ordering operation to drag.

### 6.1 Is right-click a TUI idiom at all?

It is a minority binding, but not an exotic one, and it has been
mechanically available since xterm's original X10 mouse protocol — the
right button is 2 in the encoding (0 left, 1 middle, 2 right), and SGR
1006 made press and release unambiguous. crossterm surfaces it as
`MouseEventKind::Down(MouseButton::Right)`. protolens already enables
the reporting it needs for the wheel and for left-click, so the event is
being delivered and discarded today.

Where it *is* bound, there are two traditions:

1. **A second selection verb** — the orthodox file managers. In Midnight
   Commander and FAR/far2l, left-click moves the cursor and right-click
   *marks* the item under the pointer, with right-drag marking a range.
   No menu. Marking is the most frequent operation in a two-panel
   manager, so it earns its own button.
2. **A context menu** — and this is the reading that has spread, in the
   last few years, into exactly the tools this app's readers already
   run. **tmux 3.0+** binds `MouseDown3Pane`, `MouseDown3Status` and
   `MouseDown3StatusLeft` to `display-menu` by default. **Neovim** made
   `'mousemodel'` default to `popup_setpos` in 0.9, with a stock `PopUp`
   menu whose entries include "Go to definition" — the same entry
   proposed here. **Emacs `context-menu-mode`** (28+) works under
   `xterm-mouse-mode`, i.e. in a plain terminal.

So a menu is the meaning a user is most likely to arrive with, and the
usual mechanics are settled: the application draws the box itself,
anchored at the click and flipped left or up if it would run off an
edge; while open it is modal — arrows or `j`/`k` plus `Enter`, `Esc` or
a click outside to dismiss; and each row carries its keyboard equivalent
on the right, which is the whole point of the widget.

### 6.2 Terminals that keep the right button

A terminal implementing mouse reporting is supposed to hand button 3 to
the application once the app has enabled reporting, and to keep its own
behavior only when **`Shift`** is held — the same xterm bypass the app
already documents for `Shift`-click. kitty states the model most
explicitly: its `mouse_map` entries are qualified `grabbed` /
`ungrabbed`, where "grabbed" means the application asked for the mouse.

- **Alacritty** — the easy case. It has no context menu at all, by
  design, so there is nothing for right-click to be stolen by. It
  implements the reporting and the `Shift` bypass; the button arrives.
- **GNOME Console (`kgx`)** — one of a family. It, GNOME Terminal,
  Tilix, Terminator and Ptyxis are all the *same widget*, VTE, so they
  behave alike, and VTE does have its own right-click menu. The
  available evidence says VTE gates that menu on mouse-tracking state
  and forwards the button while reporting is on: mc's right-click
  marking and tmux's right-click menus both work under GNOME Terminal,
  which they could not otherwise. **Treat this as unverified** — see the
  probe below.
- **Windows Terminal** — the real problem child: right-click has
  historically been paste, inherited from conhost, and users keep that
  setting deliberately. Relevant for anyone reaching protolens over SSH
  from Windows.
- **macOS** (iTerm2, Terminal.app) — both have their own right-click
  menus; whether they gate on reporting is unverified here.

The ten-second probe, worth running in any terminal before believing
a claim about it:

```sh
printf '\e[?1000h\e[?1006h'; cat -v      # right-click in the window
# Ctrl-C, then:
printf '\e[?1000l\e[?1006l'
```

A forwarded right-click prints `^[[<2;COL;ROWM` on press and `…m` on
release. Nothing, or a menu appearing, means that terminal keeps it.

**The design consequence is that the menu must be a progressive
enhancement.** Two requirements follow, and both are in C9: every entry
is an action that already has a key binding, so nothing is reachable
*only* by menu; and the same menu opens from the keyboard at the caret,
which removes the terminal dependency altogether and serves keyboard
users besides. The `Shift` bypass should be documented next to it, so a
user whose terminal eats the button knows the app is not broken.

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
   script loaded (where Ctrl-Up/Ctrl-Down belong to the script)". No
   arrow has belonged to the script since 2026-08-12, and since
   2026-08-14 none belongs to it under any modifier, so the rationale is
   doubly dead; these two arms are unreachable anyway (§8). **Fixed
   2026-08-14** — the help line is now "the same vertical pan, a second
   spelling".

Also: **`i` in the main pane (heat cues) is not described anywhere.**
The help documents `i` only as the override pane's sort toggle; the test
passes on the mention.

All four drifts plus the missing `i` line are A1.

---

## 8. Two unreachable arms, and what to do with them

`key_dispatch.rs:182-187` binds `Alt-Up`/`Alt-Down` in the override pane
to a vertical pan. Those arms are **unreachable**: the main pane's own
`Alt-Up`/`Alt-Down` arms at `:146`/`:147` match first. They were added
by spec 0271 for a reason that the 2026-08-12 bare-arrow amendment
retired.

Deleting them is the smaller answer. The better one is to **give them a
reachable spelling**, because the underlying feature — panning a side
pane vertically — is real and the modifier is the only thing wrong with
it. Under C6's `Shift-Alt`+arrows rule the override pane gets all four
pan directions on a modifier that no other tier claims, the arms become
live, and `Alt` goes back to meaning readline and nothing else. A2 is
therefore a *re-binding*, not a deletion; `help_text.rs:162-164`'s
rationale still has to go, since it explains them by a script
convention that no longer exists.

---

## 9. Proposals

Ranked by (value ÷ disruption). Nothing here is implemented.

### Tier A — correctness, no behavior change

**A1. Fix the four help drifts** (§7) and document main-pane `i`.
Cheap, and the help is the app's only discoverability surface.

**A2. Re-bind the override pane's unreachable `Alt-Up`/`Alt-Down`**
(§8) under C6, and delete the stale rationale at `help_text.rs:162-164`.

**A3. Make the help scan case-*sensitive* for unmodified letters.**
To be clear about where the case-insensitivity lives: **the bindings
are already case-sensitive** — `z` and `Z` reach different arms, and
crossterm reports the shift state faithfully. The insensitivity is in
`mentioned_as_a_key`, a helper in the *test* that asks whether the help
prose mentions a letter. It exists so that `Ctrl-E`, which crossterm
reports as `Char('e')` plus a modifier, still matches the help's
`Ctrl-E`. The fix is to split the scan by modifier: a literal that was
found next to `CONTROL` or `ALT` keeps the case-insensitive match, a
bare one becomes case-sensitive. That immediately catches the
undocumented `W` and every future capital. No app behavior changes.

**A4. Extend the scan to non-character keys.** `bound_chars` looks only
for the literal string `KeyCode::Char('`, so a binding written
`KeyCode::Tab` or `KeyCode::Esc` is never checked against the help at
all — which is how §7's drift 3 (`Tab` documented as doing the opposite
of what it does) survived. The extension is mechanical: scan each
dispatcher source for `KeyCode::Tab`, `::Enter`, `::Esc`, `::Home`,
`::End`, `::PageUp`, `::PageDown`, `::Delete`, `::Backspace`, `::Left`,
`::Right`, `::Up`, `::Down`, and require the corresponding word to
appear in the help.

The catch is *where* it must appear. `Tab` is mentioned in the help
today — so a flat "is this word anywhere in `HELP_TEXT`?" check passes
while the override pane's `Tab` stays undocumented. The requirement has
to be per-pane: a key bound in `override_select.rs` must be named in the
help's override-pane section. That is why A4 needs B1 — the test needs
to know which lines of the help belong to which pane, and today nothing
in the data says so.

**A5. Put `:quit` at the top of the help.** §5.5's real problem is not
the missing key but that the answer is buried under the command section.
`F1` reaches the help from anywhere, including a locked pane, so it is
the one surface a lost reader can always get to — and the first line
they see should be how to leave.

### Tier B — structure

**B1. Give `HELP_TEXT` sections as data.** Today `HELP_TEXT` is a flat
`&[&str]` — a list of rendered lines, where a heading is just a line
that happens to read like one. Nothing distinguishes a heading from a
body line, so no code can ask "which lines describe the override pane?"
The proposal is to make the grouping a value instead of a typographic
convention:

```rust
pub(super) enum Section { Motion, Folds, Search, Overrides, Manage, Commands }
pub(super) const HELP: &[(Section, &str, &[&str])] = &[
    (Section::Overrides, "Override pane", &[ "Enter  apply and close", ... ]),
    ...
];
```

The prose stays hand-written and the rendered output can stay
byte-identical (walk the slice, print the title, print the lines). What
it buys: the help-text test can map a dispatcher file to a section and
check membership (A4); `F1` can open at the section for the focused pane
(B3); a future per-pane help footer has something to read. The cost is
one flattening loop in the renderer and no runtime cost at all.

**B2. Give the focused pane's status line its own hue.** Focus *is*
shown (§5.4) — the focused pane's status bar is white + bold + reversed,
the others gray + reversed — but the two differ only in brightness, on
one row, which is the channel a low-contrast terminal theme erases.
Adding hue costs one line: because `REVERSED` is set, the `.fg()` color
is what paints the visible bar background, so `focus_style` setting a
theme accent instead of `Color::White` recolors the whole bar while
`unfocused_pane_style` stays neutral gray. Brightness and hue then both
carry the signal, and a reader scanning three panes sees which one is
live without reading a word.

**B3. Make the help context-aware.** With B1, `F1` could open at the
section for the focused pane (still scrollable to the rest). This is the
single largest discoverability win available.

### Tier C — binding changes, ordered least to most disruptive

**C4. Give the find prompt a cancel.** Keep `Esc` = leave the mode
keeping the match (§5.2); add a second key that leaves it and restores
the pre-find caret.

`Ctrl-g` is the recommendation, and yes, it really is the conventional
abort: it is bound to `abort` in readline's default keymap (it is what
GNU's own docs call the abort command, and what emacs binds to
`keyboard-quit`), which is why `Ctrl-g` in `bash` at an incremental
search returns you to where the search started — precisely the
interaction wanted here. The app does not bind it anywhere today.

`Ctrl-c` is ruled out: it is copy in the main pane (§5.5), and giving it
a second meaning inside a prompt is the kind of context-dependence this
review is otherwise arguing against. Other options, if `Ctrl-g` is felt
to be too emacs: `Esc Esc` (a second `Esc` within the prompt undoes the
first — cheap, but a timing-sensitive double-tap is not a good TUI
idiom), or `Ctrl-[`, which most terminals send *as* `Esc` and so is not
actually a distinct key. `Ctrl-g` is the only clean answer.

**C5. One rule for `Enter` / `Tab` / `Esc` across the two side panes.**
The two panes are consecutive steps of one workflow — pick a type in the
override pane, refine what it applies to in the manage pane — so give
the three keys one meaning each, in both:

| Key | Override pane | Manage pane |
|-----|---------------|-------------|
| `Enter` | **commit** the override and return to the main pane | **activate** the targeted override, *staying in the pane* |
| `Tab` | pre-commit and **advance** to the manage pane for refining | back to the main pane (unchanged) |
| `Esc` | leave, discarding | leave, keeping what was activated |

This resolves three separate complaints at once. §5.1's worst row — the
override pane answering `Tab` with a complaint about focus being locked
— disappears, because `Tab` now has the job it was reached for.
§5.6's first bullet disappears, because `Enter` stops being a commit in
one pane and a dismiss in the other: it commits in one and activates in
the other, and *returning to the main pane* becomes `Esc`'s job
exclusively. And the focus-lock message stops being needed at all in the
one place a user actually pressed `Tab`.

It is the most disruptive proposal in tier C — it changes what `Enter`
does in a pane where readers already have muscle memory — so it wants a
release note, not a silent change.

**C6. Make `Shift-Alt`+arrows the pan, in all four directions and in
every pane.** Today: the main pane pans vertically on `Alt-Up`/
`Alt-Down` and horizontally on `Shift-Alt-Up`/`Shift-Alt-Down`; the side
panes pan on `Ctrl`-arrows; the override pane's `Alt` arms are dead
(§8). Proposed: `Shift-Alt-Left`/`Right`/`Up`/`Down` pan the focused
pane in the direction the arrow points, everywhere. That gives the app
one modifier that means "pan", frees `Alt` for readline word motion
alone (§5.3), keeps `Ctrl`-arrows as the side panes' existing spelling
(they can stay as aliases), and makes §8's arms reachable. Keep
`Shift-Alt-Up`/`Down`'s old horizontal meaning as a deprecated alias for
one release if muscle memory matters.

**C7. Document `W` accurately, and add `Ctrl-w` = clear all wire
spans.** `W` is close to "the whole subtree" but not that:
`wire_subtree()` (`navigation.rs:226`) shows the wire bytes of the
smallest subtree containing the caret's line, or the whole selection if
there is one — with a special case where the caret sits inside a packed
run, in which case a single line falls back to `w`'s behavior and a
selection is widened to the run's parent. The help line should say
"wire bytes for the caret's subtree, or for the selection" and leave the
packed-run detail to the wire-row documentation. Separately, `Ctrl-w` is
free and "clear every wire span" currently has no spelling at all.

**C8. Add `Ctrl`+wheel = fold depth under the pointer**, as spelled out
in §6: wheel-down closes the deepest open generation beneath the hovered
node, wheel-up reopens one. A viewer's zoom, mapped onto the one thing
protolens can zoom, composing with the existing hover routing for free.

**C9. Add a context menu, on right-click and on a key** (§6, §6.1,
§6.2). A short list at the pointer whose entries are actions that
already have key bindings — "override this node", "fold / unfold",
"show wire bytes", "go to definition" over the main pane; "activate",
"activate with children", "duplicate", "delete" over a manage row. Each
row shows its key on the right, so the menu teaches the keyboard rather
than competing with it, and the manage pane's double-click cascade gets
a home a user might actually guess.

Two constraints from §6.2 shape it. **Nothing is reachable only by
menu** — every entry dispatches an existing binding. And it **opens
from the keyboard too**, at the caret, because a terminal may keep the
right button and because the discoverability payoff does not need a
mouse.

Implementation steps, each one independently committable and testable:

1. **The menu as data and state.** A `MenuItem { label, key_hint,
   action }` and a `Menu { items, anchor: (u16, u16), selected: usize,
   area: Rect }`, with `menu: Option<Menu>` on `App`. `action` is an
   enum, not a closure — it has to be comparable in a test. `area` is
   filled in at render time and read by the hit test, exactly as
   `help_area` already is (`render.rs:2166`).
2. **Build the item list for a target.** One function per surface:
   given a main-pane node index, or a manage-pane entry index, return
   the applicable items. This is where "override this node" is omitted
   for a node that cannot carry one, so the menu never offers a no-op.
3. **Render it.** An *anchored* sibling to `popup_frame`
   (`render.rs:2215`) — same `Clear` + rounded `Block`, but placed at
   the anchor and flipped left/up when it would cross `area`'s right or
   bottom edge. Drawn last in `render` (`render.rs:1303`), after the
   splash/help branch, since it can legitimately stand over the help.
4. **Keyboard while it is open.** A new tier in `handle_key`'s ladder,
   directly *above* the help branch at `key_dispatch.rs:496` — a menu
   raised over the help must answer first. `j`/`k`/arrows/`Ctrl-n`/
   `Ctrl-p` move, `Enter` activates, `Esc` dismisses, and the per-row
   hotkey letter activates directly.
5. **Open it from the mouse.** A `Down(MouseButton::Right)` arm in
   `handle_mouse`, beside the existing `Down(Left)` arm at
   `mouse.rs:147`, routed by the same `over_main` / `over_side` hit
   tests. It respects `main_interactive`: while the override pane locks
   focus, a right-click gets the lock message like a left-click does.
6. **Mouse while it is open.** Click inside → activate that row; click
   outside → dismiss without acting; wheel → move the selection. Same
   early-return shape as the `help_open` block at `mouse.rs:48`.
7. **Open it from the keyboard**, at the caret rather than the pointer.
   Needs a free key — the `Menu` key itself if crossterm reports it,
   with a `Char` fallback.
8. **Documentation.** A `HELP_TEXT` entry for the menu and for the
   `Shift`-bypass caveat (§6.2), plus the mention the help-text test
   will now demand for whatever key step 7 picks.

Steps 1-4 are useful on their own: they give a working keyboard-only
menu. Steps 5-6 add the mouse. Step 7 closes the terminal dependency.

**Implemented 2026-08-12** (`protolens/src/tui/menu.rs`, plus the tier in
`key_dispatch.rs`, the arm in `mouse.rs` and `render_menu`). Three
departures from the plan above, each one a simplification the writing
found:

- **Step 1's `action` enum does not exist.** A `MenuItem` carries the
  `KeyEvent` it stands for, and activating a row closes the menu and
  replays that event through `handle_key`. That is comparable in a test
  just as an enum would be, and it buys three things an enum would not:
  a row cannot drift from the binding it advertises because it *is* the
  binding; the manage pane's long inline `z`/`d` match arms are not
  duplicated into a second dispatcher; and the per-row hotkey of step 4
  falls out rather than being wired.
- **Step 7's key is `m`**, with `KeyCode::Menu` accepted beside it. The
  `Menu` key alone would not do: crossterm only ever reports it under
  the kitty keyboard protocol, and most keyboards no longer have one.
- **A third surface was added** that the proposal did not have: a
  right-click past the end of the document names no node, and that is a
  surface rather than a miss. It gets the *view's* settings — hide/show
  annotations (`a`), hide/show heat cues (`i`) — which are exactly the
  bindings the node menu has no business offering. The desktop idiom:
  right-clicking a file offers what you can do to the file,
  right-clicking the desktop offers what you can do to the desktop. The
  two toggles name the state they would move *to*, so the row does not
  leave the reader guessing which way it goes.
- **The steps are not independently committable**, as the list claimed.
  The clippy gate is `-D warnings`, so steps 1-3 each leave the menu
  written and unreachable, which is dead code and fails the gate. They
  are separately *testable*, which is the half that mattered.

**C10. Give the script pane the wheel** when the pointer is over it.
Every other surface scrolls under the pointer; this is the exception,
and it is the surface a *presenter* uses under time pressure.

**C11. Move the heat-cue toggle from `i` to `c`.** Sloppy wording in an
earlier draft claimed "`i` = inferred sort everywhere"; there is only
one sort in the app, so "everywhere" was meaningless. What is meant is
narrower: **`i` keeps its one meaning — the override pane's
inferred-type sort — and is not reused in the main pane.** The heat cues
move to `c` ("cues"), which is free there. The `i`/heat-cue link is real
(the cues rank by the same inference the sort orders by), so this is a
clarity change, not a correction.

**C12. Drop `Backspace` from the manage pane.** It deletes an entry
there and moves the caret in the main pane (§5.6). `d` and `Delete`
already cover deletion; dropping it removes a genuine footgun at the
cost of nothing.

**C13. Move the manage pane's origin-kind rotation off `z`.** `z` is
vim's fold letter in the main pane and a rotation in the manage pane,
with no reading that unifies them (§5.4). The fold binding is the one
with an external convention behind it, so the rotation moves.
**`t`/`T`** is the proposal: the origin kind is what the override
*targets*; `t` is already the main pane's "open the override pane" key,
so the letter is associated with the same subject rather than a new one;
and both cases are unbound in the manage pane today, so the
forward/backward pair survives intact.

**C14. Make the help overlay page by a screenful.** `f`/`b`/`PageUp`/
`PageDown` move ±10 lines there and a screenful everywhere else (§5.6).
Same key, same word in the help, different stride — for no reason.

### Tier D — alternates worth considering, not recommended outright

**D15. A leader key.** A "leader" is a prefix key that opens a namespace
instead of doing something itself: press it, and the *next* key is
looked up in a small private table rather than the global one. vim's is
`\`, and plugins hang whole vocabularies off it (`\ff` for find-file,
`\gs` for git-status) precisely so they never have to claim a bare
letter.

protolens already has the machinery — `ExportChord` is exactly this,
with `x` as the leader — so extending it is not new work. The payoff
would be §5.4's homonyms: instead of `z` meaning fold here and rotate
there, pane actions would live under a prefix (`,o` override, `,m`
manage, `,z` rotate) and bare letters would keep one meaning app-wide,
permanently.

The cost is that it is a real break with the vim-plus-less idiom the app
has committed to — in vim, bare letters *are* the vocabulary — and it
makes every action one keystroke longer. Neither of the two most
conventional leaders is free: `space` is the script toggle, and since
2026-08-14 `,` is the script's previous-step key — so a leader would
have to be found elsewhere, and this proposal costs more than it did
when it was written. **Recommend against for now**: with fourteen
proposals'-worth of
smaller fixes available, the homonym problem does not yet need a
structural answer. It becomes the right answer if the binding count
keeps growing.

**D16. A user-configurable keymap.** The natural next request after this
review. **Deferred until a concrete need appears** — and worth recording
why: the help text is hand-written prose, so a remappable keymap would
either make the app's one discoverability surface wrong or make it
unmaintainable. B1 is the prerequisite if it is ever wanted.

---

## 10. Summary table of proposals

| # | Proposal | Cost | Value |
|---|----------|------|-------|
| A1 | Fix four help drifts, document main-pane `i` | trivial | high |
| A2 | Re-bind the override pane's dead `Alt-Up`/`Alt-Down` (with C6) | trivial | low |
| A3 | Case-sensitive help scan for unmodified letters | small | high |
| A4 | Scan non-character keys too (needs B1) | medium | high |
| A5 | `:quit` as the help's first line | trivial | high |
| B1 | Section `HELP_TEXT` as data | small | enabling |
| B2 | A distinct hue for the focused pane's status line | small | high |
| B3 | Context-aware `F1` | medium | high |
| C4 | `Ctrl-g` = cancel at the find prompt | small | medium |
| C5 | One rule for `Enter`/`Tab`/`Esc` in both side panes | medium | high |
| C6 | `Shift-Alt`+arrows = pan, four directions, every pane | small | medium |
| C7 | Document `W` accurately; `Ctrl-w` = clear all wire spans | trivial | medium |
| C8 | `Ctrl`+wheel = fold depth under the pointer | small | medium |
| C9 | Context menu, on right-click and on a key | **done 2026-08-12** | medium |
| C10 | Wheel over the script pane | small | medium |
| C11 | Heat cues move to `c`; `i` stays the sort | small | low |
| C12 | Drop `Backspace` from the manage pane | trivial | low |
| C13 | Origin-kind rotation moves from `z` to `t`/`T` | trivial | medium |
| C14 | Help overlay pages by a screenful | trivial | low |
| D15 | A leader key | large | — (not recommended) |
| D16 | Configurable keymap | large | — (deferred) |
