<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0459 — reproto, protoback: an extend block of orphans is an orphan

Status: draft
App: reproto | protoback
Refs: docs/specs/0429-protoback-rendering.md (the rendering both tools
        share, compared by the differential harness)

## Background

An extension whose extendee no file defines, or whose type is missing, is
written as an orphan: commented out, `///`. The `extend` block around it
is not, unless the extension isn't summoned and the block is top-level:

```proto
message Ext {
  extend x.JobSpec {
/// optional Ext field = 109825718;
  }
}

extend x.JobSpec {
///optional int32 top = 5;
}
```

protoc refuses the empty block ("Expected type name") before it gets to
the missing import. A downstream build that compiles each written file
fails on two files this way: they would fail anyway, the extendee's file
being missing, but the error names the wrong cause.

Both tools decide the block's kind the same way (`_render_extend_blocks`,
`extend_blocks`): from summoning, at the top level only, never from what
the fields' lines turned out to be.

## Goals

- **G1.** An `extend` block whose every field line is commented out is
  commented out whole, its `extend` line and closing brace included, in
  both tools.

## Non-goals

- **N1.** Other blocks (`oneof`, a message): a message's empty body is
  valid, and no `oneof` of orphans has been seen. The same rule could be
  applied if one is.

## Specification

- **S1.** Once an `extend` block's fields are rendered, its `extend {…} {`
  line and closing `}` are orphans when no line between them is code: every
  line an orphan or a comment. Otherwise they are as before (code, or
  orphans by the summoning rule, or redacted under `--target portable`).

- **S2.** The same in reproto (`re_descriptor._render_extend_blocks`) and
  protoback (`render/file.rs`, `extend_blocks`): their output stays byte
  for byte the same.

## Alternatives considered

**Omitting the block.** The orphan keeps what the source declared
readable, as for every other orphan.

## Test plan

1. protoback: `an_extend_block_of_orphans_is_an_orphan` — the example
   above, nested and top-level: both blocks `///`-prefixed whole; a block
   holding one resolvable and one orphaned extension unchanged.
2. reproto: the same case, the same text.
3. The differential harness, both modes, against reproto: no difference.

## Measured outcome

Filled in at implementation.
