<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Scanning binaries for FileDescriptorProtos

Status: investigation notes, 2026-08-03.
Scope: why `protoscan` fails on a `FileDescriptorSet`, how the prior
art (protodump, Protod3, protod) solves the same problem, and what a
score-based `protoscan` would look like.

Nothing in this document has been implemented. The one-line fix in
part 1 is designed and validated but not applied.

---

## 1. The reported failure

```
protoscan /nix/store/…-googleapis-db/googleapis.desc
```

was expected to list the 7 771 file names in the descriptor set. It
printed one line, and that line was raw non-UTF-8 bytes beginning
`b'\n\x18grafeas/…'`.

### 1.1 Root cause

`googleapis.desc` is a `FileDescriptorSet`: a bare concatenation of
7 771 records, each

```
0x0A  <varint length>  <FileDescriptorProto bytes>
```

covering the file exactly, with no header and no trailer.

The collision is that `FileDescriptorSet.file` is **field 1, wire type
2** and `FileDescriptorProto.name` is *also* **field 1, wire type 2**.
An outer record header and an inner name field are the same three
bytes on the wire. Only their *lengths* differ: a name is short, a
record is long.

`fdp-scan-pyo3` already knows a candidate ends when a second field 1
appears — that is the correct rule, and the comment at
`fdp-scan-pyo3/src/lib.rs:158` states it correctly:

```rust
// Field 1 (name) is singular in FileDescriptorProto — a second
// occurrence at pos > start unambiguously marks the end of this FDP
// and the beginning of the next one.
if pos > start && group_stack == 0 && looks_like_fdp_start(data, pos) {
    return Some(pos);
}
```

But the test it uses is not "a second field 1". `looks_like_fdp_start`
(`lib.rs:128`) additionally requires that the varint be
`<= MAX_PROTO_NAME_LEN` (200, `lib.rs:61`) *and* that the payload
decode as a plausible `.proto` import path. An outer
`FileDescriptorSet` record length is 291, 295, 352, … — it is neither.

So the boundary test never fires, and the walk falls through to the
generic wire-type-2 arm, which happily skips the entire next file as
if it were a very long `name` string.

### 1.2 Trace on the real file

| offset | bytes | what the scanner does |
|---|---|---|
| 0 | `0A A3 02` (varint 291) | rejected as a name — 291 > 200. `offset += 1` |
| 3 | `0A 18 grafeas/…` | accepted: plausible path. Candidate starts here |
| 294 | `0A A7 02` (varint 295) | `looks_like_fdp_start` → false. Decoded as field 1/LEN, **skip 295 bytes** |
| … | … | repeats to EOF |

One candidate, `(3, 25660332)`. Python's `ParseFromString` then
*succeeds* on the whole 25 MB blob, because field 1 is singular and
protobuf's last-wins rule quietly overwrites `name` 7 770 times. The
final assignment is the last record's raw bytes, which is what got
printed.

### 1.3 The fix

Test for what the comment says — a field 1, wire type 2 tag — and
nothing more:

```rust
if pos > start
    && group_stack == 0
    && decode_field_tag(&data[pos..]).is_some_and(|(f, wt, _)| f == 1 && wt == 2)
{
    return Some(pos);
}
```

`looks_like_fdp_start` then has no callers and can be deleted. The
plausibility check stays where it belongs: in `walk_candidates`
(`lib.rs:109`), which decides where a candidate *starts*.

Validated with a Python replica of `walk_candidates`:

- `googleapis.desc`: **1 → 7 771** candidates, every one parses, 7 771
  distinct names.
- All seven inline `#[cfg(test)]` unit tests still hold, including
  `test_consecutive_fdps_split_correctly` and the two garbage-name
  rejection tests.

### 1.4 Two unrelated defects in the same command

Neither is the cause of the reported failure, but both were found on
the way and both matter:

- **`protoscan` never sorts.** `protoscan/src/protoscan/cli.py` prints
  `fdp.name` inside the scan loop. Output is in discovery order.
  Sorting by name needs either a `--sort` flag or a buffered print.
- **`except Exception: continue`** in the same loop swallows every
  parse failure silently. With the fix in 1.3 nothing fails on this
  corpus, but a partial or corrupt candidate would still vanish
  without a word.

---

## 2. Prior art: how other tools locate FDPs

Three tools, three different answers to *"where does the candidate
end?"* — which is the hard half of the problem.

### 2.1 protodump (Go, arkadiyt)

Cloned to `/tmp/protodump`. The whole scanner is
`pkg/protodump/scan.go`, ~100 lines.

**Anchor.** Search for the literal string `.proto`, then walk
*backwards* to the nearest `0x0A`:

```go
index := bytes.Index(data, []byte(scan))          // scan = ".proto"
start := bytes.LastIndexByte(data[:index], magicByte)  // magicByte = 0xa
```

with one correction: if the filename is exactly 10 characters long
then the `0x0A` found is the *length byte*, not the tag, so step back
one more.

**Stop.** `consumeBytes` calls `protowire.ConsumeField` in a loop and
stops on the first of:

- a parse error whose message contains `"invalid field number"`
  (treated as a clean end — a string match on a Go error message, which
  is brittle);
- any other parse error (treated as a hard failure, candidate dropped);
- a zero-length consume (infinite-loop guard);
- **a second field 1**:

  ```go
  // Only consume Field 1 once (to handle the case where protobuf
  // definitions are adjacent in program memory)
  if number == 1 {
      if consumedFieldOne { return position - start, nil }
      consumedFieldOne = true
  }
  ```

That last rule is exactly the rule our port weakened. protodump tests
the tag and only the tag; we added the `<= 200` plus plausible-path
conditions on top, and that is the bug in part 1.

**Validate.** `cmd/protodump/main.go` unmarshals each candidate and
runs `protodesc.FileOptions{AllowUnresolvable: true}.New(...)`.
Failures are dropped silently; results are only written if the
filename ends in `.proto`.

### 2.2 Protod3 (Python, Sysdream)

Cloned to `/tmp/Protod3`. Same anchor family — find `.proto`, scan
back up to 64 positions for a varint whose value equals the implied
name length, plus an `is_valid_filename` charset check.

Its stop rule is entirely different, and honest about it:

```python
"""
Probable size approach is not perfect,
we add a delta of 1024 bytes to be sure
not to miss something =)
"""
for k in range(probable_size + 1024, 0, -1):
    try:
        fds = FileDescriptorProto()
        fds.ParseFromString(stream[r-j-1 : r-j-1+k])
        protos.append(...)
        break
    except DecodeError:
        pass
```

A structural walker estimates a probable size, 1 024 bytes of slack
are added, and then it *brute-forces descending trial parses*,
accepting the longest length that parses. Up to ~1 024 full protobuf
parses per candidate. Correct-ish and very slow.

### 2.3 Our current scanner

Same family as protodump — consume-while-valid, stop on a second
field 1 — with the boundary test too strong to ever fire (part 1) and
an extra stop on a `0x00` byte at depth 0 (`lib.rs:154`).

### 2.4 Anchor cost, measured

The user's instinct to keep the `0x0A` trigger is right for
*recall* but it is the expensive anchor. Trigger points on two real
haystacks:

| haystack | size | `0x0A` bytes (our anchor) | `.proto` hits (protodump's anchor) | ratio |
|---|---|---|---|---|
| `gh` binary | 54 976 608 B | 242 293 | 2 205 | **110x** |
| `googleapis.desc` | 25 660 332 B | 1 040 408 | 44 100 | **24x** |

Both anchors are cheap to evaluate (`memchr` for one byte,
`memmem` for six). The difference is not in the search, it is in how
many *candidates* survive to the expensive stage. If the expensive
stage becomes a schema walk or a score, the anchor choice becomes the
dominant cost decision.

The two are not equivalent in coverage, though: the `.proto` anchor
structurally cannot find a descriptor whose `name` does not end in
`.proto`, and the `0x0A` anchor plus a plausible-path filter is what
lets us reject garbage that protodump would hand to the unmarshaler.
The natural resolution is the profile split in part 5: `.proto`-first
for the fast FDP profile, `0x0A` for the thorough one.

---

## 3. Where to stop

This is the genuine difference between `protoscan` and
`prototext score`: a score is given a blob whose extent is known;
a scanner is not.

### 3.1 There is no purely structural answer

At the top level of a `FileDescriptorProto`, an FDP followed by
another FDP is **indistinguishable on the wire** from one longer FDP.
Both of the things you would want to use as a boundary are legal
protobuf:

- a repeated occurrence of a singular field — legal, last wins;
- an unknown field number — legal, preserved as an unknown field.

So any stop rule is necessarily a *heuristic about intent*, not a
decision derivable from the wire format. All three tools above are
heuristics; the question is only which one is least wrong.

### 3.2 The schema-derived stop

`FileDescriptorProto` declares exactly these top-level field numbers:

| # | field | cardinality |
|---|---|---|
| 1 | `name` | singular |
| 2 | `package` | singular |
| 3 | `dependency` | repeated |
| 4 | `message_type` | repeated |
| 5 | `enum_type` | repeated |
| 6 | `service` | repeated |
| 7 | `extension` | repeated |
| 8 | `options` | singular |
| 9 | `source_code_info` | singular |
| 10 | `public_dependency` | repeated |
| 11 | `weak_dependency` | repeated |
| 12 | `syntax` | singular |
| 14 | `edition` | singular |

13 is undeclared. Two boundary signals follow, and they are strictly
stronger than protodump's rule because they use all thirteen numbers
rather than just field 1:

1. a top-level tag whose field number is **not in that set** — 13, or
   anything above 14;
2. a **second occurrence of a singular field** — 1, 2, 8, 9, 12 or 14.

protodump's `consumedFieldOne` is the special case of (2) restricted to
field 1. Generalizing it is free: the table is already in the embedded
descriptor.

### 3.3 The veto-monotone refinement

`prototext-graph`'s veto is **sticky and prefix-monotone**: `set_vetoed`
is followed by `ae.entries.clear()`, so a veto triggered by a byte in a
prefix cannot be undone by extending the buffer.

That gives a clean formulation of the boundary as a search rather than
a guess:

> the candidate ends at the **maximal prefix that does not veto**.

And because a veto is monotone, that maximum can be found in a single
pass — no descending brute force à la Protod3. The candidate set to
test is small: only the top-level field boundaries, of which a real FDP
has a few dozen.

The veto that does the heavy lifting here is the UTF-8 one
(`prototext-graph/src/score/walk.rs:1302`):

```rust
if is_string && std::str::from_utf8(payload).is_err() {
    // "invalid UTF-8 on string field {field_number}"
}
```

Measured: the bogus 25 MB whole-file candidate from part 1 is
**vetoed** against `FileDescriptorProto`, score 0. The correct 291-byte
first record is not. The veto separates them without any length
heuristic at all.

---

## 4. A score-based protoscan

### 4.1 What to embed

Almost nothing. The association set needs `descriptor.proto` and its
transitive closure — which is `descriptor.proto` alone — compiled into
a reproto database and embedded in the binary. Two roots matter:

- `google.protobuf.FileDescriptorProto`
- `google.protobuf.FileDescriptorSet`

Keeping *both* is what makes the part-1 bug structurally impossible to
reintroduce, and it is measurable rather than argued:

| blob | scored against | result |
|---|---|---|
| 291-byte first record | `FileDescriptorProto` | score 35, 35 matches, 0 unknowns, **not vetoed** |
| 291-byte first record | `FileDescriptorSet` | **vetoed** |
| whole 25 MB file | `FileDescriptorProto` | **vetoed**, score 0 |
| whole 25 MB file | `FileDescriptorSet` | score 2 829 366, **not vetoed** |

The scorer tells the two shapes apart perfectly and in both
directions. A `protoscan` that scored the file as a whole against a
two-root set would have identified `googleapis.desc` as a
`FileDescriptorSet` and framed it correctly, instead of mistaking it
for one enormous FDP.

Nothing else needs to be embedded. A user-supplied `--db` can widen the
association set for the general profile (part 5), but the FDP profile
is closed over `descriptor.proto`.

### 4.2 What score criterion

Not a threshold on `score()`. `EntryScore::score()` is

```
matches − 10·unknowns − 15·out_of_range − 20·non_canonical − 30·mismatches
```

and it is **size-proportional**. Measured across all 7 771 genuine
FDPs in `googleapis.desc`, `score()` ranges **8 … 171 309** — four
orders of magnitude, purely as a function of how big the file is. Any
absolute cut-off would reject the small files or admit garbage, or
both.

The size-independent accept rule is:

```
!vetoed && unknowns == 0 && mismatches == 0
```

Measured on the same 7 771 FDPs: **0 vetoed, 0 unknowns, 0 mismatches,
0 non-canonical, 0 out-of-range** — a clean sweep, 7 771/7 771. The
rule has no false negatives on a real corpus.

`score()` keeps one job: **ranking**, when more than one root survives
the accept rule, and when choosing among candidate end offsets in
3.3. Never as an absolute gate.

### 4.3 Cost

| operation | measured |
|---|---|
| `score_one(291 B, FileDescriptorProto)` | 10.8 µs |
| `score_all(291 B)` over 49 255 roots | 29 ms |
| 7 771 real FDPs vs `FileDescriptorProto` | 608 ms total, **78.3 µs each** |

`score_one` against a known root is ~10 µs and the mean over a real
corpus is 78 µs. `score_all` over a large database is 29 ms — three
orders of magnitude more, because it is proportional to the root count.

This is the whole argument for the profile split: the FDP profile knows
its two roots and pays `score_one`; a general "what is this blob"
profile pays `score_all` and must therefore be reserved for far fewer
candidates.

For reference, `score_all` on the 291-byte record leaves 21 444
survivors out of 49 255 roots, with `FileDescriptorProto` ranked #1 at
35 and the runners-up at −5. The ranking is right, but 21 444
non-vetoed survivors is a reminder that veto alone is a weak filter on
a short blob — the accept rule of 4.2 is what does the work.

### 4.4 Shape

The user's constraint is to keep protoscan a thin Python layer over a
Rust library. Nothing above changes that. The Rust side grows from
`scan_bytes(&[u8]) -> Vec<(usize, usize)>` to something that also
returns, per candidate, the winning root FQDN and its `EntryScore`;
the Python side keeps doing argument parsing, output formatting and
file writing. The relevant Rust entry points already exist and are
public:

- `prototext_graph::score::load::load_graph(&Path) -> LoadedGraph`
- `prototext_graph::score::walk::score_one(pb, fqdn, graph, opts)`
- `prototext_graph::score::walk::score_all(pb, graph, opts)`
- `prototext_core::build_arena(&[u8]) -> Result<Arena, CodecError>`
- `prototext_schema::lazy_pool::LazyPool::open(pb, idx, wkt_fallback)`

---

## 5. Compressed descriptors

The user's recollection is correct: some reflection frameworks do embed
descriptors as *compressed* artifacts. What follows separates what
protodump does (nothing) from what the phenomenon actually is.

### 5.1 protodump does not handle compression

Verified: grepped every file in the clone and all 25 commits for
`gzip`, `flate`, `zlib`, `compress`, `deflate` — no hits outside the
Go standard library import list, which does not include them. The
scanner reads the file, `bytes.Index`es for `.proto`, and never
decompresses anything.

[arkadiyt's own write-up][arkadiyt-post] describes the same pipeline —
`.proto` anchor, backward scan to `0x0A`, consume-while-valid,
unmarshal to validate — and **makes no mention of gzip, of Go's
`proto.RegisterFile`, or of compressed descriptors at all**.

So the answer to "how does protodump scan for compressed FDPs" is: it
does not.

### 5.2 The phenomenon is real, and it is Go's APIv1

`github.com/golang/protobuf` (APIv1) registered file descriptors
**gzipped**. Generated code called

```go
proto.RegisterFile(filename, fileDescriptor)
```

where `fileDescriptor` was a `[]byte` holding a gzip stream of the
serialized `FileDescriptorProto`. The runtime's own accessor type is
literally named `fileDescGZIP`, and its `extractFile` inflates with
`compress/gzip` before unmarshaling. A binary built against APIv1
therefore contains descriptors that a byte-level `.proto` or `0x0A`
scan cannot see at all — the plaintext never appears in the image.

`google.golang.org/protobuf` (APIv2) changed this. Generated code now
embeds an **uncompressed** `file_x_proto_rawDesc`, with a
`rawDescGZIP()` accessor that compresses lazily *at runtime* for the
legacy API's benefit. `proto.RegisterFile` is deprecated in favor of
`protoregistry.GlobalFiles`.

That transition is directly observable. Scanning the `gh` binary
(54 976 608 B, a modern APIv2 Go program) finds **3 gzip members
(`1f 8b 08`) and 0 gzipped FDPs** — the descriptors are all there, in
the clear, which is precisely why protodump works on modern Go
binaries and why its author never needed to address compression.

### 5.3 The case is recognized elsewhere

[dennwc/protod][protod-dennwc] states it explicitly. Its README lists
under *supported*:

> Extraction from uncompressed file descriptors (used in C/C++, maybe
> others)

and under *not supported*:

> Compressed file descriptors (used in Go)

So the ecosystem knows about the case and names it; no tool surveyed
here implements it.

### 5.4 What handling it would take

Cheap and self-contained, if wanted: a pre-pass that finds gzip member
starts (`1f 8b 08`), inflates each into a scratch buffer, and runs the
ordinary scanner over the inflated bytes. The offsets returned would be
in the *decompressed* coordinate space, so the API would have to report
a container reference alongside each candidate rather than a plain
`(start, end)` into the input file. That is the only real design
consequence.

This is out of scope for fixing part 1 and is recorded here so the case
is not rediscovered from scratch.

---

## 6. Proposed shape, honoring the constraints

| constraint | consequence |
|---|---|
| thin Python over a Rust lib | unchanged; the Rust surface grows a per-candidate verdict, Python keeps I/O and formatting |
| a FAST profile targeting FDPs | `.proto`-anchored or `0x0A`-anchored scan, schema-derived stop (3.2), `score_one` against two known roots at ~10 µs |
| keep the `0x0A` trigger | kept for the thorough profile; part 2.4 measures the 24-110x candidate-count penalty, which is affordable only when the per-candidate work is cheap |
| where to stop | maximal non-vetoing prefix (3.3), single pass, exploiting veto monotonicity |
| what to embed | `descriptor.proto` as a reproto db, roots `FileDescriptorProto` + `FileDescriptorSet` (4.1) |
| score criterion | `!vetoed && unknowns == 0 && mismatches == 0` to accept; `score()` to rank only (4.2) |

### Immediate, independent of all the above

The one-condition fix in 1.3 restores `googleapis.desc` from 1
candidate to 7 771 and keeps every existing test green. It does not
depend on any of parts 3-6 and can land on its own.

---

## Sources

- [protodump][protodump] — arkadiyt. `pkg/protodump/scan.go`,
  `cmd/protodump/main.go`.
- [Reverse engineering protobuf definitions from compiled
  binaries][arkadiyt-post] — arkadiyt, 2024-03-03.
- [Protod3][protod3] — Sysdream. `protod.py`.
- [protod][protod-dennwc] — dennwc. README, supported/unsupported
  lists.
- [github.com/golang/protobuf][golang-protobuf] — APIv1,
  `proto.RegisterFile`, `fileDescGZIP`, `extractFile`.
- [google.golang.org/protobuf][protobuf-go] — APIv2,
  `rawDesc` / `rawDescGZIP()`, `protoregistry.GlobalFiles`.

[protodump]: https://github.com/arkadiyt/protodump
[arkadiyt-post]: https://arkadiyt.com/2024/03/03/reverse-engineering-protobuf-definitiions-from-compiled-binaries/
[protod3]: https://github.com/sysdream/Protod3
[protod-dennwc]: https://github.com/dennwc/protod
[golang-protobuf]: https://github.com/golang/protobuf
[protobuf-go]: https://github.com/protocolbuffers/protobuf-go
