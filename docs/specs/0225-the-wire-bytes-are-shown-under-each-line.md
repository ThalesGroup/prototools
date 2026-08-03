<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0225 — the wire bytes are shown under each line

Status: implemented
Implemented in: 2026-08-02
App: protolens, reproto (the vendored textproto grammar, S12)
Refs: docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        maximal arena: `raw_start` is the first byte of the node's tag,
        so every byte boundary is one varint away);
        docs/specs/0210-a-node-counts-its-own-lines.md
        (`LinePos { node, line_in_node }` — the line identity this
        feature keys on);
        docs/specs/0219-a-length-delimited-record-can-be-read-as-a-packed-run.md
        (a packed run is one slot with N lines);
        docs/specs/0223-highlighting-yields-to-pending-input.md (the
        monochrome rule both rows follow, S7);
        docs/specs/0201-a-hash-inside-a-string-is-not-a-comment.md
        (explicit token precedence beats match length — the rule the
        new `#@` token depends on);
        docs/specs/0116-*.md (`SyntaxRole`, the theme's palette pairs);
        docs/prototext/annotation-format.md (the anomaly vocabulary and
        its EBNF)

## Background

protolens shows what the bytes *mean*. Nothing in it shows the bytes.
For a tool whose subject is a binary format that is chiefly read through
tools, that is the interesting half missing: a reader who wants to know
why a field decoded the way it did, or what an anomaly annotation is
actually complaining about, has to leave for `xxd` and count offsets by
hand.

The anomaly annotations make this sharper. `#@ varint; val_ohb: 3` says
a varint carries three redundant continuation bytes. It does not say
which three. Every annotation in `docs/prototext/annotation-format.md`
has this shape: it names a fact about bytes without showing the bytes,
which is exactly backwards for a reader trying to learn the encoding.

## Goals

- **G1.** `w` toggles a wire mode. While it is on, every drawn document
  line is followed by one row showing that line's own bytes in hex.
- **G2.** In a fully expanded document, every byte of the file appears
  under exactly one line, in document order. The wire rows concatenate
  to the file.
- **G3.** Color distinguishes the tag from the wire type from the length
  prefix from the payload, and each of those hues is *derived from the
  document row's own hue for the same thing* rather than picked
  independently.
- **G4.** Wire-level anomalies are flagged *on the offending bytes*, not
  merely named, and an accusation marks every byte it spans.
- **G5.** A wire row is legible as belonging to the row above it: it
  points at that row, is dimmer than it, and is never colored under a
  different policy.
- **G6.** Nothing about the document model changes: no line counts, no
  `LinePos`, no fold, search, export or cache key learns that wire mode
  exists.
- **G7.** The `#@` annotation stops being one undifferentiated comment
  and is colored by the same palette as the wire row, so that a reader
  learns one vocabulary and applies it to both rows.

## Non-goals

- **N1.** A wire row is not a document line. It is not walkable, not
  selectable, not searchable, not exportable, not counted by
  `lines_total`, and owns no `LinePos`. It exists only inside `render`.
- **N2.** No byte offset column. The main pane's local statusline
  already reports the cursor node's byte range, and an offset column
  costs nine columns on a row that is already indented to its node's
  depth. If correlating with an external hex dump turns out to matter,
  this is the first thing to add.
- **N3.** No ASCII gutter beside the hex.
- **N4.** No wrapping. A wire row is exactly one terminal row, elided
  past `WIRE_ROW_MAX_BYTES` and panned with everything else (S8).
- **N5.** Not general syntax-colored hex. Exactly four regions take a
  derived hue. Everything else is the subdued default, and an anomalous
  byte keeps its tier color.
- **N6.** Schema-dependent anomalies are not flagged: `TYPE_MISMATCH`,
  `ENUM_UNKNOWN`, `INVALID_STRING`, `truncated_neg`, `nan_bits`, `neg`,
  and the packed `INVALID_PACKED_RECORDS` / `pack_size` / `ohb` family.
  Every one of them is a statement about the bytes *under a declared
  type*; wire mode is a view of the bytes alone, and it must say the
  same thing whether or not a descriptor set was loaded. The document
  row directly above already carries all of them, under `a`, and after
  S12 carries them in the same colors.
- **N7.** Field numbers in 19000–19999 are **not** flagged. That range
  is a `.proto` compile-time restriction
  (`FieldDescriptor::kFirstReservedNumber` … `kLastReservedNumber`),
  enforced by `protoc` when it compiles a schema. It has no wire
  meaning — nothing rejects such a tag at decode time — and prototext is
  right to reserve `TAG_OOR` for field number 0 or ≥ 2²⁹, which is the
  only tag range the wire format itself forbids.
- **N8.** Wire types 3 and 4 are **not** flagged. A group is an
  ordinary, valid encoding, and prototext treats `group` as a plain wire
  type token. Only 6 and 7 are `INVALID_TAG_TYPE`.
- **N9.** The horizontal pan bound is still measured over document rows
  only (`max_visible_line_len`, maps `row_content`), so a wire row wider
  than every document row on screen cannot be panned to its end. S5's
  connector makes it worse by four columns and does not fix it.

## Specification

### S1 — the toggle

`wire: bool` on `App`, toggled by `w` (currently unbound), following
`annotations` exactly: a match arm in `key_dispatch.rs`, a line in
`HELP_TEXT`, and nothing else. It invalidates no cache, bumps no
`structural_version`, and touches no `heat_*` state — like `a`, it
changes only what `render` draws. It does clamp `scroll_offset` and
`pan_offset`, because it changes the pane's geometry (S8).

### S2 — which bytes belong to which line

One rule, from which the user-visible behavior follows. For node `idx`:

- **head** = `raw_start(idx) .. raw_start(first rendered child)`, or the
  whole range if it has no rendered children;
- **tail** = `raw_end(last rendered child) .. raw_end(idx)`, or empty.

`raw_start` is the first byte of the node's *tag* (spec 0216 S19), so
this needs no extra stored flag. "Rendered child" means
`structure.rs`'s accessors — `first_child` / `last_child` — which answer
about the drawn tree, not the maximal arena, so a LEN payload printed as
a string has no children here and correctly claims all of its own bytes.

`LinePos` then assigns them:

| line | bytes |
|---|---|
| bracketed node, `line_in_node == 0` | head |
| bracketed node, `line_in_node == lines_total - 1` | tail |
| flat non-packed node, `line_in_node == 0` | head (= whole range) |
| flat packed node | S4 |

This is preferred over writing the three cases out by hand because head
∪ children ∪ tail is exactly the node's range, recursively, which *is*
G2 — the invariant holds by construction rather than by three
independently correct special cases. It reproduces the intended
behavior:

- message header → tag + length; message footer → nothing;
- group header → the START tag; group footer → the END tag when one
  exists, nothing when it does not;
- scalar or string → tag + length + payload.

And it surfaces, for free, the one case a hand-written rule would have
dropped: trailing bytes inside a message that no child claimed. They
appear on the footer row, which is the only place they can appear.

### S3 — the wrapper root

Slot 0's head bytes are `Blob`'s synthetic `write_tag(1, WT_LEN)` +
length prefix — up to `HEADROOM` bytes that are not in the user's file.
Its wire row is blank. Showing them would be a fabrication in the one
view whose entire purpose is fidelity to the file's bytes. Its tail is
empty by construction.

### S4 — packed runs

A packed run is one slot with one line per element (spec 0219). Element
`k` occupies `line_in_node == k`, and its row shows that element's own
bytes; element 0's row is preceded by the tag and the length prefix, so
that G2 still holds across the run.

Element boundaries are not in the arena — the maximal walk does not
descend into a packed payload — so they are derived by walking the
payload: successive varints for a varint-packed run, a fixed stride for
I32/I64. Walking from the payload start on every row would be O(k) per
row and quadratic in the run length. The row builder therefore carries a
one-entry memo `(node, next_line, next_offset)`; the window's rows for a
single packed node are consecutive, so the memo always hits and the walk
is O(payload) once per frame.

### S5 — the row's shape

Column 0 stays the heat-cue gutter and is blank on a wire row: a cue
reports how a *node* scores, and belongs on the node's own row.

`margin` (`tui/wire.rs`) then writes `FOLD_FIELD_WIDTH + indent` spaces,
where `indent` is the document row's own, followed by `WIRE_CONNECTOR`
= `"└── "` — `tree(1)`'s elbow, drawn subdued.

```
    1: "id"  #@ string
    └── 0a:02[69 64]
```

No extra indent level for the wire row: a second indent on top of the
connector only pushes long rows further past the pan bound (N9).

**A row that drew nothing gets no connector either** — the wire row is
blank, elbow included. It still occupies its terminal row: S8's 2× is
uniform, and a conditional row would re-key every per-row vector. A line
can own a byte range and still have nothing to show for it — a message
footer whose children ended exactly where the message did is the common
case (S2), and the wrapper root has two (S3). An elbow pointing at an
empty row says only that the elbow is there. Suppressing it makes the
connector mean what it looks like it means: below this line are its
bytes.

The connector is not a label — it says nothing about what the row *is*,
which S11's banded hex already makes unmistakable. It says which
document row the hex belongs to, and indentation alone cannot: a nested
message's own first field sits at the same indent one row further down,
so on a screen of alternating rows the eye has to count to pair them up.
The elbow points at exactly one row, the one above it, and `tree` has
already taught every terminal user to read it that way.

The wire row follows its document row rather than preceding it. Placing
it first would read as evidence-then-conclusion, and would move the
wrapper root's blank row (S3) to the top of the viewport where a blank
line is less conspicuous. Against that: blank wire rows are not rare —
every message footer whose bytes are all claimed has one — so the root
is not a distinctive case and relocating it buys little; and when
`main_area.height` is odd the pane clips its last row, which costs a hex
row under document-first and costs the *subject* under wire-first,
leaving hex whose owning line is off screen.

Hex is lowercase `{:02x}`, punctuated as S11 describes. At most
`WIRE_ROW_MAX_BYTES` bytes are drawn, followed by `…×N` when more
remain. The cap exists because a LEN payload can be megabytes and a row
must stay pannable; its value is a legibility choice, not a measurement,
and belongs in a doc comment next to the constant.

### S6 — what has to be parsed

`NodeSpan.wire_type` already holds the tag's classification, so nothing
has to be re-parsed to know what kind of node this is. What the row does
need is the *boundaries* — where the tag varint ends and where the
length varint ends — and each is at most ten bytes of varint decode. The
palette itself is S11.

### S7 — one monochrome policy for both rows

Spec 0223 drops the document rows to monochrome while terminal events
are still queued, by *clearing* `window_styles`. The wire row's hues are
borrowed from exactly that vector (S11), so it resolves nothing and the
whole row falls to the subdued default — including its anomaly tiers and
its `!`.

The tiers going grey too is the point, not a side effect. A tier is
decided in Rust and would happily survive the input-pending frame; but
the `#@ pack_size` above it is decided by tree-sitter and would not. A
violet `0a 03` under a grey `pack_size` is exactly the disagreement
"one classifier, two rows" (S11) exists to prevent. One rule for the
pair costs a few transient monochrome frames and buys that the two rows
are never in different states.

The classification itself still runs under `input_pending`: it is a
per-byte pass over at most `WIRE_ROW_MAX_BYTES` bytes with no parser
involved. It is the *colors* that are unavailable, not the facts.

Mechanically: `wire_spans` takes `Option<&WirePalette>`; `Painter`
carries it, and every style accessor returns
`Style::default().add_modifier(Modifier::DIM)` when it is `None`.

### S8 — geometry: every row is twice as thick

Each window entry draws two terminal rows: window index `i` occupies
rows `2i` (the document line) and `2i + 1` (its wire row).

- A new `document_pane_height()` returns `main_area.height / 2`
  (minimum 1) in wire mode and `main_area.height` otherwise. It replaces
  the six existing reads of the pane height that are about *scroll*
  arithmetic: `clamp_pan_offset`, page up, page down,
  `max_visible_line_len`, the vertical pan, and `render`'s own
  `pane_height`.
- `window_styles`, `heat_displays`, `row_overridden` and `partner_cell`
  stay keyed by window index and need no change at all. This is the
  reason for a uniform 2× rather than a wire row only where it has
  something to say: any conditional row would re-key every one of those
  vectors.
- The `.map(…) -> Line` over the window becomes a `.flat_map(…)`
  yielding two `Line`s. The cursor row style, the caret, the drag
  selection and the brace partner all apply to the document row only.
- `main_pane_line_idx` uses `rel_row / 2`. A click anywhere in the pair
  selects the document line — the rows are simply twice as thick.

**The toggle holds the cursor's terminal row still.** `w` changes what a
document line costs, not where the user is looking. Left alone it would
move the cursor down or up the screen by however far it sat from the
top, which on a full pane is most of it: the user asked to see the
bytes, not to be scrolled. So `App::toggle_wire` measures the terminal
row the cursor is drawn on, flips `wire`, and puts it back.

It can always put it back exactly, because spec 0230 made the vertical
scroll a signed count of *terminal* rows rather than of whole document
lines. The two cases a whole-line scroll could not serve — an odd offset,
which needs the pane to start mid-line, and turning the wire rows off
near the top, which needs blank rows above the document's first line —
are that spec's subject.

`clamp_pan_offset` re-syncs `scroll_offset` only when the cursor has
moved since the last clamp, and here it has not — the pane changed
capacity under it instead. `toggle_wire` therefore forgets
`last_cursor_row` first, which is how that guard is told the clamp is
owed for a new geometry rather than for a new cursor.

### S9 — overlay rows

A preview overlay row has no *committed* node, but the preview
rendering it came from does have `NodeSpan`s into the same blob —
`render_node_as` returns them and `preview_override_highlight` today
throws them away, keeping only `rendered.lines`. `PreviewOverlay` gains
a `spans: Vec<NodeSpan>` field so it keeps them, and an overlay row's
wire row is then drawn exactly like a committed one, from the preview's
own spans and its own arena-free `raw_range`s.

This does not contradict spec 0185's N6 ("no `NodeSpan`s in
`self.tree`, and therefore no identity"). The spans stay out of
`self.tree`; the overlay remains unselectable, unfoldable and
unaddressable. They are display data, exactly like `lines`.

This is where wire mode earns its keep: a preview is a proposal to read
the same bytes as a different type, and the wire row is what shows those
bytes are indeed the same. A blank row there would hide the one
comparison a reader is making.

Consequence for G2: while a preview is on screen the concatenation is no
longer the file, because the overlay's bytes are also claimed by the
committed rows it stands in for. That is the same accounting the
document rows already have, and it lasts as long as the preview does.

### S10 — folding

G2's "every byte exactly once" is a property of the fully expanded
document. Folding hides bytes exactly as it hides lines: a folded
bracketed node draws only its header, so only its head bytes appear, and
its subtree's bytes are hidden with the subtree.

### S11 — punctuation, palette, flags

Four parts: how the bytes are punctuated, which hue each byte takes and
how bright, the handful of severity states a byte can be in, and which
anomaly puts a byte in which.

#### Punctuation — the row reads without color

```
T|FF FF FF:LL LL LL[PP PP PP PP]
│  └──┴──┴─ the tag varint, wire-type bits cleared: the field number
└─ the first tag byte's low three bits: the wire type, in decimal
```

- the tag varint comes first, its first byte drawn in two halves;
- `:` introduces the length prefix, on the wire types that have one;
- `[` … `]` enclose the payload.

The wire type is spelled as the one decimal digit it is rather than
left inside the hex, because it is three bits and hex cannot show three
bits. A nibble split — what this shipped as first — puts one bit of
field number on the wire type's side, so a colored or accused wire type
was accusing a bit it should not. Two extra columns on one byte of the
row buys an exact answer. Nothing is hidden by it: `2|08` and `0a` say
the same thing, and the second half is still a full hex pair that reads
as part of the run beside it.

`:` rather than `-` because it is textproto's own separator: the row
reads `tag : length [ payload ]` in the same shape as the `field: value`
line directly above it.

```
2|08:05[48 45 4c 4c 4f]   field 1, LEN, 5 bytes — the string "HELLO"
0|10[2a]                  field 2, varint, 42
5|08[00 00 80 3f]         field 1, fixed32
3|18                      field 3, group start — all a header row has
4|18                      field 3, group end — all a footer row has
```

One glyph, `!`, says the bytes ran out. It replaces whatever would have
come next — a closing `]`, or the rest of a varint:

```
2|08:05[48!             declared 5 payload bytes, one is present
7|f8 ff ff!             a tag varint that never terminates
```

Punctuation carries truncation rather than color because a truncation is
an *absence*: there is no byte to color. One glyph rather than two
because the distinction between "payload ran out" and "varint ran out"
is already told by *where* the `!` sits, so a second glyph would encode
what the reader can already see.

`:`, `[`, `]` and the `…×N` elision take no foreground and
`Modifier::DIM`. The `|` between the tag's two halves is the one piece
of punctuation that can be banded: it takes their band when they wear
the same one and stays unbanded when they do not — the same rule the
gap between two hex pairs follows, for the same reason. Giving them the comment hue would make the length
prefix indistinguishable from the brackets flanking it, which is the one
thing the palette below is for. Staying unbanded while the hex around
them is banded is also what makes the punctuation *separate* the
regions rather than join them.

#### Hue — four regions, borrowed from the document row

A wire byte gets a role of its own, in `theme.rs`, beside `HeatHue` —
the existing precedent for a role with no grammar capture behind it:

```rust
pub enum WireRole { Tag, Type, Length, Payload }
pub fn wire_style(role: WireRole, borrowed: Option<SyntaxRole>, theme: ThemeKind) -> Style
```

**A role of its own, not the document row's.** Painting a tag with
`SyntaxRole::Attribute`'s style would make the wire row a second, silent
consumer of a document color: retuning the field-name color would move
the hex with it, and a span that reports `Attribute` when it is a tag
misdescribes itself. What is borrowed is the *color* — never the role.
`wire_style` is the one place the borrowing happens, so the whole
mapping is retunable in one function.

The row resolves a `WirePalette` of four finished `Style`s once, from
the document row's syntax hints —
`App::window_styles[window_index]`, a `Vec<(Range<usize>, SyntaxRole)>`
over that row's *byte* offsets, already computed before the row loop.
The offsets are `display_row_text`'s, not `row_content`'s: the fold
margin the latter prepends would shift every one of them.

- **`Tag`** borrows the role covering byte offset `indent`, the row's
  first non-space byte — `(field_name) @attribute`, extension names
  included. An extension name is a field name: `[acme.blade]: 3` names a
  field, and on the wire it is a tag like any other field's, so it wears
  the field-name color rather than the type color it used to borrow.
  Reading the role off the row rather than naming `Attribute` outright
  still costs nothing and keeps the rule one sentence long.
- **`Type`** borrows the role covering the first token after `#@` —
  the declared proto type on a known field (`string`, `int64`), the
  wire type on an unknown one (`varint`, `bytes`). That is the same fact
  the tag's low three bits carry, and it is a *different* fact from the
  field number beside it, so it reads better as a different hue. A row
  with no annotation (`--no-annotations`) falls back to the field name's
  role: a tag that is one color throughout is honest, a tag with a grey
  digit in front of it is not. So does a row whose first annotation token
  is an accusation rather than a type name — spec 0232 S4, since the wire
  type on such a row is usually the one part of it that is well formed.
- **`Length`** borrows `SyntaxRole::Comment`, named by `wire_style`
  itself rather than read off the row. A field line's annotation is
  `(annotation) @comment`, but a row need not carry one, and a length
  prefix that changed color with the annotation's presence would be a
  color that means nothing. The comment *color* is the fact being
  borrowed, not some particular comment.
- **`Payload`** borrows the role covering the value's first byte: the
  first `:` in `code_part(text)`, past the spaces after it. No `:` on
  the row — a `name {` header, a `}` footer — means no payload bytes to
  color anyway. Since spec 0232 that is one color for every kind of
  value, so the payload hue no longer varies with the value's type; it
  is still read off the row rather than named outright, because the row
  is where the answer is.

The palette carries one thing that is not a color: `flaw`, spec 0232's
`Option<PayloadFlaw>`, the accusation the document row made about the
payload when it is one the hex can point at.

The painter tracks which of the four regions the pen is in, so a byte
with no tier takes that region's style. Bytes no framing claimed
(`Framing::Raw`, and the trailing fill) are in none of the four and take
the subdued default: nothing honest can be said about them.

If the row has no hints, there is no palette, and S7 is what that means.

#### Leveling — one brightness for the whole row, worn as a background

**A wire row spends two channels and each has one job.** The foreground
is the hex: one muted color, `WIRE_TEXT_DARK`/`WIRE_TEXT_LIGHT`, the
whole row long, so that the digits read as digits. The background is
meaning, and says two things in this order of precedence:

1. what the byte is *for* — the region's hue, borrowed from the document
   row and leveled;
2. that the byte is **wrong** — the tier's own color, unleveled, which
   takes the background outright.

Every borrowed hue is brought to a single brightness and moved to the
background, in `theme::banded(Style, ThemeKind) -> Style`:

- if the foreground is a `Color::Rgb`, blend it toward the palette's
  background — black for `Dark`, white for `Light` — by exactly as much
  as it takes to land its Rec. 709 relative luminance on
  `WIRE_LUMA_DARK`/`WIRE_LUMA_LIGHT`, and set that as the **background**,
  with `WIRE_TEXT_*` as the foreground. Luminance is affine in the blend
  factor, so the amount is a division, not a search. A color already past
  the target is left alone: the row recedes, it is never pushed forward.
- if it is an ANSI-16 named color, wear it as the background as it is,
  and set no foreground. ANSI-16 has no intermediate to blend to, and an
  RGB foreground over a band the terminal alone knows the brightness of
  is what the fallback exists to avoid.
- if there is no foreground, there is no band either: keep `Modifier::DIM`
  on the ordinary background. A band standing for "the row has nothing to
  say" — the ANSI-16 `Attribute` on dark, an unclaimed byte, the whole row
  under S7 — would be the loudest thing on the screen.

Blending each hue toward the background by a *fixed* amount instead
keeps the palette's own brightness spread, and a row mixing a bright tag
with a dim payload is uncomfortable to read at hex density. Leveling
makes the wire row read as one object, distinguished from the document
row by brightness and from itself by hue.

The two constants are legibility choices, not measurements, with one
hard bound each: on dark the target must sit below the dimmest borrowed
document color (comment, luma ≈138) or the "dim" becomes a brighten; on
light it must sit above the brightest (number, luma ≈104). They settled
at 58 and 197 — a little under a third of the way from the page to
`WIRE_TEXT_*`, far enough below the hex to leave it legible, far enough
above the page for the hue to be nameable. `WIRE_TEXT_DARK` and
`WIRE_TEXT_LIGHT` are themselves equally far from their pages (176 above
black, 176 below white), so neither theme's rows are the louder.

The hue is worn as a **background**, not as text. Color one glyph-pair
wide is a far weaker signal than the same color as a filled band, and the
row's whole job is to be scanned. This is also why leveling matters more
here than it would for text: an unleveled palette's brightness spread,
tolerable across foregrounds, becomes a row of mismatched blocks.

**The glyphs are not reversed, and not the terminal's own text color
either.** `REVERSED` was the first implementation and was wrong twice
over. It draws the hex in the terminal's *background* color, so the band
carries the glyphs' contrast as well as its own quiet — which pinned
`WIRE_LUMA_DARK` at 80, still loud enough that the wire row read as the
brighter of the two rows. And it leaves nothing free to distinguish an
accused byte with, since the band is already carrying the region. An
explicit foreground decouples the two, and the band is then free to be
nothing but a tint. That the foreground is *muted* rather than the
terminal's plain text color is what keeps a wire row visibly the
subordinate of the document row above it, even where its band is
quietest.

Leveling has one consequence for the palette itself, which is why the
wire row is where it surfaced: a color's *hue* survives leveling but its
identity may not. `Dark`'s `Type` was a vivid orange, and an orange at
luma 58 is a muted red — sitting next to the bright red an `Invalid`
tier paints, so a well-formed wire type read as an accused byte.
It is now a pale yellow (`#F2EA8C`), against the field name's pale blue.

Matching on the *returned* color rather than calling `supports_rgb()` a
second time is deliberate: `style_for` has already resolved which
palette is in play, so the color's own shape is the exact discriminator
and cannot disagree with it. The result is hue only — inherited
modifiers (`UNDERLINED` on a URL, `ITALIC` on an ANSI-16 comment) are
dropped: a modifier on hex is a locator, and this is not one.

#### Severity — the tier axis, shared with the text row

The annotation taxonomy has exactly two abnormal classes: non-canonical
(legal, round-trips, nobody should write it) and invalid. It *spells*
them in its capitalization — lower case, ALL CAPS — but only as a
convention, with known counterexamples in both directions, which is why
S12 classifies by keyword rather than by case. The severity axis has
exactly those two classes, plus a landmark and plain:

| tier | color | wire row | annotation |
|---|---|---|---|
| plain | the region's borrowed hue | ordinary bytes | `varint`, `bytes` |
| landmark | accent | a packed record's tag and length | `pack_size: N` |
| non-canonical | yellow | the redundant bytes | `tag_ohb: 3` |
| invalid | red | the offending bytes | `TAG_OOR` |

Giving hue the severity — rather than the earlier scheme where a white
background meant invalid and a red foreground meant overlong — is what
lets the two rows share one vocabulary (G7). A reader who has learned
"yellow means nobody should write this, red means this is not legal"
reads both rows with the same rule, and the eye can pair a red token in
the annotation with the red bytes underneath it.

The annotation wears the tier as **text** and the wire row wears it as a
**band**, at full strength and never leveled: `theme::tier_band(Tier,
ThemeKind)` sets the tier color as the background and the page's own
background as the foreground — black on `Dark`, white on `Light` RGB,
and black on both ANSI-16 palettes, where all six tier colors are
mid-brightness whatever the terminal renders them as and black is the
safer end against all of them.

The tier taking the background means it *displaces* the region's hue,
and that ordering is the point: an anomaly is the one thing that
outranks what a byte is for. The alternative — the tier in the
foreground over the region's own band, so that both facts show at once —
was implemented first and is worse in practice. It reads as a slightly
different shade of text on a screen already full of colored text; a band
swap at full strength does not. Nothing is lost by displacing the region
hue, because the accused byte's neighbours still carry it and the
annotation above already names the field.

The first tag byte is drawn in **two halves** — `t|nn`, per the
punctuation section above. They are colored separately because they fail
separately, and, when neither fails, they borrow from two different
tokens of the row above. It is the one byte on the row that is ever
split, and it is always a tag's first.

#### One unbroken ribbon

The separator space between two hex pairs is filled too, but only when it
lies *inside* one band: the byte about to be drawn wears a background,
and the byte just drawn wore the same one.

```
4|80 80 80 80 10
  ^  ^^ ^^ ^^ ^^     tiles
4|80 80 80 80 10
  ^^^^^^^^^^^^^^     one ribbon
```

One remembered band suffices, because the split byte ends on its
field-number half and that is the half a multi-byte tag's remaining
bytes continue. Spelling the wire type ahead of the tag rather than
inside it is what earns that: while it sat in the middle of byte 0 the
painter had to remember both of the byte's bands, or the field number's
ribbon broke where it stepped over the type.

The test is on the **background alone**, not the whole style: a space
has no glyph, so the color the hex is drawn in says nothing about it,
and two bytes on one band are one run whether or not anything else about
them differs. Since a tier *is* a band (above), the same rule joins a run
of accused bytes into one solid block of the tier's color — which is what
an accusation should look like. Bytes saying one thing are one fact, and
a ribbon broken every third column reads as several.

A band means the row has something to say about those bytes; a *tier*
band is what says a defect of the **message**. That is the line between
`!×N` below and the `…×N` elision of S5: the elision has neither, because
the row ran out of columns and the message did not run out of bytes. A
screen full of `…` must never read as a screen full of alarms.

#### Flags

| anomaly | what the row shows |
|---|---|
| `tag_ohb: N` | the N redundant tag bytes yellow |
| `len_ohb: N` | the N redundant length bytes yellow |
| `val_ohb: N` | the N redundant value bytes yellow |
| `etag_ohb: N` | the N redundant END-tag bytes yellow, on the footer row |
| `TAG_OOR` | the field portion of the tag red |
| `ETAG_OOR` | the same, on the footer row |
| `INVALID_TAG_TYPE` | the wire-type digit red |
| `INVALID_VARINT` | the varint's present bytes red, closed by `!` |
| `INVALID_LEN` | the length's present bytes red, closed by `!` |
| `TRUNCATED_BYTES` | the payload's present bytes red, closed by `!×N` |
| `INVALID_FIXED64` / `INVALID_FIXED32` | the payload's present bytes red, closed by `!` |
| `INVALID_GROUP_END` | the whole row red, except the wire-type digit |
| `END_MISMATCH: N` | the END tag's field portion red |
| `OPEN_GROUP` | the footer row is a bare `!` |
| `INVALID_STRING` | the payload's ill-formed UTF-8 sequences red |
| `INVALID_PACKED_RECORDS` | the payload's undecodable tail red |

The last two are spec 0232's, and are the two entries the row is *told*
rather than finding for itself; the rest of this table it derives. A
truncated payload's present bytes are red with the `!` that closes them
(spec 0232 S3): they are the bytes a reader inspects, and leaving them
plain put the only mark on the one glyph that stands for nothing.

`OPEN_GROUP` and `END_MISMATCH` are shown rather than spelled. `!`
already means *the byte that should be here is not*, and an unterminated
group is precisely that one level up: the END tag is the missing byte.
A mismatched END tag's field number is wrong and is right there in the
hex, so reddening it says everything the word did — the wire type is `4`
and is correct, and keeps its hue.

`INVALID_GROUP_END` is the same shape and needs saying explicitly,
because it is the one anomaly whose *row* is the defect. The core
renders a group end that closes a group nobody opened as a bytes-valued
pseudo-field whose "value" is however much of the buffer followed the
tag, so the whole row is accused: those bytes are on it only because the
stray tag was accepted, and drawing them unclaimed would say the row
found nothing to complain about after the first two. The wire type is
spared, as it is under `TAG_OOR` and `END_MISMATCH` — it
reports a group end and there is one. What is wrong is that the tag is
here at all, which is a fact about the row and not about those three
bits. A tag too malformed to parse is left to the ordinary tag path,
which has a better answer for it; `INVALID_GROUP_END` would be claiming
to know a wire type that was never read.

Knowingly collapsed: `ETAG_OOR` and `END_MISMATCH` now render
identically. Both say "this END tag's field number is wrong", the row
cannot show which kind of wrong, and the annotation above it already
does.

When two anomalies apply to the same byte, red wins over yellow. `N`
stays readable in every such case, because a varint is little-endian:
the redundant bytes of an overlong one are its **trailing** `0x80`…
`0x00` run, which is exactly what `parse_varint`'s `varint_ohb` counts,
and the reader can count them by position.

#### The one trailing marker

`TRUNCATED_BYTES` is the only anomaly with anything after its bytes:
`!×N`, glued to the `!` and wearing the `Invalid` tier with it, so
`0a:05[48!×4` is one accusation. `×N` reads as "N of these", the same as
it does after S5's elision; the tier color is what tells the two apart.

Everything else in the table is fully expressed by the bytes themselves
— a reader counts the yellow bytes to get `tag_ohb`'s N — and a marker
repeating it would only crowd the row. `TRUNCATED_BYTES` is the
exception because the bytes it counts are precisely the ones absent from
the row. `N` is `declared - available` and can reach 2⁶⁴−1 —
`grpconf/anomalies.pb` produces `MISSING: 18446744073709551615` — so it
cannot be spelled in repeated glyphs either.

#### Where the facts come from

Derived in the wire-row builder, from the bytes, not read back out of
the annotation string, because the annotation carries no byte positions
and byte positions are the whole point (G4). That does mean the same
fact is computed in two crates; the cross-check test below is what keeps
them honest, in preference to an abstraction spanning a crate boundary
that neither side wants.

The two exceptions are `INVALID_STRING` and `INVALID_PACKED_RECORDS`
(spec 0232 S5), which are schema questions — whether these bytes are a
`string` or a `bytes`, whether this record's elements are varints or
eight bytes wide — and so cannot be derived here at all. The verdict is
read off the annotation; only the byte positions are the row's own,
which is the division of labor this section describes rather than an
exception to it.

The row is built as a `Vec<Span<'static>>` in plain Rust — no parser.
The builder *generates* the text, so it already knows each byte's tier
at the moment it writes it; running a grammar over hex it has just
printed would re-derive what it knew. `Vec<Span>` is also what ratatui
and `pan_spans` already take, so a wire row travels the rest of the
render path as an ordinary row.

#### One classifier, two rows

The two rows must not be able to disagree about severity, and there are
two places they could. Both are closed:

- **The hue.** One table, `theme::tier_color(Tier, ThemeKind, rgb)`. The
  wire row reaches it through `theme::tier_band`, which puts the color
  in the background; the annotation reaches it through its capture and
  its `SyntaxRole`, which puts it in the foreground. Two channels, one
  hue table.
- **The classification.** One table, `annotation::tier_of(keyword) ->
  Tier`, listing the annotation vocabulary verbatim (S12). The wire-row
  builder does not invent tiers: having detected an anomaly it names the
  *keyword* prototext-core would have emitted — `"tag_ohb"`,
  `"INVALID_TAG_TYPE"` — and asks `tier_of`. So the flags table above is
  a mapping from bytes to keywords, and severity is decided once.

`highlights.scm` is the one copy of that table living outside Rust,
since a query file cannot call it. A test walks `tier_of`'s vocabulary
and asserts `colorize` gives each keyword the matching role, so the copy
cannot drift.

### S12 — the annotation gets the same palette, from the grammar

The `#@` annotation is currently one undifferentiated `(comment)
@comment` span. It gains structure in the vendored textproto grammar,
and `highlights.scm` maps that structure onto four tiers.

#### The four tiers

| tier | members | style |
|---|---|---|
| echo | the declaration before the first `;` — `google.protobuf.FileDescriptorProto = 2`, `repeated int32 = 85`, `Color(5) = 3`, a bare wire-type name | the document's own captures |
| landmark | `pack_size`, `[packed=true]` | accent |
| non-canonical | `tag_ohb`, `val_ohb`, `len_ohb`, `etag_ohb`, `ohb`, `packed_ohb`, `nan_bits`, `neg`, `truncated_neg`, `packed_truncated_neg`, `ENUM_UNKNOWN` | yellow |
| invalid | `TAG_OOR`, `ETAG_OOR`, `TYPE_MISMATCH`, `MISSING`, `END_MISMATCH`, `OPEN_GROUP`, `INVALID_TAG_TYPE`, `INVALID_VARINT`, `INVALID_FIXED64`, `INVALID_FIXED32`, `INVALID_LEN`, `INVALID_GROUP_END`, `INVALID_STRING`, `INVALID_PACKED_RECORDS`, `TRUNCATED_BYTES` | red |

The echo's wire-type names are `varint`, `fixed64`, `fixed32`, `bytes`
and `group`.

`ENUM_UNKNOWN` is yellow, though `annotation-format.md` files it as
informational, and though it is ALL CAPS. Yellow means "legal on the
wire, but no conformant writer emits one", which is verbatim what the
scorer's `out_of_range` counter is for — and an undeclared value in a
**closed** enum is exactly what charges it (`score/walk.rs`, −15, never
a veto).

It is unconditional, which over-accuses one case: prototext-core emits
the token whenever `enum_desc.get_value(n)` returns `None`, with no
`is_closed` gate, so an **open** enum carrying a forward-compatible
value gets it too — and there the scorer charges nothing, because
`reproto`'s `phases.py` graphs an open enum as plain `int32` with no
range at all. The grammar cannot tell the two apart: `is_closed` is a
schema fact and the annotation text carries no trace of it.

Accepted, because the alternative is worse. Untiered, the token says
nothing in the case where it is genuine evidence; yellow, it overstates
in the case where it is routine. A reader who checks a yellow
`ENUM_UNKNOWN` and finds an open enum has learned something in one look;
a reader who never notices the closed one has not. If prototext-core
later splits the token on `is_closed`, the yellow becomes exact and the
`#any-of?` list is the only thing that changes.

The lists below are the complete vocabulary, not a sample. Verbatim from the
emitters (`helpers/annotations.rs`, `helpers/scalar.rs`, `packed.rs`,
`varint.rs`, `sink.rs`, and `malformity_marker`'s exhaustive match) and
cross-checked against `encode_text/encode_annotation.rs`'s
`parse_annotation`, which is the closing argument: a token the encoder
does not name does not round-trip, so nothing outside these lists can be
emitted.

The echo and the landmark are both "structural/informational", and it is
tempting to give them one color. They are split because the echo is on
*every* annotated line while `pack_size` appears once per packed record,
and a thing cannot stand out while wearing the color of the thing beside
it on every line. Standing out is `pack_size`'s entire job: in a run of
a thousand identical element lines it is the only mark of where one wire
record ends and the next begins. It shares the accent with the wire
type and the length bytes on the same element's wire row (S4's
element-0 row), so the record boundary appears in both rows at the same
place — but not with that row's field number, which is the document's
own and wears the field-name hue on a packed row as on any other. Only
what is *about* the packing takes the accent. `[packed=true]` takes it
too: both it and `pack_size` are statements about packing, one in the
declaration and one in the modifiers, and a reader who has learned the
color on one meets it on the other.

#### The echo is the document's own vocabulary, so it wears its colors

Being colored like the document is not a resemblance imposed on the echo
— it *is* the document's vocabulary, quoted. Reading
`type: TYPE_INT32  #@ Type(5) = 5` left to right, five facts and four
colors:

| in the echo | color | because |
|---|---|---|
| `Type`, `repeated`, and a bare wire-type name | type | what the field *is*. The label and the type are two halves of one statement, and together they are what the wire row's wire type borrows, so splitting them would make that half of the tag depend on whether the field happened to be repeated. A wire-type name fills the same slot when there is no schema |
| `(5)` — an enum's wire value | number | the value the symbolic name on the document half stands for. One value, one color |
| `= 5` — the field number | field name | the document's own field number: the same color the name it belongs to wears at the head of the line, and the same the wire row's tag bytes borrow |
| `[packed=true]` | landmark accent | see above |

The type color is **warm**, not the blue-green VSCode gives a type. A
type name and its wire type sit beside a field name and a value on every
annotated line, and the borrowed blue-green read as a cousin of the field
name's blue. Warm is the one direction no other role uses. It shipped as
an orange and is now a pale yellow — `#F2EA8C` on dark, `#AF3A03` on
light — because a leveled orange on a wire row read as an anomaly, which
is the one confusion a warm type color can cause; see the leveling
section above and spec 0231 for how far apart the two yellows had to be
pushed. The blue-green stays in the palette under its own name for the
Any-key domain, which is a URL and not a type.

Two captures had to move for this to hold together. `(scalar_value
(identifier)) @constant` — `TYPE_INT32`, an enum value name — takes the
*number* color, because a symbolic value and the number it stands for
are one fact seen twice, once per half of the line. And an extension
name loses `@type` for the blanket `@attribute`: `[acme.blade]: 3` names
a field, and on the wire it is a tag like any other field's.

The ANSI-16 fallback keeps its cyan/blue type. Sixteen colors hold no
orange, and both candidates for the warm slot are already spoken for by
a severity tier — red by invalid, yellow by non-canonical — so borrowing
one would make a declared type look like an anomaly. What that palette
can deliver is separation between roles, and cyan already delivers it.

The four lists become four `#any-of?` predicates in `highlights.scm`,
declared after a blanket `@comment` — exactly the shape §4 of that file
already uses for `true`/`false`/`inf` against a blanket `@constant`.
`pack_size` and `ENUM_UNKNOWN`, the two tokens whose tier contradicts
their capitalization, need no special handling at all: each is simply in
the list it belongs to and out of the ones it does not.

#### Why the grammar, and what it costs

Because a `#@` body is structured text with a published EBNF
(`annotation-format.md`, "Grammar"), and because everything else that
colors a rendered line already travels the one path
`highlights.scm` → `SyntaxRole` → `theme` → `window_styles`. A scanner
in protolens would be a second mechanism painting the same line, and it
would have to merge into `window_styles` by byte offset anyway. Going
through the grammar means pan, the spec 0223 monochrome rule, override
bold and drag selection all keep working with no new code.

The wire row does not go through tree-sitter and never will: it
classifies *bytes*, the grammar classifies *text*. What the two share is
named above — `annotation::tier_of` for which tier, `theme::tier_color`
for what that tier looks like, and `window_styles` for the hues the wire
row borrows. The grammar's half of `tier_of` is the `#any-of?` lists,
kept in step by test 35.

Three constraints the grammar work must respect:

1. **`annotation` is a new `extra`, alongside `comment`, opening on a
   `#@` token with explicit precedence.** Spec 0201's lesson applies
   directly: tree-sitter compares explicit token precedence *before*
   match length, so the contest between `comment`'s `'#'` and
   `annotation`'s `'#@'` must be settled by precedence rather than left
   to the longer match. A composite (non-terminal) extra is not new
   here — `comment: $ => seq('#', /.*/)` already is one.
2. **The rule must be total.** Every `;`-separated item must have a
   catch-all alternative (`/[^;\n]+/`) so that no annotation, however
   malformed, fails to parse. Error recovery in this grammar swallows
   *following* siblings, so a rule that can fail would lose the
   highlighting of lines after it — the defect spec 0187's synthetic
   enclosing context exists to avoid.
3. **The grammar stays keyword-blind; the query carries the keywords.**
   The grammar splits an annotation into `;`-items and an item into a
   key, an optional `: value`, and the catch-all — nothing more.
   `highlights.scm`'s four `#any-of?` lists decide the tier.

   The one thing the query cannot do on its own is *split a token*, so
   an enum field's `Color(5)` is two tokens in the grammar
   (`annotation_word` then `annotation_enum_value`), the name being a
   type and the value in the parentheses a number. Splitting costs
   nothing structurally — the pair is still adjacent, and nothing but a
   type name can precede it — and the token sits above `annotation_junk`
   in precedence so the whole group is taken rather than its first
   character. An unterminated `(` matches neither and falls to junk,
   which is what keeps rule 2 true of a malformed annotation. The
   parentheses go with the number: they are one token, and a third split
   to make them punctuation would buy nothing the eye needs.

   The declared type is found by what *follows* it rather than by a list
   the query would have to keep: a word immediately before an `=`, an
   enum value, or a `[packed=true]` is the type.

   Deliberately not a capitalization test (`/[a-z_]…/` versus
   `/[A-Z]…/`), which was the earlier design. Case is a *convention* the
   format documents, not a rule it enforces: it already has two
   exceptions in a vocabulary of twenty-five, so the regex would have
   needed its own exception list anyway, and a third exception added
   upstream would be miscolored silently rather than caught. Naming the
   keywords costs a query edit when prototext-core adds one — which is
   the moment a human should be deciding its severity, not a regex.

   Splitting the work this way also puts the volatile half in the cheap
   file: a `highlights.scm` edit needs no `tree-sitter generate`, while a
   `grammar.js` edit rebuilds the Rust world (~15 min).

New `SyntaxRole` variants: `AnnotationLandmark`, `AnnotationNonCanonical`,
`AnnotationInvalid`. Added as pairs in `RECOGNIZED_NAMES`, per the
existing rule that a role and its capture name cannot come apart.

Non-goal N6 stands and is now visible: a schema-dependent anomaly such
as `TYPE_MISMATCH` shows red in the annotation and has no counterpart in
the wire row. That is the correct reading — the wire row has nothing to
say about it.

## Alternatives considered

**A separate hex pane.** Rejected: the byte-to-line correspondence *is*
the feature, and a second pane hands the correlation back to the reader.

**Real document lines.** Rejected: `lines_total`, `LinePos`, folding,
search, export, the override machinery and every cache key would each
have to learn that some lines are not part of the document — for a view
that is read-only and transient.

**Reusing the `#@` annotation text to drive the flags.** Rejected: it
carries no byte positions, and G4 is precisely about byte positions.

**A wire row only where there is something to show.** Rejected: it
breaks the fixed 2× that lets `window_styles`, `heat_displays` and
`row_overridden` stay keyed by window index untouched, and it makes the
pane's capacity content-dependent, which every scroll and page
computation would then have to ask about.

**Wrapping a long payload over several rows.** Rejected for the same
reason, plus it makes a row's height content-dependent.

**Paint the wire row with the document `SyntaxRole`s directly, with no
`WireRole`.** Fewer types, and it makes the wire row an undeclared
second consumer of every document color: `style_for(Attribute)` could no
longer be retuned for field names without moving the hex, and there
would be no single place to change what the wire row borrows.

**Name `SyntaxRole::Attribute` for the tag instead of reading the row.**
Simpler by one lookup, and wrong for an extension or an `Any` type name,
which the document row already colors `@type`. The lookup is into a
vector that is built for this frame anyway.

**Dim each hue by a fixed fraction instead of leveling.** Rejected: it
preserves the palette's brightness spread, so a bright tag next to a dim
payload stays uncomfortable at hex density. Leveling to one luminance is
what makes the row read as a single object.

**One hue for the whole tag byte.** Rejected: the field number and the
wire type are two different facts, and the row already splits that byte
to show a tier on one of them. Splitting the hue too costs nothing and
says more.

**Indent the wire row one level past its document row.** Rejected: once
the row is leveled to a lower brightness, the brightness step already
separates the pair, and it does so across the whole row rather than at
its left edge only. The indent buys nothing and costs columns against
N9.

**Keep the anomaly tiers colored while input is pending, subdue only the
borrowed hues.** Rejected: it is the two-policy state in miniature, with
the wire row's tiers contradicting the annotation's own greyed-out
tokens on the same pair of rows.

**Spell `MISSING` as N red placeholder pairs (`!! !! !! !!`).**
Rejected: N is unbounded — a declared length of 2⁶⁴−1 is a real case in
the fixture — so the row's width would be attacker-controlled.

**`!…` with no count.** Rejected: it throws away the one fact the row
cannot otherwise convey.

**Uppercase hex.** Rejected on idiom: every tool this row sits beside —
`xxd`, `hexdump -C`, `od`, `gdb x/`, Wireshark's byte pane — is
lowercase, as are `git` object ids, `{:02x}` and `%02x`. Uppercase
survives mainly in RFC 4648 Base16. Lowercase also has ascender variety
(`b d f`) where uppercase is a uniform block of same-height glyphs,
which matters once the row is leveled down. The counter-argument — that
uppercase separates hex from the lowercase textproto above it — is
answered by the bands, which say it more directly.

**Label the row `\x ` (or `0x: `).** Rejected. It was the first
implementation, and once the hex became a row of bands the label had
nothing left to disambiguate: no prototext line looks like that. It cost
two columns of a row that is already the one competing with the pan
bound (N9). S5's `└── ` is not this: it answers *which row*, not *what
row*, and that question survives the bands.

**Indent alone, with no connector.** Rejected, and it was the state
between the label and `└── `. Alignment says which *column* the row
belongs to, not which row: a nested message's own first field carries
the same indent one row further down, so in a screen of alternating
rows the pairing has to be counted out. The elbow is unambiguous and
needs no convention taught, `tree` having taught it.

**Reverse only the anomalies, leaving ordinary bytes as colored text.**
Rejected, and it was the first implementation. Reverse then had to serve
as a *locator* — "look here" — which is exactly the signal that stops
working when the thing being located is common. Making every byte a
block moves the distinction onto brightness, which the tier colors
already carry: they are the palette's most saturated, and the borrowed
hues are leveled down. The gain is that an ordinary region also reads as
one object, so the eye picks out where the tag ends and the payload
begins without reading a single digit.

**Put the wire row above its document row.** Rejected — see S5 for the
argument, which turns on blank wire rows being common rather than rare,
and on which of the pair survives an odd pane height.

**One "structural/informational" color covering both the declaration
echo and `pack_size`.** Rejected: the echo is on every annotated line
and `pack_size` on one line per record, so sharing a color is exactly
the way to make `pack_size` invisible — the opposite of what it is for.

**Two regexes on capitalization driving the annotation's colors.**
Rejected, though it was the earlier design and it is tempting: the
format does spell severity in case, so `/[a-z_]…/` and `/[A-Z]…/` would
cover most of the vocabulary and would color a newly added modifier with
no edit here. But case is a convention the format documents rather than
a rule it enforces — `pack_size` and `ENUM_UNKNOWN` already break it —
so the regexes need an exception list, and the next exception is
miscolored in silence instead of being caught. Naming the keywords costs
one query edit when prototext-core adds one, at the moment a human
should be deciding its severity anyway; and it lets the wire row and the
annotation share `tier_of` rather than agreeing by coincidence.

## Test plan

### Which bytes (S2–S4, S9)

1. `a_message_header_row_shows_only_its_tag_and_length`.
2. `a_message_footer_row_is_empty_when_every_byte_is_claimed`.
3. `an_unclaimed_trailing_byte_shows_on_the_footer_row` — the case the
   partition surfaces for free.
4. `a_group_footer_row_shows_the_end_tag`.
5. `a_string_row_shows_its_tag_length_and_payload`.
6. `a_packed_run_splits_its_elements_one_per_row` — including the tag
   and length on element 0's row.
7. `the_wrapper_root_has_a_blank_wire_row` — S3.
8. `a_preview_overlay_row_shows_the_preview_nodes_bytes` — S9, and that
   they are the bytes the committed rows it replaces show.
9. `every_byte_appears_exactly_once_in_document_order` — G2: fully
   expand, no preview on screen, concatenate all wire rows, compare
   against `Blob::payload` as `{b:02x}`.

### The row's shape and its hues (S5, S7, S11)

10. `a_wire_row_is_aligned_with_its_document_row` — the row is the
    document row's own margin, then `└── `, then its first hex digit.
11. `the_punctuation_reads_without_color` — `2|08:05[48 45 4c 4c 4f]`
    for a five-byte string, `2|80 80 80!` for an unterminated tag
    varint.
12. `a_region_is_one_unbroken_ribbon` — every hex byte is banded and the
    punctuation is not, so the mask over `2|08:05[48 45 4c 4c 4f]` is
    `# ## ## ############## `; and every span inside the payload,
    separators included, carries the one payload band.
13. `the_tag_the_type_the_length_and_the_payload_wear_four_hues` — four
    regions, four distinct fixture bands, with the tag's two halves in
    two of them.
14. `the_tag_takes_the_field_names_hue_dimmed` — the tag's style is
    `banded(style_for(Attribute))`, and is dimmer than the document's
    either by color or by `Modifier::DIM`.
15. `the_payload_takes_the_values_hue` — a string field's payload takes
    `banded(style_for(StringLiteral))`, a numeric field's takes
    `banded(style_for(Number))`, and the two differ.
16. `the_length_prefix_takes_the_comment_hue_with_or_without_an_annotation`
    — same color with `annotations` on and off.
17. `a_row_with_nothing_to_borrow_is_gray_throughout` and
    `a_wire_row_goes_monochrome_with_the_document_row` — S7: no span
    carries a band or a foreground, tiers included.

### Anomalies (S11)

18. `the_wire_type_is_styled_apart_from_the_field_number` — on a wire
    type 7 tag: the wire-type digit takes the `Invalid` band, the field
    number keeps its own.
19. `an_open_group_footer_row_is_a_bare_bang` — the row is `!`, and the
    painter's flag list still contains `OPEN_GROUP`.
20. `a_group_footer_names_the_group_it_closes` — on a mismatch the
    field-number half of byte 0 takes the `Invalid` band, the wire-type
    digit keeps the type band, and no marker text follows.
21. `a_truncation_counts_the_bytes_it_cannot_show` — the row ends `!×4`,
    and both glyphs wear the `Invalid` band.
22. `an_accusation_marks_the_bytes_it_names` — the two cases above,
    asserted as a caret mask over the drawn row: a run of accused bytes
    is one solid block, separators included.
23. `an_overlong_varint_shows_its_trailing_padding` — the two padding
    bytes take the tier band and `aa` does not, and the separator between
    them is filled because both sides share it.
24. `a_long_payload_is_elided_rather_than_wrapped` — the row ends `…×67`
    and the marker has neither a band nor a foreground.
25. `a_group_end_closing_nothing_accuses_its_whole_row` — every byte of
    `d4 06 08 01` is `Tier::Invalid` except the wire-type digit, which
    keeps its band, and the row reports `INVALID_GROUP_END`.
26. `the_flags_agree_with_the_prototext_annotation` — on a malformed
    fixture, each row's flag set equals the wire-level subset of the
    `#@` annotation on the line above it. This is what keeps the two
    independent derivations honest ("where the facts come from").

### The annotation's coloring (S12)

Grammar-level cases go in `reproto/tree-sitter-textproto/test/`;
role-level cases go in `colorize.rs` and use `roles_across`, never
`roles_at` — the latter is structurally blind to a capture whose span
is split wrongly.

27. `a_hash_annotation_is_not_a_plain_comment` — constraint 1: `#@`
    opens an `annotation`, a bare `#` still opens a `comment`, and a
    `#@` inside a quoted string is still string content (spec 0201).
28. `a_malformed_annotation_does_not_disturb_the_next_line` —
    constraint 2: a body matching nothing in the format still parses,
    and the following field keeps its captures.
29. `a_modifier_takes_the_hue_of_its_keyword` — one line carrying both
    `len_ohb: 2` and `TAG_OOR`.
30. `an_unlisted_modifier_stays_a_comment` — constraint 3's accepted
    cost: an invented modifier gets no tier rather than a guessed one,
    so a keyword prototext-core adds is visibly uncolored until it is
    listed.
31. `the_declaration_echo_matches_the_documents_own_styles` — the type
    name and the label carry `@type`, the field number after the `=`
    carries `@attribute` (the same capture the field name at the head of
    the line carries), and `[packed=true]` carries the landmark accent.
    An extension name is `@attribute` too, not `@type`.
32. `an_enum_declaration_splits_its_name_from_its_value` — `Type(5)` is
    a type and a number, and the document's own `TYPE_INT32` beside it
    is `@constant`, which is now the number color. A packed enum's
    `Color([1, 2])` is still one number-colored token.
33. `pack_size_and_its_wire_bytes_share_the_accent` — on the first
    element line of a packed record: the wire type and the length
    prefix wear the accent, and the field number does not.
34. `enum_unknown_is_yellow_not_red` — ALL CAPS, non-canonical tier.
    The capitalization rule's counterexample in the direction
    `pack_size` does not cover.
35. `every_keyword_is_colored_by_its_tier` — the drift test: iterate
    `annotation::tier_of`'s whole vocabulary, colorize a line carrying
    each keyword, and assert the role matches the tier — `@type` for the
    untiered wire-type names, the tier's own role for the rest. Fails
    when `highlights.scm`'s copy of the lists falls behind the Rust one.

### Geometry and the toggle (S1, S8)

36. `the_lines_that_fit_halve_when_wire_mode_is_on`.
37. `a_click_on_a_wire_row_selects_the_line_above_it`.
38. `page_down_advances_by_the_halved_height`.
39. `toggling_wire_mode_invalidates_no_cache` — S1:
    `structural_version` unchanged, `heat_states` unchanged.
40. `toggling_wire_mode_keeps_the_cursor_on_its_terminal_row` — in both
    directions, from an even offset and from an odd one.
41. `turning_wire_mode_off_near_the_top_pads_above_the_first_line` — the
    case with no lines above the cursor to give back.

### Manual

42. On `grpconf/anomalies.pb` with `w`: every one of the six sections of
    `grpconf/README.md` reads as intended in both themes and at both
    color depths (`COLORTERM=` unset forces ANSI-16).

## Measured outcome

`w` toggles the pane between one and two terminal rows per document
line, with no re-render and no re-score behind it: `structural_version`
and every `HeatState` are unchanged across the toggle (test 39). It also
holds the cursor's terminal row exactly, in every case, once spec 0230
gave the scroll a unit fine enough to say so (tests 40 and 41).

`WIRE_LUMA_DARK = 58`, `WIRE_LUMA_LIGHT = 197`, against measured palette
lumas of: dark — attribute 209, number 198, string 156, type 147,
comment 138; light — attribute 49, string 51, type 79, comment 92,
number 104. Both started one step inside their bound — 130 and 150 —
and ended far past it, because a filled band is not a dimmer version of
colored text: it emits light across the whole cell where glyphs emit it
across a fraction of one, so a wire row at the document row's own
brightness still reads as the *louder* of the two. Reported as the
document rows looking muted; nothing dims them, and the fix is on the
wire row's side.

They can go that far only because the hex is neither reversed nor left
in the terminal's own text color. Under `REVERSED` the terminal draws
the glyphs in its own *background* color, so the band carried their
contrast as well as its own quiet, which pinned the dark constant at 80.
With `WIRE_TEXT_DARK = #B0B0B0` / `WIRE_TEXT_LIGHT = #4F4F4F` the two
channels are independent: the hex has a fixed contrast of its own and
the band is free to be nothing but a tint, a little under a third of the
way from the page to the hex. The pair sit 176 from their respective
pages, so neither theme's rows are the louder. Four rounds of user
feedback were spent on the band's brightness before the two channels
were separated, and the two-channel option had been on the table from
the first report.

The three tier colors moved the other way, since what separates an
anomaly from an ordinary byte is a wide brightness gap and it should be
used. `leveled` never touches a tier, so on a row of bands brought down
to `WIRE_LUMA_*` an anomaly is the one bright thing. On dark, VSCode's
three are each scaled up until one channel saturates — same hue, same
relative mix, as much light as the hue can carry: `#C586C0` → `#FFAEF8`
(luma 152 → 196), `#CCA700` → `#FFD100` (163 → 204), `#F14C4C` →
`#FF5555` (111 → 121). On light the same word means the opposite,
prominence against white being depth rather than brightness, and only
one of the three was weak: `#BF8803` → `#9C6A00` (138 → 109), which had
been the quietest mark on the light theme while meaning more than the
landmark it sat beside.

Those colors then went into the **background**, not the foreground. The
first implementation put the tier in the foreground over the region's
own band, keeping both facts visible at once; it was reported as not
reading as an alarm at all, which is right — it is a slightly different
shade of text on a screen already full of colored text. A band swap at
full strength is unmistakable, and the region hue it displaces is
carried by the accused byte's neighbours anyway.

One palette color had to move for the same reason. `Dark`'s `Type` was
`#FE8019`, a vivid orange, and `leveled` at 58 turns an orange into a
muted red — beside the bright red of an `Invalid` tier, which made a
perfectly good wire type read as accused. It became a pale yellow
against the field number's pale blue, and was pushed further from the
non-canonical tier's own yellow afterwards; spec 0231 records where both
ended up and why one move was not enough.

The row carries no label. Ahead of its first hex pair it carries only
`tree`'s `└── `, subdued, at the document row's own indent — four
columns that answer which row the hex belongs to, which alignment on its
own does not; and a row that drew no hex carries nothing at all, elbow
included.

Wire styles are resolved once rather than per byte. The inputs are
finite — one `SyntaxRole` per capture, times the two resolved themes —
so `theme::wire_styles` fills a `OnceLock` table on first use and
`wire_style` indexes it. Before that it ran `supports_rgb` (whose first
branch is an *uncached* environment read that allocates a `String`) and
then `leveled`'s blend, four times for every drawn row of every frame.

`cargo test -p protolens` passes 619, plus 25 in `tests/batch_export.rs`;
the vendored grammar's
`nix-build -A tree-sitter-textproto-highlight-test` passes 126
assertions, 20 of them new. Eight protolens tests fail in a nix-shell
entered before the grammar was rebuilt — `TREE_SITTER_TEXTPROTO_QUERIES_DIR`
then points at a `highlights.scm` with no annotation captures, and
`colorize.rs` `include_str!`s that file at *build* time, so the binary
queries for captures its baked-in copy never emits and every annotation
modifier falls through to plain `Comment`. On screen this shows as S12's
colors silently missing from the document row. Re-entering the shell
clears it; there is nothing to fix in the code, and the eight tests are
what catch the drift.

Three findings from the grammar half (S12) that the design did not
anticipate, all recorded in `grammar.js` beside the rules they shaped:

- **A composite `extra` cannot end in a `repeat`.** tree-sitter rejects
  it outright ("Extra rules must have unambiguous endings"), so the
  annotation carries an explicit `annotation_end` newline token rather
  than trailing off at end of line. `colorize` therefore appends a
  newline to whatever it is handed, since a rendered window is joined
  without one and its last line may well be annotated.
- **A token that may begin with whitespace poisons its whole lex
  state.** Spelling the item tokens `token.immediate(/[ \t]*…/)` so each
  absorbed its own spacing made `outer {` lex `{` as a two-character
  `open_squiggly` starting at the space — tree-sitter's DFA state
  merging carries the "a token may start with spaces" property outward.
  Every item token is an ordinary token now, and the spacing is left to
  the `/\s/` extra. This is what the `#any-of?` predicates need: they
  compare a capture's text exactly, and a leading space is not
  `pack_size`.
- **Consuming the newline revives `token.immediate` on the next line.**
  `token.immediate` means "no extras were skipped", not "no whitespace
  precedes"; with the annotation's terminator eating the line break, the
  next line's first byte is adjacent to it, and `float`'s immediate
  `[Ff]` suffix (spec 0196 S5) attached to a field named `f` one line
  below an annotation. The suffix is folded into a single
  `float_suffixed` token, leaving no `token.immediate` in the grammar
  outside the annotation rules.
