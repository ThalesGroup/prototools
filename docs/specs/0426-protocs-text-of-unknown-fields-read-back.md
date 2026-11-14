<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0426 — protoc's text of unknown fields, read back

Status: implemented
Implemented in: 2026-11-14
App: prototext-upb, reproto, protoback
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (S5: the
      text-format parser and the group correction this specifies the
      rules of)

## Background

A descriptor set in the text format is usually made by
`protoc --decode=google.protobuf.FileDescriptorSet descriptor.proto`:
protoc then knows `descriptor.proto` and nothing else. A custom option is
an extension declared in the input's own files, so protoc does not know
it, and prints it by number, with its value as protoc can make it out
without a schema. Two things are lost on the way.

**String or message.** In the binary format a string, a `bytes` value and
a sub-message are all a field number, a length and bytes; only the schema
tells them apart. protoc prints such a field as a message, `N { … }`,
when its bytes happen to read as one (they are not empty, and they parse
as fields), and as a string, `N: "…"`, otherwise.

**Group or length-delimited.** Inside what it prints as a message, protoc
prints a group and a length-delimited sub-message alike: `N { … }`.

`protoc --encode` refuses fields by number: protoc does not read this text
back. It is a view of the bytes, and reading it back means undoing
protoc's guesses.

**How often.** googleapis (7,771 files), printed so and read back with
every `N { … }` taken as length-delimited and the declarations applied,
gives back 7,753 files byte for byte, and 18 not. In all 18 a string
option value, a resource name of `google.api.resource`
(`shoppingProduct`, `supportedDatabaseFlag`, `UrlList`, …), reads as a
message because an `s` (0x73: field 14, start of group) comes before a
`t` (0x74: field 14, end of group): protoc printed
`6 { 14 { 13: 111 14: 112 13: 0x… } }` for `"shoppingProduct"`. Read as
length-delimited, the group's `s … t` became `r` (field 14,
length-delimited) and a length byte, `\r`: the string came back as
`"r\rhoppingProduc"`, the file the same size, and nothing said so.

## Goals

- **G1.** protoc's text of a descriptor set, read back, gives back the
  binary set wherever the input's declarations settle what protoc's text
  leaves open.
- **G2.** Where they do not, the most likely bytes, and a warning naming
  the field.

## Non-goals

- **N1.** Exact recovery in every case: protoc's text does not hold the
  information (Limitations).
- **N2.** Text printed with the options' schema, `[pkg.ext] { … }` by
  name: nothing is ambiguous there; reading it is the parser's ordinary
  work.

## Specification

- **S1. The parser frames every `N { … }` by number as a group,** and
  `N: "…"` as length-delimited. What a field by number holds otherwise
  (a varint, a fixed32 or fixed64 in hex) protoc prints without loss.
- **S2. The correction**, once every file of the input is read, walks
  each file read from text, with the type of each value in hand, from the
  declarations of the input and of the pool. A group by number is:

  | declared as | becomes |
  |---|---|
  | a message, length-delimited | length-delimited, its content corrected as that message |
  | a message, delimited (a proto2 group, or editions' `message_encoding = DELIMITED`) | kept, its content corrected as that message |
  | a string or bytes | length-delimited, its content kept as it is, groups included (S3) |
  | nowhere | length-delimited, its content treated alike, recursively; warning W1 |

  Everything else is copied byte for byte.
- **S3. Inside a string or bytes value protoc printed as a message**, a
  group by number is kept a group. When it is empty, `N { }`, that is
  exact: protoc prints an empty length-delimited field as `N: ""`. When
  it is not, it is a guess, warning W2.
- **S4. The warnings**, one per value, naming the file, the message type
  and the field number:
  - W1: "field N of T is declared nowhere in the input: read as
    length-delimited";
  - W2: "field N of T is a string protoc printed as a message: rebuilt
    taking its sub-fields for groups". Both end with the remedy: decode
    the text with the files that declare the options.

## Rationale

The groups of S3 are a probability, not a deduction. Bytes that happen to
read as fields more often hold a group than a length-delimited field: a
group needs a byte opening it and a byte closing it, and in text many do
(`s` and `t`, `c` and `d`, `k` and `l`); a length-delimited field needs a
byte equal to the exact length of the bytes after it, which must parse as
fields in turn, and text has few such bytes. All 18 cases of googleapis
are groups.

S2's default for an undeclared field is the other way: there, the value is
of a type nobody declared, and a message-valued option, length-delimited,
is far more common than a group-valued one.

## Alternatives considered

### Every `N { … }` length-delimited, by default

What the parser first did: 18 corrupted strings in googleapis, and no
warning.

### Every `N { … }` a group, and no warning

Right on googleapis, and silent where it guesses wrong: a string holding a
length-delimited field by chance would come back corrupted, unnoticed.

### Trying both framings

Each gives back bytes protoc would print as the same text: nothing to
choose between them by.

## Limitations

- A string or bytes value whose bytes hold, by chance, a non-empty
  length-delimited field that parses as fields comes back with a group in
  its place: wrong, and W2 says so.
- In a value of a type declared nowhere (W1), a group inside comes back
  length-delimited.
- Text printed by another tool than protoc may print unknown fields
  otherwise; these rules are protoc's.

## Test plan

1. googleapis printed by `protoc --decode` with `descriptor.proto` only,
   read back and corrected: every file byte for byte, and one W2 per
   string protoc printed as a message with a non-empty group inside.
2. The fixtures of `prototext-upb`: a message-valued option, an option
   declared delimited in its own file with a delimited field nested in
   its value, a string option printed as a message, empty and not (W2 or
   none), an option declared nowhere (W1).

## Measured outcome

2026-11-14, `prototext-upb` (`text_format`, `delimited`).

- googleapis printed by `protoc --decode` (protoc 34.1, `descriptor.proto`
  only), 56 MB of text, read back and corrected: all 7,771 files byte for
  byte, with 19 W2 and no W1. The first version, every `N { … }`
  length-delimited, corrupted 18 strings.
- On the same text, the parser and Python's `text_format` give the same
  bytes for every field Python knows; Python drops the fields by number.
  The parser takes 1.3 s, Python 75 s.
- The fixtures: the options fixture's text gives back its set, with its 11
  custom options, without a warning; `delimited.proto`'s gives back its
  set, with one W2 (`"shoppingProduct"`) and none for `"UrlList"`, an empty
  group inside; read without the file that declares them, its four options
  give four W1, each length-delimited.
