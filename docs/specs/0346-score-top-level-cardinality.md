<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0346 — score: charge non_canonical for top-level repeated-singular fields

Status: implemented
Implemented in: 2026-08-23
App: prototext-graph (src/score/walk.rs)
Refs: docs/specs/0343-the-last-one-wins-and-the-others-say-so.md (defines
      repeated_singular as a non-canonical encoding)

## Background

`apply_cardinality_multi` charges `non_canonical` for each occurrence of a
singular field beyond the first.  The existing depth-0 EOF path
(`score_message_multi_inner:1578-1580`) calls it correctly when the buffer is
cleanly exhausted — confirmed by `tc19b` (field repeated 3× at top level →
`non_canonical: 2`).

The reproduction `prototext --descriptor alice/app.desc decode bob/logfile`
scores the logfile as `google.protobuf.BytesValue`.  Wire inspection shows
four top-level occurrences of field 1 (`bytes value`), the last of which
extends past the end of the buffer (truncated).  The score reports
`matches: 3, non_canonical: 0, truncated: 1` — the `non_canonical` is wrong;
it should be 2.

### Root cause

The LEN body of the 4th `value` field extends past the end of the buffer.
`score_message_multi_inner` calls `cut_or_veto` (line 1883) with
`end_undeclared = true` (the `score` subcommand sets `end_undeclared: true`),
taking the **cut** path: `truncated += 1` is charged and `active.clear()` is
called.

Then the main loop checks `pos == buflen || active.is_empty()` — `active` is
now empty — and returns immediately, **skipping the `if !active.is_empty()`
block** that would have called `apply_cardinality_multi`.

`non_canonical` is never written incrementally during the walk — only
`apply_cardinality_multi` sets it.  The 3 field-1 occurrences were recorded
in `ae.occurrences`, but the `ActiveEntry` was destroyed by `active.clear()`.
The charge never fires.

### Why the normal (non-truncated) case works

When the buffer ends cleanly, the cut path is never taken.  The EOF branch
at `score_message_multi_inner:1578-1580` runs `apply_cardinality_multi` on
all surviving entries before returning, correctly charging `non_canonical`.

## Goals

- **G1.** Every repeated occurrence of a singular field at any depth —
  including the top-level message frame — contributes `count - 1` to
  `non_canonical`, exactly as sub-frame occurrences do today.

## Non-goals

- **N1.** The render path's `repeated_singular` annotation is not changed.
- **N2.** The score formula coefficients are not revisited here.

## Specification

**S1.** In `cut_or_veto`, before calling `active.clear()` on the cut path,
call `apply_cardinality_multi` for every surviving `ActiveEntry`:

```rust
fn cut_or_veto(active: &mut Vec<ActiveEntry>, ws: &mut WalkState, cut: bool, reason: &str) {
    if !cut {
        veto_all(active, ws, reason);
        return;
    }
    for ae in active.iter_mut() {
        flush_pending(ae, ws.scores);
        apply_cardinality_multi(ws.graph, ae, ws.scores);  // NEW
        for &e in &ae.entries {
            ws.scores[e as usize].truncated += 1;
        }
    }
    active.clear();
}
```

This charges cardinality for all fields observed before the cut, which is
correct: the frame ended (the bytes ran out), so the occurrence counts are
complete for what was actually consumed.

**S2.** The truncated field itself — whose LEN body extends past the buffer —
is never recorded in `occurrences` (the `record_occurrence` call only fires
after the payload is successfully consumed, which line 1882's `payload_end`
check gates).  However, the tag and length prefix of the truncated field were
successfully read: its field number is known.  The renderer independently
detects the same occurrence and annotates it with `repeated_singular`.  For
consistency with the renderer, the truncated occurrence should also count.

Therefore, for the cut path only, record the occurrence of the truncated
field before calling `apply_cardinality_multi`:

```rust
// In the WT_LEN arm, immediately before cut_or_veto when payload_end fails:
for ae in active.iter_mut() {
    if matches!(ae.verdict, Verdict::Found(_, _)) {
        record_occurrence(&mut ae.occurrences, field_number as u32);
    }
}
cut_or_veto(active, ws, end_undeclared, "LEN body extends past end of buffer");
return buflen;
```

This must only run when the field was declared (`Verdict::Found`), to match
what the renderer does — an unknown field's truncation does not produce a
`repeated_singular` annotation.

**S3.** The prohibition on calling `apply_cardinality_multi` in the veto path
(`!cut`) is unchanged — see spec 0310 S5.  A veto means the frame
contradicted itself; the occurrence counts are not meaningful.

## Test plan

1. `top_level_repeated_singular_then_truncated` — a blob where a singular
   (optional) field appears 3× cleanly followed by a 4th occurrence whose
   LEN body is truncated; scored with `end_undeclared: true`; assert
   `non_canonical == 3, matches == 3, truncated == 1`.
2. `top_level_repeated_singular_clean` — same field repeated 3× cleanly;
   scored with default opts; assert `non_canonical == 2` (exercises the
   existing EOF path, regression guard).
3. `top_level_required_repeated_then_truncated` — required field repeated
   2× cleanly then truncated 3rd; `end_undeclared: true`; assert
   `non_canonical == 2, mismatches == 0`.

## Measured outcome

`prototext --descriptor alice/app.desc score --type google.protobuf.BytesValue
bob/logfile` now reports `non_canonical: 3` (was 0), consistent with the three
`repeated_singular` annotations the renderer emits for the same blob.
All 108 `prototext-graph` unit tests pass; full nix build clean.
