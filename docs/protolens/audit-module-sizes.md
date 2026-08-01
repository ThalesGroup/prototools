<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens codebase audit — module sizes and proposed splits

*written 2026-07-31, against `bc586dc`*

One of three companion audit documents:

- **this file** — which modules have outgrown a reasonable size, and how
  to split each one.
- [audit-duplication.md](audit-duplication.md) — factorization and
  deduplication opportunities.
- [audit-quality.md](audit-quality.md) — correctness, safety and
  documentation findings.

None of the three is a spec. They are a snapshot of the codebase taken
after several weeks of uninterrupted feature work, so that the cleanup
can be planned rather than improvised. Every claim below was verified
against the code on the date above; line numbers will drift.

## The measured picture

`wc -l` over `protolens/` totals **42 623 lines**. The largest modules:

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

(The git status snapshot in some tooling still lists
`protolens/src/tui/compact.rs`; that file does not exist — spec 0216
deleted compaction.)

## The constraint that shapes every split

`src/tui/` is one privacy island. Every sibling module opens with
`use super::*;`, and that is what makes `App`'s roughly 110 **private**
fields visible to `navigation.rs`, `render.rs`, `override_apply.rs` and
a dozen others. Rust's `pub(super)` reaches exactly one level up, so:

- **A new module must be a direct child of `tui`.** `tui::foo::bar` does
  not see `App`'s private fields, and adding a nested layer would force
  a `pub(super)` cascade.
- **`struct App` itself must stay in `mod.rs`.** Moving the definition
  out would require annotating ~110 fields `pub(super)` — a large,
  noisy, permanently-maintained diff for no benefit.

The good news is that the codebase already proves the pattern works:
`impl App` is spread over fourteen-plus modules, and `structure.rs` and
`lines.rs` are small, clean examples of an extraction that cost nothing.
Every split below follows them.

## Priority 1 — `src/tui/override_apply.rs` (2707 lines)

The strongest candidate. It is the largest production module, it is the
only file of this size with **no section banners**, and it holds four
unrelated concerns that happen to be reachable from the same entry
point. Proposed three new siblings of `tui`:

**`preview_truncate.rs` (~230 lines)** — lines 126-351: `TruncShape`
(:135), `truncate_interior` (:183), `trunc_shape_for` (:231),
`insert_truncation_marker` (:281), `cut_at` (:320). This is
self-contained line surgery for the preview pane; nothing else in the
module touches it.

**`override_resolve.rs` (~700 lines)** — lines 361-1035, plus
`format_fqdn_label` (:2692) and `fqdn_needs_dot_prefix` (:2705). This is
the "which type does this node want, and does it exist" half, which is
pure lookup and has no contact with the splice.

**`line_patch.rs` (~460 lines)** — `LinePatchTarget` / `LinePatch`
(:42-73), `finalize_override_batch` (:1662),
`assert_line_counts_are_exact` (:1752), `materialize_line_patches`
(:1888), `resolve_line_patch` (:1991). This is the spec 0210 line
bookkeeping, and it is the piece most likely to be read on its own.

That leaves ~1050 lines of actual splice in `override_apply.rs`, which
is a reasonable module.

**Cost:** three `pub(super)` annotations — `field_name_for_by_path`
(:880), `resolve_active_override_entry_index_by_path` (:935),
`finalize_override_batch` (:1662).

**One trap to know about:** the test file mentions several private
methods by name. They are all doc-comment references, not calls, so the
split does not break `src/tui/tests/override_apply.rs` — but do not
assume that from a grep of names alone.

## Priority 2 — `src/tui/mod.rs` (2706 lines)

Split shallowly. Three clean lift-outs, all direct children of `tui`:

**`terminal.rs` (~530 lines)** — lines 2160-2685: `run`, `run_loop`,
`warm_up_heat_cues`, `suspend`, `restore_terminal`, the
keyboard-enhancement push/pop pair, `drain_pending_input`,
`KITTY_KEYBOARD_ENHANCED`, and the constants only these read. This is
the terminal lifecycle, and it is the part of `mod.rs` least related to
everything else in it.

**`help_text.rs` (~165 lines)** — lines 559-719, the `HELP_TEXT`
constant. Pure data.

**`prefetch.rs` (~325 lines)** — lines 1740-2061, the read-ahead
scheduler.

That leaves ~1690 lines, of which `struct App` (:790-1465, 676 lines) is
the bulk and must stay.

**Cost:** one `pub(super)` (for `HELP_TEXT`) and a `pub use
terminal::run;` re-export so `main.rs` keeps calling `tui::run`.

## Priority 3 — `src/decode.rs` (2548 lines)

**Do not split the production half.** It is 1483 lines and is already
navigable: section banners at :39, :60, :333, :435 and :927 divide it
correctly, and the wire-format walk is genuinely one subject.

**Do move `mod tests`** (lines 1484-2548, 1065 lines) into
`src/decode/tests/`. This is a mechanical move that halves the file
without touching any production code.

## Priority 4 — `src/tui/render.rs` (1517 lines)

No new file. The problem is one function: `render()` at :884-1350, **467
lines**. Split it *in place* into three private methods —
`render_main_pane`, `render_main_statusline`, `render_command_row` —
matching the four `render_*` methods that already sit at :1351-1516. The
module's size is then unremarkable and its shape becomes uniform.

## Explicitly no action

`navigation.rs`, `key_dispatch.rs`, `manage_pane.rs`,
`override_select.rs`, `theme.rs`, `heat_worker.rs`, `tiered.rs` and
`override_pane.rs` are all either under 900 production lines or cohesive
enough that a split would cut across a single subject.
`command_line.rs` (904) is borderline and low priority.

## Test modules

Test files are the largest single category in the codebase, and three of
them are worth splitting. The same privacy rule applies with extra
force.

**`src/tui/tests/support.rs` (1789 lines, 31 fixtures, 0 tests)** — the
shared fixture library. Split into **flat siblings** of `tests`, not
into a nested `support/` directory: the 31 fixtures are `pub(super)`,
and nesting would demote every one of them. Natural seams:

| lines | content |
|---|---|
| 28-97 | inspection helpers |
| 98-285 | basic fixtures |
| 286-714 | packed fixtures |
| 715-1114 | typed fixtures |
| 1115-1514 | `Any` and MessageSet fixtures |
| 1515-1789 | export and prune fixtures |

**`src/tui/tests/override_apply.rs` (2759 lines, 57 tests)** — mirror
the production split: splice / `export_fields` (:1477-1670) /
`preview_truncate` (:1671-2151) / `line_patch` (:2152-2759). Doing this
at the same time as the production split keeps the two halves aligned.

**`src/tui/tests/override_select.rs` (2235 lines, 60 tests)** — split
into `override_select` / `search` / `override_preview`. The `search`
group should absorb the `main_pane_search_*` tests at :799-1063, which
are misfiled here and belong with search either way.

**Leave alone:** `tests/profiling.rs` (a manual harness, not a suite —
splitting it would obscure that), `tests/manage_pane.rs`,
`tests/render.rs`.

## Suggested order

1. `decode.rs` test move — mechanical, zero risk, −1065 lines.
2. `render()` in-place decomposition — no new files, no privacy change.
3. `mod.rs` → `terminal.rs` + `help_text.rs` + `prefetch.rs`.
4. `override_apply.rs` → three siblings, **with** its test file split in
   the same change.
5. `tests/support.rs` and `tests/override_select.rs`.

Each step is independently landable and independently revertible.
