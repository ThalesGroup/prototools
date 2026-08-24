<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0348 — override cmd: prefill --as and --card from selection pane

Status: implemented
Implemented in: 2026-08-24
App: protolens (src/tui/override_cmd.rs, src/override_pane.rs,
      src/tui/override_apply.rs, src/tui/override_select.rs)
Refs: docs/specs/0236-override-cmd.md (introduced `:override` and `o`),
      docs/specs/0237-override-cmd-origin-flag.md (introduced --as,
      --field-name, Tab completion for origin and type),
      docs/specs/0253 (introduced --card / Cardinality to register_wrapper)

## Background

`o` in the override selection pane opens a pre-filled `:override` command
line (spec 0236 S15).  The current pre-fill includes the origin,
`--as <type>`, and `--field-name <name>` — three of the four dimensions
that a committed override carries.

The fourth dimension is **cardinality** (`optional` / `repeated` /
`required`).  Cardinality is already threaded through
`register_wrapper` via `field_cardinality` (spec 0253), which reads it
from the parent field's schema descriptor and falls back to `Optional`
when the parent is unknown.  However the `:override` command has no
`--card` flag, so the caller cannot override the cardinality, and the
pre-fill does not show what value is currently in effect.

Two problems result:

1. When the schema says `repeated` but the user wants to assert that a
   particular occurrence is `optional` (or vice versa — e.g. the
   descriptor is wrong or unavailable), there is no way to do so from
   the command line.
2. The selection pane's `o` line is silent about cardinality, so the
   reader cannot confirm what value will be used before committing.

## Goals

- **G1.** Add a `--card <cardinality>` flag to the `:override` command.
  Accepted values: `optional`, `repeated`, `required` (case-sensitive,
  matching `prost_reflect::Cardinality`'s display names).
- **G2.** When `--card` is absent, cardinality is derived from the parent
  field's schema exactly as today — no semantic change to existing
  invocations.
- **G3.** `o` in the override selection pane pre-fills `--as <type>` (as
  today) **and** `--card <cardinality>` with the cardinality currently in
  effect for the target node (`field_cardinality`).
- **G4.** Tab on the `--card` value rotates through `optional` →
  `repeated` → `required` → `optional` (cyclic), unfiltered — the same
  rotation style as `--field-name`.
- **G5.** When `--card` is present on a committed `:override` line, the
  stored entry carries the explicit cardinality and `register_wrapper` /
  `splice_override` use it in place of the schema-derived one.

## Non-goals

- **N1.** `o` from the **management** pane is not changed.  The
  management pane pre-fills the highlighted entry's stored values; it
  does not re-derive anything from the selection pane's target.  If an
  entry was committed without `--card`, its stored cardinality is absent,
  and the pre-fill says so — the user who wants to add it edits the line.
- **N2.** The `:type-as` shorthand command is not extended.  Only
  `:override` gains `--card`.
- **N3.** The YAML serialisation format of override entries is not changed
  in this spec.  Persisting `card` to YAML is a follow-up.

## Specification

**S1.** Add `cardinality: Option<Cardinality>` to `OverrideArgs`
(`override_cmd.rs`).  `None` means absent — inherit from schema,
as today.

**S2.** In `parse_override`, recognise `--card` as a flag that consumes
the next token and parses it as one of the three literal strings
`optional`, `repeated`, `required`.  Any other value is a parse error:

```
override: --card value must be optional, repeated, or required
```

**S3.** In `prefill_override_cmd`, when the caller is the **selection
pane** (i.e. the `entry.is_none()` branch — the management pane branch is
excluded by N1):

- Pre-fill `--as` with `effective_type(idx)` (what the node is currently
  rendered as).  When `effective_type` returns `None` (raw node with no
  active override and no natural type), fall back to the type currently
  highlighted in the selection pane's candidate list, excluding the `none`
  sentinel — so navigating to a type and pressing `o` always produces a
  usable `--as` value.
- Add `--card <cardinality>` immediately after `--as <type>`.  The value
  comes from `field_cardinality(idx)`, formatted as its lowercase string:

```
:override /path/to/node --as google.foo.Bar --card optional --field-name f4
```

`field_cardinality` already falls back to `Cardinality::Optional` when no
schema parent exists, so the pre-fill is always well-defined.

**S4.** In `complete_override_cmd`, recognise `--card` as a flag whose
value token is subject to Tab completion.  When the previous token is
`--card`, call a new `complete_card` helper that performs an **unfiltered
rotation** through `["optional", "repeated", "required"]` — the same
`apply_rotation` pattern `complete_field_name` uses, with no prefix
filter.

**S5.** In `run_override_cmd`, thread the parsed `cardinality` into
`activate` (or a new parallel path) so that the committed entry carries it.
When `cardinality` is `None`, the entry's cardinality field stays `None`
and `register_wrapper` / `splice_override` fall back to `field_cardinality`
as today.  When `cardinality` is `Some(c)`, the wrapper is registered and
the splice is applied with `c`, ignoring the schema.

**S6.** `OverrideEntry` gains an `Option<Cardinality>` field.
`OverridePaneEntries::activate` stores it.  The re-apply path
(`render_overrides`, `splice_override`) reads it with the same
`field_cardinality` fallback as S5.

**S7.** In `complete_override_cmd`'s dispatch, add `"--card"` to the
flag-value branch beside `"--field-name"` and `"--as"`:

```rust
Some("--card") => self.complete_card(token_start, token),
```

## Alternatives considered

**Prefill `--card` always (even from the management pane).** Rejected:
the management pane pre-fills a stored entry, and a stored entry that
carries no explicit cardinality should not silently acquire one on edit —
`o`-then-`Enter` in the management pane must remain a no-op (spec 0236
S6).

**Use `--cardinality` instead of `--card`.** Longer and less aligned with
the command line convention of short flag names for frequent options.
`--card` is unambiguous given the existing flag set.

**Omit `--card` from the pre-fill when cardinality is `optional`** (the
default).  Ruled out: the pre-fill's contract (spec 0236 S6) is that
`o`-then-`Enter` is a no-op.  If the field_cardinality happens to be
`optional` and we omit `--card`, then when the user edits only `--as` and
presses `Enter`, the entry is stored without an explicit cardinality — fine
for now, but loses the "what you see is what is committed" invariant.
Always showing `--card` is clearer.

## Test plan

1. `prefill_includes_card` — open the selection pane on a schema-declared
   `repeated` field; press `o`; assert the command line contains
   `--card repeated`.
2. `prefill_card_optional_fallback` — open on a field whose parent type is
   unresolved (no schema); assert the command line contains
   `--card optional` (the `field_cardinality` fallback value).
3. `tab_card_rotates` — with `--card optional` on the line and the cursor
   on `optional`, press Tab; assert the value becomes `repeated`; press
   Tab again; assert `required`; press Tab again; assert `optional`.
4. `run_card_overrides_schema` — commit `:override /path:4 --as bytes
   --card required`; assert the stored entry's cardinality is `Required`;
   assert `register_wrapper` is called with `Cardinality::Required` rather
   than the schema-derived value.
5. `run_card_absent_inherits_schema` — commit `:override /path:4 --as
   bytes` (no `--card`); assert the stored entry's cardinality is `None`;
   assert `register_wrapper` uses the schema cardinality.
6. `parse_bad_card_value` — parse `:override /p --card foobar`; assert an
   error containing `"optional, repeated, or required"`.

## Measured outcome

`OverrideEntry` gains `cardinality: Option<Cardinality>` (not persisted to
YAML, per N3). `parse_override` recognises `--card optional/repeated/required`.
`prefill_override_cmd` emits `--as <type>` and `--card <cardinality>` in the
selection-pane branch; when `effective_type` returns `None` (raw node), `--as`
falls back to the currently highlighted candidate (excluding `none`).
The management-pane branch is unchanged. `run_override_cmd` normalises away a
`--card` value that already matches `field_cardinality` (mirrors the
`--field-name` normalization, spec 0236 S8), so `o`-then-`Enter` remains a
no-op. Both `splice_override` and `warm_visible_override_wrappers` read the
stored cardinality first and fall back to `field_cardinality`.

All 1235 protolens tests pass.
