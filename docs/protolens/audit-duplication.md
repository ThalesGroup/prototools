<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens codebase audit — duplication and factorization

*written 2026-07-31, against `bc586dc`. Second pass, whole crate.*

One of three companion audit documents:

- [audit-module-sizes.md](audit-module-sizes.md) — module sizes and
  proposed splits.
- **this file** — factorization and deduplication opportunities.
- [audit-quality.md](audit-quality.md) — correctness, safety and
  documentation findings.

The first pass covered the TUI shell and found ten items. This pass read
**every production line of the crate** — the wire-format and descriptor
half, the heat-cue subsystem, the theme and highlight layer, the whole
override system — and roughly quadrupled the count. Items are numbered
so they can be cited; the numbering is new in this pass, so do not carry
letters over from the first version.

Total reduction if every clear win is taken: **roughly 450 lines**,
about 1% of the crate. The line count is not the point. Half the clear
wins below are places where **a comment is currently doing a compiler's
job** — two copies of a rule that must agree, with prose as the only
thing making them agree. Those are listed first.

---

## Part 1 — duplication a comment is holding together

These seven are the reason to do this work. Each is a rule written twice
where a silent divergence produces wrong output, not a compile error.
Ordered by how bad the divergence would be.

### 1.1 The heat range, seven sites and two spellings

Two ways to answer "which bytes of this node get scored":

| site | form |
|---|---|
| `heat_cue.rs:362-378` | `heat_scored_range` — the definition |
| `heat_cue.rs:387` | calls it |
| `mod.rs:1702` | calls it |
| `heat_cue.rs:524-527` | inlines `extract::message_payload_range` |
| `mod.rs:2034-2037` | inlines it — **byte-identical** to the above |
| `override_select.rs:355-356`, `:427-428`, `:480-481` | inline it |

**The two implementations are now provably equal, and one of them is
dead code.** `heat_scored_range`'s packed-run branch (`:367-377`) exists
to reconstruct a record's payload for an element of a packed run, whose
own `raw_range` used to be narrower. Spec 0216 dissolved that premise —
`decode.rs:913-916` now writes `span.raw_range = raw_start[slot]..raw_end[slot]`
and then sets `packed_record_start` to that same `raw_start[slot]`, so
for every packed node the record *is* the `raw_range`. Traced on
`nested_packed_run_fixture` (record at `6..11`), both arms return `8..11`.
The comment at `heat_cue.rs:368-369` asserting a difference is stale.

**Why it matters beyond tidiness:** `heat_cue_resolve` (`:387`) writes
the cache keyed by `heat_scored_range`, and `recheck_pending_heat_states`
(`:524-527`) reads it back by the inlined form, for the same node. If
those ever disagree the recheck reads a key the push never wrote and the
node never settles. That invariant holds today by accident.

Collapse to `extract::message_payload_range(&self.blob, &self.tree[idx].span.raw_range)`
and route all seven through the one method. **~21 lines.** One
behavioral difference to make deliberately: on a malformed length varint
the two clamp differently (both stay in bounds, so neither can panic —
it is only a question of which garbage range gets scored).

**Done 2026-08-01, and it was already half done.** The packed-run branch
described above no longer existed: the quality audit's C1 fix had already
deleted it, leaving `heat_scored_range` as the single line this item
argues for. So there was no divergence left to reconcile, only five
inline copies to route through it — in `heat_cue.rs`
(`recheck_pending_heat_states`), `prefetch.rs` (`prefetch_step`) and
`override_select.rs` (three). The `mod.rs` sites in the table had moved
into those two files with the module split. `heat_scored_range`'s doc
gained the paragraph this item's "why it matters" section is: that every
heat cache key goes through it, and that a divergence has no failure
signal because the recheck would read a key the push never wrote.

### 1.2 `Head::cmp` hand-copies `candidate_order`'s inverse

`sweep.rs:100-102` defines the ranking order, under a doc comment
saying sharding makes it "**the single definition rather than merely the
tidy one**", because "`Merged` assumes each shard sorted under exactly
the relation the merge compares with, which hand-copied closures cannot
guarantee".

170 lines later, `sweep.rs:270-279` hand-copies it:

```rust
// candidate_order
b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))
// Head::cmp
self.score.cmp(&other.score).then_with(|| other.fqdn.cmp(&self.fqdn))
```

These are the same relation — `Head::cmp` is exactly
`candidate_order(&other.entry, &self.entry)`. Change the tie-break in
`candidate_order` and the merge heap silently keeps the old one.

Give `Head` an `entry: (String, i64)` field instead of separate
`score`/`fqdn` (`Merged` already owns those tuples and destructures them
apart only to reassemble the fields) and `cmp` becomes one line.
**~5 lines**, and the rule stops existing twice.

**Done 2026-08-01, exactly as written.** `Head` now holds `entry:
(String, i64)` and `cmp` is `candidate_order(&other.entry, &self.entry)`.
The equality was checked by expansion before the rewrite, not assumed:
`candidate_order(&other.entry, &self.entry)` is
`self.1.cmp(&other.1).then(other.0.cmp(&self.0))`, which is the old body
verbatim. Both construction sites lost their field-by-field
reassembly and pass the tuple straight through.

### 1.3 The wrapper-target ladder, and its `packed` companion

`override_select.rs:613-622` (`warm_visible_override_wrappers`) and
`override_apply.rs:2411-2431` (`render_node_as`) both run the same
three-branch ladder in the same order: message → primitive keyword →
enum. And both compute the same `packed` predicate
(`override_select.rs:605-606`, `override_apply.rs:2375-2376`).

The comment at `override_select.rs:602-604` says it outright: *"Spec
0219 S4: must agree with `render_node_as`'s own `packed` — see the
comment there — or warming registers a wrapper the splice then never
looks up."*

Extract `wrapper_target_for(name, is_group)` and a free
`packed_framing(span)`. Each caller keeps only its own fall-through
(`continue` vs `Err`). **~14 lines**, and an invariant becomes a call.

### 1.4 `OverrideOrigin::label()`'s format, open-coded in the lookup

`override_apply.rs:942-948` builds `format!("{parent_path}:{field}")`
and `format!("{fqdn}:{field}")` by hand;
`override_pane.rs:154-160` (`OverrideOrigin::label`) produces exactly
those two strings.

They must agree: `active_entry_with_label` **binary-searches** a list
sorted by `origin.label()`. Change the separator and `partition_point`
silently stops matching — every `PathField` and `FqdnField` override
quietly stops resolving, with no error anywhere.

Two label constructors on `OverrideOrigin`, used by `label()` and by the
lookup. **~2 lines saved, and a silent-failure mode closed.**

**Done 2026-08-01, with one constructor rather than two.** The two
formats are not merely alike, they are required to be the same:
`origin_is_at_or_under` relies on a `path:field` label extending its
`path` label by exactly a `:field` suffix, and would need rewriting if
they diverged. Two constructors would have made that divergence
expressible; one, `OverrideOrigin::field_label(container, field)`, does
not. The lookup lives in `override_resolve.rs` now, not
`override_apply.rs` — the line numbers above predate the module split.

### 1.5 The tier bitmask, three spellings across two modules

`heat_worker.rs:139-147` maps `Tier → u8`; `:160-171` maps `u8 → Tier`;
`tiered.rs:307-311` builds the same three bits by shifts.
`set_in_flight`'s own doc (`:137-138`) says it is "encoded as the same
bitmask `band_occupancy` uses, so `activity` can simply `|` the two
together" — a cross-module invariant with prose as its only enforcement.

`Tier::bit()` and `Tier::highest_in(mask)` next to `Tier` itself.
**~10 lines**, and a fourth tier can no longer be half-added.

### 1.6 `colorize.rs` — three parallel 13-element lists

`SyntaxRole` (`:45-59`), `RECOGNIZED_NAMES` (`:69-83`) and
`from_highlight_index` (`:86-103`). The doc at `:61-63` states the
invariant — "in the exact order of `SyntaxRole`'s discriminants" — and
`from_highlight_index` is a longhand, hand-written inverse of the
array's own indexing.

Insert a variant mid-enum, append its name to the end of the array, and
it compiles cleanly while mis-coloring every role after the insertion
point.

One `const ROLES: [(SyntaxRole, &str); 13]` that both derive from.
**~20 lines**, and the ordering becomes structural.

### 1.7 The effective wire type, twice

`command_line.rs:233-237` and `:523-527` are byte-identical: spec 0135
§G1's "a packed element's wire type is `WT_LEN`" rule, written twice.
One copy is in completion (`complete_type_as_fqdn`), the other in
validation (`type_as`). If they disagree, the prompt offers a candidate
that the commit then rejects.

A free `effective_wire_type(span) -> u32` — the free form is better than
a method, because `override_select.rs:605-606` (item 1.3) computes a
closely related predicate over the same two fields, and one named
function is what makes their agreement checkable.

**Done 2026-08-01. There were three copies, not two.** The third is
`override_export.rs`'s tier-4 untyped-field guess (`export
--descriptor`), which reads the same rule off a child span. It was in
`override_apply.rs` when this audit was written, which is why the
comparison missed it. `decode::effective_wire_type` now sits beside
`primitive_keywords_for_wire_type`, its main consumer, and all three call
it.

---

## Part 2 — two things found on the way

Neither is duplication, but both were surfaced by comparing near-copies,
which is exactly what this kind of sweep is for.

### 2.1 A probable missed early-out in the heat worker

`heat_worker.rs:371-391` (`HeatCaches::window`) answers "does the cache
cover `[start, end)`?" in **two** steps: the `by_range` `top_n` probe,
*and* a fallback onto the `complete` slot. Its doc (`:361-370`) explains
the fallback is necessary or callers "busy-loop".

`heat_worker.rs:485-488`, inside `heat_worker_loop`, asks the same
question with **only the first step**:

```rust
let covers_window = c.by_range.peek(&start, req.tier)
    .is_some_and(|e| e.top_n.len() >= req.end);
```

Consequence: a second request for the same range with a larger `end` —
which is exactly what `upgrade_active_override_to_complete` does
(`override_select.rs:431` asks `usize::MAX` after
`recompute_override_candidates` asked `override_list_height` at
`:361-367`) — sees `covers_window == false` and pays a **whole second
`score_all`**, even though `complete` already holds the full list for
that very range.

Reasoned from the code, not reproduced. Worth investigating before
unifying the two into one `covers()` predicate — fix the behavior first,
then remove the second copy.

### 2.2 `render_cache.rs` cites a type that no longer exists

`render_cache.rs:7` calls `RenderCache` "structurally identical to
`override_pane::CandidateCache` (spec 0114 §6)", and `:40` cites
`CandidateCache::candidates_bytes`'s doc comment. **`CandidateCache` was
deleted** by commit `234dcc8` (the background heat-cue scoring thread);
those two comments are the only references left in the crate.

So the standing question "should the two byte-bounded MRU caches share a
generic?" is **moot — there is no second cache.** The nearest relative,
`tiered.rs`'s `TieredBounded<K, V>`, is not the same structure and
cannot be merged: it is bounded by entry *count* where `RenderCache` is
bounded by *bytes*; it has four priority bands and a deliberately
non-promoting low-tier read where `RenderCache` promotes
unconditionally; it is an intrusive linked slot arena with a `HashMap`
index where `RenderCache` is a `Vec` with a linear `position()` scan
(the right choice for its tens of entries, the wrong one for the
worker's).

Action: **fix the two comments**. Do not write the generic.

---

## Part 3 — clear wins by volume

Mechanical, low-risk, each independently landable.

### 3.1 Test-fixture boilerplate — the largest single item

Across `src/tui/tests/` and `tests/`: 26 identical `App::new(...)`
calls, **31 identical write-temp / load / remove triples**, 26
near-identical `COUNTER`-based unique-path blocks, 26 identical `use`
preambles, 245 descriptor struct literals across nine files, and
`type_as_fixture` (`tests/support.rs:766-843`) ≈ `empty_message_fixture`
(`:853-924`), the same 78 lines with a different payload.

**The machinery to fix this already exists and was never reused**: a
`Fixture` type with a `Drop` impl at `decode.rs:2218-2289`, and
`TempFile` / `temp_path` / `write_temp` at `tests/batch_export.rs:43-72`.

There is a robustness argument on top of the size one: the hand-written
triples end in `fs::remove_file(...).unwrap()`, which is **not
panic-safe** — a failing assertion leaks the temp file. A `Drop` guard
is.

Do this first. It touches only tests, and it shrinks the files that
[audit-module-sizes.md](audit-module-sizes.md) also wants to split.

### 3.2 The rest, in one table

| # | finding | sites | ~lines |
|---|---|---|---:|
| 3.2.1 | `theme.rs`: seven byte-identical `ThemeKind::System => unreachable!(...)` arms, and four copies of the same 4-way `Dark/Light × rgb/16` dispatch. One `fn pick<T>(theme, dark_rgb, dark_16, light_rgb, light_16) -> T` | `theme.rs:39,362,415,433,449,583,612`; `:410-418`, `:428-436` byte-identical | 30 |
| 3.2.2 | `theme.rs`: `style_for_dark_rgb` / `style_for_light_rgb` are verbatim modulo the module path; both palettes already declare the same nine constant names. One `RgbPalette` struct + two `const`s | `:233-255`, `:260-282` | 23 |
| 3.2.3 | `tiered.rs`: `fix_head_if_matches` / `fix_tail_if_matches`, 21 lines each differing only in `.head`/`.tail` (the second's whole doc is "Symmetric with the first"). One `bands_for(tier) -> (&mut Band, Option<&mut Band>)` | `:420-440`, `:443-463` | 25 |
| 3.2.4 | `children_with_field(parent, field)` — four hand-rolled sibling walks. Since spec 0216 children are a contiguous slot block, this can be a range filter, not a pointer walk | `override_apply.rs:718-728`, `:1470-1478`; `manage_pane.rs:287-293`; `command_line.rs:730-737` | 24 |
| 3.2.5 | `materialize_line_patches` / `resolve_line_patch` are the same consuming merge written twice, including two independently-worded copies of the overlap asserts (a doc comment already says "the same consuming merge … for consistency of shape"). One `merge_replacements(base, pieces, what)` | `override_apply.rs:1949-1982`, `:2007-2038` | 22 |
| 3.2.6 | `open_command_line(kind, prefill)` — the three-field prompt-open written nine times; two `manage_pane` sites open-code `prefill_export`'s body rather than calling it. A new prompt site can silently forget `command_cursor` today | `key_dispatch.rs:106,111,412,676,681,284`; `manage_pane.rs:455,460,671,677` | 20 |
| 3.2.7 | `heat_cue.rs`: `read_heat_state(start, current_key, tier)` — a 10-line cache-read block byte-identical at two sites, each with its own copy of the promoting-`peek` comment | `:405-416`, `:536-547` | 18 |
| 3.2.8 | `tiered.rs`: `slot()` / `slot_mut()` — nine 3-4 line `slots[idx].as_ref().expect(...)` chains, seven with the same message | 9 sites | 18 |
| 3.2.9 | `decode.rs`: `register_synthetic(...)` — `register_wrapper` and `register_message_set_item` share an early return, a file-proto literal and a registration tail. **Caveat:** `register_wrapper`'s `drop(target)` at `:1123` is load-bearing and must stay before the call, and `file_name` must remain a parameter | `:1073-1136`, `:1292-1330` | 18 |
| 3.2.10 | `gg` chord prologue, three copies of the same 10-line arm/reset. One `take_g_chord(code)` | `key_dispatch.rs:15-25`, `:451-460`; `manage_pane.rs:348-358` | 16 |
| 3.2.11 | `own_field_override` scans the whole collection twice, linearly, to compute what `active_entry_with_label` already answers by `partition_point`. Its own two halves are also the same 7-line `find_map` | `override_apply.rs:448-475` | 20 |
| 3.2.12 | `main.rs`: two byte-identical output blocks. One `fn emit(bytes, output) -> ExitCode` | `:549-563`, `:572-586` | 15 |
| 3.2.13 | `RangeHeatEntry::new(stats, top_n)` / `.stats()` — the struct literal spelled four times, its inverse twice | `heat_worker.rs:530`, `heat_cue.rs:473`, `mod.rs:1713`, `override_select.rs:249`; `heat_cue.rs:407`, `:537` | 12 |
| 3.2.14 | `applicable_override_entry_index(idx)` — spec 0139's "Step A then Step B" written three times, as an index, a boolean and a type, with three doc comments cross-referencing each other in place of a function | `manage_pane.rs:50-59`; `override_select.rs:213-217`, `:93-99` | 12 |
| 3.2.15 | `require_arg(args, missing)` — the same 4-line empty-args guard plus `args.join(" ")` at four commands | `command_line.rs:478,754,771,855` | 12 |
| 3.2.16 | `override_pane.rs`: four byte-identical "deactivate every entry sharing this origin" loops, enforcing an invariant documented in five separate places. One `deactivate_origin`. **Do not fold in** the fifth copy at `:367-371` — it uses `origin_is_at_or_under`, deliberately | `:286-290`, `:331-335`, `:402-406`, `:439-443` | 12 |
| 3.2.17 | `command_line.rs`: `complete_fs_path` / `complete_dir_path` — `:269-284` ≡ `:311-326` and `:302-304` ≡ `:342-344` verbatim. One `complete_path_impl(..., dirs_only)` | `:268-305`, `:310-345` | 12 |
| 3.2.18 | `navigation.rs`: `fold_all_siblings` / `unfold_all_siblings`, 12 lines each differing in one predicate. `toggle_all_siblings` already dispatches on exactly this boolean | `:812-824`, `:826-838` | 11 |
| 3.2.19 | `decode.rs`: `patch_synthetic_field_name` / `patch_raw_field_name` — identical bodies, the only difference being the needle (`'_'` vs the field number). Puts the load-bearing `": "` / `" {"` anchor pair in one place | `:1153-1162`, `:1174-1187` | 9 |
| 3.2.20 | `manage_pane.rs`: three "toggle active at highlight" arms, two byte-identical, the third differing only in `toggle_active` vs `toggle_active_cascading`. Also serves `handle_manage_click:773-780`. Preserve arm ordering — the comment at `:487-503` explains why | `:504-510`, `:511-517`, `:521-526` | 14 |
| 3.2.21 | `manage_pane.rs`: Shift-Down / Shift-Up, nine-line bodies differing only by `1` vs `-1` | `:383-391`, `:392-400` | 8 |
| 3.2.22 | `mod.rs`: `pan_by_step_clamped` and `pan_vertical_by_step` have **byte-identical bodies** modulo parameter names and the same signature. Keep one. **Do not fold in** `pan_by_step` (`:349-355`) — it saturates, and passing `usize::MAX` as the max would overflow | `:364-370`, `:381-387` | 7 |
| 3.2.23 | `decode.rs:988-992` hand-rolls hex in a loop that `override_pane::sha256_hex` already produces — **and this file already calls that helper at `:243`.** Also drops 16 transient `String`s per new wrapper, on the cursor-move path | `:988-992` | 5 |
| 3.2.24 | `format_fqdn_label` is open-coded twice in `override_select.rs`, because it is `fn` where its sibling `fqdn_needs_dot_prefix` is `pub(super)`. Widen it. (Its doc claiming `render.rs` shares it is also stale) | `override_apply.rs:2692`; `override_select.rs:651-655`, `:658-662` | 6 |
| 3.2.25 | `blob.rs`: the wrapper prefix — the framing every span coordinate in the document is relative to — is written twice, with the three `helpers` imports duplicated for it | `:150-155`, `:345-348` | 5 |
| 3.2.26 | `manage_pane.rs`: `set_manage_highlight(i)` — "moving the highlight cancels a pending `z` rotation" restated at eight mutation sites, one of which is order-dependent and correct only by inspection | 8 sites | 7 |
| 3.2.27 | `render.rs`: `popup_frame(...)` — the six-line `centered_rect` / `Clear` / bordered block prologue in `render_help` and `render_splash`. The only repeated area/border computation in the entire render path | `:1464-1470`, `:1488-1494` | 7 |
| 3.2.28 | `HeatCaches::commit_sweep(...)` — the "write a scored candidate list into the caches" sequence at four sites. **Do this last:** the four use *two different* cap rules (the worker widens against `req.end`, the two UI paths use `max(override_list_height, HEAT_CUE_PREVIEW)`) and nothing says which is authoritative. That is a design decision, not a refactor | `heat_worker.rs:512-541`; `heat_cue.rs:456-487`; `mod.rs:1698-1721`; `override_select.rs:236-263` | 45 |

---

## Part 4 — marginal

Real, but small or awkward. Do only while already editing nearby.

- **`lines.rs`: one line-axis descent instead of two.**
  `descend_line_pos` (`:218-266`) and `visible_row_of_line` (`:360-407`)
  descend the same axis and differ only in what they carry and return; a
  merged `descend_line(line) -> (LinePos, row)` with a `frozen` flag
  saves ~40 lines. **The one refactor in this report with real semantic
  risk:** the folded-node rule is subtle, documented in three separate
  comments, and it is what puts the cursor on screen. `visible_row_pos`
  (`:287-346`) is the inverse direction and should stay separate.
- **`theme.rs` ANSI-16 pair** (`:286-312`, `:316-342`) — six of thirteen
  arms genuinely differ, and the modifier columns differ from the RGB
  pair. ~20 lines for a less readable palette table. Take it only in the
  same change as 3.2.2, or not at all.
- **`decode.rs`'s three primitive-keyword tables** (`:1196`, `:1226`,
  `:1245`). A single table is blocked: the per-wire-type slices are
  `&'static`, and their ordering is a completion-ranking choice, not
  alphabetical — real data, not a projection. **The cheap fix is a test**
  asserting the three agree, which turns silent drift into a build
  failure at zero structural cost.
- **`Kind` → type, enumerated three times** — `natural_type`
  (`override_apply.rs:421-439`), `resolve_export_fields` (`:554-579`)
  and `decode::primitive_type_for_keyword` (in reverse). ~15 lines for
  one site; the reverse map does not exist and would have to be added.
  Worth noting as drift risk if protobuf ever gains a scalar type.
- **`mod.rs`: `enter_terminal_modes()`** (`:2302-2308` vs `:2270-2279`)
  — only three lines, but they must stay the exact inverse of
  `restore_terminal`'s three (`:2227-2232`), and a duplicated enter
  sequence is the classic way that pairing drifts. Recommended despite
  the size.
- **The two `render_*_pane` heads** (`render.rs:1385-1418` vs
  `manage_pane.rs:802-831`) — ~28 lines on top of the statusline tail,
  so the pair is ~60 together. Borrow-checker hostile. If anyone
  revisits the tail, do the head in the same change.
- Smaller: `list_search_start` for the two pane searches
  (`override_select.rs:750-770`, `manage_pane.rs:231-252`);
  `nvim_remote(addr, args)` (`neovim.rs:189-197`, `:198-206`);
  `display_name(path)` (`main.rs:292-295`, `:415-419`);
  `set_cursor_fold(bool)` (`navigation.rs:752-767`);
  `node_payload_range(idx)` (five copies of a two-liner);
  `OverrideKind::ALL` (`manage_pane.rs:68-72`,
  `override_apply.rs:2676-2680`); `set_end`/`close_end` in the prefetch
  scheduler (`mod.rs:1992-2020`); `ExtractFormatArg` (`main.rs:215-220`)
  vs `ExportFormat` (`command_line.rs:10-15`), worth it mainly because
  it deletes the `unreachable!` at `main.rs:227`.

---

## Part 5 — rejected, with reasons

The most valuable section. These are the places a reader will *think*
need factoring; recorded so the check is not repeated.

### Already factored — nothing left

- **Scrolling, clamping, panning.** Seventeen helpers in `mod.rs`, and
  the panes genuinely **use** them — verified call sites for
  `clamp_scroll_to_visible`, `clamp_highlight`, `pan_vertical_by_step`,
  `pan_by_step_clamped`, `statusline_text`, `viewport_label`,
  `pane_focus_style`. The residual per-pane wrappers derive
  `max_offset`/`max_scroll` differently, which is the actual difference.
  **One deliberate exception that must NOT be "fixed":**
  `App::pan_horizontal` (`navigation.rs:533-539`) open-codes
  `pan_by_step_clamped`'s arithmetic on purpose — `max_pan_offset()`
  takes `&mut self` and runs a full `build_window`, so keeping it inside
  the `else` branch means a left-pan never pays for it. Calling the
  helper would force eager evaluation on every keystroke.
- **`render()`'s 467 lines contain no repeated span construction.** Row
  building already funnels through `row_spans` → `spans_with_insertions`
  → `make_span`, and the file's own doc comments enforce that as an
  invariant ("there must not be a second line-rendering path",
  `render.rs:468` and `:622`). The length is sequential single-execution
  phases. `heat_chrome` is an *anti*-finding: its doc says it is a
  function precisely so `render` can call it twice, to draw and to
  measure — the factoring one would propose, already done.
- **`structure.rs`** — `first_child`/`last_child`/`nth_child` are three
  4-line functions over a shared `child_slots`. Done.
- **One decode path.** Exactly one site builds a synthetic wrapper and
  decodes through it (`render_node_as`). `splice_override` and
  `preview_override_highlight` both route through it by design. The
  specs' "one rendering path" claim holds.
- **`extract`'s payload/text range pair** (`:89-111`, `:120-125`) — the
  shared thing is a *rule*, already stated in prose in the right place;
  the implementations are a wire-tag parse and a subtraction.
- **`decode::build_tree`/`overlay_spans`** — already the *result* of a
  de-duplication; its doc says "that there is one function rather than
  two is the point".
- Cross-pane helpers confirmed in use: `is_double_click`,
  `rect_contains`, `search_wrap` + `SearchPattern`,
  `override_row_display`.

### Superficially similar, genuinely different

- **The four key-dispatch functions share no prologue or epilogue.**
  `handle_key`'s head is a *router* — the other three are reached
  *through* it and therefore inherit the splash reset, message clear,
  Ctrl-Z, F1, `:` and `v` rather than restating them. No modifier
  normalization is repeated anywhere.
- **The Up/Down/PageUp/Home/End arms are not copy-pasted per pane.**
  Same *binding*, different *concept*: the main pane steps a visible-line
  sequence and carries a caret; the side panes move a `Vec` index through
  `clamp_highlight`.
- **`step_down`/`step_up`, `move_down`/`move_up`,
  `move_page_down`/`move_page_up`** — the remaining mirror is the residue
  of a *deliberate* prior refactor (spec 0215 S1), not neglect.
- **`cut_at`'s two backward-scan arms** (`override_apply.rs:327-346`) —
  different bits *and* different index conventions, because one seeks a
  UTF-8 character start and the other a varint end.
- **`tiered.rs`'s three tiers** — `Prefetch` deliberately links at the
  tail where the others link at the head (documented, load-bearing);
  `pop_highest` and `evict_one` walk different band sets in opposite
  orders by design. `link_at_head`/`link_at_tail` are an 11-line mirror
  whose asymmetry is the entire point.
- **`heat_lookup`/`heat_lookup_ex`** — a deliberate thin wrapper.
- **`complete.rs` vs `command_line.rs`'s path completion** — the
  trailing-slash policies are *opposite by design*, the string types
  differ (`OsStr` vs `&str`), the error paths differ. A unified helper
  would take a policy enum, an optional base and an error sink, and be
  longer than the two it replaced.
- **`main`'s three "load a file, map the error, announce it" blocks** —
  different error types, different messages, different fallbacks.
- **`manage_affected_nodes`'s `FqdnField` arm vs
  `collect_descend_targets`'s `fqdn_fields` set** — same question,
  deliberately opposite access patterns (a document walk for one origin
  vs a per-node hash probe over all origins).
- **`assert_line_counts_are_exact`'s child-summing loop mirrors
  `lines.rs`'s `refresh_line_counts` — and that is the point.** It is an
  independent recomputation of what the incremental path maintains;
  unifying them would make the check vacuous. Explicitly not a defect.
- **`jump_to_match` vs `jump_to_override_match`** — different algorithms
  despite adjacent names; one deliberately avoids `search_wrap` for a
  documented reason.
- `DescriptorContext::message`/`enumeration` (two unrelated
  prost-reflect types, no shared trait); `pool`/`pool_mut` (the standard
  Rust accessor pair); `synthetic_message_name`/`synthetic_field_name`
  (the asymmetry is documented and deliberate).

### Blocked by a library or the language

- **The three YAML entry structs** (`override_pane.rs:541-580`) share
  four trailing fields with six serde attributes each. The obvious fix,
  `#[serde(flatten)]`, is **hard-blocked**: serde does not support
  `flatten` together with `deny_unknown_fields`, and that attribute is
  exactly what makes the `untagged` discrimination work (the doc at
  `:517-532` explains that without it, a `PathField` mapping matches
  `Path` first and the `field` key is silently dropped). The remaining
  option is a macro, which would hide the on-disk file format — the one
  thing a reader most wants to read literally. **Recorded so nobody
  tries `flatten` again.**
- `to_yaml`/`from_yaml`'s arms move the same four fields, but as *field
  initializers* over different struct types — not liftable without the
  common struct the point above just ruled out.

### Not worth the mechanism

- **`complete.rs` is a fork of `prototext/src/complete.rs`** — ~75 lines
  identical, comments and wrapping included, with protolens having
  dropped two parameters. Sharing it needs a new crate in the build graph
  for a completion helper that mirrors clap_complete's own logic and
  changes roughly never. **Recommendation: decline, and record the
  decision.** Extract if a third consumer ever appears. (One divergence
  worth noting: both carry a "no trailing slash" comment with *different
  justifications* for the same behavior.)
- The eight pan wrappers (`navigation.rs:541-593`) — collapsing them
  just moves literals to the call sites.
- `self.message = format!(...)` (22 sites in `command_line.rs`) and
  `"pattern not found: {pattern}"` (3 sites) — house style.
- 38 `saturating_*` sites — no recurring composite.
- `self.heat_caches.lock().unwrap_or_else(|e| e.into_inner())` (8 sites)
  — replaces one line with one line, and items 1.1/3.2.7/3.2.28 remove
  five of them anyway.
- Two-line `map_err` at `decode.rs:291`/`:308`; the `#@` magic tested
  two ways at `decode.rs:292`/`:312` (the third, in `blob.rs:77-82`, is
  a genuinely stricter test on a different input class — do not fold it
  in).
- **`tests/batch_export.rs`'s fixture duplication is unavoidable** and
  already documented at `:9-14`: protolens is a binary-only crate, so an
  integration test can only drive the compiled executable as a
  subprocess.

---

## Suggested order

1. **3.1** — test fixtures. Largest, touches only tests, and it shrinks
   the files the module-split audit also wants to touch.
2. **1.1, 1.2, 1.4, 1.7** — the four comment-held invariants whose
   divergence is silent. 1.1 also deletes dead code.
3. **2.1** — investigate the missed early-out *before* unifying it.
4. **2.2** — fix the two stale `CandidateCache` comments. Two lines.
5. **3.2.1, 3.2.2, 3.2.3, 3.2.7** — the biggest mechanical wins, all in
   files nobody else is editing.
6. **1.3, 1.5, 1.6** — the remaining invariants; each needs a little
   thought about where the new function lives.
7. Everything else in Part 3, opportunistically.
8. **3.2.28** last — it forces a decision about the two cap rules.

Part 4 only while already in the file. Part 5 never, unless a premise
changes.
