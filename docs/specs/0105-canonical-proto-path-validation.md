<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0105 — Plausible `.proto` path validation in the FDP scanning heuristic

**Status:** implemented
**Implemented in:** 2026-07-01
**App:** fdp-scan-pyo3

---

## Background

`walk_candidates()` scans an arbitrary binary buffer for embedded
`FileDescriptorProto` (FDP) candidates.  The heuristic:

1. Looks for byte `0x0A` (protobuf tag: field 1, wire type 2 =
   length-delimited).
2. Reads a varint length.
3. Checks whether the following bytes decode as valid UTF-8 and end in
   `.proto`.
4. If so, treats the offset as the start of an FDP and walks forward via
   `walk_protobuf_fields()`, assuming the rest is valid protobuf wire
   format.

`looks_like_fdp_start()` repeats the same `.proto`-suffix check to detect
FDP boundaries mid-walk (a second occurrence of field 1 signals the start
of the next FDP).

### The false-positive

Arbitrary non-protobuf binary content can coincidentally decode as valid
wire-format bytes whose "name" field happens to end in `.proto`, despite
being garbage.  A real-world example, found via `protoscan` on a customer
package file, is a `name` field whose bytes decoded to:

```
/p>
{{- end}}
{{end}}

$google/api/expr/v1alpha1/value.proto
```

This is an HTML/Go-template fragment, not a `.proto` import path.  It
satisfies every current check (valid UTF-8, `<= MAX_PROTO_NAME_LEN`, ends
with `.proto`), so it was accepted as a genuine FDP name.

### Downstream impact

The garbage name round-trips through `prost::Message::decode`, which is
permissive — it validates wire format only, not semantic content — so it
does not reject this.  A downstream consumer
(`ThalesGroup/prototools`'s `protosets_extension`) used the accepted
`name` to build a `gix` tree path; the leading `/` produced an empty path
component, crashing with `RuntimeError: Empty path components are not
allowed`.  That consumer has since added defensive sanitization, but the
correct fix belongs at the source of the false positive: this scanner.

---

## Goals

- Reject candidate names that are not plausible canonical `.proto` import
  paths, eliminating this class of false positive at the source.
- Keep `walk_candidates()` and `looks_like_fdp_start()` consistent by
  sharing a single validation helper (they currently duplicate the
  `.ends_with(".proto")` check).
- Preserve existing behavior for genuine `.proto` paths, including
  non-ASCII ones (e.g. `google/protobuf/descriptor.proto`, `café.proto`).

## Non-goals

- **POSIX path legality.** POSIX/Linux places almost no constraints on a
  path component: the only universally forbidden bytes are `NUL` and
  `/`; control characters (e.g. a literal newline), unicode, and
  arbitrary punctuation are all technically legal in a real filename.
  We are deliberately *not* checking "is this a legal POSIX path" — a
  filename containing `\n` is legal but not a plausible `protoc` import
  name.
- **Full replication of `protoc`'s file-naming behavior.** `protoc`
  itself imposes no dedicated grammar on the `name` field beyond it
  being the path used to reach the file. We're not modeling `protoc`'s
  actual (very permissive) behavior — we're modeling *plausibility as a
  genuine, protoc-managed canonical import path*, which is a narrower,
  convention-driven heuristic used purely for garbage rejection.
- Rejecting `.proto` names that are syntactically unusual but still
  plausible (non-ASCII letters, spaces, punctuation) — the goal is
  defense-in-depth against garbage (particularly control characters and
  absolute/traversal paths), not a strict charset grammar.

---

## Specification

Add a shared helper:

```rust
/// Returns true if `name` looks like a plausible canonical `.proto`
/// import path: ends in `.proto`, is not absolute, and every
/// `/`-separated component is non-empty, is not `.`/`..`, and contains
/// no control characters.
///
/// This is deliberately *not* a POSIX path-legality check — POSIX
/// forbids only `NUL` and `/` in a filename, and control characters
/// (e.g. a literal newline) are technically legal there.  This checks
/// plausibility as a genuine, protoc-managed import path instead:
/// no real `.proto` import name is absolute, contains `.`/`..`
/// components, or embeds control characters, even though the
/// filesystem would tolerate all of these.
fn is_plausible_path(name: &str) -> bool {
    name.ends_with(".proto")
        && !name.starts_with('/')
        && name.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.chars().any(|c| c.is_control())
        })
}
```

Replace the `.ends_with(".proto")` check with a call to
`is_plausible_path()` in both:

- `walk_candidates()` (the point where a fresh FDP candidate is accepted).
- `looks_like_fdp_start()` (the point where a second field-1 occurrence
  is checked to mark the boundary between two FDPs).

`MAX_PROTO_NAME_LEN` and the UTF-8 validity check are unchanged and stay
in place ahead of this check.

### Regression test

Add a test in the existing `#[cfg(test)] mod tests` block using the
exact garbage string from the bug report. Note: because the garbage
string contains an embedded `\n` immediately followed by bytes that
happen to decode as a valid varint length matching a genuinely
plausible trailing substring (`google/api/expr/v1alpha1/value.proto`,
36 bytes), `scan_bytes()` on the full string does **not** return an
empty list — the scanner correctly continues past the rejected outer
(garbage, leading-`/`) candidate and finds this coincidental but
genuinely clean embedded name, which is safe to accept (no leading `/`,
no control characters, cannot reproduce the crash).

The regression test should therefore assert the property that actually
matters:
- The full garbage span (the whole buffer, as one candidate) is never
  returned by `scan_bytes()`.
- Add a second, simpler garbage string (no embedded coincidental valid
  substring) as a `scan_bytes() == []` sanity check.
- Add direct unit tests on `is_plausible_path()` covering: absolute
  paths, `.`/`..` components, empty components (`//`), control
  characters (e.g. `"foo\nbar.proto"`), and — per the relaxed charset —
  confirm non-ASCII names like `"café.proto"` are still accepted.

---

## Implementation status

Implemented in `fdp-scan-pyo3/src/lib.rs`: `is_plausible_path()` shared
helper, used by both `walk_candidates()` and `looks_like_fdp_start()`;
regression tests added per the Regression test section above.
