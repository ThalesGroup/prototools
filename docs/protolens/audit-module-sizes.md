<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens codebase audit — module sizes and proposed splits

*written 2026-07-31 against `bc586dc`; second pass 2026-08-01*

One of three companion audit documents:

- **this file** — which modules have outgrown a reasonable size, how to
  split each one, and what each split actually costs.
- [audit-duplication.md](audit-duplication.md) — factorization and
  deduplication opportunities.
- [audit-quality.md](audit-quality.md) — correctness, safety and
  documentation findings.

None of the three is a spec. They are a snapshot of the codebase taken
after several weeks of uninterrupted feature work, so that the cleanup
can be planned rather than improvised. Every claim was verified against
the code; line numbers will drift.

The second pass worked out the implementation of each split in detail.
That turned up several things the first pass had wrong, and one that
matters more than any individual split: **the privacy model in the next
section was stated backwards**. Read it first — every cost estimate
below depends on it.

## The measured picture

`wc -l` over `protolens/` totals **42 592 lines**. The largest modules:

| lines | file | lines | file |
|---:|---|---:|---|
| 2759 | `src/tui/tests/override_apply.rs` | 2707 | `src/tui/override_apply.rs` |
| 2706 | `src/tui/mod.rs` | 2548 | `src/decode.rs` |
| 2235 | `src/tui/tests/override_select.rs` | 1789 | `src/tui/tests/support.rs` |
| 1741 | `src/tui/tests/profiling.rs` | 1717 | `src/tui/tests/manage_pane.rs` |
| 1618 | `src/tui/tests/render.rs` | 1517 | `src/tui/render.rs` |
| 1385 | `src/tui/tests/navigation.rs` | 1290 | `src/tui/heat_worker.rs` |
| 1113 | `src/tui/tests/heat_cue.rs` | 1106 | `src/override_pane.rs` |
| 1067 | `tests/batch_export.rs` | 941 | `src/tui/tests/key_dispatch.rs` |

Raw line counts overstate five modules, which carry their tests inline
rather than in `src/tui/tests/`. Their **production** halves are much
smaller, and none of them needs splitting on size grounds:

| file | total | production | `mod tests` starts |
|---|---:|---:|---:|
| `src/decode.rs` | 2548 | 1483 | 1484 |
| `src/override_pane.rs` | 1106 | 698 | 699 |
| `src/tui/heat_worker.rs` | 1290 | 645 | 646 |
| `src/theme.rs` | 901 | 646 | 647 |
| `src/tui/tiered.rs` | 863 | 492 | 493 |

## The privacy model — corrected

The first pass said `src/tui/` is one privacy island held together by
`use super::*;`, that a new module must be a direct child of `tui`, and
that `struct App` must stay in `mod.rs`. **All three conclusions stand.**
But the mechanism was described wrongly, and the wrong description
understates the cost of every split by roughly an order of magnitude.

**Rust privacy is descendant visibility.** An item private to module `M`
is nameable from `M` *and every descendant of `M`* — not just from `M`
itself. So:

- **Descendant → ancestor is free.** `tui::navigation` may name any
  private item of `tui`, including `App`'s ~110 private fields, with no
  annotation at all. Proof in-tree: `neovim.rs:17` writes
  `use super::{restore_terminal, App};` and `restore_terminal`
  (`mod.rs:2227`) carries **no visibility modifier whatsoever**.
  `tests/prefetch.rs:13-16` states the same rule in a comment.
- **Sibling → sibling is the direction that costs.** `tui::terminal` is
  *not* a descendant of `tui::prefetch`. Reaching into a sibling needs
  `pub(super)` on the item — which, written inside a child of `tui`,
  means `pub(in crate::tui)`, i.e. the whole island.

Two consequences that shape every plan below:

1. **Moving an item out of `mod.rs` into a sibling demotes it**, from
   "visible island-wide for free" to "needs `pub(super)`". Everything
   that referenced it must be re-checked. This is the real cost, and it
   is why the "three `pub(super)` annotations" estimate for
   `override_apply.rs` was too low.
2. **`use super::*;` will not find a sibling's contents.** `mod foo;` is
   private, so the glob imports only the *name* `foo`. Free functions,
   types and constants therefore need either a module-qualified path
   (`override_apply::fqdn_needs_dot_prefix(..)`, as
   `override_select.rs:651` already writes) or an explicit import
   (`navigation.rs:5`: `use super::command_line::{next_word_boundary,
   prev_word_boundary};`). **Methods are the exception** — method-call
   syntax resolves by visibility alone, so an `impl App` method that
   moves to a sibling needs no import anywhere, only `pub(super)`.

   The cheap fix for a free item with many callers is a re-export in
   `mod.rs`, which the existing `use super::*;` globs then pick up
   unchanged. `mod.rs:58` already does this: `pub(crate) use
   lines::LinePos;`.

Note also that `main.rs:16` declares `mod tui;` privately, so inside
`mod.rs` there is no practical difference between `pub`, `pub(crate)`
and `pub(super)`.

There is no `foo.rs` + `foo/` sibling layout anywhere in the workspace;
every multi-file module uses `mod.rs`. New directories should follow.

---

## Priority 1 — `src/decode.rs`: move the tests out

The cheapest win in the codebase: **−1065 lines, zero production
change, zero visibility change.**

**Do not split the production half.** It is 1483 lines and already
navigable: section banners at `:39`, `:60`, `:333`, `:435` and `:927`
divide it correctly, and the wire-format walk is genuinely one subject.

### What moves

`decode.rs:1484-2548` — `#[cfg(test)] mod tests { … }`, one flat block
with no nested `mod`. Turn `src/decode.rs` into `src/decode/mod.rs` and
the block into `src/decode/tests.rs` (or a `tests/` subtree, below),
replacing it with `#[cfg(test)]\nmod tests;`.

### What it costs

**Nothing.** The tests reach exactly one item that is private to
`decode` — `arena_gap` (`:783-784`) — and a child module sees its
parent's private items, before and after the move alike. The two
sibling-module reaches, `crate::blob::wrapped` (`blob.rs:196`) and
`crate::override_pane::sha256_hex` (`override_pane.rs:584`), are both
`pub`, so module depth is irrelevant.

### The two traps

1. **One import moves with the tests.** `decode.rs:24-25` —
   `#[cfg(test)] use prototext_core::helpers::{write_tag, write_varint,
   WT_LEN};` — is used only at `:2145-2146`, inside `mod tests`. Leave
   it behind and it warns. (`WT_LEN` also appears at `:1227`, but that
   is a separate function-local `use`; deleting line 25 is safe.)
2. **Six `#[cfg(test)]` items outside `mod tests` must stay.** Most
   importantly **`arena_gap`** (`:783`), which is called from
   *production* code at `:1468` inside `render_resolved`; moving it
   would not compile. The others are `DescriptorContext::empty_for_test`
   (`:253`), `for_test_with_graph` (`:277`), `arena_of` (`:743`, called
   from six test files), `decode` (`:1348`, ~25 call sites), and the
   `#[cfg(test)] use std::collections::HashMap;` at `:12-13` that
   `arena_gap` consumes.

**No path breakage.** There is no `include_str!`, `include_bytes!`,
`include!`, `#[path]`, `file!()` or `CARGO_MANIFEST_DIR` in the file.
Every test path is built at runtime from `std::env::temp_dir()`, and the
corpus test reads env vars. The classic one-directory-deeper breakage
does not apply.

### Optional: four files instead of one

The block has one section banner, at `:2009` (`Spec 0197: on-demand
descriptor loading`), and everything after it is a self-contained suite
with its own fixture harness that nothing before it uses. If the module
keeps growing, the natural cut is:

| file | source range | contents |
|---|---|---|
| `tests/root_type.rs` | 1493-1600 | 4 root-type-resolution tests + 2 helpers |
| `tests/patching.rs` | 1602-1644 | 5 `patch_synthetic_field_name_*` tests |
| `tests/render.rs` | 1646-2007 | 5 decode/arena tests |
| `tests/lazy_pool.rs` | 2009-2547 | the spec-0197 suite + its harness |

Each needs `use super::super::*;` plus its own extras. Start with the
single file; this is only worth it later.

---

## Priority 2 — `src/tui/render.rs`: decompose `render()` in place

No new file. `render()` is `render.rs:884-1329` — **446 lines**, and the
module's only size problem.

### The house style it must match

All the render methods are in one `impl App` block (`:269-1517`), shaped
`fn render_x(&self|&mut self, frame: &mut Frame, area: Rect)`:

| method | line | visibility |
|---|---:|---|
| `render_activity_dot` | 1351 | private — only `render.rs` calls it |
| `render_override_pane` | 1381 | `pub(super)` |
| `render_help` | 1463 | `pub(super)` |
| `render_splash` | 1487 | `pub(super)`, and `&self` |
| `render_manage_pane` | `manage_pane.rs:801` | `pub(super)` |

New helpers should be **private `fn`**, like `render_activity_dot` —
nothing outside `render.rs` would call them.

**`render_override_pane` is the template, and it settles the shape of
the split.** It takes the pane's *outer* rect, does its own
`[Min(0), Length(1)]` layout (`:1389-1393`), draws its local statusline
(`:1428-1429`) and then its rows (`:1459`) — pane and statusline in
**one** method, with the statusline's `viewport_label` (`:1426`)
computed from the scroll offset its own clamp (`:1410`) just settled.

### Why the proposed three-way split is wrong

Cutting at the pane/statusline boundary (`:1165`) forces five locals
across it: `right_outer` (`:905`, read at `:1188`/`:1225`), `window`
(`:981`, read at `:1223`), `total_rows` (`:973`, read at `:1223`),
`main_split` (`:941`, read at `:1233`) and `main_style` (`:936`, read at
`:1240`). The pane helper would have to *return* a tuple and the
statusline helper would take six arguments. That is exactly the
shared-state smell the split is meant to avoid.

### The split that falls out instead — two methods, not three

```rust
fn render_main_pane(&mut self, frame: &mut Frame, area: Rect, half_width: bool)
```
`render.rs:930-1242` (313 lines), called with `main_outer`. It does its
own `[Min(0), Length(1)]` split, exactly like `render_override_pane`.
All five cross-boundary locals become method-local and vanish.
`half_width` replaces the two `right_outer.is_{none,some}()` tests —
`right_outer` is used there *only* as a boolean.

```rust
fn render_command_row(&mut self, frame: &mut Frame, area: Rect)
```
`render.rs:1244-1314` (71 lines), called with `chunks[1]`. **Perfectly
sealed** — its only local input is the rect, it defines and consumes
`cmd_text` in the same span, and everything else it touches is `self`.
It carries `render_activity_dot`'s call site (`:1277`) with it. **Do
this one first; it is free.**

That leaves `render()` at roughly 60 lines: geometry and the separator
(`:884-928`), the two calls, then the side-pane dispatch (`:1316-1322`)
and the splash/help overlay (`:1324-1328`).

If 313 lines is still too big, cut **inside** `render_main_pane`, not at
its edge:

- **`fn compose_lines(&self, …) -> Vec<Line>`** from `:1075-1153`, the
  `text_lines` closure. It is *already* structurally forced to be an
  immutable borrow — the comments at `:975-978` and `:1001-1003` say the
  `&mut self` passes at `:989` and `:1013` were hoisted out of it for
  precisely this reason. It needs six inputs, and this is the one place
  a small `struct FrameState` would pay for itself.
- **`fn render_main_statusline(&self, frame, area, half_width,
  visible_rows, total_rows)`** from `:1166-1242`, called from *inside*
  `render_main_pane` where all its inputs are still locals. This gives
  the proposed name with none of the threading cost — and it can take
  **`&self`**, which the outer split could not.

### The one ordering constraint to write down

`clamp_scroll_to_visible` (`mod.rs:331`, called at `render.rs:960`)
mutates `self.scroll_offset`; the statusline reads it at `:1223` via
`viewport_label`. Benign as long as the pane draws before the
statusline, which source order guarantees — but it means the statusline
**cannot be hoisted or reordered**, and cannot be computed from `&App`
before the pane draws.

Same shape one level up: `track_message_timeout()` (`:885`) writes
`self.message`, read at `:1264`; `track_splash_timeout()` (`:886`)
writes `self.splash`, read at `:1324`. Both calls must stay at the top
of `render()` rather than migrating into a helper.

### Free of complications

- **No early `return` in `render()`.** The only `?` in the range is at
  `:1068`, inside the `and_then` closure at `:1065-1071`.
- **No test changes.** All 85 `app.render(frame)` call sites across six
  test files are `terminal.draw(|frame| app.render(frame))`; none
  reaches a sub-region, because none exists yet. `render()` keeps its
  signature.

---

## Priority 3 — `src/tui/mod.rs` → three siblings

Three lift-outs, all direct children of `tui`. **−1082 lines, leaving
`mod.rs` at about 1627.** Every range below is corrected — the first
pass cut at the `fn`/`const` line, orphaning doc comments into the
previous region.

### 3a. `help_text.rs` — lines **555-719** (165 lines)

`HELP_TEXT`, plus the 4-line doc comment at `:555-558` the first pass
missed. Flanked by `enum ExportChord` (ends `:553`) and `PreviewOverlay`
(starts `:721`); it is a top-level `const`, not inside an `impl`.

**Zero dependencies** — `&[&str]` literals only, no `use` line at all.

**Cost:** `pub(super) const HELP_TEXT` in the new file, plus `use
help_text::HELP_TEXT;` in `mod.rs` so `render.rs`'s bare `use super::*;`
still resolves it at `:1474`, `:1476`, `:1477`. That is the whole cost.
Nothing else reads it. **Do this one first — it validates the
demote-then-re-export pattern on a single consumer.**

### 3b. `prefetch.rs` — lines **1724-2059** (336 lines)

The first pass's `1740-2061` is wrong at both ends: it drops the 16-line
doc block at `:1724-1739` and runs into `centered_rect`'s doc at `:2061`.

Contents: `PREFETCH_WALK_MAX_ROWS` (`:1724-1740`), a `const _ assert`
(`:1742-1747`), `struct PrefetchWalk` (`:1749-1780`) + its `impl`
(`:1782-1853`), `enum PrefetchStep` (`:1855-1858`), `PrefetchTrace`
(`:1860-1885`) + its `impl` (`:1887-1909`), and the `impl App` block at
`:1911-2059`.

**Cost — the real one is the test file, not the production code.**
`tests/prefetch.rs` constructs `PrefetchWalk` with a struct literal at
four sites (`:25-34`, `:48-57`, `:76-85`, `:104-113`) and reads its
fields at `:214`, `:220`, `:224`, `:261`. Today those fields are free to
touch because `tests::prefetch` is a descendant of `tui`. After the move
it is not a descendant of `tui::prefetch`, so **all eight fields need
`pub(super)`**, as do `PrefetchWalk` itself, `next_row` and
`PREFETCH_WALK_MAX_ROWS`. Add `use prefetch::{PrefetchStep,
PrefetchTrace, PrefetchWalk};` to `mod.rs` and `tests/prefetch.rs:9`'s
`use super::super::*;` keeps working. The comment at
`tests/prefetch.rs:13-16` explaining the old rule becomes false and must
be rewritten.

**One entanglement, and it exposes a pre-existing bug.**
`App::heat_activity` (`:1927-1929`) sits *inside* the prefetch `impl App`
block and has nothing to do with prefetch — it is spec 0190's activity
dot, read only by `run_loop`. Worse, the doc comments there are already
scrambled: `:1912-1922` describes `prefetch_step`, but Rust attaches the
whole `:1912-1926` block to `fn heat_activity` at `:1927`, leaving
`prefetch_step` (`:1931`) **undocumented**. Move `:1911-1926 + :1931-2058`
into `prefetch.rs` with `:1912-1922` restored as `prefetch_step`'s doc,
and relocate `heat_activity` + its own `:1923-1926` doc into
`terminal.rs` beside its two callers.

### 3c. `terminal.rs` — lines **2159-2685** *plus* **129-182**

The first pass said ~530 lines; it is **581**, because five constants
belong with it. Ten items from `drain_pending_input` (`:2159`) through
`run_loop` (`:2452-2685`), ending where `mod.rs`'s `mod` declarations
begin. Contiguous, and the item before it (`emit_osc52_copy`, `:2147`)
is unrelated clipboard code that stays.

The five constants at `:129-182` — `WARMUP_FIRST_DRAW_DELAY`,
`WARMUP_REDRAW_INTERVAL`, `ACTIVITY_TICK`, `HEAT_REPAINT_INTERVAL`,
`PREFETCH_STEPS_PER_ITERATION` — are read **nowhere else in the crate**.
Note the last belongs to `terminal.rs`, not `prefetch.rs`: it is
`run_loop`'s per-iteration budget. `MESSAGE_TIMEOUT` (`:114`),
`SPLASH_TIMEOUT` (`:127`) and `DOUBLE_CLICK_THRESHOLD` (`:89`) must
stay — `render.rs:858`/`:874` and `tests/render.rs:446` read them.

**Cost — four items get demoted, each with a named consumer:**

| item | consumer | fix |
|---|---|---|
| `run` | `main.rs:611` — the only call site in the crate | `pub fn run` + `pub use terminal::run;` in `mod.rs` |
| `restore_terminal` | `neovim.rs:17` | `pub(super)` + either retarget that import or re-export |
| `enable_raw_mode_and_reenter` | `neovim.rs:179`, `:230`, as `crate::tui::…` | already `pub(crate)`; keep the path with a re-export |
| `warm_up_heat_cues` | `tests/heat_cue.rs:834` | `pub(super)` + a re-export or a test-side import |

`suspend`, `run_loop`, `drain_pending_input`, the keyboard-enhancement
pair and `KITTY_KEYBOARD_ENHANCED` need **nothing** — they call only
each other. (`manage_pane.rs:380`/`:497` and `tests/key_dispatch.rs:48`
mention them in comments only.)

**Watch the unused imports afterward.** `mod.rs` will stop using about
seventeen crossterm/ratatui/std names. Do not sweep them blindly:
`Event`, `KeyEventKind` and `MouseEventKind` are consumed by `mouse.rs`
and `key_dispatch.rs` **through `mod.rs`'s glob**, so deleting them from
`mod.rs` breaks those files.

### Order and outcome

**`help_text.rs`, then `prefetch.rs`, then `terminal.rs`.** The middle
constraint is real: `run_loop` names `PrefetchStep` at `:2546` and
`:2572-2573`. Move prefetch first and the `mod.rs` re-export is already
in place when `terminal.rs`'s `use super::*;` needs it; the reverse
order forces a fix-up pass on a file just written.

After all three, `mod.rs` is ~1627 lines, of which `struct App`
(`:790-1464`, **675 lines** of per-field doc comments) is 41%, and
`impl App` (`:1466-1722`, `App::new` plus `seed_root_heat`) is another
257. A fourth extraction, if ever wanted, is the ~19 free helpers and
small value types — but `SearchDir`, `CommandLineKind` and
`SearchPattern` appear in `App`'s field types, so they are cheaper to
move than `App` is.

---

## Priority 4 — `src/tui/override_apply.rs` → three siblings

The largest production module, the only file of this size with **no
section banners**, and four unrelated concerns reachable from one entry
point. This is also the split whose cost the first pass most
underestimated: it named three `pub(super)` annotations; there are five
visibility changes and about a dozen import edits.

**Take them in the order below.** The three regions are pairwise
disjoint — no call runs between any two of them — so nothing forces them
to land together.

### 4a. `preview_truncate.rs` — lines **126-351** (226 lines). Do this first.

Five free items, confirmed exactly where the first pass said:
`TruncShape` (`:126-165`), `truncate_interior` (`:167-222`),
`trunc_shape_for` (`:224-260`), `insert_truncation_marker` (`:262-316`),
`cut_at` (`:318-350`). **Zero `App` contact, zero sibling callers, zero
test coupling** — the ideal rehearsal for the mechanics of the other two.

Cost:

- **`insert_truncation_marker` must become `pub(super)`** — it is bare
  `fn` at `:281` and `render_node_as` calls it at `:2574`, which stays.
  *The first pass missed this.* `cut_at` stays private; both its callers
  move.
- `override_apply.rs:9-11`, the whole `prototext_core::helpers` import
  block, moves out entirely.
- `NodeSpan` (`:12`) and `Type` (`:7`) must be **duplicated** — both
  sides need them, and `mod.rs` does not import `NodeSpan`, so
  `use super::*;` will not supply it.
- One `use super::preview_truncate::{trunc_shape_for, truncate_interior,
  insert_truncation_marker};` in `override_apply.rs`.

### 4b. `line_patch.rs` — lines **42-74** + **1652-2040** (422 lines). Second.

`LinePatchTarget` (`:42-52`), `LinePatch` (`:54-73`) and four `impl App`
items: `finalize_override_batch` (`:1653-1728`),
`assert_line_counts_are_exact` (`:1730-1872`, `#[cfg(test)]`),
`materialize_line_patches` (`:1874-1983`), `resolve_line_patch`
(`:1985-2040`).

Second because it is the one that forces the visibility problem below —
better found on a 422-line move than tangled in the 725-line one.

Cost:

- **`finalize_override_batch` → `pub(super)`.** Two remaining callers,
  not one: `:1213` in `render_overrides` and `:2284` in
  `splice_override`.
- **`probe::TEXT_US` must be widened.** `override_apply.rs:1693` reads
  it, and it will not compile from a sibling: `mod probe` is
  `pub(super)` at `:17` so the *module* is island-visible, but its
  statics are `pub(super)` **relative to `probe`**, i.e.
  `pub(in tui::override_apply)`. A sibling is not a descendant of
  `override_apply`. Widen `TEXT_US` — or the whole probe block, for
  uniformity — to `pub(in crate::tui)`. *The first pass missed this
  entirely, and it is the kind of thing that only shows up at compile
  time.*
- `LinePatch`/`LinePatchTarget` are free types, so `override_apply.rs`
  needs an explicit import for `splice_override`'s uses at `:2182`,
  `:2195`, `:2200`, `:2216`. Their four `pub(super)` fields must stay —
  `:2216` constructs and `:2196-2197` read them.
- **Two files outside the pair must change in the same commit:**
  `mod.rs:930` declares `pending_line_patches: Vec<override_apply::LinePatch>`
  (and `:924` names it in a doc comment), and
  `tests/override_apply.rs:5` imports
  `super::super::override_apply::{LinePatch, LinePatchTarget}`. **That
  single test-file line is the only place the source split and the test
  split are not independent.**

`materialize_line_patches` keeps `pub(super)` for the tests at
`tests/override_apply.rs:2186`, `:2211`, `:2227`.
`assert_line_counts_are_exact` and `resolve_line_patch` stay private.

One judgment call: `assert_line_counts_are_exact` is a document-wide
counter/text invariant oracle — spec 0210 territory — that rides along
only because `finalize_override_batch` is its sole caller.
`tests/lines.rs:29` already documents it as belonging to the `lines`
family. Defensible either way, but worth naming.

### 4c. `override_resolve.rs` — lines **353-1003** + **2686-2707**. Last.

The first pass said `361-1035`. **Both endpoints are wrong**: `:361` is
the `fn` line, not the doc-comment start (`:353`), and `:1035` lands
*inside* `resettle_node`'s doc comment. The coherent group ends at
`:1002`.

22 `impl App` methods, plus `format_fqdn_label` (`:2687-2698`) and
`fqdn_needs_dot_prefix` (`:2700-2707`).

**The "no contact with the splice" claim is CONFIRMED**, and mechanically
so: across `:353-1003` there is not one reference to `self.lines`,
`self.line_styles`, `self.pending_shift`, `self.pending_line_patches`,
`self.pending_patch_min_line`, `self.folded`, `self.heat_states`,
`self.render_cache`, `self.descend`, `self.refusals`, `self.arena` or
`self.override_batch_depth`. The only match is a doc-comment mention of
`self.folded` at `:360`. It reads `self.tree`, `self.fqdns`, `self.ctx`,
`self.overrides`, `self.blob` and nothing else.

**But the region is not one concern; it is four.**

| # | concern | lines | size |
|---|---|---|---:|
| 1 | override resolution proper | 381-475, 863-1002 | ~370 |
| 2 | `Any`/MessageSet auto-expansion (spec 0120) | 668-861 | ~194 |
| 3 | descriptor export (spec 0156 G6c) — `resolve_export_fields` | 477-620 | 144 |
| 4 | status-line display formatting | 622-666 + 2687-2707 | ~65 |

Group 2 is not even read-only: `auto_expand_type` is `&mut self` and
calls `self.ctx.pool_mut()` (`:828`) and `ctx.load_extension` (`:851`).
Group 3 has exactly one caller, `command_line.rs:611`. Group 4's callers
are `render.rs:1186` and `override_select.rs:651`/`:658`.

`collect_descendants` (`:361-368`) belongs with none of them — it is a
plain tree walk called three times from the *splice* side (`:1350`,
`:1596`, `:2150`) and never from the resolution side. Leave it in
`override_apply.rs`, or move it to `structure.rs`.

**Recommendation: take groups 1+2 (~565 lines) as `override_resolve.rs`
and leave 3 and 4 where they are** — or split 4 into a small
`override_display.rs`. A 700-line module holding four subjects that
share only "reads schema, writes nothing" is not obviously better than
what it replaces.

**Done 2026-08-01, one file per group rather than one file for four.**
The objection above is to the *merge*, not to the split, and once
groups 1 and 2 leave, 3 and 4 are the only non-splice tenants left —
strays rather than residents. So: `override_resolve.rs` (508, groups
1+2 with `ParentFieldOrExt`), `override_export.rs` (161, group 3 —
which already had its own test file, `tests/export_fields.rs`),
`override_display.rs` (83, group 4), and `override_apply.rs` down to
1586 and purely the splice. `collect_descendants` stayed put as
advised. Every cost below was paid exactly as written; nothing
unforeseen turned up. The one item that had gone stale in the other
direction is `fqdn_needs_dot_prefix`'s doc comment, which already named
`override_select.rs`'s `override_row_display` correctly — the
`render.rs` claim had been fixed since.

Cost:

- `field_name_for_by_path` (`:880`) and
  `resolve_active_override_entry_index_by_path` (`:935`) → `pub(super)`.
  Both first-pass claims confirmed; note the second has two callers,
  `:1044` and `:1641`, the latter inside a `#[cfg(test)]` branch.
- **`ParentFieldOrExt` (`:86-124`) should move with `parent_field`**, its
  return type. It is a free type, so one side or the other needs an
  explicit import either way. *The first pass did not mention it.*
- `override_select.rs:651` and `:658` write
  `override_apply::fqdn_needs_dot_prefix(fqdn)` module-qualified; both
  lines must be retargeted.
- Ten `pub(super)` methods are called from six other siblings
  (`override_select.rs`, `key_dispatch.rs`, `heat_cue.rs`,
  `command_line.rs`, `manage_pane.rs`, `render.rs`). All are methods, so
  **none needs an import** — visibility alone carries them.
- `Label` and `Cardinality` (`:7-8`) become dead in `override_apply.rs`;
  `Type` must be duplicated.
- `fqdn_needs_dot_prefix`'s doc at `:2703` claims `render.rs`'s
  `render_override_pane` shares it. **Stale** — grep finds no such call.
  See `audit-quality.md` G3.

### The remainder

`override_apply.rs` ends at about **1350 lines** (1389 if
`ParentFieldOrExt` stays), and is coherent: `mod probe`, `RenderedAs`,
`resettle_node`, `render_overrides` and its mark/walk helpers,
`splice_override`, `render_node_as`, `packed_record_extent`, and the
`OverrideOrigin` constructors at the tail. Those last four (`:2615-2684`)
are a fifth small concern that could also go, if the remainder should be
purely splice.

New file sizes: `preview_truncate.rs` ~236, `line_patch.rs` ~434,
`override_resolve.rs` ~725 (or ~590 for groups 1+2 only).

---

## Test modules

Test files are the largest single category in the codebase. The same
privacy rule applies with extra force, and the same doc-comment-orphaning
error runs through every range the first pass proposed — **each seam is
off by three to six lines, always cutting at the `fn` line rather than
the blank before the doc comment.** The corrected ranges below are
directly cut-and-pasteable.

`src/tui/tests/mod.rs` is **18 lines: fourteen bare `mod` declarations,
zero tests, zero helpers, zero imports.** Adding a file means one
alphabetically-placed line there and nothing else.

Every new test file copies this header verbatim
(`tests/navigation.rs:1-6`):

```rust
// SPDX header

use super::super::*;   // reaches App / crate::tui
use super::support::*; // reaches the fixtures
```

### `tests/support.rs` (1789 lines, 31 items, 0 tests)

**Flat siblings, not a nested `support/` directory** — and the reason is
sharper than "it would demote them". A `pub(super)` item in
`tests/support/basic.rs` resolves to `pub(in tui::tests::support)`, one
level short of `tests`. The obvious patch does not work either:
`pub(super) use self::basic::*;` in `support/mod.rs` re-exports at a
*wider* visibility than the item has, which is **E0364/E0365, a hard
error**, not a silent narrowing. A nested layout would require rewriting
all 31 items to `pub(in crate::tui::tests)`.

Corrected seams:

| group | **corrected** | first pass | content |
|---|---|---|---|
| A | **13-96** | 28-97 | inspection helpers |
| B | **98-276** | 98-285 | basic fixtures |
| C | **278-707** | 286-714 | packed/repeated fixtures |
| D | **709-1109** | 715-1114 | typed fixtures |
| E | **1111-1491** | 1115-1514 | `Any` / MessageSet fixtures |
| F | **1493-1789** | 1515-1789 | export and prune fixtures |

Implementation notes:

- **Group A must also take `type Shape` (`:13-23`) and `type LineOwner`
  (`:25-26`)** — they are the return types of `shape_of`/`live_shapes`/
  `line_owners`. The first pass's `28-97` orphans them.
- **The three `pub(super) use` re-exports at `:7`, `:8`, `:11`
  (`WT_LEN`/`WT_VARINT`, `NodeSpan`, `TestBackend`) must land in exactly
  one file**, and every test file should glob that one. Two siblings
  re-exporting `NodeSpan` compiles — both globs resolve to the same item
  — but it is fragile. Put them with group A.
- **Only one cross-group edge**, A ← D: `type_as_fixture` (`:838`),
  `empty_message_fixture` (`:918`) and `group_type_fixture_with_blob`
  (`:1107`) all call `node_with_type`, which is already `pub(super)`.
  **No visibility change is needed anywhere in this split** — the D file
  just adds `use super::support_inspect::*;`. B, C, E and F have zero
  outbound edges.
- **Two fixtures are misgrouped.** `wide_sibling_scalars_app`
  (`:442-496`) is in the packed group but its own doc calls it
  "`sibling_leaves_app` at the scale spec 0191's budget tests need" —
  move it to B. `eager_fallback_app` (`:709-761`) is in the typed group
  but exists for spec 0197's *eager descriptor-load warning*; its sole
  consumer is `tests/render.rs`, so **move it into `render.rs`
  outright** — no callers or callees inside `support.rs`.
- **Two items can be demoted once split**, which is a small real win:
  `group_type_fixture_with_blob` (`:1030`) and `shape_of` (`:28`) have no
  consumer outside `support.rs` and become private `fn` in their new
  files. (`Shape`/`LineOwner` must stay `pub(super)` — they leak through
  the return types.)
- **No fixture is dead.**
- **Do not be fooled by apparent single-consumer fixtures.**
  `tests/lines.rs:40-60`'s `fn real_decodes()` enumerates **thirteen**
  fixtures in one `vec![…]`, and is the silent second consumer of
  `repeated_scalar_fixture`, `nested_packed_run_fixture`,
  `export_fields_fixture`, `export_fields_group_error_fixture` and
  `pruned_tail_fixture`. Also `message_node_app_with_root_candidates`
  and `test_scoring_graph` look single-consumer but are called from
  inside `support.rs` itself (`:124`, `:208`).

### `tests/override_select.rs` (2235 lines, exactly 60 tests)

**No `mod` nesting, no file-local helper, no file-local `const` or
`static`** — every `fn` in the file is preceded by `#[test]`. So the
only shared state to place is the four targeted imports at `:5-12`;
`use std::thread;`, `OverrideCollection`, `HeatWorkerHandle`/
`RangeHeatEntry` and `Tier` all serve the async candidate-polling tests
and belong with the `override_select` bucket.

- **`search` — `795-1058`, 11 tests**, not the first pass's `799-1063`.
  `:795-797` is the first test's doc comment and `:1060-1063` is the
  *next* test's, so the original range would both orphan one doc comment
  and steal another.
- **Consider taking five more.** The override-pane search tests at
  `:553-713` and `:772-793` are the same subject and share
  `jump_to_override_match`/`SearchDir`. They are **not contiguous** —
  two unrelated tests sit at `:715-770` — so this costs a deliberate
  reorder. Decide explicitly; both answers are defensible.
- **`override_preview` — `1596-2027`, 10 tests.**
- **`override_select` — everything else, 37 tests**: `14-551`,
  `553-793`, `1060-1594`, and `2074-2235` (the spec-0200 block, the
  file's one section banner).
- **Two tests fit no bucket**: `a_node_level_jump_puts_the_caret_on_the_first_non_blank`
  (`:2029-2049`) and `a_search_hit_puts_the_caret_on_the_match`
  (`:2051-2072`) are spec 0194 caret-placement tests. Send both to
  `search` — the second clearly belongs, and the first is its paired
  control and reads as nonsense apart from it.

### `tests/override_apply.rs` (2759 lines, exactly 57 tests)

| group | **corrected** | first pass | tests |
|---|---|---|---:|
| splice (stays) | 1-1475 | — | 25 |
| `export_fields` | 1477-**1665** | 1477-1670 | 10 |
| `preview_truncate` | **1667**-**2147** | 1671-2151 | 10 |
| `line_patch` | **2149**-2759 | 2152-2759 | 12 |

The `1477` start is exactly right — it is the file's section banner. The
other three endpoints each straddle a doc comment: `:1667-1670` belongs
to `preview_budget_fixture_bytes`, `:2149-2151` to `seed_committed_lines`.

**One cross-group helper use blocks a naive cut.** The splice-group test
`an_unknown_length_delimited_blob_can_be_read_as_a_packed_run`
(`:393-436`) calls `preview_budget_fixture_bytes` at `:413` and
`bare_lines` at `:428`, both of which live in the `preview_truncate`
group. Cheapest fix: **move that single 44-line test into the
`preview_truncate` file** — it does read as a reinterpretation test —
which leaves every helper file-private. The alternative, exporting two
helpers for one foreign caller, is worse.

Other placement notes:

- `impl Debug for export_descriptor::ResolvedField` (`:1489-1499`) moves
  with `export_fields` and **must move exactly once** — a duplicate is
  E0119.
- The import at `:5` (`LinePatch`, `LinePatchTarget`) serves
  **`line_patch` only**; the one at `:8` (`Label`, `Type` from
  `prost_reflect::prost_types`) serves **`export_fields` only**. Every
  other `Label::`/`Type::` in the file sits inside a function with its
  own local `use` from a *different* crate root (`prost_types`). Copying
  either line into a file that does not need it warns.

**Leave alone:** `tests/profiling.rs` (a manual harness, not a suite —
splitting it would obscure that), `tests/manage_pane.rs`,
`tests/render.rs`.

---

## Explicitly no action

`navigation.rs`, `key_dispatch.rs`, `manage_pane.rs`,
`override_select.rs`, `theme.rs`, `heat_worker.rs`, `tiered.rs` and
`override_pane.rs` are all either under 900 production lines or cohesive
enough that a split would cut across a single subject.
`command_line.rs` (904) is borderline and low priority.

## Suggested order

Each step is independently landable and independently revertible.

1. **`decode.rs` test move** — mechanical, zero risk, −1065 lines, no
   visibility change anywhere. **Done 2026-08-01**: 2647 → 1501, tests in
   `src/decode/tests.rs` (1146). `decode.rs` stays a file, not a
   `decode/mod.rs` — the 2018 layout puts a submodule of `foo.rs` in
   `foo/`. Only one import moved; the six `#[cfg(test)]` items outside
   `mod tests` all stayed, `arena_gap` included.
2. **`render_command_row`** — the free half of the `render()`
   decomposition. Then `render_main_pane`. **Done 2026-08-01**: `render`
   446 → 78 lines. `render_main_pane` takes `half_width: bool` rather
   than `right_outer`, which is all it read of it.
3. **`mod.rs` → `help_text.rs`, then `prefetch.rs`, then `terminal.rs`**
   — in that order; `run_loop` names `PrefetchStep`. **Done 2026-08-01**:
   2826 → 1637. The visibility cost was as predicted and modest —
   `PrefetchWalk` and its eight fields plus `next_row` and
   `PREFETCH_WALK_MAX_ROWS` went `pub(super)` for the tests;
   `enable_raw_mode_and_reenter` went from `pub(crate)` to `pub(super)`
   and `neovim.rs` now names it through `super::terminal`; `run` stays
   `pub` and is re-exported from `mod.rs` so `main.rs` is untouched.
4. **`override_apply.rs` → `preview_truncate.rs`, then `line_patch.rs`,
   then `override_resolve.rs`** — cheapest first, and the third one's
   scope is a decision, not a mechanic (four concerns, not one).
   **4a and 4b done 2026-08-01**: 2714 → 2286. `preview_truncate.rs`
   turned out to need no `use super::*` at all, which is the strongest
   evidence there was that it belonged outside. `line_patch.rs` owns the
   two types and the two methods that consume them; `override_apply.rs`
   still produces the patches and imports both types back.
   **4c done 2026-08-01**: 2286 → 1586, as `override_resolve.rs` (508),
   `override_export.rs` (161) and `override_display.rs` (83) — one file
   per concern, since the section's objection is to merging the four,
   not to separating them. See 4c for the reasoning.
5. **`tests/support.rs`**, then `tests/override_select.rs`, then
   `tests/override_apply.rs`. All independent of the production splits
   except `tests/override_apply.rs:5`, which must land with step 4's
   `line_patch.rs` — it did: the import now reads
   `super::super::line_patch::{LinePatch, LinePatchTarget}`.
   **Done 2026-08-01.** `support.rs` is now a 21-line facade over six
   siblings (`support_inspect`, `_basic`, `_repeated`, `_typed`, `_any`,
   `_export`), so all fourteen consumers keep their unchanged
   `use super::support::*;`. The section's E0364/E0365 warning does not
   apply to flat siblings — under `tests/`, `pub(super)` means "visible
   in `tests`" for the facade and for the re-exported items alike, so
   the globs are legal as written. Two helpers were demoted to private
   by the split, as predicted (`shape_of`, `group_type_fixture_with_blob`);
   `eager_fallback_app` moved into `tests/render.rs` outright.
   `override_select.rs` went 2235 → 1492, giving up `search.rs` (301)
   and `override_preview.rs` (443). The two spec-0194 caret tests split
   rather than travelling together: the search-hit one is in `search.rs`,
   the node-level-jump one in `navigation.rs`, which is where a test
   about a jump belongs. `tests/override_apply.rs` was done as part of
   step 4's aftermath — 2828 → 1434, giving up `export_fields.rs` (200),
   `preview_truncate.rs` (541) and `line_patch.rs` (692).
