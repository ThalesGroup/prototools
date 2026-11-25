<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0481 — fdp-scan reads Java class files

Status: implemented
Implemented in: 2026-11-25
App: fdp-scan, protoscan, git-seriate-fdp
Refs: docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md (the
        container walker this extends, its origins and limits; N5, which
        this corrects)

## Background

`scan_inflating` (spec 0480) reads a jar's class files, and scans their
bytes as it scans an executable's. Java's generated code keeps each
descriptor as string constants (`descriptorData`, an array concatenated
at run time). A class file stores each string as modified UTF-8: a char
below U+0080 as one byte, itself; one from U+0080 to U+07FF, which every
byte ≥ 0x80 of the descriptor becomes, as two.

So the raw bytes hold a descriptor's bytes as they are up to its first
byte ≥ 0x80, and something else after. The scanner keeps the clean
prefix of a record (spec 0313 S3), which was meant for a record followed
by unrelated bytes. Here it reports the first part of a descriptor as a
whole file. Nothing says it is cut.

Measured on `bundletool-all-1.17.2.jar` (32.5 MB, 13 715 class files),
2026-11-25, `protoscan --inflate`:

| descriptors found | count | state |
|---|---|---|
| in class files, raw bytes scanned | 36 | 30 truncated, 6 whole |
| in class files, string constants decoded first | 36 | all whole |
| in the native executables the jar carries (`aapt2`) | 20 | whole |

Examples of truncation: `Configuration.proto` 30 of 2 710 bytes,
`commands.proto` 63 of 4 558, `Resources.proto` 258 of 5 751. Decoding
every class file finds no descriptor beyond the 36. A truncated
descriptor in a snapshot reads as a file that lost most of its contents:
false history.

## Goals

- **G1. Whole descriptors from class files:** a class file's string
  constants are decoded to the bytes Java's generated code hands
  protobuf, and those are scanned.
- **G2. No descriptor from a class file's raw bytes.** Its raw bytes are
  not scanned at all once it reads as a class file.
- **G3. The origin says so:** a descriptor from decoded strings has a
  `class` layer.
- **G4. Failure is safe.** When the strings don't reassemble a descriptor
  whole, it is reported skipped, never an item: a missed descriptor,
  visible in the record, rather than a false history.

## Non-goals

- **N1. No change without inflating.** `scan` and `scan_explained`, and
  protoscan and extract without `--inflate`, still scan a class file's
  raw bytes. A bare class file is an unusual input; the walker is where
  containers are read (0480 G3).
- **N2. No class file carved out of a larger buffer.** A class file is
  recognized only as a whole buffer: the input, or a zip member. Its
  length is not recorded inside it, so its end can't be found in the
  middle of a binary.
- **N3. No Android dex files, no C# base64.** Dex files hold strings in
  modified UTF-8 too, but sorted, so a descriptor's parts are not
  adjacent and reassembling them would mean reading the code. Android
  code is mostly built with protobuf-lite, which embeds no descriptor.
  C#'s generated code holds a base64 literal. Each would be a decoder of
  its own; none is measured.
- **N4. No Java semantics.** No bytecode is read: which constants form
  one descriptor is not checked against the code that concatenates them
  (S2 says why that isn't needed).

## Specification

- **S1. Recognized:** a buffer the walker scans (the input, or a member)
  that starts with `ca fe ba be` and whose major version (bytes 6–7) is
  45 or more, and whose constant pool parses to its end. The version
  check tells a class file from a Mach-O universal binary, which starts
  with the same magic followed by a count of architectures, a small
  number.
  - **A class file that doesn't parse is skipped, not scanned raw**
    (`SkipReason::ClassFile`): the raw bytes are what truncates. So is a
    zip member named `*.class` without the magic and version.
  - A buffer with the magic but a major version below 45, and not named
    `*.class`, is not a class file: scanned raw, as before.
- **S2. Decoded:** every `CONSTANT_String`, in pool order, gives its
  `CONSTANT_Utf8` decoded from modified UTF-8, each char one byte, as
  protobuf-java converts `descriptorData` (`ISO_8859_1`). Strings
  holding a char above U+00FF can't be descriptor data and are left out.
  The rest are concatenated, in that order, into one buffer.
  - **Why pool order works for javac:** protoc writes each element of
    `descriptorData` as a `+` of literals, a compile-time constant that
    javac folds into one `CONSTANT_String` (JDK 9's indified
    concatenation applies to non-constant operands only). javac adds a
    string to the pool where the code first loads it, and `<clinit>`
    loads the elements in order, so they are adjacent and in order.
    Strings from methods emitted before `<clinit>` come before them, not
    between.
  - **Where it breaks:** javac pools one entry per distinct string. A
    part equal to a string loaded earlier in the class is not repeated
    where the descriptor needs it, which is unlikely for kilobyte parts
    but possible for a short last one. Shrinkers (R8, ProGuard) rewrite
    the pool in an order nothing promises. S3's check catches both.
- **S3. Scanned, and checked whole:** the decoded buffer is scanned as
  any buffer is, with the same schema and options, and visited as a
  `Visit::Scanned` whose origin ends with a `class` layer. The class
  file's own raw bytes are visited with nothing found (G2). No member is
  looked for in the decoded buffer: strings hold no archives.
  - **A descriptor counts only if it starts and ends where string
    constants start and end.** `descriptorData` is a descriptor cut into
    strings, so a whole one fills a run of them exactly; one reassembled
    wrong (a part missing, another in its place) ends inside a string.
    Any other is taken out of `found` and reported as a
    `Visit::Skipped`, reason `SkipReason::Cut`, origin its range in the
    decoded buffer: in extract, `skipped <origin>` = `cut <name>`.
  - **Why not "the record ended at a clean prefix":** a whole descriptor
    is followed by unrelated strings too, and ends the same way. In the
    measured jar, 35 of the 36 whole descriptors are followed by more
    strings; all 36 start and end on string boundaries.
  - **The run of strings must have protoc's shape.** protoc cuts
    `descriptorData` into parts of a fixed size P: every part but the last
    is exactly P bytes, the last 1 to P. P depends on protoc's version
    (`java/shared_code_generator.cc`, formerly
    `java_shared_code_generator.cc`, read at each tag on 2026-11-25):

    | protoc | P | the code |
    |---|---|---|
    | 2.5.0 to 3.4.x | 400 | `i % 400 == 0`, `i` a byte offset, though its comment says "every 400 lines" |
    | 3.5.0 to `main` | 16 000 | `i % kBytesPerPart`, `kBytesPerLine 40 × kLinesPerPart 400` |

    A descriptor counts only if, for P = 400 or P = 16 000, every part of
    its run but the last is exactly P bytes and the last is shorter. A
    run whose parts are all exactly P is skipped as `cut`: its next part
    may be missing, which is the case the boundaries can't see (a part
    deduplicated by javac, S2). A run of other shapes wasn't cut by
    protoc, so it wasn't reassembled from its parts: also `cut`.
  - **What that costs:** a whole descriptor whose last part happens to
    be full is skipped, recorded, not lost: 1 in 400 for code from
    protoc before 3.5, 1 in 16 000 after, and a descriptor from protoc
    3.5 or later exactly 400 bytes long (one part, which could be an older
    protoc's first). Code from another generator, or a protoc with
    another P, is skipped as `cut` too, visibly.
  - **What it still misses:** nothing known short of a shrinker that
    splits or merges strings into protoc's shape by chance.
- **S4. Origin:** `MemberKind::Class`, written `class`, with no offset:
  `zip@10541!com/android/bundle/AppDependenciesOuterClass.class!class!0..1159`.
  The grammar of 0480 S6 gains a rule for such layers in general: **a
  layer without `@N` decodes its whole enclosing buffer, and the range
  after it is into the decoded bytes.** `layer = … | transform`,
  `transform = "class"`. A later decoder (dex, base64) adds a name, not a
  grammar change.
- **S5. Limits:** the decoded buffer is at most the class file's size;
  it counts toward neither inflation limit, and needs no depth of its
  own.
- **S6. Spec 0480's N5** is corrected: Java is read (this spec); C#
  and Python sources still aren't.

## Alternatives considered

- **Drop descriptors found in class members.** Safe, but loses all 36
  descriptors of the measured jar, 6 of which were found whole.
- **Mark them suspect instead of dropping them.** The scanner can't tell
  a whole descriptor from a cut one in the raw bytes: both are clean
  records. Every descriptor from a class file would be suspect, for the
  same loss.
- **Skip what the scanner kept only as a clean prefix.** It would skip
  whole descriptors with them (S3: 35 of 36 in the measured jar).
- **Reassemble only the strings the bytecode concatenates.** Exact, but
  needs a bytecode reader, for no case S2's rule is known to get wrong.

## Test plan

1. `class_file_strings_decoded`: a class file built in the test, a
   descriptor with bytes ≥ 0x80 split across three string constants: found
   whole, origin `class!0..len`, nothing found in its raw bytes.
2. `class_file_in_a_jar`: the same class file as a deflated zip member:
   origin `zip@0!x/Y.class!class!0..len`.
3. `not_a_class_file`: a Mach-O universal header (`ca fe ba be 00 00 00
   02`) is scanned raw; a class file whose pool is cut short, and a
   member `x.class` without the magic, are skipped as `class-file`.
4. `cut_descriptor_skipped`: the descriptor's middle part replaced by
   another string: no item, a `Skipped` visit, reason `cut`.
5. `part_shapes`: a descriptor in 400-byte parts and one in 16 000-byte
   parts are found; the same with their last part dropped, so every part
   full, are skipped as `cut`; parts of 300 bytes are skipped as `cut`.
6. `inflate_off_is_unchanged`: still holds (0480 test 1).
7. Measurement: `bundletool-all-1.17.2.jar`: 36 descriptors from class
   files, all as long as the check script's decoding gives them, and the
   20 from `aapt2` unchanged.

## Measured outcome

2026-11-25, `bundletool-all-1.17.2.jar`, `protoscan --inflate --origin`:
56 descriptors, as before 0481, now all whole. The 36 from class files
come with a `class` layer, each exactly as long as decoding the class
file's strings independently gives it, where 30 were cut before. The 20
from the `aapt2` executables are unchanged. No class file of the 13 715
was skipped, and no descriptor was skipped as cut.

Not measured: a jar run through a shrinker (R8, ProGuard), the case S3's
check exists for. The jar's 36 descriptors each fit in one 16 000-byte
part (protoc 3.5 or later), none exactly 400 or 16 000 bytes long: with
the part-shape check, protoscan's output on the jar is byte-identical to
its output without it. Test 5 covers both part sizes.
