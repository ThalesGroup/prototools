<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0354 — Portable overrides: decouple save/restore from blob and descriptor set

Status: implemented
Implemented in: 2026-08-25
App: protolens
Refs: docs/specs/0117-protolens-override-collection.md
        (override collection, YAML format, `:save`/`:restore` commands),
      docs/specs/0128-protolens-yaml-untagged-overrides.md
        (untagged YAML format, `from_yaml` error wrapping)

## Background

The current `:save` / `:restore` workflow ties a saved overrides file to
the exact blob and descriptor set that were loaded when it was written.
On restore, mismatched SHA-256 hashes produce warnings; on
`--load-overrides` at startup, a descriptor-set hash mismatch is a hard
error (spec 0117 §4, `main.rs` lines ~839–845).

This coupling prevents the primary use case the grpconf2026 demo needs:

1. Open `bob/logfile` with `alice/app.desc` (Places-only descriptor set).
2. Build overrides interactively — name some fields, clarify the
   structure as far as `alice/app.desc` allows.
3. Save those overrides.
4. Reopen `bob/logfile` with `$PROTOTEXT_GOOGLEAPIS_DESC` (full
   descriptor set) **and load the saved overrides** from the command
   line, continuing where step 2 left off.

Steps 3–4 fail today: the blob hash matches (same file), but the
descriptor-set hash differs (different schema), so `--load-overrides`
aborts with a hard error.

More generally, overrides are *annotations the user built* — they belong
to the user's analysis session, not to a particular descriptor set or
even a particular binary. A `fqdn-field` override referencing
`google.maps.places.v1.SearchTextRequest` field 1 is meaningful under
any descriptor set that knows that FQDN, and meaningless to lock to the
one that was loaded when it was written.

The YAML file's `target` block (blob and descriptor-set hashes) was
designed as a consistency check, not as a correctness invariant.
This spec removes the lock while keeping the check as an informational
warning.

## Goals

- **G1.** Rename `:save` → `:save-overrides` and `:restore` →
  `:restore-overrides`, both in the command registry (`COMMANDS`) and
  everywhere they appear (key-binding hints, help text, manage-pane
  `s`/`r` pre-fills, tests). The old short names `save`/`restore` are
  removed entirely — no alias.

- **G2.** The descriptor-set hash mismatch is demoted from a hard error
  to a warning in all code paths: `--load-overrides` at startup,
  `:restore-overrides` interactively, and batch mode. Only an unreadable or
  unparseable file is a hard error.

- **G3.** The blob hash mismatch remains a warning (no change from
  current behavior for blob hash).

- **G4.** The YAML `target` block becomes optional on read. A file that
  omits it entirely loads without any hash warnings — it is an explicit
  "portable, no target anchoring" file. On write (`:save-overrides`),
  the `target` block is still written as today.

- **G5.** The manage-pane `r` key pre-fills `:restore-overrides ` (no
  default path — same as the current `:restore` pre-fill, just renamed).

- **G6.** The `s` manage-pane key pre-fill changes from `:save <path>`
  to `:save-overrides <path>` (renamed).

## Non-goals

- **N1.** *Changing the YAML format's `overrides` entries.* The entry
  structure (`path`/`path-field`/`fqdn-field`, `type`, `active`,
  `name`) is unchanged.

- **N2.** *Stripping the `target` block from written files.* The hashes
  are still written so the reader can see at a glance what file and
  schema the overrides were built against. Removing them would lose that
  provenance.

- **N3.** *Adding a `--no-target-check` flag or similar.* The
  descriptor-set hash mismatch being a hard error was the only thing
  that needed fixing; demoting it to a warning (G2) is sufficient.

- **N4.** *Per-entry portability metadata.* Every entry is already as
  portable as its origin kind allows — a `fqdn-field` entry is
  schema-portable by construction; a `path` entry is blob-portable. No
  new per-entry fields are needed.

- **N5.** *Changing the CLI flag name `--load-overrides`.* It already
  has an intuitive name and requires no change.

## Specification

### S1 — Command renames

In `COMMANDS` (`mod.rs`):

- Remove `"save"` and `"restore"`.
- Add `"save-overrides"` and `"restore-overrides"`.

In `command_line.rs`'s dispatch match:

```rust
Ok("save-overrides") => self.run_save_overrides(tokens.collect()),
Ok("restore-overrides") => self.run_restore_overrides(tokens.collect()),
```

In `complete_command_name`'s path-completion branch:

```rust
"save-overrides" | "restore-overrides" => self.complete_fs_path(cmd, rest),
```

In `run_save_overrides`: error prefix changes from `"save: missing path"`
to `"save-overrides: missing path"`.

In `run_restore_overrides`: error prefix changes from `"restore: missing
path"` to `"restore-overrides: missing path"`, and the success/warning
message changes from `"restored overrides from {path}"` to
`"loaded overrides from {path}"`.

### S2 — Manage-pane pre-fill strings

In `handle_manage_key` (or whichever function generates the pre-fill
strings for `s` and `r`):

- `s` pre-fills `:save-overrides <default path>` (was `:save <path>`).
- `r` pre-fills `:restore-overrides ` (was `:restore `).

### S3 — Descriptor-set hash mismatch demoted to warning everywhere

In `load_overrides` (`command_line.rs`): the
`descriptor_set_sha256` comparison already pushes a string onto
`warnings` and sets `hash_mismatch = true`. No logic change here —
the warning already exists.

In `main.rs` batch/startup path: remove the block that turns
`hash_mismatch` into a hard error. Instead treat `hash_mismatch` the
same as any other warning: print to stderr with a `warning:` prefix and
continue.

Before this spec, `main.rs` contained roughly:

```rust
if load.hash_mismatch {
    eprintln!("error: --load-overrides: the overrides file was written against a \
               different descriptor set …");
    std::process::exit(1);
}
```

After this spec that block is removed; warnings (including
descriptor-set hash mismatch) are printed as `warning: --load-overrides:
{w}` and startup continues.

### S4 — Optional `target` block on read

`from_yaml` currently requires `target.blob_sha256` and
`target.descriptor_set_sha256`. Change the `YamlEnvelope` struct so
`target` is `Option<YamlTarget>`. When `target` is absent, `from_yaml`
returns a sentinel `YamlTarget` with both hashes set to the empty
string `""`.

In `load_overrides`: when the returned `YamlTarget` has both hashes
empty (the "no target" sentinel), skip both hash comparisons entirely —
no warnings are produced for a targetless file.

The sentinel value `""` is safe because `target_hashes` never returns
an empty string: SHA-256 of any byte sequence produces a 64-character
hex string.

### S5 — Help text and key-binding hints

Update every occurrence of `:save`, `:restore`, `save overrides`,
`restore overrides` in `help_text.rs` and any status/message strings to
use the new command names.

## Alternatives considered

**Keep `save`/`restore` as aliases alongside the new names.** Aliases
bloat the `COMMANDS` list, make prefix completion ambiguous between
`save` and `save-overrides`, and give two spellings for one command with
no benefit — the old names were internal, not user-facing documentation.
Removing them is cleaner; the project has no backward-compatibility
commitment for command names (same precedent as spec 0156's renames).

**Make the descriptor-set check a flag (`--skip-descriptor-check`).** A
flag requires documentation, a new CLI argument, and a decision about
whether it applies to the interactive `:restore-overrides` command too.
Demoting to a warning achieves the same result everywhere with no new
surface.

**Strip `target` from written files entirely.** Provenance is useful —
a saved file with `target` hashes tells the reader exactly which blob
and schema it was built against. Keeping the write intact costs nothing
and preserves that information for cross-session inspection or `git diff`
review.

## Test plan

1. `save_overrides_command_replaces_save` — `:save-overrides` resolves;
   `:save` no longer resolves (returns unknown-command error).
2. `load_overrides_command_replaces_restore` — `:restore-overrides` resolves;
   `:restore` no longer resolves.
3. `descriptor_set_mismatch_is_a_warning_not_an_error` — `load_overrides`
   with a mismatched descriptor-set hash returns `Ok` with a non-empty
   `warnings` list, not `Err`.
4. `targetless_yaml_loads_without_warnings` — a YAML file with no
   `target` block loads via `from_yaml` and produces no hash warnings in
   `load_overrides`.
5. `manage_pane_s_prefills_save_overrides` — the `s` key in the manage
   pane produces a command line starting with `:save-overrides `.
6. `manage_pane_r_prefills_load_overrides` — the `r` key produces
   `:restore-overrides `.
7. Existing `resolve_command_reflects_the_0156_renames` test updated:
   `save-overrides` and `restore-overrides` now resolve; `save` and
   `restore` do not.
8. `reuse lint` passes.

## Measured outcome

(to be filled in after implementation)
