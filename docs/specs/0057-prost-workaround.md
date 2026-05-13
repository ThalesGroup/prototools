<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0057 — reproto: --prost-workaround flag for editions FDP patching

**Status:** implemented
**Implemented in:** 2026-05-12
**App:** reproto

---

## Background

prost-reflect 0.16.x (the Rust reflection library used by `prototext-core`)
does not support protobuf editions 2023.  When `DescriptorPool::decode` is
called on a `FileDescriptorSet` containing a FDP with `syntax = "editions"`,
prost-reflect panics with an index-out-of-bounds error in its internal
`visit_file` path.

This blocks `prototext` from loading any `schemas.pb` or `--descriptor` file
that was built from a corpus containing editions files — even if the user only
wants to render proto2/proto3 messages from that corpus.

Upstream work is in progress (prost PR #1347) but not yet merged.  As a
stopgap, reproto can optionally patch editions FDPs to make them appear as
proto2 to prost-reflect.

---

## Root cause

A FDP produced by `protoc` for an editions file carries:

```
syntax  = "editions"
edition = EDITION_2023
```

along with per-field `FieldOptions.features` entries such as:

```
features { field_presence: IMPLICIT }
features { field_presence: LEGACY_REQUIRED }
features { message_encoding: DELIMITED }
features { repeated_field_encoding: EXPANDED }
```

Proto2 FDPs omit the `syntax` field entirely (protoc leaves it empty/unset).
prost-reflect's `visit_file` treats any non-empty, non-`"proto2"`,
non-`"proto3"` value as an unknown syntax and panics before the file is
registered in its internal pool.

---

## Goals

1. Add a `--prost-workaround` flag to reproto.
2. When the flag is set:
   - In `--build-schema-db` mode: patch each editions FDP in `schemas.pb`
     before writing it to disk — clear `syntax` and `edition` fields.
   - In rendering mode (normal reproto run): for editions input files, force
     proto2 output (equivalent to `--force-proto2-output` scoped to editions
     files only).  The output FDP written to disk will have no `syntax` or
     `edition` field.
3. The `.rkyv` scoring graph is **not** affected — it is built from the
   unpatched pool.
4. The patch is opt-in and not the default, so that correct behaviour is
   preserved once prost-reflect gains editions support.
5. A warning is emitted to stderr for each patched file, naming the file and
   noting the limitation.

---

## Non-goals

- Translating `features` entries into equivalent proto2 field options (e.g.
  `LEGACY_REQUIRED` → `label = REQUIRED`, `DELIMITED` → group wire type).
  The patch is intentionally shallow: it only silences the prost-reflect
  panic.  Rendering and instantiation of fields that use non-default editions
  features will be semantically incorrect.
- Fixing prost-reflect itself.  That is tracked separately upstream.

---

## Correctness contract

The patched output is **correct** for editions files that use only
proto3-equivalent defaults (the common case for corpora that have adopted
editions purely as a syntax upgrade).  It is **silently wrong** for fields
that rely on non-default features:

| Feature | Expected behaviour | Patched behaviour |
|---|---|---|
| `field_presence: LEGACY_REQUIRED` | Required field | Optional field |
| `message_encoding: DELIMITED` | Group wire encoding | Length-delimited |
| `repeated_field_encoding: EXPANDED` | Unpacked repeated | Default packing |
| `field_presence: IMPLICIT` | No hasbits | Hasbits present (proto2) |

Users must be aware of these limitations when using `--prost-workaround`.

---

## Specification

### CLI

`--prost-workaround` is a boolean flag meaningful in all reproto modes:

```
# Rendering mode
reproto --prost-workaround -I <pb_dir> --output-root <out> .

# DB build mode
reproto --prost-workaround --build-schema-db <path> -I <pb_dir> .
```

It is registered in `cli.py` alongside the other rendering flags.

---

### Context field

`context.py`:

```python
prost_workaround: bool = False
```

Set from `cli.py` immediately after argument parsing, alongside
`force_proto2_output`.

---

### Rendering mode (`re_file.py`)

In `ReFileDescriptorProto.render()`, where `ctx.target_syntax` is set:

```python
ctx.syntax = fdp_syntax(self.this)
if not ctx.force_proto2_output and ctx.syntax in ("proto2", "proto3", "editions"):
    ctx.target_syntax = ctx.syntax
else:
    ctx.target_syntax = "proto2"
```

Change to:

```python
ctx.syntax = fdp_syntax(self.this)
if ctx.force_proto2_output:
    ctx.target_syntax = "proto2"
elif ctx.prost_workaround and ctx.syntax == "editions":
    ctx.target_syntax = "proto2"
    _warn_prost_workaround(self.name)
elif ctx.syntax in ("proto2", "proto3", "editions"):
    ctx.target_syntax = ctx.syntax
else:
    ctx.target_syntax = "proto2"
```

When `target_syntax` is forced to `"proto2"` for an editions file, the
existing proto2 rendering path already omits the `syntax` and `edition`
statement lines from the `.proto` output (proto2 files conventionally omit the
`syntax` line).  The output FDP, when re-serialised via `protoc`, will
therefore have no `syntax` or `edition` field.

---

### DB build mode (`phases.py`, `_phase_build_schema_db`)

Immediately before writing `schemas.pb`, after the FDS is assembled from
`ctx.pool_db`:

```python
if ctx.prost_workaround:
    for fdp in fds.file:
        if fdp.syntax == "editions":
            _warn_prost_workaround(fdp.name)
            fdp.ClearField("syntax")
            fdp.ClearField("edition")
```

Note: `ClearField("syntax")` leaves the field unset (empty string in
protobuf), which is how proto2 FDPs are written by protoc — consistent with
`fdp_syntax()` treating `""` as proto2.

---

### Warning helper

A module-level helper in `phases.py`, using `cli_warning` from
`reproto.lib.warnings`:

```python
def _warn_prost_workaround(filename: str) -> None:
    cli_warning(
        "'%s' is an editions file; patching to proto2 for prost-reflect "
        "compatibility (--prost-workaround). Fields using non-default "
        "editions features (LEGACY_REQUIRED, DELIMITED, EXPANDED, IMPLICIT) "
        "will be rendered incorrectly. Remove --prost-workaround once "
        "prost-reflect supports editions (upstream PR #1347).",
        filename,
    )
```

---

### cli.py registration

```python
'--prost-workaround': 'Workarounds',
```

```python
parser.add_argument(
    '--prost-workaround',
    action='store_true',
    default=False,
    help=(
        'Patch editions-syntax FDPs to appear as proto2 in output and '
        'schemas.pb, working around a prost-reflect limitation '
        '(upstream PR #1347). Editions fields using non-default features '
        '(LEGACY_REQUIRED, DELIMITED, EXPANDED, IMPLICIT) will be '
        'rendered incorrectly. Remove once prost-reflect supports editions.'
    ),
)
```

---

## Usage in the stress-test harness

`tests/stress/test_stress.py` passes `--prost-workaround` to the reproto
invocation that builds `stress.rkyv`.  The editions fixture
(`editions_rendering.proto`) is retained in `STRESS_PROTOS` so it contributes
to the scoring graph, and `.reproto.test.rendering.AllFeatures` is kept in
`types.yaml` with a caveat comment:

```yaml
  # - Editions 2023: DELIMITED encoding, IMPLICIT/EXPLICIT presence,
  #   LEGACY_REQUIRED.
  #   NOTE: rendered as proto2 approximation due to --prost-workaround;
  #   remove caveat once prost-reflect supports editions (upstream PR #1347).
  - .reproto.test.rendering.AllFeatures
```

---

## Future removal

Once prost-reflect merges editions support:

1. Remove `--prost-workaround` from the reproto invocation in
   `test_stress.py`.
2. Remove the `--prost-workaround` flag, `prost_workaround` context field,
   `_warn_prost_workaround` helper, and patch logic from reproto.
3. Remove the limitation comment from `types.yaml`.
4. Update this spec status to `superseded`.
