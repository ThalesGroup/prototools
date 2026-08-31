<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# GreHack 2026 — Submission draft

- **Deadline:** August 31, 2026
- **Notification:** September 16, 2026
- **Conference:** November 13, 2026 — Grenoble, France

---

## Proposed submissions

1. **Talk (30 min standard)** — "Cracking protobufs open: schema recovery,
   type inference, anomaly detection"
2. **Workshop (2h, proposed as an option)** — "Reverse-engineer these
   protobufs" — a hands-on CTF-style session

---

## Talk synopsis

### Title

**Cracking protobufs open: schema recovery, type inference, anomaly detection**

---

### Abstract

Protocol Buffers are ubiquitous in modern infrastructure — gRPC services,
cloud platforms, mobile apps — yet they are routinely treated as opaque.
The standard tool, `protoc --decode`, requires the original descriptors and
message type and silently normalizes non-canonical encodings, making
wire-level anomalies invisible.

This talk presents prototools, an open-source suite built during the
inspection of Google software updates at S3NS (a Thales–Google joint
venture operating a "Cloud de confiance" platform).
It is a live working session rather than a slide deck: the real tools,
running in a terminal, taking an analyst's investigation end to end.
We show how to extract and decompile
embedded descriptors from an unknown binary, infer the message type of
an undocumented network capture, and surface the anomalies that `protoc`
would have silently discarded: shadowed fields carrying data the application
never sees, non-canonical varints used as fingerprints or covert channels,
and truncated messages that standard decoders reject entirely.

A recovered corpus rarely contains the message you are holding — but it
often contains the messages inside it. Every node of a blob is scored
against the corpus on its own, and the interactive viewer marks each one
with a heat cue: how well those particular bytes fit the best type the
corpus can offer for them. An unknown wrapper stays cold while a
familiar sub-message lights up. The analyst applies a type override
there, the structure around it resolves, and an unknown schema is
recovered piece by piece out of the parts that are known.

Attendees will leave with a clear mental model of the protobuf wire
format, a taxonomy of encoding anomalies and their forensic significance,
and a freely available toolset they can apply immediately.

---

### Description

#### Who I am

I am a reverse engineer at S3NS, a Thales–Google joint venture that
operates PREMI3NS — a Google Cloud region hosted in French data centers
and operated autonomously by French personnel as a SecNumCloud-qualified
"Cloud de confiance" platform. I developed prototools as part of the
inspection work S3NS performs on every Google software and configuration
update before it reaches production: static analysis and dynamic
assessment in an isolated quarantine environment.
prototools is MIT-licensed and available at
github.com/ThalesGroup/prototools.

#### Context

Google's infrastructure is the limit case: protobufs carry not only
the RPC traffic but the configuration, the update metadata, and the
descriptors of the binaries themselves. We meet them as bare blobs: no
schema, no message type name, and often no indication that a given
byte range is a protobuf at all.

Two consequences shape the work. First, we have to recover the schema
before we can say anything about the content — and since the
descriptors are usually embedded in the very binaries under
inspection, the corpus is there for the taking, provided we can
extract it. The proto sources we decompile this way are then embedded
in our own analyzers, which screen every subsequent update.

Second, decoding is not enough. A blob that decodes cleanly can still
be non-canonical, and non-canonicity is precisely what a normalizing
decoder destroys. A duplicated field, a padded varint, a message that
stops mid-write — none of these are curiosities: each marks a gap
between what the wire carries and what the application will see. That
gap is where we look.

#### State of the art

The reference tool is `protoc --decode` (or `--decode_raw`). Its hard
limits:

- It requires the original `.proto` source files and the exact message
  type name.
- It silently normalizes non-canonical encodings. Wire-level anomalies
  vanish without a trace.
- It fails entirely on malformed or truncated inputs.

A number of open-source tools address parts of the problem:

- **protoscope** (protocolbuffers/protoscope) — the Protobuf team's
  human-editable language for the wire format. Schema-ignorant by
  design and tolerant of corrupted input, it is the closest thing to a
  low-level protobuf hex editor. But it does not apply a schema, infer
  types, or classify anomalies: the analyst gets raw structure and does
  the semantics by hand. And it normalizes silently too — non-canonical
  varints and similar constructs are transcribed to their canonical
  form, so the very details we treat as evidence are gone by the time
  the analyst reads the output.
- **protodump** (arkadiyt/protodump), **pbtk** (marin-m/pbtk) —
  extract embedded descriptors from binaries. protodump locates
  descriptors by searching for the ASCII string `.proto` and scanning
  heuristically around it; pbtk targets specific runtimes (Java
  flavors, C++ reflection metadata, JsProtoUrl web apps). Extraction is
  where these tools stop: no decompilation to buildable sources across
  syntaxes, no downstream analysis.
- **blackboxprotobuf** (NCC Group) — decodes and re-encodes messages
  without a schema, guessing field types from the wire data alone.
  Built for Burp-style traffic interception. Its type model is derived
  per-message; it does not match a blob against a corpus of recovered
  descriptors, and non-canonical encodings do not survive a round trip.
- **protobuf-inspector** (mildsunrise) — heuristic pretty-printer for
  schema-less blobs, with hand-written partial type definitions.
- **Wireshark** — has a protobuf dissector, but requires the schema
  upfront and offers no anomaly detection.

Each covers one stage. None of them answers the full question — *"I
have a binary blob and no schema: what is it, and what is wrong with
it?"* — and none of them treats wire-level anomalies as first-class
evidence rather than noise to be normalized away.

#### Work and findings

**The use case first.** The talk is a demo of a working session with
the prototools — slides serve only as framing; the substance happens
in the terminal. Two fictional characters carry the scenario: Bob, who
comes across the artifacts, and Alice, who analyzes them.

- *Bob* downloads an unknown executable and a truncated log file, and
  captures a network call. `protoc --decode_raw` gives him field
  numbers and no semantics; on the log file it fails outright.
- *Alice* runs `protoscan` on the executable: embedded descriptors found
  — a subset of the Google Maps API. `reproto` decompiles them.
  `prototext` infers the capture's type: `SearchTextRequest` — with a
  non-canonical encoding flag. `protolens` opens the capture and shows
  what `protoc` missed: a shadowed `text_query` field carrying data the
  application never sees.
- Against the log file, `protolens` guides Alice through heat cues to
  identify two services: Maps Places and Routes. The overhanging bytes
  on `travel_mode` confirm intentional non-canonical encoding. A
  shadowed field in the first log entry, once typed, contains an API
  key.
- The annotated log file is exported as prototext and re-encoded to the
  original bytes, byte-exact. Alice's report goes back to Bob.

A teleprompt proposes each command, so nothing hinges on live typing —
but this is not a replay: the tools really are running underneath, and
the speaker can step off script at any moment to try a variant, follow
a detail, or answer a question. Every claim is demonstrated in a
terminal.

**The tools.** prototools is a suite of four tools; each maps to a
stage of the demo, and each pushes past what the existing tools listed
above provide.

**protoscan** scans an arbitrary binary for embedded
`FileDescriptorProto` blobs, at roughly 1 GiB/s. Like protodump, it
uses credible file paths ending in `.proto` as an initial filter — but
the rest of the analysis is structural: the wire-level invariants of
the `FileDescriptorProto` type definition itself (canonical encoding,
known field structure) serve as a sieve, so candidates are validated
and their boundaries recovered structurally rather than
heuristically.

**reproto** decompiles those blobs to compilable `.proto` source files
— all three syntaxes: proto2, proto3, and editions. It operates on
incomplete descriptor
sets: missing imports and pruned symbols degrade the output gracefully
instead of aborting the run. It also produces an indexed descriptor
database used by the other tools for fast type lookup, in which
structurally identical type definitions coming from thousands of
schemas are collapsed into a single entry (a Hopcroft-style partition
refinement over the type graph).

**prototext** is a lossless, bidirectional converter between binary
protobuf and human-readable text. "Lossless" is the differentiator:
every non-canonical byte is preserved as an inline annotation, and the
encoded output round-trips byte-exact — including for malformed or
adversarial inputs, and including when the descriptor in hand is not a
perfect match for the blob (schema/wire disagreements are annotated,
not fatal). Given an indexed descriptor database, prototext infers the
message type automatically: all candidate types are ranked in a single
wire walk, and ties are surfaced rather than silently resolved. This
corpus-driven inference is the piece no existing tool has —
blackboxprotobuf guesses types from the wire data alone; prototext
matches the blob against thousands of recovered schemas at once.
Inference cost scales with the size of the target blob and of the
corpus. As a data point, `googleapis.desc` is convenient because it is
both at once: it is a 25 MB protobuf (a `FileDescriptorSet` holding
some 8,000 `FileDescriptorProto` entries), and it is a corpus of tens
of thousands of message types. Inferring the type of that 25 MB blob
against the corpus it itself defines takes 0.7 s on a 12-CPU machine.

**protolens** is the interactive TUI version of prototext. Interactive
protobuf viewers exist (blackboxprotobuf's Burp extension, a handful of
schema-less GUI viewers), but to our knowledge none combines
schema-driven decode, wire-level anomaly classification, and live
corpus-based type inference in one interactive UI.
It displays a protobuf as a navigable tree, color-codes nodes
by anomaly severity, and overlays *heat cues*. The cues are what make
an unrecognized message tractable: scoring runs per node, not per
document, so a blob whose top-level type is nowhere in the corpus
still lights up wherever a sub-message does match one. Each node
carries the best type the corpus can offer for its own bytes and how
well those bytes fit it — including nodes that have no type yet, which
is exactly where the question is worth asking. The analyst applies a
type override on the node that lit up, the decode re-runs, and the
structure around it resolves; repeat, and a schema nobody has ever
seen is reconstructed out of the fragments that are known. Overrides
are saved with the document, and the override pane lists every
candidate with its score, so the analyst always sees what was rejected
and on what evidence. The underlying wire bytes can be shown side by
side with the decoded text, down to individual tag, length, and
payload bytes; the annotated result is exportable as prototext for
reporting. The TUI stays fluid on documents and
descriptor corpora of tens of megabytes: indexes are pre-built and
memory-mapped, inference runs in parallel across cores, and the
renderer materializes only the visible viewport.

**Wire-level anomaly taxonomy.** A concrete output of this work is a
classification of protobuf encoding anomalies — some two dozen distinct
conditions, every one of them reproducible from a single reference blob
shipped with the tools — organized by what an anomaly means to the
analyst rather than by where it sits in the wire format.

*Legal but non-canonical.* Overhanging bytes: a varint padded beyond
its minimal encoding — and not only value varints, but tags, length
prefixes, and group-closing tags. Non-canonical negative integers
(`-1` in five bytes where a ten-byte sign-extended varint is expected).
Non-canonical NaNs (IEEE 754 has 2^52 distinct NaN payloads; a
re-encoder picks one and the others are lost). Parsers accept all of
these without a word; no mainstream encoder emits any of them. That
asymmetry is the whole point: for the attacker, a covert channel — the
padding bytes never reach the application — and an evasion primitive
against naive signature matching; for the defender, a fingerprint that
betrays a non-standard producer. They also defeat canonicalization-based
integrity checks: hash the re-encoded message and the anomaly is gone.

*Ambiguous — the decoder decides.* A singular field that appears twice
on the wire, where last-write-wins silently discards the first value. A
group opened as field 100 and closed as field 101 (groups are proto2
legacy, but they never left the wire format, and decoders still have to
take a position on them). Fields the schema does not declare, and enum
values it has no name for. A field declared `string` that arrives as a
varint, or as bytes that are not valid UTF-8. These are
parser-differential primitives: two conformant decoders can read two
different messages out of the same bytes. That is a smuggling vector
invisible at the application layer, and a well-known way for a
validator and the service behind it to disagree about what they just
approved.

*Malformed — where analysis normally stops.* Truncated messages.
Varints and length prefixes with no terminating byte. A fixed64 field
with three bytes behind it. Wire type 6, which does not exist. Field
number 0, which is out of range. Groups that are never closed, and
`END_GROUP` tags that close nothing. `protoc --decode` rejects the
entire input on any of these; prototext localizes the damage, annotates
it, and resumes at the next tag — one bad byte costs one field instead
of the whole document. In log forensics that pays twice, because
truncation is itself evidence: a process killed mid-write, and a
precise marker of when.

#### Limitations

Type inference matches a blob against a corpus of recovered
descriptors, and the heat cues in protolens come from that corpus.
Three consequences are worth stating up front.

*The binary may embed no descriptors at all* — stripped builds,
non-reflective runtimes. prototext and protolens still work: they fall
back to best-effort discrimination between bytes, strings, and
embedded messages, which is roughly the view protoscope offers, minus
the normalization. From there the analyst can handcraft and apply type
overrides. Sidecar corpora are the other way out: descriptors
recovered from another version of the same binary, or from an
unrelated source, are often close enough to type the message.

*The tool narrows the search; it does not close it.* Candidates tie, or
come within noise of one another, and no amount of scoring turns that
into certainty. So protolens never picks silently: the override pane
lists every candidate with its score, a node's cue reports how many
others tie with the one it names, and the analyst arbitrates. This is
a division of labor rather than a shortcoming — but it does mean the
output is an analyst's reasoned reconstruction, not a decompilation
that can be taken on trust.

*Without any corpus, the heat cues disappear.* The structural view and
manual overrides remain, but the guidance that makes the workflow fast
is gone.

#### On-site setup

Laptop + terminal only. No special hardware.

#### Expected duration

30 minutes (talk standard), including questions.

#### Prior and planned submissions

Accepted at gRPConf 2026 (San Francisco) — same tools, but an
engineering/practitioner framing rather than a security one. That talk
takes place a few days after this submission deadline, so the attached
video is a recording of the same demo scenario (20-minute cut) rather
than of the conference itself. The GreHack version deepens the
forensic and adversarial analysis.

---

### Notes (for the programme committee)

A 2-hour hands-on workshop ("Reverse-engineer these protobufs") is
proposed as an optional complement to the talk. Participants receive a
set of binaries, network captures, and log files with no schema, and
must answer escalating questions about their content — CTF-style. The
workshop is independent: the talk stands alone if no workshop slot is
available.

A 20-minute recording of the same demo scenario (the gRPConf 2026 cut)
is attached as supporting material.

All tools are MIT-licensed. No proprietary technology. The demo scenario
uses entirely fictional data (no real captures, no real API keys).

---

### Tags

reverse engineering, forensics, protobuf, gRPC, covert channels,
binary analysis

---

### Author

- Frederic Ruget
- Reverse engineer — S3NS (Thales x Google joint venture, Cloud de confiance)
- GitHub: @douzebis

---

### Video upload

Upload the 20-minute recording of the demo scenario (the gRPConf 2026 cut).

---

## Workshop proposal

### Title

**"Reverse-engineer these protobufs" — a hands-on workshop**

### Duration

2 hours

### Motivation

The talk demonstrates the techniques; the workshop lets participants apply
them. The format is CTF-style: participants are given a set of binaries,
network captures, and log files with no schema provided, and must answer
a series of escalating questions about what the protobufs contain.

This is not a product tutorial. The tools are a means to an end; the
learning objective is fluency with the wire format and the recon workflow.

Attending the talk is not a prerequisite: the workshop is self-contained
and opens with the wire-format background it needs.

### Prerequisites

- Familiarity with protobuf at the "I have used it in a project" level.
- A laptop with a working terminal. prototools will be provided as a
  static binary (Linux x86-64 and macOS arm64) to avoid build environment
  issues.

### Structure

| Time | Activity |
|------|----------|
| 0:00 – 0:20 | Setup and warmup: verify installs, decode a known protobuf by hand with `protoc --decode_raw`, observe the limits |
| 0:20 – 0:40 | Challenge 1 (guided): `protoscan` a provided binary, `reproto` the descriptors, identify the message type of a capture |
| 0:40 – 1:10 | Challenge 2 (semi-guided): an unknown log file, no schema. Use heat cues to type the fields. What service is this? What anomalies are present? |
| 1:10 – 1:45 | Challenge 3 (open): a more complex binary with multiple embedded schemas and a capture that uses a different service than the binary itself. What is in the shadowed field? |
| 1:45 – 2:00 | Debrief and Q&A |

### References

- github.com/ThalesGroup/prototools
- Protocol Buffers encoding specification: protobuf.dev/programming-guides/encoding/

---

## Notes for the submission form

- The talk and workshop are proposed together but are independent: the
  workshop is offered as an option if GreHack has a workshop slot available.
  The talk stands alone.
- No proprietary technology is involved. All tools are MIT-licensed and
  will remain so.
- The demo scenario uses entirely fictional data (generated specifically for
  the demo). No real captures, no real API keys.
- The "Cloud de confiance" / S3NS context provides industrial origin; the
  techniques themselves are general and applicable to any gRPC target.
