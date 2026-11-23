<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0476 — protoback: what the text comments out leaves the pool

Status: draft
App: protoback
Refs: docs/specs/0473-protoback-repairs-reach-the-pool-and-the-schema-database.md
        (S2 extended here),
      docs/specs/0469-protoback-repair-skew.md

## Background

0473 gives the descriptor pool, under `--repair-skew`, each repaired file
as its text has it, without what `--repair-skew` comments out (W34, W35).
The renderer comments out more than that, though. An extension whose
extendee is defined in no file the run has (the extendee's file a missing
dependency, W5) is not summoned: the text writes its `extend` block
commented out, and the `.pb` beside it leaves the extension out. The pool
still loads it, and refuses the file:

```
'a/x.proto' : the descriptor pool refused it: couldn't resolve name '.M'
```

The written text no longer names `M`, and protoc compiles it. But the file
is still refused, and so left out of the schema database.

On a downstream corpus run with `--repair-skew`, after 0473 to 0475, 3 W19
remain. 2 are this case, and protoc compiles both written files. The
third is a defect of the input (an edition set with an empty `syntax`).

## Goals

- **G1.** Under `--repair-skew`, the pool loads a written file without
  the extensions its text comments out, as the `.pb` already leaves them
  out: such a file is accepted, and reaches the schema database.

## Non-goals

- **N1.** Without the option: as today. The pool, the warnings and the
  schema database stay as reproto has them (the harness compares them).
- **N2.** Other elements the text comments out (messages, enums,
  services not summoned). A refusal comes from a name that doesn't
  resolve. Of the elements the text comments out, only an extension
  names something defined elsewhere that can be missing (its extendee):
  a field of a type in no file is 0469's W34 already.
- **N3.** The warnings saying what is missing: W5 for the file and W4
  for the type stay.

## Specification

- **S1.** Under `--repair-skew`, the descriptor 0473 S2 gives the pool
  also leaves out every extension the text writes commented out because
  it isn't summoned, at file level or in a message. The `.pb` written
  beside the text uses the same predicate (`binary.rs`), so the two can't
  disagree.

- **S2.** A file with such an extension and nothing else to repair gets
  the treatment 0473 S2 gives a repaired file. W19 is said of it only if
  it is still refused (0473 S3).

- **S3.** Reported: nothing new. No W34 or W35 is said for these
  extensions; the renderer's commenting out is not a repair. 0473 S5's
  report is unchanged.

## Alternatives considered

**Always, the option or not.** It would also fix runs without
`--repair-skew`. But reproto's pool refuses these files the same way. The
change would then be needed in both tools, with the schema database of
every default run changing, for a case only runs with skewed inputs
meet. `--repair-skew` is already the option that says "make what is
written whole".

## Test plan

1. `an_extension_of_a_missing_extendee_leaves_the_pool`: a file
   importing a missing file and extending its `M`, and a message of its
   own. Under `--repair-skew`: no W19 nor W20, the file in the schema
   database without the extension, W5 still said. Without the option:
   W19 as today.
2. The self-check without the option: no change.

## Measured outcome

Filled in at implementation.
