<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0024 — Rendering anomaly taxonomy and reporting specification

**Status:** draft
**App:** reproto

---

## Purpose

This spec catalogues every place in the reproto rendering pipeline where
something is "off" — a descriptor value that cannot be faithfully reproduced
in the target syntax, a structural inconsistency, or a recoverable rendering
failure.  It is also the normative specification for how each anomaly is
reported: what message is printed to stderr and what comment is inserted into
the emitted `.proto` text.

---

## Definitions

**Fatal** — reproto omits the construct entirely.  The rendered file is
structurally incomplete; recompiling it to a descriptor set will not match the
original.  A `// OMITTED[tag]:` comment stands at the location the construct
would have occupied.

**Warning** — reproto produces degraded but syntactically valid output.  The
rendered file compiles, but the resulting descriptor set differs from the
original in the affected field.  A `// WARNING[tag]:` comment appears on the
line immediately before the degraded construct.

Both severities always emit a `cli_warning` to stderr and always insert a
dedicated comment line into the rendered `.proto`.

---

## Comment format

Every anomaly inserts exactly one comment line on its own dedicated line —
never appended to an existing code line — to avoid collisions with semicolons,
options brackets, or other text already postpended to a line.

### Tag prefixes

| Tag | Meaning |
|-----|---------|
| `[proto3]` | The construct is illegal in proto3 syntax |
| `[downconvert]` | Syntax downconversion via `--force-proto2-output` |
| `[editions]` | Editions syntax, not yet supported |
| `[error]` | Internal reproto rendering error (bug or unrecognised descriptor) |

### Grep handles

- `// OMITTED[...]` — fatal loss; the construct is absent from the output.
- `// WARNING[...]` — the immediately following line is degraded output.

---

## Implementation: `anomalies.py`

### Data model

```python
# reproto/anomalies.py

from dataclasses import dataclass
from .text import Block, BlockLine, COMMENT
from lib.warnings import cli_warning

@dataclass(frozen=True)
class Anomaly:
    tag: str       # one of: "proto3", "downconvert", "editions", "error"
    severity: str  # "OMITTED" or "WARNING"
    stderr: str    # format-string: printed to stderr via cli_warning()
    comment: str   # format-string: body of the // OMITTED/WARNING comment line

ANOMALIES: dict[str, Anomaly] = {
    # populated below — see Taxonomy section
}

def report(code: str, depth: int, **kwargs) -> BlockLine:
    """Emit cli_warning to stderr and return a BlockLine for the .proto comment.

    `code` is the anomaly identifier (e.g. "C3").  `**kwargs` supplies all
    available context; each format string uses whatever subset it needs via
    str.format_map() — unused keys are silently ignored.

    Returns a BlockLine ready to insert into a Block at the given depth.
    """
    anomaly = ANOMALIES[code]
    cli_warning(anomaly.stderr.format_map(_Ignore(kwargs)))
    prefix = f'// {anomaly.severity}[{anomaly.tag}]:'
    text = f'{prefix} {anomaly.comment.format_map(_Ignore(kwargs))}'
    return BlockLine(text, depth, COMMENT)


class _Ignore(dict):
    """dict subclass that returns '' for missing keys in format_map()."""
    def __missing__(self, key: str) -> str:
        return ''
```

`_Ignore` is the key affordance: each template uses `{name}`, `{file}`,
`{extendee}`, etc. as needed; call sites pass all available context and the
templates silently ignore what they don't need.  This means the two
format strings in an `Anomaly` are fully independent — the stderr string
can include context the comment omits, and vice versa.

### Call pattern

```python
# Fatal — insert comment where the construct would have been, emit nothing else
comment = report("A5", depth, file=self.name, extendee=extension_proto.extendee)
out.append(comment)

# Warning — insert comment immediately before the degraded line
comment = report("C3", depth+1, name=field.name, file=ctx_file_name)
out.append(comment)
out.append(BlockLine(degraded_line, depth+1))  # degraded construct follows
```

---

## Taxonomy table

Each `Anomaly` entry is defined here.  The `stderr` and `comment` columns are
the exact format-string values stored in `ANOMALIES`.  Available kwargs for
each entry are listed; any subset may be used by either string.

---

### A. File level

**A1** — `re_file.py render()`: editions file, fallback to proto2

- **Current severity:** Silent (no message, no comment)
- **Target severity:** Warning
- **Available kwargs:** `file`
- **stderr:** `"'{file}': editions syntax is not yet supported; rendering as proto2"`
- **comment:** `"original file used editions syntax; rendered as proto2"`

---

**A2** — `re_file.py render()`: syntax downconversion (`--force-proto2-output`)

- **Current severity:** Silent on stderr (comment already in file, but wrong format)
- **Target severity:** Warning
- **Available kwargs:** `file`, `syntax`
- **stderr:** `"'{file}': output syntax downconverted from {syntax} to proto2"`
- **comment:** `"original file used \"{syntax}\" syntax; rendered as proto2"`

---

**A3** — `re_file.py render_file_options()`: exception rendering file options

- **Current severity:** Fatal (no comment)
- **Target severity:** Fatal
- **Available kwargs:** `file`, `exc_type`, `exc_msg`
- **stderr:** `"'{file}': failed to render file options: {exc_type}: {exc_msg}"`
- **comment:** `"file options could not be rendered ({exc_type}: {exc_msg})"`

---

**A4** — `re_file.py render()`: `import weak` in proto3, downgraded to plain import

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `file`, `dep`
- **stderr:** `"'{file}': 'import weak' is not valid in proto3; rendering as plain import: \"{dep}\""`
- **comment:** `"'import weak \"{dep}\"' is not valid in proto3; rendered as plain import"`

---

**A5** — `re_file.py render()`: file-level `extend UserMessage` in proto3, omitted

- **Current severity:** Fatal (no comment)
- **Target severity:** Fatal
- **Available kwargs:** `file`, `extendee`
- **stderr:** `"'{file}': top-level extend block for '{extendee}' is not valid in proto3; omitting"`
- **comment:** `"extend block for '{extendee}' is not valid in proto3"`

---

### B. Message level

**B1** — `re_descriptor.py render_extensions()`: message-nested `extend UserMessage` in proto3, omitted

- **Current severity:** Fatal (no comment)
- **Target severity:** Fatal
- **Available kwargs:** `msg`, `extendee`
- **stderr:** `"message '{msg}': nested extend block for '{extendee}' is not valid in proto3; omitting"`
- **comment:** `"extend block for '{extendee}' is not valid in proto3"`

---

**B2** — `re_descriptor.py render_reserved()`: `extensions N to M;` range in proto3, omitted

- **Current severity:** Fatal (no comment)
- **Target severity:** Fatal
- **Available kwargs:** `msg`, `start`, `end`
- **stderr:** `"message '{msg}': extension range [{start}, {end}) is not valid in proto3; omitting"`
- **comment:** `"extensions {start} to {end}; — not valid in proto3"`

---

**B3** — `re_descriptor.py render()`: `message_set_wire_format = true` in proto3, silently excluded

- **Current severity:** Silent
- **Target severity:** Warning
- **Available kwargs:** `msg`
- **stderr:** `"message '{msg}': 'message_set_wire_format' is not valid in proto3; omitting"`
- **comment:** `"'message_set_wire_format = true' is not valid in proto3; omitted"`

---

### C. Field level

**C1** — `re_field.py _render_map_field()`: non-canonical map entry, fallback to `repeated MessageType`

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `field`, `entry`, `found`
- **stderr:** `"field '{field}': non-canonical map entry '{entry}' (found fields: {found}); rendered as repeated message — wire semantics differ"`
- **comment:** `"non-canonical map entry '{entry}'; rendered as repeated message — wire semantics differ from original"`

---

**C2** — `re_field.py render()`: `TYPE_GROUP` in proto3, rendered as plain message field

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `name`
- **stderr:** `"field '{name}': groups are not valid in proto3; rendering as plain message field"`
- **comment:** `"group field; rendered as plain message field — wire semantics differ from original"`

---

**C3** — `syntax.py field_label()`: `LABEL_REQUIRED` in proto3, rendered as implicit singular

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `name`
- **stderr:** `"field '{name}': 'required' label is not valid in proto3; rendering as implicit singular"`
- **comment:** `"'required' label is not valid in proto3; rendered as implicit singular"`

---

**C4** — `re_field.py render()`: explicit `default_value` on proto3 field, omitted

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `name`
- **stderr:** `"field '{name}': explicit default values are not valid in proto3; omitting"`
- **comment:** `"explicit default value is not valid in proto3; omitted"`

---

**C5** — `re_field.py render()`: exception rendering field options, field rendered without options

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `name`, `exc_type`, `exc_msg`
- **stderr:** `"field '{name}': failed to render options: {exc_type}: {exc_msg}"`
- **comment:** `"field options could not be rendered ({exc_type}: {exc_msg})"`

---

### D. Option rendering

**D1** — `simple_types.py ReFieldDescriptor.get_scalar()`: unexpected Python type for option value, emits `0`

- **Current severity:** Warning (no comment)
- **Target severity:** Warning
- **Available kwargs:** `name`, `type`
- **stderr:** `"option '{name}': unexpected value type {type}; rendered as 0"`
- **comment:** `"option '{name}': unexpected value type {type} — rendered as 0, value may be wrong"`

---

**D2** — `simple_types.py ReFieldDescriptor.dump_option()`: descriptor type falls through all match arms, emits syntactically invalid placeholder

- **Current severity:** Warning (invalid proto text in output)
- **Target severity:** Fatal
- **Available kwargs:** `name`
- **stderr:** `"option '{name}': unrecognised descriptor type; omitting"`
- **comment:** `"option '{name}' has unrecognised descriptor type"`

---

## Summary of changes from current state

| Code | Change |
|------|--------|
| A1 | Add `cli_warning`; add `// WARNING[editions]:` comment |
| A2 | Add `cli_warning`; replace existing freeform comment with `// WARNING[downconvert]:` |
| A3 | Add `// OMITTED[error]:` comment |
| A4 | Add `// WARNING[proto3]:` comment on line before the degraded `import` |
| A5 | Add `// OMITTED[proto3]:` comment |
| B1 | Add `// OMITTED[proto3]:` comment |
| B2 | Add `// OMITTED[proto3]:` comment |
| B3 | Add `cli_warning`; add `// WARNING[proto3]:` comment |
| C1 | Add `// WARNING[error]:` comment on line before the `repeated` fallback |
| C2 | Add `// WARNING[proto3]:` comment on line before the plain message field |
| C3 | Add `// WARNING[proto3]:` comment on line before the implicit-singular field |
| C4 | Add `// WARNING[proto3]:` comment |
| C5 | Add `// WARNING[error]:` comment on line before the options-less field |
| D1 | Add `// WARNING[error]:` comment on line before the `= 0` value |
| D2 | Replace invalid placeholder text with `// OMITTED[error]:` comment; reclassify as Fatal |

---

## Non-goals

- Translations or locale support (the `_Ignore`/format-string approach makes
  this straightforward to add later by swapping the template strings).
- Machine-readable anomaly output (JSON, SARIF, etc.).
- Suppressing anomaly comments via a CLI flag.
