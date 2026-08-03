<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0236 — an override is edited as one command

Status: implemented
Implemented in: 2026-08-03
App: protolens
Refs: docs/specs/0113-….md (D26, the command line and Tab completion),
        docs/specs/0114-….md (`:type-as`, the override pane, argument
        completion — this spec retires the two commands),
        docs/specs/0117-….md (the override collection, the
        per-origin active invariant), docs/specs/0119-….md (G4, the
        display-name override — this spec replaces its key and its
        input mechanism), docs/specs/0134-….md (`origin_for_kind`, the
        three origin kinds), docs/specs/0185-….md (S5, the selection
        pane's focus lock), docs/specs/0198-….md (the `exit`
        subcommand — this spec renames it), docs/specs/0200-….md (S1,
        why `q` is unbound in the selection pane),
        docs/specs/0221-….md (a refused override is reported)

## Background

An override entry has four user-visible dimensions — origin, type,
display name, active — and today each is reached by a different
mechanism:

| Dimension | How it is set |
|---|---|
| type | the override pane (`t`), or `:type-as <fqdn>` / `:type-as-raw` |
| origin kind | only at creation, via the override pane's kind cycling |
| display name | the manage pane's `f`, a bespoke inline text-entry sub-mode |
| active | the manage pane's `a`/Space |

Three consequences:

1. **The origin cannot be changed after creation.** Re-scoping an
   override from "this one node" to "every occurrence of this type's
   field" means deleting the entry and rebuilding it from the pane.
2. **`manage_rename` is a second text-entry implementation.** It
   duplicates the command line's editing, cursor and rendering
   (`render.rs:1540-1583`, its own `RENAME_PREFIX`, its own arm in
   `track_message_timeout`) while supporting strictly less: no
   completion, no word motion, no history.
3. **`f` is spent on it**, which blocks the `f`/`b` paging keys that
   spec 0236's sibling change gives every other pane.

### The knock-on: freeing a key for it

The new command wants one plain letter in two panes, and `o` is the
mnemonic. `o` is taken in both:

| Where | Today |
|---|---|
| main pane | `o` opens/closes the management pane |
| manage pane | `Esc`/`o`/`q` all close the pane |
| selection pane | `Esc`/`t` both close the pane |

Two of those are *exits*, and having three spellings for one exit is
what spends the letters. Collapsing every override pane onto `Esc`
alone frees `o` and `q` in the manage pane at no cost — `Esc` is
already bound there, already the universal pane exit elsewhere in the
app, and is what a user reaches for anyway.

That still leaves the main pane's `o`, which is not an exit but the
management pane's only unconditional opener (`Enter` reaches it only
when the cursor node already has an applicable entry — see
`open_smart_override_or_manage`). It moves to `m`.

Separately, `q` in the main pane is `request_quit`, a two-press
`q`-then-`q` gesture with a confirmation prompt behind it. `:quit`
already exists and `:q` resolves to it unambiguously, so the prompt
machinery is a second quit path guarding a command the user can spell
directly. Retiring it frees `q` everywhere and leaves one answer to
"how do I get out".

## Goals

- **G1.** One command that sets an override's type, origin and display
  name together: `:override-as [<type>] [--origin <origin>]
  [--field-name <name>]`.
- **G2.** Pre-filled from the current state, so the command line opens
  showing what is already true and `Enter` on an unedited line is a
  no-op.
- **G3.** Tab-completion for `<type>` and `<origin>`.
- **G4.** `o` opens it, in the main pane and the manage pane.
- **G5.** `f`/`b` page in the override pane, the manage pane and the
  help overlay, matching the main pane (spec 0236's sibling change).
- **G6.** Retire `manage_rename` entirely.
- **G7.** `Esc` is the only key that exits an override pane.
- **G8.** `:quit` is the only way to quit protolens; `quit_confirm`
  and its two-press `q` gesture are retired. `q` is left unbound
  everywhere, the help overlay included.
- **G9.** The CLI's `exit` subcommand is renamed `quit`, matching the
  TUI command it is named after.
- **G10.** `:type-as` and `:type-as-raw` are retired — `:override-as`
  subsumes them exactly.
- **G11.** A command-line token beginning with `-` completes against the
  command's own options, as a shell's completion does.
- **G12.** `:help` opens the help overlay.

## Non-goals

- **N1. No `o` in the override selection pane.** That pane locks focus
  for the preview overlay's lifetime (spec 0185 S5) and `Enter` is
  already its apply gesture; a second, competing apply path there would
  have to define what happens to the in-flight preview. `f`/`b` paging
  is still added there (G5) — that is navigation, not application.
  `:override-as` remains *typeable* there, since `:` opens the command
  line regardless of focus (spec 0126); it simply has no key.
- **N2. No compatibility alias for `:type-as`.** G10 retires the two
  commands outright rather than aliasing them. An alias would keep two
  spellings of one operation in `COMMANDS`, in the help text and in the
  completion list — the same duplication the spec exists to remove — and
  the shorter one would keep being the one people learn.
- **N3. No validation that `<origin>` covers the cursor node.** An
  `fqdn:field` origin is *meant* to cover nodes other than the one you
  are looking at. Instead the success message reports how many nodes
  the origin now affects (S9), which is the fact a re-scope actually
  needs.
- **N4. No identifier validation on `<name>`.** Existing entries and
  restored YAML carry arbitrary strings; tightening the rule here would
  reject files that load today. Whitespace is already impossible to
  express (S3).
- **N5. Not a general `--active` flag.** `a`/Space already toggles
  active, in the pane where the entries are listed. Adding a third
  spelling buys nothing.
- **N6. No confirmation on `:quit`.** The prompt existed because `q`
  was one keystroke away from an accidental exit. `:q` plus `Enter` is
  already deliberate, and protolens holds no unsaved document state
  the way an editor does — overrides are saved explicitly with `:save`
  and were never covered by the prompt either.

## Specification

### The command

- **S1.** `override-as` joins `COMMANDS` (`tui/mod.rs:150`). No existing
  command begins with `o`, so `:o` resolves to it and `:e` keeps
  resolving to `export` — pinned by the existing
  `resolve_command_prefix_and_exact_match` test.

- **S2.** Grammar:

  ```
  :override-as [<type>] [--origin <origin>] [--field-name <name>]
  ```

  `<type>` is the sole positional. The two flags may appear in either
  order, before or after it, and each consumes exactly the next token.

- **S3.** Arguments are whitespace-split; no argument may contain a
  space. This is not merely a parsing convenience for `<name>`: the
  display name is rendered as a prototext field name
  (`field_name_for_by_path`, `override_resolve.rs:367`), and a name
  containing a space produces a document that cannot be re-parsed.

- **S4.** **Absent means default**, uniformly:

  | Argument | Absent |
  |---|---|
  | `<type>` | raw — `r#type: None`, exactly what `:type-as-raw` did |
  | `--origin` | the cursor node's `Path` origin, exactly what `:type-as` did |
  | `--field-name` | `name: None` — keep the schema-derived name |

  Because every argument is pre-filled (S6), deleting one from the
  pre-filled line is how the user asks for its default.

- **S5.** `<origin>` parses by shape, the inverse of
  `OverrideOrigin::label()`:

  | Shape | Origin |
  |---|---|
  | `/…`, no `:` | `Path { path }` |
  | `/…:<n>` | `PathField { path, field: n }` |
  | `<fqdn>:<n>` | `FqdnField { fqdn, field: n }` |

  Anything else is an error naming the three accepted shapes. The `:`
  split is on the *last* `:`, and the suffix must parse as a `u64`.

  The container is an FQDN whenever it does not begin with `/` — not
  merely when it is dotted. A top-level message in no package has a
  legal, undotted FQDN, and `/` is already the discriminator the other
  two rows turn on.

### The pre-fill

- **S6.** `o` opens the command line (`CommandLineKind::Command`)
  pre-filled with the full command for the current subject — every
  argument present, none elided. The subject is the cursor node in the
  main pane, and the highlighted entry in the manage pane.

- **S7.** Each slot's pre-filled value:

  | Slot | Main pane (cursor node) | Manage pane (highlighted entry) |
  |---|---|---|
  | `<type>` | the node's currently effective type — what it is being rendered as right now (`fqdns[span.type_fqdn]`); omitted when the node is raw | the entry's `r#type`; omitted when `None` |
  | `--origin` | the node's `Path` origin | the entry's own origin label |
  | `--field-name` | see S8 | see S8 |

- **S8.** `--field-name` pre-fills with the first of: the applicable
  entry's `name`; the schema-derived field name; `f<P>`, where `<P>` is
  `sibling_position(idx)` — the node's 1-based position among its
  siblings, the same number that forms the last segment of its
  positional path.

  **A `--field-name` equal to the schema-derived name is stored as
  `None`.** Without this, accepting the pre-fill on a schema-named
  field would write a redundant name override into the entry and into
  the saved YAML. With it, `o`-then-`Enter` is exactly a no-op, which
  is what G2 claims.

### Applying

- **S9.** Validation reuses what `:type-as` did — `can_override`, then
  the primitive-keyword wire-compatibility check — against the origin's
  **subject node**: its first affected node, falling back to the cursor
  when it currently affects none. Not the cursor unconditionally:
  editing an unrelated entry from the manage pane must not be refused
  because the main-pane cursor happens to sit on an ineligible node.
  An origin that also covers nodes of other wire types is handled where
  it already is — `render_overrides` reports a refusal per node
  (spec 0221).

  On success the message names the origin, the type and the blast
  radius, from `manage_affected_nodes`:

  ```
  /1:7 as pkg.Msg — 3 nodes
  ```

  The count is what makes a re-scope safe to perform blind: changing
  `PathField` to `FqdnField` silently widens an override from one node
  to every occurrence of that field, and this is the only place that
  widening is visible.

- **S10.** Application is `OverrideCollection::activate(origin, type)`
  followed by `rename`. `activate` already reactivates an existing
  entry with the same origin *and* type rather than duplicating it, and
  already deactivates every other entry sharing the origin (spec 0117
  §1) — so an edit that lands on an existing entry merges into it,
  which is the only non-destructive answer.

- **S11.** In the manage pane, the highlight follows the edited entry:
  changing origin or type re-sorts the collection, so the highlight is
  re-derived from the resulting entry rather than left on its index.

  The affected-node count in S9's message is taken *after* the render
  pass: an `FqdnField` origin matches on types the pass itself may have
  just resettled.

### Completion

- **S12.** `<type>` completes exactly as `:type-as`'s argument did —
  `complete_type_at` (formerly `complete_type_as_fqdn`), i.e. the
  wire-compatible primitive keywords followed by `all_type_fqdns`.

- **S13.** `<origin>` completes against the **≤3 origins
  `origin_for_kind` can build for the cursor node**, not against free
  text. This is what keeps re-scoping honest: `FqdnField` needs the
  parent's resolved type FQDN, which does not exist when the parent is
  raw, and `PathField` needs a parent at all. `origin_for_kind` already
  returns `Err` in exactly those cases (`override_apply.rs:1309`), so
  the candidate list is the set of re-scopings that are actually
  possible here — the user Tabs through them instead of typing an FQDN
  by hand and discovering the constraint from an error.

- **S14.** `start_tab_completion` currently dispatches on the command
  name with the argument assumed to be the second token
  (`command_line.rs:296-316`). It gains a `override-as` arm that
  dispatches on the *token being completed*: the token after
  `--origin` completes as an origin, the token after `--field-name`
  does not complete, and any other bare token completes as a type.

### Key bindings

- **S15.** `o` is bound in the main pane and the manage pane to open
  the pre-filled command. In the main pane an ineligible node
  (`!can_override`) leaves the existing message rather than opening the
  line; in the manage pane an empty collection does nothing.

- **S16.** `f`/`b` are added next to `PageDown`/`PageUp` in
  `handle_override_key`, `handle_manage_key` and `handle_help_key`.

- **S17.** The manage pane's `f` binding, the `manage_rename` field,
  its sub-mode block (`manage_pane.rs:313-341`), its two render sites
  (`render.rs:1550`, `1580`, including `RENAME_PREFIX`) and its arm in
  `track_message_timeout` (`render.rs:955`) are deleted.

### Pane exits and quitting

- **S18.** `Esc` becomes the sole exit from both override panes:
  `handle_override_key`'s `t` arm and `handle_manage_key`'s `o`/`q`
  arms are dropped from their `Esc` arms. The main pane's own
  `Esc`-closes-the-open-pane arms (`key_dispatch.rs:803`, `807`) are
  unchanged — they are already the same gesture from the other side.

  `t` and `m` therefore *open* rather than toggle. `toggle_override`
  and `toggle_manage_pane` keep their close-when-open branches, which
  are still reached from `open_smart_override_or_manage`, the mouse
  handlers and each other; only the key arms stop calling them to
  close.

- **S19.** The main pane's `o` (open the management pane) moves to
  `m`. `m` is unbound in every pane, and reads as "manage" beside `t`
  for "type" — the two panes' existing mnemonics.

- **S20.** `q` is unbound everywhere. Deleted: the `quit_confirm`
  field (`mod.rs:1319`, `1513`), `request_quit` (`key_dispatch.rs:399`),
  the confirmation-resolution block at the top of `handle_key`
  (`key_dispatch.rs:436-446`), the main pane's `q` arm
  (`key_dispatch.rs:635`), the empty-tree `q` arm
  (`key_dispatch.rs:501`) and the help overlay's `q` arm
  (`key_dispatch.rs:950`, leaving `Esc`/`F1`). `track_message_timeout`
  (`render.rs:955`) loses its `quit_confirm` disjunct along with its
  `manage_rename` one (S17), leaving `command_buffer` alone.

  `:quit` is unchanged and `:q` still resolves to it — no other
  command begins with `q`, and `override-as` begins with `o` (S1).
  With the empty-tree `q` arm gone that branch becomes an unconditional
  `return`, which is correct: `:` is dispatched centrally above it
  (`key_dispatch.rs:473`), so `:q` still works on an empty tree.

- **S21.** The CLI's `Command::Exit` (`main.rs:208`) is renamed
  `Command::Quit`, so the subcommand is spelled `quit`. It is the
  startup-benchmark target named after the TUI command that ends the
  session, and that command is `:quit`.

### Retiring `:type-as`

- **S22.** `:type-as` and `:type-as-raw` leave `COMMANDS`, `run_command`
  and `HELP_TEXT`; `run_type_as`, `run_type_as_raw` and `type_as` are
  deleted. No new option is needed to keep their function: `:type-as
  <fqdn>` **is** `:override-as <fqdn>` and `:type-as-raw` **is** a bare
  `:override-as`, since S4 makes an absent `--origin` the cursor node's
  `Path` origin and an absent `<type>` raw.

  `type_as`'s two responsibilities are already elsewhere:
  `validate_override_target` holds its checks (S9) and
  `run_override_as` does its own `activate`.

### Option completion

- **S23.** A registry `command_flags(cmd) -> &[&str]` sits beside
  `COMMANDS` in `tui/mod.rs`, listing each command's options. It has the
  same standing: the source of truth for a flag's *spelling* and nothing
  else — a flag listed there still needs the arm in its command's
  argument loop that acts on it.

  `start_tab_completion` checks for a leading `-` on the token being
  completed **before** dispatching to any value completer, and if it
  finds one completes against `command_flags(resolved)`. The check is
  command-independent, which is safe because no value the command line
  accepts begins with `-`: not a path, not an FQDN, not an origin
  (S5's three shapes all start with `/` or an identifier).

### `:help`

- **S24.** `help` joins `COMMANDS` with an arm setting `help_open`. The
  overlay is the one thing a newcomer cannot discover by pressing keys
  — it is what lists the keys — so it needs a spelling that does not
  require already knowing `F1`.

## Alternatives considered

**Name the command `:edit`.** `resolve_command("e") == Ok("export")` is
asserted (`tests/command_line.rs:11`); `:edit` makes `:e` ambiguous.
Breaking a resolved prefix to gain a mnemonic is a bad trade when
`:override-as` is unambiguous and says what it edits.

**Keep the rename inline and add separate origin/type editors.** Three
sub-modes instead of one command, each needing its own rendering,
cursor and completion. The command line already has all three.

**Let `--origin` accept free text with validation.** Rejected in favor
of S13's closed candidate list. Free text means the user learns the
`Path`/`PathField`/`FqdnField` constraints from error messages, one at
a time; the completion list teaches them by showing only what works.
Free text is still *accepted* (S5) — a user pasting a saved origin must
work — it is simply not what completion offers.

**A `--raw` flag instead of "absent type means raw".** A flag whose
meaning is "the positional argument I did not give" is redundant. S4's
rule is uniform across all three slots and needs no exception.

**Name the command's key `e`, leaving `o` alone.** `e` is free in both
panes and needs none of S18-S20. But `o` is the letter for *override*,
and the two dimensions it would have cost — a pane exit and a
one-keystroke quit — both had a better spelling already bound (`Esc`,
`:q`). Spending a mnemonic to avoid retiring two redundancies is the
wrong way round.

**`O` for the management pane instead of `m`.** Keeps the letter, but
`o` and `O` would then be two unrelated actions one Shift apart, in the
same pane, on the same subject. Every other capital in this app is a
*variant* of its lowercase (`z`/`Z`, `a`/`A`, `d`/`D`, `n`/`N`), and
this would be the exception that teaches the rule wrong.

**Keep `q` in the help overlay.** The overlay is not an override pane,
so G7 does not reach it, and `q`-closes-help is a common convention.
Dropped for consistency: `q` doing something in exactly one overlay,
after it stops doing anything anywhere else, is the kind of residue
that reads as a bug.

**Keep `:type-as` as a `--type` flag on `:override-as`.** The obvious
way to "keep the functionality" while retiring the command — and
unnecessary, because the functionality never needed a flag: `<type>` is
already the positional. A `--type` spelling would make the same value
expressible two ways.

**Complete options only for commands known to take them.** i.e. gate
the leading-`-` check on `resolved`. Rejected as a distinction without
a difference: `command_flags` returns an empty slice for a command with
no options, and completing against an empty list is already the
"nothing matches" path. The command-independent check is one branch
instead of a growing match.

## Test plan

A new `tui/tests/override_as.rs` holds items 1-3 and 5-9.

1. `override_as_sets_type_origin_and_name_together` — one command,
   all three dimensions changed, entry re-read from the collection.
2. `override_as_absent_arguments_take_their_defaults` — bare
   `:override-as` is what `:type-as-raw` was, on the cursor node; each
   flag omitted independently yields its S4 default.
3. `override_as_parses_all_three_origin_shapes` — `/1/2`, `/1:7`,
   `pkg.Msg:7` produce the three `OverrideOrigin` variants, an
   undotted container is still an FQDN, and three bad shapes each
   error naming all three good ones.
4. `override_as_command_rejects_a_wire_incompatible_primitive_keyword`
   (in `tests/command_line.rs`) — the S9 reuse of the `:type-as` check
   still fires.
5. `o_prefills_the_current_state_and_enter_is_a_no_op` — `o` then
   `Enter` on a schema-named node leaves the collection unchanged,
   proving S8's schema-name normalization.
6. `o_prefills_f_position_when_the_schema_names_nothing` — the `f<P>`
   fallback, on the group fixture, where the node is field 5 and the
   first child: `<P>` is the sibling position, not the field number.
7. `override_as_reports_the_affected_node_count` — an `FqdnField`
   origin reports 3 nodes where the `Path` origin reports 1.
8. `override_as_merges_into_an_existing_entry` — S10: editing an entry
   onto another's origin+type leaves one entry, not two.
9. `override_as_completes_types_and_origins` — Tab completes the
   positional as a type, before *and* after a flag; `--origin ` offers
   exactly the origins `origin_for_kind` can build; `--field-name`'s
   value completes to nothing.
10. `f_and_b_page_in_every_pane` — the override, manage and help panes,
    beside the existing main-pane
    `space_and_f_page_down_shift_space_and_b_page_up`.
11. The two `only_the_bound_ctrl_and_alt_chords_do_anything_*` gate
    tests keep passing with `o` newly bound plainly in the manage pane.
12. `esc_is_the_only_key_that_closes_the_manage_pane` and
    `t_opens_the_override_pane_and_esc_is_the_only_way_out` — `o`/`q`
    in the manage pane and `t` in the selection pane leave the pane
    open; `Esc` closes it. S18.
13. The four `m_*` manage-pane opener tests, renamed from `o_*`, plus
    `colon_opens_the_command_line_from_override_and_manage_focus`
    reaching manage focus via `m`. S19.
14. `q_no_longer_quits_and_quit_is_reachable_only_as_a_command` —
    `q` twice, `q` in the help overlay and `q` on an empty tree all
    leave `should_quit` false; `:q` sets it. Replaces the three
    `quit_confirm` tests. S20.
15. `quit_runs_the_startup_phases_and_returns` — the renamed
    `exit_runs_the_startup_phases_and_returns`, plus its
    `_accepts_the_root_options` sibling. S21.
16. `resolve_command_no_longer_knows_type_as` — both names error. S22.
17. `tab_completes_an_option_from_a_leading_dash` — `--f` on
    `:override-as`, `--o` on it, and `--desc` on `:export` (whose two
    matches yield their longest common prefix). S23.
18. `help_command_opens_the_help_overlay`. S24.

## Measured outcome

Implemented 2026-08-03. `protolens` is green at 663 unit tests and 25
integration tests.

Net effect on the command surface: `COMMANDS` goes from 7 names to 6
(`type-as` and `type-as-raw` out, `override-as` and `help` in), and the
key bindings lose three redundant spellings (`t` and `o`/`q` as pane
exits, `q` as a quit) while gaining two that do something new (`o` for
the command, `m` for the manage pane).

`manage_rename` is gone, and with it the second text-entry
implementation: its field, its sub-mode block, its `RENAME_PREFIX`
render path and its `track_message_timeout` disjunct. `quit_confirm`
takes the same three with it. `track_message_timeout`'s guard is now a
single `command_buffer.is_some()`.
