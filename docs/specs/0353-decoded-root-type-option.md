<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0353 — `Decoded::root_type: Option<String>`

Status: implemented
Implemented in: 2026-08-24
App: protolens
Refs: docs/specs/0168-protolens-resolve-root-type-before-decode.md
        (`Decoded` structure and root-type resolution pipeline),
      docs/specs/0352-node-kind-replaces-is-message.md
        (exposed the sentinel in three new tests, motivating this cleanup)

## Background

`Decoded::root_type` is declared as `String`. The absence of a root type
is encoded as the sentinel value `"<raw / no type>"`, produced in one
place (`render_resolved`, `decode.rs` line ~2191) and checked by string
comparison in `App::new`:

```rust
let root_override_type = if decoded.root_type == "<raw / no type>" {
    None
} else {
    Some(decoded.root_type.clone())
};
```

This is stringly-typed: a typo in any producer or consumer silently
passes the compiler and causes wrong behaviour at runtime. The sentinel
string also collides visually with the management pane's display label
for an entry whose `r#type` is `None` (`manage_entry_type_label` returns
`"<raw / no type>"` for such entries), even though the two concepts are
unrelated:

- `Decoded::root_type == "<raw / no type>"` — the decode pipeline found
  no root-type descriptor; the tree is rendered raw.
- `entry.r#type == None` — the user explicitly cleared an override,
  requesting raw bytes for a specific node.

The collision surfaced concretely in spec 0352's erratum tests, which
had to use `root_type: "<raw / no type>".to_string()` to prevent
`App::new` from seeding a spurious root override entry, with a comment
explaining why — a sign that the sentinel is not self-evident.

## Goals

- **G1.** Change `Decoded::root_type` from `String` to `Option<String>`,
  where `None` means "no root type resolved" and `Some(fqdn)` means the
  root was decoded as `fqdn`.
- **G2.** Remove every `== "<raw / no type>"` string comparison in
  favour of pattern matching on the `Option`.
- **G3.** `App::new`'s title line and root-override seeding use the
  `Option` directly.
- **G4.** Every test that previously wrote
  `root_type: "<raw / no type>".to_string()` writes `root_type: None`.
  Every test that wrote a real FQDN writes `root_type: Some(fqdn)`.

## Non-goals

- **N1.** *Keeping `manage_entry_type_label`'s display string separate.*
  That function's display for a `None` entry type changes to `"(no type)"`
  alongside the other presentation sites (S4) — unifying the UI label —
  but its logic and data model are otherwise untouched.
- **N2.** *Changing `RootType` (the decode-time request enum).* That is
  a separate type controlling what the caller *asks for*, not what the
  pipeline *produces*. Renaming or restructuring it is out of scope.
- **N3.** *Changing `decode::decode`'s public API beyond `Decoded`.*
  `resolve_root_type_and_arena` and related functions are not part of
  this change.

## Specification

- **S1.** Change `pub root_type: String` to `pub root_type: Option<String>`
  in `Decoded` (`decode.rs`).

- **S2.** In `render_resolved` (`decode.rs`), change:
  ```rust
  None => ("<raw / no type>".to_string(), None),
  ```
  to:
  ```rust
  None => (None, None),
  ```
  and adjust the `root_type` field of the returned `Decoded` accordingly.

- **S3.** In `App::new` (`tui/mod.rs`), replace the string-comparison
  guard:
  ```rust
  let root_override_type = if decoded.root_type == "<raw / no type>" {
      None
  } else {
      Some(decoded.root_type.clone())
  };
  ```
  with:
  ```rust
  let root_override_type = decoded.root_type.clone();
  ```

- **S4.** All presentation sites that display the root type when absent
  use `"(no type)"` instead of `"<raw / no type>"`:

  - `App::new` title line:
    ```rust
    let header = format!(
        "protolens — {blob_label} — {}",
        decoded.root_type.as_deref().unwrap_or("(no type)")
    );
    ```
  - `main.rs` status line: replace `.unwrap_or("<raw / no type>")` with
    `.unwrap_or("(no type)")`.
  - `manage_entry_type_label` (management pane, `manage_pane.rs`):
    replace `"<raw / no type>"` with `"(no type)"`. This entry displays
    the type of a user override whose `r#type` is `None` — a different
    concept from the root type, but the same display string is now used
    consistently for "no type" throughout the UI.

  `"(no type)"` is shorter, reads naturally, and the parentheses signal
  metadata rather than a keyword or FQDN. The angle-bracket form was
  never a deliberate choice — it was a placeholder that outlived its
  draft status.

- **S6.** All test sites that set `root_type: "<raw / no type>".to_string()`
  become `root_type: None`. All sites that set `root_type: some_fqdn.to_string()`
  become `root_type: Some(some_fqdn.to_string())`.

- **S7.** All `assert_eq!(x.root_type, "<raw / no type>")` assertions
  become `assert_eq!(x.root_type, None)`. All assertions against a real
  FQDN gain `Some(...)`.

## Alternatives considered

**Keep the sentinel, define it as a named constant.**  A
`const RAW_ROOT_TYPE: &str = "<raw / no type>"` would prevent typos and
make grep easier, but the type would still be `String` and the compiler
would still accept a plain string in its place. `Option<String>` makes
the absent case unrepresentable as a valid FQDN. The constant would also
perpetuate the awkward `<raw / no type>` string rather than replacing it.

**`"(no type)"` only in the title, keep `"<raw / no type>"` in the
management pane.** The two strings label the same concept ("this node
has no type assigned") in two different UI surfaces. Using the same
`"(no type)"` string everywhere is simpler and more consistent.

**Use an enum `RootKind { Raw, Named(String) }`.** This is isomorphic to
`Option<String>` for protolens's needs but introduces a new type that
every call site must import. `Option` is already in scope everywhere and
its semantics (`None` = absent, `Some` = present) are immediately clear.

## Test plan

1. `root_type_none_on_raw_decode` — `decode` with `RootType::Raw` sets
   `Decoded::root_type = None`.
2. `root_type_some_on_named_decode` — `decode` with a resolvable FQDN
   sets `Decoded::root_type = Some(fqdn)`.
3. `no_resolved_root_type_seeds_no_override_and_still_renders_raw` —
   existing test, now asserting `root_type == None` instead of the
   sentinel string.

## Measured outcome

`Decoded::root_type` changed from `String` to `Option<String>`. The
sentinel `"<raw / no type>"` is removed from the data layer entirely;
the string-comparison guard in `App::new` replaced by a direct `clone()`
of the `Option`. Presentation sites updated to `"(no type)"`:
`App::new`'s title line, `main.rs`'s startup announcement, and
`manage_entry_type_label`. Three test initializer sites changed to
`root_type: None`; ten to `root_type: Some(fqdn)`. Three assert
sites changed to `assert_eq!(x.root_type, None)` or `Some(fqdn)`.

1221 protolens tests, 25 theme tests, 3 batch tests, 134
prototext-core tests — all pass.
