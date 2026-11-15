<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0039 — reproto: Fix `json_name` rendering — correct camelCase algorithm and empty-value guard

**Status:** implemented
**Implemented in:** 2026-05-06
**App:** reproto

---

## Purpose

This document records the findings from an investigation into two bugs in the
`should_render_json_name` / `_camel_case` helpers introduced by spec 0019, and
specifies the fixes.  It supersedes the `json_name` subsection of spec 0019
Background (§spec 0015 §16), which contained an incorrect algorithm description.

---

## Findings

### Finding 1 — `_camel_case` does not match protoc's algorithm

Spec 0019 described and implemented `_camel_case` using Python's
`str.split('_') + str.capitalize()`:

```python
parts = name.split('_')
return parts[0] + ''.join(p.capitalize() for p in parts[1:] if p)
```

Protoc's authoritative algorithm is `ToJsonName` in
`src/google/protobuf/descriptor.cc` (verified from source):

```cpp
std::string ToJsonName(const absl::string_view input) {
  bool capitalize_next = false;
  std::string result;
  result.reserve(input.size());

  for (char character : input) {
    if (character == '_') {
      capitalize_next = true;
    } else if (capitalize_next) {
      result.push_back(absl::ascii_toupper(character));
      capitalize_next = false;
    } else {
      result.push_back(character);
    }
  }
  return result;
}
```

Every `_` sets a "capitalize next" flag and is dropped.  The next non-`_`
character is emitted uppercased if the flag is set (`ascii_toupper` is a no-op
on digits, so digits are passed through unchanged).  No first-character
lowercasing is applied (`ToJsonName` differs from `ToCamelCase` in this
regard).

Note: `ToCamelCase` (used elsewhere in protoc for message/field name
generation) has the same core logic but additionally lowercases the first
character when called with `lower_first=true`.  `json_name` derivation uses
`ToJsonName`, not `ToCamelCase`.

The split-based implementation diverges in several cases:

| Input | protoc (`ToJsonName`) | split-based | Notes |
|---|---|---|---|
| `foo_` | `'foo'` | `'foo'` | agree (trailing `_` dropped) |
| `__` | `''` | `''` | agree (all underscores dropped) |
| `foo_1bar` | `'foo1bar'` | `'foo1bar'` | agree (`_` before digit dropped) |
| `street_1` | `'street1'` | `'street1'` | agree |
| `foo__bar` | `'fooBar'` | `'fooBar'` | agree |
| `FOO_BAR` | `'FOOBAR'` | `'FOOBar'` | **differ**: `capitalize()` lowercases existing caps |
| `_foo_bar_` | `'FooBar'` | `'FooBar'` | agree |
| `_1foo` | `'1foo'` | `'1foo'` | agree |
| `foo_123_bar` | `'foo123Bar'` | `'foo123Bar'` | agree |

The only divergence is with **uppercase input**: `capitalize()` lowercases
already-uppercase letters (e.g. `'FOOBar'`), whereas protoc's `ascii_toupper`
never lowercases.

In all diverging cases, the `json_name` stored in the `.pb` (auto-derived by
protoc) differs from what our `_camel_case` returns.  This causes
`should_render_json_name` to incorrectly return `True`, producing spurious
`[json_name = "..."]` annotations on those fields.

**Fix:** replace the split-based algorithm with protoc's exact
character-by-character algorithm.

### Finding 2 — `json_name = ""` can appear when the `.pb` omits the field

`FieldDescriptorProto.json_name` is a proto2 `optional string` (field number
10).  When a `.pb` file does not include a `json_name` entry for a field — as
is common in hand-crafted or legacy `.pb` files — the Python protobuf library
returns `""` (the default for an unset `string` field).

When `self.this.json_name == ""` and `_camel_case(field.name)` is non-empty
(the common case), the old `should_render_json_name` returns `True` and the
output contains `[json_name = ""]`, which is syntactically invalid in `.proto`
files.

`""` is never a legitimate explicit `json_name` value — protoc does not allow
empty `json_name` in source.  An empty value unambiguously means "not set in
the `.pb`".

**Fix:** add a `bool(field.json_name)` precondition.  When `json_name` is the
empty string, `should_render_json_name` returns `False` immediately.

### Finding 3 — explicit `json_name` cannot be distinguished from auto-derived

Protoc always writes `json_name` into the `.pb`, whether or not the user wrote
`[json_name = "..."]` in source.  The serialized value is identical in both
cases.

The only reliable way to detect an explicit annotation is via `source_code_info`:
when `json_name` is explicitly set, a location entry with a path ending in `10`
(the field number of `json_name` in `FieldDescriptorProto`) is present.
However, `source_code_info` is optional and absent from most `.pb` files in
practice — confirmed: none of the large corpora's fixture files include
it.

**Consequence for reproto:** comparison against the auto-derived value is the
only feasible strategy.  When a user explicitly wrote `[json_name = "x"]` on a
field whose auto-derived name is also `"x"`, reproto suppresses the annotation
— correctly, since it is redundant (protoc would auto-derive the same value on
recompilation).

---

## Specification

### 1. Replace `_camel_case` with protoc's exact algorithm

In `syntax.py`, replace the split-based implementation with a direct
translation of protoc's `ToJsonName` (descriptor.cc):

```python
def _camel_case(name: str) -> str:
    """Derive the default JSON name (camelCase) for a proto field name.

    Mirrors protoc's ToJsonName() from descriptor.cc:
    each '_' sets a capitalize-next flag and is dropped; the next non-'_'
    character is uppercased if the flag is set (ascii_toupper is a no-op on
    digits, so digits are passed through unchanged).  No first-character
    lowercasing — ToJsonName differs from ToCamelCase in this regard.

    Examples (matching protoc):
        'field_name'   -> 'fieldName'
        'foo_'         -> 'foo'       (trailing underscore dropped)
        'foo_1bar'     -> 'foo1bar'   (underscore before digit dropped)
        'foo__bar'     -> 'fooBar'    (both underscores consumed)
        'FOO_BAR'      -> 'FOOBAR'    (no lowercasing of existing caps)
    """
    result = []
    capitalize_next = False
    for c in name:
        if c == '_':
            capitalize_next = True
        elif capitalize_next:
            result.append(c.upper())
            capitalize_next = False
        else:
            result.append(c)
    return ''.join(result)
```

### 2. Add empty-string guard to `should_render_json_name`

In `syntax.py`:

```python
def should_render_json_name(field: FieldDescriptorProto) -> bool:
    """Return True iff [json_name = "..."] should be emitted for this field.

    Guards:
    - Empty json_name means the .pb did not populate the field (protobuf
      default for an unset optional string).  Never emit json_name = "".
    - When json_name equals the auto-derived camelCase of field.name, the
      annotation is redundant and must be suppressed.
    """
    return bool(field.json_name) and field.json_name != _camel_case(field.name)
```

---

## Test coverage

- Add or update `_camel_case` unit tests covering: trailing underscore,
  underscore before digit, consecutive underscores, all-caps input,
  leading underscore before digit, leading underscore before letter.
- Confirm `should_render_json_name` returns `False` for `json_name = ""`.
- The existing `test_roundtrip_polyglot[json_name.proto]` must continue to pass.

---

## References

- [Compilation and Descriptors — The Protobuf Language](https://protobuf.com/docs/descriptors)
- [Descriptors — Buf Docs](https://buf.build/docs/reference/descriptors/)
- [protobuf descriptor.proto (GitHub)](https://github.com/protocolbuffers/protobuf/blob/main/src/google/protobuf/descriptor.proto)
- [JSONCamelCase — Go strs package](https://pkg.go.dev/google.golang.org/protobuf/internal/strs)
- [Issue #7192 — protoc: bad json_name camelCase logic](https://github.com/protocolbuffers/protobuf/issues/7192)
