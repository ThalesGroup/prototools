<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0360 — override selection pane: Enter/`o` derive field name from type

Status: draft
App: protolens
Refs: docs/specs/0236-an-override-is-edited-as-one-command.md
      (introduced `--field-name` and spec 0236 N4, which explicitly
      deferred identifier validation; spec 0236 S8, the four-derivation
      chain this spec extends);
      docs/specs/0237-the-origin-is-the-argument.md
      (introduced `field_name_candidates`, which this spec's derivation
      parallels)

## Background

When the user presses `Enter` (or double-clicks) a type in the override
selection pane, `activate` is called with `name: None` — the entry's
display name is never set.  The field is therefore rendered either with
its schema-derived name (if the parent's type is known) or as a bare
field number (`field_name_for`'s fallback).

The type name itself contains the most useful signal: the last
leg of `google.maps.places.v1.SearchTextRequest` is
`SearchTextRequest`, which snake-cased is `search_text_request` — a
field name the user would otherwise have to type by hand via `o` →
`--field-name`.  Surfacing it automatically removes a routine manual
step and makes the selection pane self-contained for the common case.

## Goals

- **G1.** When `Enter`/double-click commits a type in the override
  selection pane, derive a field name from the selected FQDN and store
  it as `entry.name` — skipping the `o` → `--field-name` round trip.
- **G2.** The derivation is a pure function of the FQDN and the set of
  names already in use by sibling active entries at the same level: no
  UI prompt, no interaction.
- **G3.** When the `None` sentinel is confirmed (raw bytes, no type),
  no field name is set.
- **G4.** When `o` is pressed in the selection pane to open a prefilled
  `:override` command, the `--field-name` value is derived by the same
  rule — the highlighted FQDN's snake-case last leg, with the same
  fallback chain — so that `o`-then-`Enter` produces the same name as a
  plain `Enter`.

## Non-goals

- **N1.** No change to the `:override` command path when typed by hand
  or opened from the management pane — only the selection pane's `Enter`
  and `o` paths are affected.
- **N2.** No change when the pane was opened from the management pane
  and the entry already has a stored name: silently replacing a name the
  user deliberately set would be surprising.  If the entry's stored name
  is `None` (it was created before this spec, or was created untyped),
  the retype path applies the same derivation as `Enter` (S2).
- **N3.** No identifier validation beyond the rules below.  Spec 0236
  N4's position stands: any string is accepted as a name; what this spec
  adds is an automatic choice, not a gate.

## Specification

### S1 — Derivation

Given the selected FQDN (a non-empty string, never `None` — S3 handles
that case), derive a candidate field name as follows:

If the schema already provides a name for the field (i.e.
`schema_field_name(idx)` returns `Some`), return `None` immediately —
storing a derived name would shadow the schema name in rendering, which
mirrors spec 0236 S8's rule that a stored name equal to the schema name
is redundant.

Otherwise:

1. **Last-leg snake case.** Take the last `.`-separated segment of the
   FQDN.  Convert it from UpperCamelCase to snake_case using the
   algorithm in S4.
   Example: `SearchTextRequest` → `search_text_request`;
   `Status` → `status`.

   Use this as the primary candidate if it is not already in use by a
   sibling.

2. **`fN` fallback.** If the primary candidate is in use, try `f<N>`
   where `<N>` is the node's wire field number.

3. **No name.** If `fN` is also in use (or the node's field number is
   0, the virtual-wrapper sentinel), return `None`.

"Already in use" means: an active entry whose first affected node shares
`idx`'s parent has that name as its `field_name_for` result.

### S2 — Call site and factoring

Extract a helper:

```rust
fn derive_field_name(fqdn: &str, idx: usize, app: &App) -> Option<String>
```

It implements S1's three-step derivation (snake-case last leg → `fN` →
`None`) and is the single implementation shared by every call site below.

**`Enter` in the selection pane** (`KeyCode::Enter` arm of
`handle_override_key`): call `derive_field_name` on `new_fqdn`, pass
the result to `activate_with_name`.

**`o` prefill** (`prefill_override_cmd`): see S5.

**Retype from the management pane** (`override_opened_from_manage`
path): after `overrides.activate`, if the entry's stored `name` was
`None` before the call and `new_fqdn` is `Some`, apply
`derive_field_name` and write the result into the entry's `name` field
directly.  If the stored name was already `Some`, leave it untouched.

`activate_with_name` behaves like `activate` in every respect, except
that when it pushes a new entry it stores the supplied `name` instead of
`None`, and when it reactivates an existing entry it **does not**
overwrite a stored `Some` name (the entry may have been renamed by the
user via `o` → `--field-name` since it was first created), but **does**
fill in a `None` name with the derived value.

### S3 — `None` sentinel

When `override_candidates[override_highlight]` is `None` (the raw
sentinel), no type was selected, so no derivation is attempted and
`name: None` is stored — same as today.

### S4 — Snake-case algorithm

Google's rule: a run of consecutive uppercase letters is one word.
An underscore is inserted before an uppercase letter when either:

- it is preceded by a lowercase letter or digit (`aB` → `a_b`), or
- it is preceded by an uppercase letter and followed by a lowercase
  letter (`ABc` → `a_bc` at the `B`).

```rust
fn fqdn_last_leg_to_snake(fqdn: &str) -> String {
    let last = fqdn.rsplit('.').next().unwrap_or(fqdn);
    let bytes = last.as_bytes();
    let mut out = String::with_capacity(last.len() + 4);
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_uppercase() && i > 0 {
            let prev_lower = bytes[i - 1].is_ascii_lowercase()
                || bytes[i - 1].is_ascii_digit();
            let next_lower = bytes
                .get(i + 1)
                .map_or(false, |nb| nb.is_ascii_lowercase());
            let prev_upper = bytes[i - 1].is_ascii_uppercase();
            if prev_lower || (prev_upper && next_lower) {
                out.push('_');
            }
        }
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}
```

Examples: `SearchTextRequest` → `search_text_request`;
`HTTPRequest` → `http_request`; `getHTTPSURL` → `get_https_url`;
`Status` → `status`; `ComputeRoutesRequest` → `compute_routes_request`.

### S5 — `o` prefill

`prefill_override_cmd` builds the `:override` command string and calls
`display_name_for(node, entry_name)` for the `--field-name` token.
`display_name_for` uses `field_name_candidates`, which returns the
four-derivation chain (spec 0236 S8): stored name, schema name, `fN`,
`pP`.

When called from the selection pane (not `override_opened_from_manage`)
and a non-None FQDN is highlighted, prepend the snake-case derivation
(S4) as derivation **(0)** — before the stored name — so the chain
becomes: snake-case last leg, stored name, schema name, `fN`, `pP`,
with duplicates dropped.  The duplicate-drop already in
`field_name_candidates` ensures that if the snake-case name coincides
with the stored name or schema name, it does not appear twice.

The FQDN to use is the same `effective` variable that
`prefill_override_cmd` already computes for `--as`.  No new lookup is
needed.

## Alternatives considered

### Prompt the user before committing

Rejected (G2).  The selection pane's contract is that `Enter` commits
immediately; adding a prompt would break that contract and make every
`Enter` slower for users who accept the derived name.  The `o` escape
hatch is already one key away for users who want a different name.

### Always set the name, even when it duplicates a sibling

Rejected.  Duplicate names in the same parent produce ambiguous
prototext output and confuse the export path.  Falling back to `fN`
gives a unique name in every ordinary case.

### Use the full FQDN as the name

Too long for display; proto field names are single identifiers, not
qualified paths.

## Test plan

1. `enter_sets_snake_case_name` — `Enter` on `google.maps.places.v1.SearchTextRequest`
   sets `name = Some("search_text_request")`.
2. `enter_on_none_sentinel_sets_no_name` — `Enter` on the `None` row
   leaves `name = None`.
3. `duplicate_name_falls_back_to_fN` — if `search_text_request` is
   already in use by a sibling entry, the new entry gets `f<N>`.
4. `duplicate_fN_sets_no_name` — if both `search_text_request` and
   `f<N>` are in use, `name = None`.
5. `retype_from_manage_preserves_existing_name` — confirming a retype
   from the management pane does not change a stored `Some` name.
5b. `retype_from_manage_fills_missing_name` — confirming a retype from
   the management pane on an entry with `name: None` derives and stores
   the snake-case name from the new FQDN.
6. `snake_case_single_word` — `Status` → `status`.
7. `snake_case_camel` — `ComputeRoutesRequest` → `compute_routes_request`.
8. `snake_case_acronym` — `HTTPRequest` → `http_request`.
9. `snake_case_trailing_acronym` — `getHTTPSURL` → `get_https_url`.
10. `o_prefill_uses_same_derivation` — `o` with `SearchTextRequest`
    highlighted produces `--field-name search_text_request` in the
    prefilled command, matching what `Enter` would store.
11. `reuse lint` passes.

## Measured outcome

(To be filled in at implementation.)
