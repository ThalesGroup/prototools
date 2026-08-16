<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0306 — the wrapper hides its prefix, not its payload

Status: superseded by 0307
Implemented in: 2026-08-16
App: protolens
Refs: docs/specs/0225-….md (S3 suppresses the wrapper root's wire row;
        S2's head/tail rule, which this reuses unchanged),
      docs/specs/0268-….md (`w` / `W` / Ctrl-w, the bindings this
        restores on one more line),
      docs/specs/0299-….md (the untyped blob whose whole document is
        the wrapper root)

## Background

A document opened with no root type is wrapped: `Blob` prepends a
synthetic field-1 `WT_LEN` tag and length prefix so that every span
coordinate is relative to the wrapped bytes. Slot 0 is that wrapper.

`wire_slice` returns `None` for it (`tui/wire.rs:393-399`), so `w` draws
nothing on its line. Spec 0225 S3's reason is sound and is not in
question: the wrapper's head is bytes that are *not in the user's file*,
and fabricating them in the one view whose entire purpose is fidelity
would be a lie.

But the suppression is applied to the whole node rather than to the
fabricated bytes. The head of a flat wrapper root runs from the
synthetic tag to the end of the buffer — three or four invented bytes,
then the user's entire file. Refusing the row throws the file away to
avoid the prefix.

It bites hardest exactly where the wire view is most wanted. An untyped
blob that the probe declines collapses to a single flat line (spec
0299), so slot 0 is not merely the first line of the document, it is the
only one — and `w`, `W` and the whole wire view are unreachable for
every byte of the file.

## Goals

- **G1.** `w` on the wrapper root's line shows the user's bytes.
- **G2.** No fabricated byte is ever drawn. G1 must not be bought by
  weakening this.

## Non-goals

- **N1.** Drawing the synthetic tag and length dimmed, marked, or behind
  a toggle. Spec 0225 S3's reasoning is unchanged and this spec does not
  reopen it: the wire view's value is that what it shows is what is on
  disk. A byte that is there "but greyed out" still has to be counted
  past by a reader checking an offset.
- **N2.** Changing `Blob`, `wrapper_offset`, or the arena coordinate
  frame. The prefix is load-bearing — it is what makes one coordinate
  system serve both the wrapped and the unwrapped case. This is a
  presentation fix in the one place that presents.
- **N3.** Showing wire rows for unrendered slots. That arm of
  `wire_slice` is a different `None` and stays.

## Specification

- **S1.** The wrapper root's slice is computed like every other node's,
  through the same `head_or_tail`, and is then **intersected with
  `wrapper_offset..`**. The guard changes from "this node shows
  nothing" to "these bytes are not the user's".

  This is the rule `display_range` already applies to the same node:
  every coordinate has `wrapper_offset` subtracted, and the wrapper's
  own node displays as `[0, n)` (`tui/navigation.rs:1418-1433`). One
  document, one rule about the prefix.

- **S2.** If the intersection is empty, `wire_slice` returns `None` —
  the same answer as today, now reached because there is nothing to say
  rather than because the question was refused. This is not a rare path:
  a *bracketed* wrapper root's head is exactly the prefix, since its
  first child begins at `wrapper_offset`.

- **S3.** The narrowed slice's framing is `Framing::Raw`, not
  `Framing::Tagged`. After S1 the slice does not begin with a tag, and
  it does not begin with the node's own tag under any reading — the only
  tag it ever had was invented. `Raw` is already defined as "bytes no
  framing claims … drawn plain, which is all that can honestly be said
  about them" (`wire.rs:77-80`), which is precisely their status.

- **S4.** The wrapper root's *tail* narrows by the same rule and is
  normally empty, since the last child ends where the buffer does. It
  becomes non-empty only when trailing bytes no child claimed exist —
  real bytes of the user's file, which S1 therefore shows.

## Alternatives considered

**Suppress only the first row and allow the rest.** Fails on the case
that motivates the spec: the flat wrapper root *has* only a first row.

**Offset the slice in the painter instead.** The painter would need
`wrapper_offset` and a rule for when it applies, duplicating a decision
`wire_slice` is already positioned to make and giving the fabricated
bytes a second chance to escape through the other caller of
`head_or_tail` (the preview overlay).

**Give slot 0 a real span covering only the user's bytes.** That is N2 —
it moves the wrapper's seam into the arena, where every consumer of
`raw_start()[0]` would have to learn about it, to fix one row.

## Test plan

1. `w_on_a_flat_wrapper_root_shows_the_whole_file` — an untyped blob
   that renders as one flat line: the slice is `wrapper_offset..end`,
   framing `Raw`, and its bytes equal the input file byte for byte.
2. `a_wrapper_root_never_shows_a_fabricated_byte` — over both a flat and
   a bracketed wrapper root, assert `slice.bytes.start >= wrapper_offset`
   whenever a slice is returned. This is G2 stated as an assertion rather
   than as a review rule.
3. S2's `None` — no new test. `each_line_claims_its_own_bytes` already
   asserts that a bracketed wrapper root's head and footer rows are
   empty, and after this change they are empty *because* the narrowing
   emptied them. Reaching the same assertion by the new path is what
   makes it the regression test; a second copy would assert nothing more.

A fourth test for `wrapper_offset == 0` was dropped as vacuous: the guard
is literally `wrapper_offset > 0`, and the typed-path no-op is what test 3
already measures on a real wrapped document.

## Measured outcome

**Implemented 2026-08-16.** `protolens/src/tui/wire.rs` — the guard in
`wire_slice` moved below `head_or_tail` and became
`without_the_wrapper_prefix`.

83 tests in `wire::` pass, including the two new ones and
`each_line_claims_its_own_bytes` unmodified in its assertions.

The flat case is the whole of the gain: on `CUT_SHORT` the wire view went
from nothing at all to `wrapper_offset..blob.len()`, which compares equal
to the input file. The bracketed case is unchanged in output and changed
in reason, exactly as S2 predicted.
