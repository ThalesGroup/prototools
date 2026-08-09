<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0267 — packing is a declaration, not an anomaly

Status: implemented
Implemented in: 2026-08-09
App: protolens
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (S11's
        borrowed hues, S12's keyword-keyed tiers),
        docs/specs/0231-the-document-rows-loudness-means-one-thing.md
        (the leveling this removes an exception from),
        docs/specs/0249-a-large-document-answers-the-user-first.md (S12's
        `Unbaked` violet, borrowed from the landmark),
        docs/specs/0260-a-fold-nobody-has-read-says-so.md (that violet's
        measured value)

## Background

A packed field's annotation reads

```
int32Pk: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3
```

and two of its tokens wear a violet nothing else on the row wears:
`[packed=true]` and `pack_size`. They are the sole members of a whole
severity tier, `Tier::Landmark`, invented for them by spec 0225 S12.

The tier was a mistake in kind. `[packed=true]` is part of the *field
declaration* — it sits between the type and the `=`, in the same
sentence as `repeated` and `int32`, and it says what the field is.
`pack_size: N` is an ordinary modifier: it counts the elements in one
wire record, which is a plain fact about the encoding and not a defect
of any sort. Neither is a third thing needing a color of its own, and
giving them one costs the reader a hue to learn and the row an
exception to spec 0231's "one loudness, one meaning".

## Goals

- **G1.** `[packed=true]` reads as part of the declaration it belongs to
  — the type color, the same one `repeated` and `int32` already wear.
- **G2.** `pack_size: N` reads as an ordinary modifier — the comment
  color every unclassified annotation token already falls through to.
- **G3.** A packed record's wire row is drawn like every other record's:
  its wire type in the borrowed type band, its length prefix in the
  ordinary length band.

## Non-goals

- **N1. The emitted vocabulary does not change.** prototext-core still
  writes `[packed=true]` and `pack_size: N` exactly as it does today.
  This is a question of what protolens paints them, and of where
  `annotation-format.md` files `pack_size` in its modifier tables.
- **N2. The violet itself stays.** `Status::Unbaked` uses it (spec 0249
  S12) and spec 0260 measured the value it uses. It stops being a
  *tier* color and becomes what it now solely is: the unbaked-fold
  color.

## Specification

- **S1. `[packed=true]` takes `@type`.** `highlights.scm`'s
  `(annotation_attribute)` rule joins the two that already give
  `repeated`/`required` and the type name the same capture. The wire
  row's borrowed wire type is unaffected: it reads the *first* token
  after `#@ `, which on a packed row is `repeated`, and that was
  already a type.

- **S2. `pack_size` falls through to `@comment`.** Its `#any-of?` rule
  is deleted; `(annotation) @comment`, declared first and narrowed by
  everything below it, is what remains.

- **S3. `Tier::Landmark` is deleted, and with it
  `annotation::LANDMARK`, `SyntaxRole::AnnotationLandmark` and every
  arm serving them.** S2 leaves the tier with no members, and a tier
  `tier_of` can never return is dead weight that the next reader has to
  work out is dead.

- **S4. `Status::Unbaked` gets the palette entry it was borrowing.**
  `RgbPalette::tier_landmark` is renamed `status_unbaked`, keeping
  spec 0260's `#D24DFF` / `#AF00DB` and the `LightMagenta` / `Magenta`
  ANSI-16 fallbacks, and `status_color_in` reads it directly. The
  borrowing was only ever an argument about two things saying
  "provisional"; with one of them gone there is nothing to borrow from.

- **S5. The wire row's packed-length exception goes.**
  `Painter::packed_len` and the `style_in` arm that painted a packed
  record's length prefix in the landmark violet are deleted, so the
  prefix wears `palette.len` like every other length prefix. The
  exception existed to mirror `pack_size`'s accent in the annotation
  above; there is no accent left to mirror. `draw_packed_head` keeps
  its `accuse("pack_size")` — the row still names what it found, and
  `tier_of` now answers `None`, which is the correct severity.

- **S6. `annotation-format.md` files `pack_size` as informational.**
  Its "non-canonical modifiers" table and the `noncanon_valued`
  production both list it today; a packed record's element count is not
  non-canonical, and spec 0266 made that table's classification
  load-bearing for what `--raw` will read as a message. (It changes no
  verdict there — the split that matters is invalid vs. everything
  else, by case — but a table nobody can trust is worse than one nobody
  reads.)

## Alternatives considered

### Keep the tier, recolor it

Painting both tokens some quieter violet keeps a rung on a severity
ladder for two tokens that carry no severity, and keeps `is_a_type`
having to exclude it so a packed row's wire type does not borrow it.
The problem is the classification, not the color.

### Move `pack_size` to the non-canonical tier

Where `annotation-format.md` files it today. It would go yellow, next
to `tag_ohb` and `ENUM_UNKNOWN` — which says the encoder did something
no conformant writer does. Every packed record has a `pack_size`.

### Give `[packed=true]` the comment color with `pack_size`

Both are about packing, so both could go quiet. But `[packed=true]` is
inside the declaration, between the type and the `=`; graying it splits
one sentence into three colors and leaves the reader deciding whether
the gray part is still part of the declaration.

## Test plan

1. `the_declaration_echo_matches_the_documents_own_styles` (colorize,
   existing) gains `[packed=true]` — it already asserts `repeated` and
   `int32` on the very row this is about, so a second test would have
   restated it.
2. The same test gains `pack_size` → `SyntaxRole::Comment`.
3. `every_keyword_is_colored_by_its_tier` (colorize, existing) — still
   passes over the two remaining tiers.
4. `a_packed_record_is_drawn_like_any_other` (wire, replacing
   `a_packed_records_length_wears_the_landmark_in_the_foreground`) —
   the head row's wire type is the type band, its field number the tag
   band, its length prefix the length band, and no span carries a
   foreground of its own.
5. `pack_size_and_its_wire_bytes_are_both_ordinary` (tui/tests/wire,
   replacing `pack_size_and_its_wire_bytes_share_the_accent`) — on the
   real fixture, `pack_size` is `Comment` in the annotation and the
   record's length prefix is `palette.len` in the wire row.
6. The tree-sitter highlight fixture — `[packed=true]` asserts `type`,
   `pack_size` asserts `comment`.

## Measured outcome

Implemented 2026-08-09. `cargo test -p protolens` 832 + 25 green,
`clippy --all-targets` and `fmt --check` clean,
`nix-build -A tree-sitter-textproto-highlight-test` 126 assertions.

**G3 was already true, and the wire row's real outlier was elsewhere.**
Probing the dark theme on the real `packed_run_with_tail_fixture`
printed the packed head's type band as
`bg = Rgb(82, 79, 47)` — bit for bit `banded(style_for(Type))`, the same
band every other record's wire type wears. `wire_palette` resolves `ty`
from `type_offset(text)`, which on a packed row lands on `repeated`, and
`highlights.scm` already gave that `@type`. What did stand out was the
**length prefix**, drawn `fg = Rgb(227, 144, 255)` with no background at
all — `Painter::packed_len`, the one place in the module where a tier
was painted in the foreground instead of as a band. S5 deletes it, and
that is the whole of G3's implementation.

`SyntaxRole` is down to 15 variants from 16, `RECOGNIZED_NAMES` and
`highlights.scm` with it, and `theme.rs` loses six arms plus the
leveling exception in `a_tier_looks_the_same_named_as_it_does_captured`
— the two surviving tiers are both anomalies, so the test now compares
outright with no `doc_leveled` special case.

Two notes for whoever changes a query next. `HIGHLIGHTS_QUERY` is
`include_str!`'d from the *Nix-built* grammar output, so editing
`reproto/tree-sitter-textproto/highlights.scm` alone leaves the binary
querying for `annotation.landmark` and fails
`the_recognized_names_are_exactly_the_grammars_captures` with a diff
that reads like the edit never happened. And the two override vars must
be `export`ed, not set inline on the `cargo` line — inline they arrive
empty, and `build.rs` accepts an empty var, so the failure surfaces as
`couldn't read /queries/highlights.scm`.
