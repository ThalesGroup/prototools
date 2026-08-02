<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0226 — a fixture shows every anomaly

Status: implemented
Implemented in: 2026-08-02
App: protolens | prototext
Refs: docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md (the `w`
        row and the three severity tiers this fixture exists to
        exercise); docs/prototext/annotation-format.md (the definitive
        list of tokens it must cover)

## Background

The gRPConf 2026 demo needs a blob to point protolens at. Nothing in the
repo serves: `prototext-core/fixtures/descriptor.pb` is a canonical,
anomaly-free descriptor, `tests/fixtures/instances/` holds ordinary
googleapis instances, and the anomaly coverage that does exist lives
inside Rust unit tests as byte literals, one anomaly at a time, invisible
to a demo.

So the coloring shipped by spec 0225 — landmark / non-canonical / invalid,
on both the text row and the `w` wire row — has never been seen with more
than one or two tiers on screen at once, and its vocabulary has never been
checked against what prototext-core actually emits. Two gaps turned up
while scoping this spec, both by reading rather than running:

- **`INVALID_PACKED_RECORDS` and `INVALID_STRING` are emitted but not
  classified.** `render_text/packed.rs:315` and `render_text/sink.rs:539`
  write them; `protolens/src/annotation.rs`'s `INVALID` does not list
  them and neither does `highlights.scm`. They render in plain comment
  gray today, exactly as if nothing were wrong.
- **`packed_ohb` and `packed_truncated_neg` are classified but never
  emitted.** They appear in `annotation.rs`'s `NON_CANONICAL` and in
  `highlights.scm`, but the only occurrence anywhere in prototext-core is
  `encode_text/encode_annotation.rs:82,87` — the encoder accepts them,
  the renderer writes `ohb` and `neg` instead.

A third turned up while scoping the comment convention of G2, and this
one is a genuine defect: **a `#` comment on a field line that carries no
annotation silently deletes the line.** `seconds: 5  # note` has no
`  #@ ` to split on, so the value becomes `5  # note`, fails to parse,
and is dropped — a document of only that line encodes to zero bytes and
exits 0. See I3.

A fixture that is *required* to cover the whole vocabulary is what makes
gaps of this shape fall out on their own. All three have proposals; see
Proposals below.

## Goals

- **G1.** A single hand-authored fixture under `./grpconf` exhibiting
  every annotation token prototext-core can emit: the five wire-type
  names, `pack_size`, every non-canonical modifier and every invalid one.
- **G2.** It is authored in `#@` prototext text, against
  `googleapis.desc`, and is readable on its own: ordinary `#` comments
  explain how each anomaly was crafted, so opening it in a plain editor
  teaches the format.
- **G3.** No build step and no derived artifact. What is committed is
  what protolens opens, so the demo cannot fail on a stale blob.
- **G4.** The coverage claim in G1 is enforced by a test, not by
  discipline.

## Non-goals

- **N1.** *Implementing* the fixes. They are specified under Proposals,
  because a finding without a remedy is a bug report rather than a spec,
  but each lands on its own — P3, P5 and P6 touch the encoder and
  deserve their own review. Building the fixture does not depend on any
  of them; it is what makes them checkable.
- **N2.** A binary form of the fixture, generated or committed. S4's
  point is that there is no second artifact at all.
- **N3.** A scripted demo — slides, a `.sh` walkthrough, timing. `demo/`
  already exists for that and is out of scope here.
- **N4.** Covering anomalies protolens cannot *reach*. Everything in the
  vocabulary is reachable, so this is currently empty; it is stated so
  that a future token which is not gets excluded deliberately rather
  than forgotten.

## Specification

### S1 — `prototext encode` ignores whole-line comments

Verified, not assumed. `encode_text/mod.rs:265` skips any line whose
trimmed content starts with `#` and not with `#@`, so a whole-line
comment contributes nothing to the output. Round-tripped: a
`google.protobuf.Duration` fixture carrying a top-level comment, an
indented comment and two annotated fields encodes to exactly
`08 05 10 07`.

This is not a concession the fixture makes to the encoder; it is what
the format already does. `prototext decode` emits `# Type:` and
`# Score:` comment lines of its own above the first field, so a
whole-line comment is a first-class part of every document the tool
produces. Comments and blank lines also survive *inside* a packed run
without disturbing its element accounting (verified — see I2).

**Trailing comments are a different matter, and today both forms are
hazardous.** After an annotation a `#` is annotation text — the
annotation runs to end of line — so a `;` in the "comment" changes the
bytes. Before one, a `#` is not recognized at all and silently destroys
the line. Both are detailed in I3; P5 bounds the first and P3 fixes the
second. The fixture puts every explanatory comment on **its own line**
regardless, independently of whether those land: it is the readable form
anyway, since an anomaly needs a sentence and not a fragment.

### S2 — the coverage inventory

Taken from the renderer, which is the only thing that decides what a real
blob looks like. `docs/prototext/annotation-format.md` is the prose
reference for each entry.

| Group | Tokens |
|---|---|
| Wire types (no tier) | `varint`, `fixed64`, `fixed32`, `bytes`, `group` |
| Landmark | `pack_size` |
| Non-canonical | `tag_ohb`, `val_ohb`, `len_ohb`, `etag_ohb`, `ohb`, `nan_bits`, `truncated_neg`, `neg`, `ENUM_UNKNOWN` |
| Invalid | `TAG_OOR`, `ETAG_OOR`, `TYPE_MISMATCH`, `MISSING`, `END_MISMATCH`, `OPEN_GROUP`, `INVALID_TAG_TYPE`, `INVALID_VARINT`, `INVALID_FIXED64`, `INVALID_FIXED32`, `INVALID_LEN`, `INVALID_GROUP_END`, `TRUNCATED_BYTES`, `INVALID_PACKED_RECORDS`, `INVALID_STRING` |

Thirty tokens. Note the two asymmetries with `protolens/src/annotation.rs`
recorded in the Background: this table is the renderer's list, and the
last two invalid entries are the ones protolens does not yet classify.

Every one is authorable from text — the encoder has a branch for each
invalid wire type (`encode_text/fields.rs:175-220`) and for each modifier
(`encode_annotation.rs`).

### S3 — every terminal anomaly is nested

Roughly half the invalid tokens mean *the scan cannot continue*: a
malformed varint, an unrecognized wire type, a length prefix longer than
the buffer, a group with no end. The decoder cannot find the next tag
after one, so it consumes the rest of the enclosing region. Measured, on
two synthetic blobs each ending in a valid `3: 9  #@ varint`:

```
1: 5  #@ varint
2: "\001\002\030\t"  #@ TRUNCATED_BYTES; MISSING: 3
```
```
1: 5  #@ varint
0: "&\030\t"  #@ INVALID_TAG_TYPE
```

Both swallowed the trailing field. Note `MISSING` reported 3 rather than
the 5 the blob was built with — the missing count is computed against
what was actually left, so it is not independently choosable.

"Enclosing region", though, is not the whole buffer. A LEN submessage is
bounded by its own length prefix, so a terminal anomaly placed inside one
is contained by it and the parent's scan resumes at the next tag. That
containment is what makes the single fixture of S4 possible at all:
**every terminal anomaly goes inside its own submessage**, and the
document is a sequence of them.

One submessage each, not one shared: two terminal anomalies in the same
region would still swallow one another. The clean way to get an
arbitrary number of sibling submessages out of one root type is a
**`repeated <Message>` field** — each occurrence is a separate LEN
region, so the fixture can hold as many terminal anomalies as it likes
without inventing a container per anomaly. That is criterion 2 of S7.

The containment works only where the schema declares the field a message,
so that the decoder descends. Without a schema, `--raw` renders a LEN
payload as an opaque string and the anomaly inside it is never seen — the
same three-submessage blob decodes to three uninformative `string` lines
under `--raw`. The demo therefore always runs with a descriptor set (S7
says which).

### S4 — one fixture, committed, named `.pb`

```
grpconf/
  README.md              how to run the demo, and what the fixture shows
  anomalies.pb           the fixture: `#@` prototext text, not wire bytes
  anomalies.pb.license   its SPDX metadata
```

**No build step, and no generated artifact.** Both tools decide the
format by *content*, never by extension: `Blob::load`
(`protolens/src/blob.rs:90`) peeks the first 13 bytes and calls
`is_prototext_text`, and `prototext` does the same — its
`decode --assume-binary` flag exists precisely to switch that detection
*off*. So the fixture can be authored in text, named `.pb`, committed as
the source, and opened directly:

```
protolens --descriptor-set prototext-core/fixtures/descriptor.pb \
          --type google.protobuf.FileDescriptorProto \
          grpconf/anomalies.pb
```

The root type goes on **its own `# Type:` comment line**, directly under
the magic line, rather than after the colon on the magic line itself:

```
#@ prototext: protoc
# Type: google.protobuf.FileDescriptorProto
```

That is the shape `prototext decode` already emits when it infers a type
(`prototext/src/run.rs:253`), so the fixture's header is the tool's own
header and not a second convention.

Verified. A two-field `Duration` document copied to `fixture.pb`, and
the four bytes it encodes to, export the same rendering character for
character:

```
$ protolens --raw fixture.pb export /      # 190 bytes of `#@` text
$ protolens --raw cmt.pb     export /      # the 4 bytes `08 05 10 07`
#@ prototext: protoc
1: 5  #@ varint
2: 7  #@ varint
```

One trap worth recording, because it looks like a counter-example: the
`(N bytes)` in protolens's startup line is `size_suffix(&cli.blob)`
(`main.rs:454`), the size of the file *on disk*. A text fixture reports
the text's size there, not its payload's, and that is not evidence of
the wrong producer having run.

This costs one round trip through the encoder at every open, which for a
fixture of this size is nothing, and buys the property that matters for a
demo: **what is committed is what is read, and it is legible.** There is
no stale-artifact failure mode because there is no artifact.

**How it gets written the first time.** By hand where that is easy, and
by scaffolding where it is not: build the awkward cases as Rust byte
literals in a throwaway, `prototext decode` them, and paste the rendered
lines in. That is a one-time bootstrap, not a pipeline — once the lines
exist, the fixture is edited directly, because `#@` prototext is a
human-writable format and every anomaly in it is one keyword. A token
that turns out *not* to be hand-writable is a finding about the format
worth recording rather than working around.

The SPDX header goes in a `.license` sidecar rather than in the file.
`is_prototext_text` is a `starts_with`, so the very first bytes must be
`#@ prototext:` — a copyright comment above it would break detection in
both tools. Annotate with the repo's standard invocation, which already
passes `--fallback-dot-license`.

### S5 — ordered by audience interest, captioned in-band

**Order.** The fixture is read top to bottom on a screen during a talk,
so it is ordered by what an audience gets the most from, not by the
grouping of S2's table. Roughly:

1. **Non-canonical encodings of ordinary values** — `tag_ohb`,
   `val_ohb`, `len_ohb`, `ohb`. The strongest opening: the bytes are
   *legal*, every parser accepts them, the value is exactly what it
   claims, and yet the encoding is not the one a canonical writer
   produces. This is the part of protobuf almost nobody knows exists.
2. **Values that survive a round trip but not a re-encode** —
   `truncated_neg`, `neg`, `nan_bits`. Same lesson, sharper: two
   producers disagree and both are right.
3. **`ENUM_UNKNOWN`** — the everyday one. A newer producer sent a value
   this schema does not know, and nothing is broken.
4. **Packed runs** — `pack_size`, and `ohb`/`neg` inside one. The place
   where the wire row earns its keep, because one text line is several
   records.
5. **Schema disagreements** — `TYPE_MISMATCH`, `INVALID_STRING`,
   `INVALID_PACKED_RECORDS`. The blob and the descriptor set do not
   agree; protolens says which.
6. **Malformed wire bytes** — the terminal anomalies of S3, each in its
   own submessage: `TRUNCATED_BYTES`/`MISSING`, `INVALID_VARINT`,
   `INVALID_LEN`, `INVALID_FIXED64`, `INVALID_FIXED32`,
   `INVALID_TAG_TYPE`, `TAG_OOR`.
7. **Groups** — `group`, `etag_ohb`, `ETAG_OOR`, `OPEN_GROUP`,
   `END_MISMATCH`, `INVALID_GROUP_END`. Last because proto2 groups are
   history to most of the audience, and because googleapis declares none
   (S7), so all of them arrive as unknown fields.

That covers all twenty-five non-wire-type tokens of S2's table.
`varint`, `fixed64`, `fixed32` and `bytes` are not stages of their own —
they ride along on the lines above, since every field line carries a
wire type. `group` is listed because reaching it takes deliberate work.

The order is a specification of the fixture, not of the test: S6's
coverage assertion is a set comparison and does not care.

**Captions.** The `#` comments of G2 explain each anomaly to a reader
with the file open in an editor — but they are dropped by the encoder
(S1) and so are invisible in protolens, which is where the audience will
actually be looking. The fixture therefore also carries its captions
**in band, as string field values**:

```
description: "Legal, and not canonical: the tag is a 3-byte varint."  #@ string
```

A string field costs one line, renders as itself, needs no schema
gymnastics, and puts the explanation on screen next to the anomaly it
explains. That is criterion 3 of S7. Where a chosen root type has no
convenient string field, an unknown field number carrying a `bytes`
payload renders the same way — but a declared one reads better, since it
gets a name.

Captions are used where they earn their place, not on every line: a
`pack_size` run wants one sentence, not one per element.

### S6 — the coverage test

The fixture is also a regression test, which is the cheapest way to keep
G1 true:

1. `encode` `grpconf/anomalies.pb` to bytes;
2. `decode` those bytes back, with the same descriptor set and root type;
3. `encode` the result again and assert the bytes are **identical** to
   step 1's.

Step 3 is the assertion, not a textual comparison with the source: the
authored file carries `#` comments and hand-chosen spacing that the
renderer does not reproduce, so the text will differ legitimately. The
bytes may not.

On top of that, one test asserts the set of annotation tokens appearing
in step 2's output **equals** S2's table. Equality, not containment, in
both directions — a missing token means the fixture stopped covering
something, and an extra one means the renderer grew a token nobody
classified, which is exactly the gap this spec was scoped against.

It lives in `prototext-core/tests/anomaly_fixture.rs` (Q3). The root
`tests/` tree is the Python suite and would have needed the CLI; the test
needs only `render_as_bytes` / `render_as_text` / `parse_schema`, all
public on `prototext-core`, and the descriptor set it resolves against is
that crate's own `fixtures/descriptor.pb`.

### S7 — the root type

**`google.protobuf.FileDescriptorProto`.** Chosen against these criteria, in
priority order:

1. resolvable in `googleapis.desc` and recognizable to a gRPC audience;
2. carries a **`repeated <Message>`** field, so S3's containment is
   available once per terminal anomaly;
3. carries a **string** field, for S5's in-band captions;
4. carries an enum field (for `ENUM_UNKNOWN`) and a repeated numeric one
   (for `pack_size` / `ohb` / `neg`).

Criteria 2 and 3 are the ones that decide the choice, because they are
what the single-fixture layout depends on; 4 is a convenience.
`FileDescriptorProto` satisfies all four: `repeated DescriptorProto
message_type = 4` is the terminal-anomaly container, `name` gives every
submessage a caption, `FieldDescriptorProto.label` is the enum, and
`SourceCodeInfo.Location.path` is a genuine `repeated int32
[packed = true]`.

It also resolves against **`descriptor.proto` alone**, which is what makes
the coverage test hermetic: `prototext-core/fixtures/descriptor.pb` (18 KB,
already in the repo) is enough, and `prototext` needs no descriptor set at
all thanks to its built-in `google.protobuf.*` fallback. The spec was
written expecting `googleapis.desc`; any set carrying `descriptor.proto`
works, and the in-repo one is preferred for exactly the reason G3 gives.

Two tokens turned out to need a specific vehicle, found by reading the
renderer rather than by trying:

- **`nan_bits` requires a schema-typed float or double field.**
  `sink.rs:363` and `helpers/scalar.rs:82` emit it only inside the
  known-field branch, so an *unknown* fixed64 carrying NaN payload bits
  does not produce it (confirmed by running one). `descriptor.proto`
  declares exactly one double: `UninterpretedOption.double_value`,
  reached as `options.uninterpreted_option`. Without it the token would
  have been unreachable under this root type and S6's equality assertion
  could not have passed.
- **`bytes` requires a LEN payload that is not valid UTF-8.** An
  undeclared LEN field whose payload happens to be text renders as
  `string` (`sink.rs:494` is the else branch of that test).

Criterion 4 does not have to be satisfiable by one type in general: an
*unknown* field renders by wire type with full annotations, so a packed
run or a group can simply be added at a field number the type does not
declare. The fixture does that anyway, for `varint`, `fixed64`,
`fixed32`, `bytes` and every group token — a real blob from a newer
producer looks exactly like this.

Groups in particular cannot be schema-declared here: googleapis is
proto3 and has none. `group`, `etag_ohb`, `ETAG_OOR`, `OPEN_GROUP`,
`END_MISMATCH` and `INVALID_GROUP_END` therefore all appear as unknown
fields.

## Alternatives considered

**Keep the fixture as Rust byte literals.** Precise and needs no encoder
round-trip, but it fails G2 outright: the artifact a reader opens would
be a `[u8]` array, and the explanation of each anomaly would live in a
comment next to bytes rather than next to the rendered line it produces.
The `#@` format exists to make wire bytes legible; a fixture
demonstrating it should be written in it. Byte literals are still the
right tool for *bootstrapping* an awkward case — see S4 — but that is a
scaffold that comes down.

**One fixture per theme — non-canonical, invalid, and one more per
terminal anomaly.** The first shape this spec took. Rejected: S3's
nesting contains every terminal anomaly given a `repeated <Message>`
field, so the fixture count was never forced above one, and several
files means several things to keep in sync, several `.license`
sidecars, and a demo that has to explain which file it is looking at.
The ordering of S5 does the work the split was meant to do — the
non-canonical material comes first because it is the more interesting
half, not because it lives in another file.

**Reuse `prototext-core`'s existing anomaly tests as the demo.** They are
byte literals scattered across modules, each exercising one token in
isolation, with no root type and no descriptor set. They are a useful
*source* for S4's bootstrap; assembling them into one document with a
root type is the work this spec describes, not an alternative to it.

**Commit a real binary blob beside the text, built by a `build`
script.** The conventional shape, and the one S4 replaced once the
content-sniffing was verified. It costs a second artifact that can go
stale, a `.license` sidecar for a file nobody can read, and a build step
between editing the fixture and seeing the change. The only thing it
buys is skipping an encode that takes microseconds on a fixture this
size.

**Caption the anomalies with `#` comments alone.** They are the right
thing for a reader with the file open (G2) and useless for the audience,
because the encoder drops them (S1) and protolens never sees them. Hence
S5's string-valued captions *in addition*, not instead.

## Test plan

1. `the_fixture_round_trips_byte_exact` — S6 steps 1-3. Establishes that
   every anomaly the fixture claims is actually representable in both
   directions.
2. `the_fixture_covers_the_whole_vocabulary` — set equality against S2's
   table. Fails on both a fixture that lost coverage and a renderer token
   nobody classified.
3. `a_plain_comment_does_not_reach_the_wire` — the S1 round trip, kept as
   a test because the whole readability premise of G2 rests on it.
4. Manual, and the real acceptance criterion: open each blob in
   protolens with `googleapis.desc`, press `w`, and confirm all three
   tiers are visible on one screen and that the text row and the wire row
   agree on every one of them.

## Open questions

Answered while drafting, and folded into the specification above: there
is no build step (S4), there is one fixture (S4, S5), and the audience's
explanations are string field values rather than a second canonical blob
(S5).

## Investigation

Everything below was run, not reasoned. Commands are given where the
result is reproducible in one line.

### I1 — a `#@` inside a *string value* is safe, by an invariant nobody wrote down

The suspicion was that `prototext decode` could emit a line the encoder
then mis-splits, because the payload of a `bytes` or `string` field can
contain the annotation marker verbatim.

It cannot, and the reason is structural: `annotation_bounds`
(`encode_text/mod.rs:56`) scans **right to left** with `memrchr`, and the
annotation is always the last thing on a field line. Any `  #@ ` inside
a value is therefore to the *left* of the real one and is never reached.

Round-tripped byte-exact:

```
1: "evil  #@ varint; TAG_OOR"  #@ string
2: 7  #@ varint
```

The invariant this rests on — *every field line the renderer emits ends
with its annotation* — was checked against the corpus rather than
assumed. Decoding the 375 googleapis instances at
`/nix/store/qds4bx8dbr64hx474jsk8bvr0dgp05zl-googleapis-db/instances`
and scanning all 2088 non-empty body lines found **no** line lacking
`  #@ ` other than close-brace lines and the tool's own `# Type:` /
`# Score:` header comments.

Two things would break it, and neither is live:

- A renderer that put anything after the annotation. Nothing does; the
  grammar in `docs/prototext/annotation-format.md` puts `NEWLINE`
  directly after it.
- `--no-annotations`. Its output has no `#@ prototext:` header, so
  `prototext encode` refuses it outright with a diagnostic naming the
  cause. The dangerous document cannot be constructed by accident.

The tree-sitter side agrees, and this too was checked rather than
inferred from the precedence numbers: parsing the line above yields one
`double_string_contents` spanning the whole quoted payload, with no
`annotation` node inside it. Spec 0201 raised string contents to
`prec(2)` and spec 0225's `annotation_marker` sits at `prec(1)`, so the
marker loses inside a string body.

**Conclusion: not a bug.** It is an undocumented and untested invariant,
which is a different and smaller problem — addressed by T5 below.

### I2 — a `#@` inside a *whole-line comment* is safe

`encode_text/mod.rs:265` drops any line whose trimmed content starts
with `#` and not with `#@`, and it does so *before* the value is split,
so nothing in the comment's text is ever looked at. Indentation is
irrelevant (the test is on the trimmed line), and a comment cannot be
mistaken for a close-brace line either, since that test
(`mod.rs:221`) requires every byte to be `}` or space.

Also verified, because the fixture depends on it: comments and blank
lines interleaved between the element lines of a packed run do not
disturb `packed_remaining`. This

```
path: 1  #@ repeated int32 [packed=true] = 1; pack_size: 3
# A comment in the middle of a packed run.
path: 2  #@ repeated int32 [packed=true] = 1

  # an indented one, after a blank line
path: 3  #@ repeated int32 [packed=true] = 1
```

encodes to `0a 03 01 02 03` and decodes back to the three element lines.

**Conclusion: not a bug.** The tampering scenario the question imagined
requires the tamperer to write `#@`, which is not a comment.

### I3 — after `#@`, everything is annotation (a feature); before it, a `#` deletes the line (the defect)

Two different behaviors were found here and they have opposite verdicts.

**Settled as a feature: the annotation runs to end of line, `#`
included.** `parse_annotation` (`encode_annotation.rs:63`) splits the
whole annotation string on `;` and interprets each token; nothing
terminates it at a `#`, and nothing should. That is exactly what
`annotation-format.md` already says ("The annotation runs to end of
line"), so text that *looks* like a comment after `#@` is annotation:

| Line | Bytes |
|---|---|
| `seconds: 5  #@ varint; int64 = 1` | `08 05` |
| `seconds: 5  #@ varint; int64 = 1  # note; val_ohb: 4` | `08 85 80 80 80 00` |
| `seconds: 5  #@ varint; int64 = 1  # careful; tag_ohb: 2` | `88 80 00 05` |

The value is unchanged in all three; the *encoding* of it is not. This
is not a defect: the annotation already has a terminator, the newline,
and `annotation-format.md` already says so. It stays specified (P4) and
the fixture does not write there.

It is, however, cheaply *bounded*. The reason to make `#` special inside
an annotation was assumed to be a scan of every annotation on the hot
path; it is not, because that scan has already happened by the time the
marker is found. P5 gives the annotation an end for free.

Recorded because it bounds what the rule has to defend against:

- An effect requires a `;` inside the trailing text **followed by a
  recognized modifier name**. The full trigger set is `tag_ohb`,
  `val_ohb`, `len_ohb`, `etag_ohb`, `ohb`, `pack_size`, `nan_bits`,
  `MISSING`, `END_MISMATCH`, `packed_ohb`, `packed_truncated_neg`, and
  the bare flags `OPEN_GROUP`, `truncated_neg`, `neg`.
- Prose without a `;` is inert. It is glued onto the preceding token
  instead, and the field-declaration parser is tolerant of the result:
  `int64 = 1  # see rfc x=9`, `int64 = 1  # TODO: check` and
  `int64 = 1  # a, b` all still encode to `08 05`. The `x=9` case is
  the one worth noting — a second `=` in the comment does *not*
  capture the field number.

**The defect is the other case: a `#` on a line with no annotation.**
`seconds: 5  # note` has no `  #@ `, so `split_at_annotation` returns
the whole line as the value part, `5  # note` fails to parse, and the
line is dropped. A file whose only field line is that one encodes to an
empty blob and exits 0 — no output, no diagnostic, no exit code.

Two things make this worth fixing rather than documenting. It is
**silent**, where the annotated case at least produces bytes the author
can inspect. And it is **asymmetric with the annotated case**: the same
comment is tolerated when an annotation precedes it and fatal when one
does not, which is the opposite of what anyone would guess. P3 fixes it,
at no cost to any document the renderer produced.

### I4 — `INVALID_PACKED_RECORDS` and `INVALID_STRING` are reachable and trivial to produce

Both confirmed against real types, so S2's table is not aspirational:

```
# INVALID_STRING — google.rpc.Status, field 2 (string) holding \377\376
2: "\377\376"  #@ INVALID_STRING

# INVALID_PACKED_RECORDS — google.protobuf.SourceCodeInfo.Location,
# field 1 (repeated int32 [packed=true]) with an undecodable payload
1: "\001\002\200"  #@ INVALID_PACKED_RECORDS
```

`SourceCodeInfo.Location.path` is the convenient vehicle: it is a
genuine packed varint field in the built-in well-known types, so the
case needs no descriptor set at all. Four different malformed payloads
were tried and all four produced the token — a trailing continuation
byte, an over-long varint, an eleven-byte varint and the example from
`annotation-format.md`. Note that a payload of twelve `\200` bytes does
*not*: it decodes as a single zero-valued element with overhang, i.e.
non-canonical rather than invalid.

Neither token appears in `protolens/src/annotation.rs`'s `INVALID` nor
in `highlights.scm`'s `#any-of?` list, so both currently render in plain
comment gray on the text row and get no tier on the `w` row. That is the
first gap the fixture exists to catch.

### I5 — `packed_ohb` and `packed_truncated_neg` are v1 format residue

Not merely unemitted — **the wrong shape**. Both are *list*-valued:
`encode_annotation.rs:82,87` parse `packed_ohb: [1, 2, 3]` and
`packed_truncated_neg: [0, 1, 0]`, and `docs/prototext/design.md:175`
documents the first as "per-element varint overhangs". That is the older
rendering, where one line carried a whole packed record. The v2 format
emits one line per element and spells the same facts `ohb: N` and `neg`
on the element's own line, both of which are separately present in the
vocabulary.

So the encoder keeps them only to read old documents, and no renderer
will ever produce one again. Their presence in `annotation.rs` and
`highlights.scm` is harmless in itself — the grammar would color them
correctly if they appeared, since the bracketed list lexes as
`annotation_attribute` — but they are two entries of a 30-entry
vocabulary that no fixture can ever cover, which is what S6's *equality*
assertion will refuse to accept.

They are also not two isolated entries. Both are read only by
`encode_packed_array_line` (`fields.rs:428`), which is reached from
exactly one place: `fields.rs:227`, `if value_str.starts_with('[')` —
the v1 *value* form `field: [v1, v2, …]`. So the two modifiers, the
`Ann` fields they fill (`records_overhung_count`,
`records_neg_int32_truncated`), the sibling `enum_packed_values` used by
the same function for the v1 `Color([1, 2])` declaration form, the
function itself and its call site all stand or fall together. P2
removes the set.

Two documentation drifts found alongside, both in
`docs/prototext/design.md`'s modifier table: it names the END_GROUP
overhang `end_tag_ohb` where the code and `annotation-format.md` both
say `etag_ohb`, and it omits `ohb`, `neg`, `truncated_neg` and every
`INVALID_*` wire type. `annotation-format.md` is the accurate reference;
`design.md`'s table should point at it rather than restate it.

### I6 — what this changes in the spec above

S1 is amended: no trailing comment, in either direction. S2's table is
unchanged and stays at 30 tokens — I4 confirms both of its last two
entries are real, and P2 removes nothing from it, since `packed_ohb` and
`packed_truncated_neg` were never in it. P1 and P2 together bring
`annotation.rs` into agreement with it in both directions: `INVALID`
13 → 15, `NON_CANONICAL` 11 → 9. S6's assertion compares the fixture's
decoded output against the table and so needs neither; it is P1's
exhaustiveness test that does. The test plan gains items 5-9.

## Proposals

Each is independently landable. P1 and P4 are documentation-sized; P2 is
a deletion; P5 is three lines and costs nothing. P3 and P6 touch the
encoder's line loop, and both are designed around not being felt there —
P3 by adding work only to an arm the renderer's output never enters, P6
by adding it only to arms that already exist and now end the encode.

### P1 — make prototext-core the single source of the vocabulary

The minimal fix for I4 is two list entries: add `INVALID_PACKED_RECORDS`
and `INVALID_STRING` to `protolens/src/annotation.rs`'s `INVALID` and to
`highlights.scm`'s invalid `#any-of?`. Both are invalid-tier without
argument — each one says the payload cannot be what the schema declares.

But that is the *same* fix the next new token will need, and nothing
would prompt it. The list drifted precisely because it is a hand-kept
copy of something the renderer decides. So:

- prototext-core exports the keyword names it emits, as `pub const`
  slices next to the code that writes them. Names only.
- `protolens/src/annotation.rs` keeps the tiers — severity is a
  presentation decision and does not belong in the codec — but gains a
  test asserting that `tier_of` returns `Some` for every exported name.
  A token added to the renderer then fails protolens's test suite
  instead of rendering gray.
- `every_keyword_is_colored_by_its_tier` iterates the exported list
  rather than `vocabulary()`, which extends the same guarantee to
  `highlights.scm`.

`highlights.scm` cannot be compile-checked, and that is accepted: the
drift test already covers it, and a query edit needs no
`tree-sitter generate`, which is why the vocabulary lives there in the
first place (spec 0225).

### P2 — delete the v1 packed-array path

Per I5, one connected set:

- `packed_ohb` and `packed_truncated_neg` from `parse_annotation`
  (`encode_annotation.rs:82,87`);
- the `Ann` fields `records_overhung_count`,
  `records_neg_int32_truncated` and `enum_packed_values`;
- `encode_packed_array_line` (`fields.rs:428`) and its guard
  `if value_str.starts_with('[')` (`fields.rs:227`);
- the two names from `annotation.rs`'s `NON_CANONICAL` (11 → 9) and
  from `highlights.scm`.

Nothing in v2 reaches any of it: v2 renders one line per packed element
with `pack_size`/`ohb`/`neg`, and its packed enum declaration is the
singular `Color(0)`, not `Color([0, 1])`. The `starts_with('[')` guard
cannot fire on renderer output — an extension *key* is bracketed, but
that is the left-hand side, never the value.

The cost is that a `.textproto` produced by a pre-v2 `prototext` stops
encoding. The header carries no version (`#@ prototext: protoc`), so
this cannot be detected and reported; such a file would fail the way a
malformed one does. Accepted: the repo has no v1 documents, the format
is not published as stable, and `prototext decode` regenerates any of
them from the binary in one command.

Verification is the existing suite: `prototext/fixtures/index.toml`'s
`test_records_overhung_count` and the rest of protocraft go through
binary, so they exercise the v2 text form and must stay green.

### P3 — end the value at an unquoted `#`, but only where no annotation exists

The fix for I3's defect, designed around the performance question.

**Where.** In the encoder's line loop, in the arm where
`split_at_annotation` returned an empty annotation. Not inside
`annotation_bounds`, and not in `annotation_start` — protolens uses the
latter to *hide* annotations, and a plain comment is not one.

**What.** Scan the value part left to right, tracking quote state over
`"` and `'` with backslash escaping, and end the value at the first `#`
outside a quote. Quote-awareness is not optional: `1: "a # b"` with no
annotation parses correctly *today* and must keep doing so, so a rule
that cut at the first `#`, or at the first `  #`, would trade a silent
drop for a silent corruption.

**Why it costs nothing.** Every field line the renderer emits carries an
annotation — that is I1's invariant, checked over 2088 corpus lines — so
`split_at_annotation` returns `Some` and this arm is never entered. For
machine-generated input the change is one already-existing match arm
that stays untaken. There is no added work on the hot path to measure,
because there is no added work on the hot path.

The scan itself is O(line) and runs only on hand-written annotation-free
lines, which are by construction rare: a document made entirely of them
would not encode to anything today.

**How to confirm it.** The structural argument above is the real one,
but it should be checked rather than asserted, because "the branch is
never taken" is a claim about the corpus and not about the code:

1. Encode all 375 googleapis instances before and after, and diff the
   outputs — they must be byte-identical, which also proves the arm is
   never entered.
2. `bin/bench -p prototext-core --bench codec`, baseline against
   baseline first to establish the noise floor for that target, then
   before against after. Expect the delta inside the floor.

**Alternatives.** Doing the strip unconditionally in
`split_at_annotation` is the obvious shape and is rejected: it puts a
quote-aware scan on every line of every document, changes
`annotation_start`'s meaning for protolens, and defends a form that
appears in no generated file. Doing it in a pre-pass over the whole text
is worse still — an extra traversal of the document to fix a line the
document does not contain.

**Complement, not substitute.** P3 removes one cause of the silence; P6
removes the silence. They are independent — P3 makes the line encode
correctly, P6 makes any line that still cannot be encoded say so — and
either is worth landing without the other.

### P4 — one home for the modifier reference

`docs/prototext/design.md`'s modifier table is a stale second copy: it
names the END_GROUP overhang `end_tag_ohb` where the code and
`annotation-format.md` both say `etag_ohb`, and it omits `ohb`, `neg`,
`truncated_neg` and every `INVALID_*` wire type. Replace it with a
pointer to `annotation-format.md`, which is accurate and is where the
next reader looks anyway.

While there: `annotation-format.md` gains a sentence recording I3's
settled behavior as P5 leaves it — the annotation runs to the next `#`
or to end of line, whichever comes first, and an explanatory comment
belongs on its own line regardless.

### P5 — end the annotation at the next `#`, for free

Defense in depth for I3's first case, at no cost, because the work is
already done.

**The observation.** `annotation_bounds` (`encode_text/mod.rs:56`) walks
`#` positions **right to left** with `memrchr`, rejecting each one that
is not a real marker:

```rust
let mut end = b.len();
while let Some(p) = memrchr(b'#', &b[..end]) {
    if /* p is a real `  #@ ` marker */ { return Some((p - 2, p + 3)); }
    if /* p is a leading `#@ ` marker */ { return Some((0, p + 3)); }
    end = p; // keep searching leftward
}
```

At the moment of a successful return, `end` holds exactly the position
of the nearest rejected `#` to the **right** of the marker, or `b.len()`
if there was none. That is precisely where the annotation should stop.
The value is live in a register and is currently discarded.

**The change.** Return it. `annotation_bounds` becomes a triple, and
`split_at_annotation` slices `&line[ann_start..ann_end]` instead of
`&line[ann_start..]`. `annotation_start` is unaffected — it reads only
the first element, and protolens uses it to hide annotations, which is a
different question.

**Cost: none, and not "negligible".** No byte is examined that was not
examined already, and no branch is added to the loop. The one added
instruction is the slice's second bound.

**Correctness: a `#` in a string value cannot cause a false
truncation**, because it is to the *left* of the marker and the
right-to-left scan never reaches it (I1). A `#` to the right of a marker
appears in no document any renderer produces — annotations are
identifiers, numbers, `=`, `;` and bracketed attributes, and
`annotation-format.md` lists no token that can contain `#`. So the only
documents whose bytes change are the hand-edited ones this proposal
exists to protect.

The behavior it buys, from I3's table: `#@ varint; int64 = 1  # note;
val_ohb: 4` goes back to encoding `08 05`. Together with P3 that makes a
trailing `#` mean the same thing on every line, annotated or not, which
is the asymmetry I3 complained about.

**Confirm it** the same way as P3: re-encode all 375 googleapis instance
renderings and require the bytes to be identical. That is the assertion
that no real annotation contains a `#`.

### P6 — refuse a line the encoder cannot parse, naming where

I3's defect is silent; P3 removes one cause of the silence but not the
silence itself. Any other malformed line — a missing colon, an
unparseable value, an unknown wire-type keyword — is still `continue`d,
and a file of nothing but such lines encodes to an empty blob and exits
0.

**What.** `prototext encode` fails with a message naming the line number
and what could not be parsed, instead of dropping the line.

**Cost, examined because it is the condition on doing this at all:**

- **Detection is free.** The failures are already detected — they are
  the `else { continue }` arms of existing `let ... else` bindings. The
  change is what happens in an arm that is already reached, on a path
  that by construction runs at most once (it now ends the encode).
- **The line number costs one add per line.** The loop becomes
  `.enumerate()`. Nothing else in the loop grows.
- **The real cost is the signature.** `encode_text_to_binary_into`
  returns `()`, and its own doc comment records that it "has nowhere to
  report a violation" — which is why `protolens/src/blob.rs:104` has to
  pre-validate UTF-8 itself to avoid turning a bad byte into an empty
  document. Giving it a `Result` ripples to `Blob::load`, to the
  `prototext` CLI, and to the pyo3 bindings. That is a mechanical change
  but not a small one, and it is the whole substance of this proposal.

**Confirm it the same way as P3:** encode all 375 googleapis instance
renderings under the strict encoder and expect **zero** errors. A single
error there means either a renderer that emits something the encoder
cannot read — a real bug, and worth finding — or a tolerance that is
load-bearing, in which case it gets named and kept deliberately. Either
outcome is better than the current silence.

Landing it also lets `Blob::load` drop its pre-validation and report the
encoder's own diagnostic, which is strictly better than "not valid
UTF-8 at byte N".

## Test plan (continued)

5. `a_hash_inside_a_value_is_not_an_annotation` — encode a document whose
   string payload contains `  #@ varint; TAG_OOR` and assert the bytes
   round-trip exactly. Pins I1's invariant, which is load-bearing for P3
   and currently untested.
6. `an_annotation_runs_to_end_of_line` — the I3 table, asserted as the
   documented behavior it now is.
7. `a_comment_after_an_unannotated_value_is_a_comment` — P3's fix, with
   `1: "a # b"` alongside it so the quote-aware half cannot regress.
8. `an_annotation_ends_at_the_next_hash` — P5. The I3 table again, with
   the expectations it now has: all three rows encode `08 05`. Paired
   with T5, which is the case that must *not* truncate.
9. `a_malformed_line_is_an_error_naming_its_number` — P6, plus the
   corpus assertion that all 375 instance renderings encode clean.

## Open questions (continued)

- **Q6.** Does P1's export belong in `prototext-core` proper or behind a
  feature, given that the names exist only so a *consumer* can classify
  them? The crate already emits them, so no new data is introduced —
  only a public name for it.
- **Q7.** P6 changes `encode_text_to_binary_into`'s signature. Does the
  pyo3 binding raise, or return a status? That is a compatibility
  decision for the Python side and is not settled here.

Answered while drafting: a stricter encoder is wanted, provided it does
not cost anything on the hot path — promoted to P6, where the cost is
examined and found to be in the signature rather than in the loop.

## Measured outcome

Implemented 2026-08-02: goals G1-G4 and specification S1-S7 only. P1-P6
are Non-goal N1 and remain open, as does test-plan item 4 (the manual
`w`-row acceptance pass) and items 5-9, which belong to the proposals
they pin.

Shipped:

- `grpconf/anomalies.pb` (10 092 bytes of `#@` prototext text, encoding
  to **1 619 wire bytes**) + `anomalies.pb.license` + `README.md`.
- `prototext-core/tests/anomaly_fixture.rs` — the three tests of the
  test plan's items 1-3, all green.
- `default.nix`'s `workspaceSrc` gains `(fixtureFilter ./grpconf)`; the
  test `include_str!`s the fixture and cannot compile without it.

Coverage is **exact**: the set of annotation keywords in the fixture's
own rendering equals S2's thirty-token table, in both directions. The
round trip is byte-exact, so every anomaly in it is representable both
ways.

Two of the three findings the fixture was scoped to catch are now
mechanically pinned rather than argued: an extra token in the renderer
fails `the_fixture_covers_the_whole_vocabulary`, and so would the
removal of `packed_ohb` / `packed_truncated_neg` have failed an
*equality* assertion had they ever been in the table (I5 — they were
not, which is why P2 changes nothing here). The gray-rendering gap of
I4 is untouched: `INVALID_PACKED_RECORDS` and `INVALID_STRING` are in
the fixture and still unclassified in `protolens/src/annotation.rs`,
which is P1's job.
