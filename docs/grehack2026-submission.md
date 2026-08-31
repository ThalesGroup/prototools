<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# GreHack 2026 — Submission draft

**Deadline:** August 31, 2026
**Notification:** September 16, 2026
**Conference:** November 13, 2026 — Grenoble, France

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
venture operating a "Cloud de confiance" platform for European customers).
The talk is organized as a live working session: an analyst's
investigation replayed end-to-end in the terminal, not a slide deck.
We show how to extract and decompile
embedded descriptors from an unknown binary, infer the message type of
an undocumented network capture, and surface the anomalies that `protoc`
would have silently discarded: shadowed fields carrying data the application
never sees, non-canonical varints used as fingerprints or covert channels,
and truncated messages that standard decoders reject entirely.

Attendees will leave with a clear mental model of the protobuf wire
format, a taxonomy of encoding anomalies and their forensic significance,
and a freely available toolset they can apply immediately.

---

### Description

#### Who I am

Frederic Ruget is a reverse engineer at S3NS, a Thales–Google joint
venture that operates PREMI3NS — a Google Cloud region hosted in French
data centers and operated autonomously by French personnel as a
SecNumCloud-qualified "Cloud de confiance" platform.
He developed prototools as part of the inspection work S3NS performs
on every Google software update before it reaches production.
prototools is MIT-licensed and available at
github.com/ThalesGroup/prototools.

#### Context

The Google infrastructure is saturated with protobufs. S3NS inspects
every software and configuration update that Google ships to PREMI3NS
before it reaches production, using static analysis and dynamic
assessment in an isolated quarantine environment. The protobufs we
encounter come as binary blobs with no accompanying schema. prototools
helps us make sense of them and verify that they are canonical, free
of nefarious constructs. The proto sources decompiled from the Google
binaries are also embedded in our analyzers, which continuously scrub
the software updates we receive from Google.

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
  the semantics by hand.
- **protodump** (arkadiyt/protodump), **proto-dump** (obriensp),
  **pbtk** (marin-m/pbtk) — extract embedded descriptors from
  binaries. protodump locates descriptors by searching for the ASCII
  string `.proto` and scanning heuristically around it; pbtk targets
  specific runtimes (Java flavors, C++ reflection metadata, JsProtoUrl
  web apps). Extraction is where these tools stop: no decompilation to
  buildable sources across syntaxes, no downstream analysis.
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
in the terminal:

- *Bob* downloads an unknown executable and a truncated log file, and
  captures a network call. `protoc --decode_raw` gives field numbers and
  no semantics; the log file fails entirely.
- *Alice* runs `protoscan` on the executable: embedded descriptors found
  — a subset of the Google Maps API. `reproto` decompiles them.
  `prototext` infers the capture's type: `SearchTextRequest` — with a
  non-canonical encoding flag. `protolens` opens the capture and shows
  what `protoc` missed: a shadowed `text_query` field carrying data the
  application never saw.
- Against the log file, `protolens` guides Alice through heat cues to
  identify two services: Maps Places and Routes. The overhanging bytes
  on `travel_mode` confirm intentional non-canonical encoding. A
  shadowed field in the first log entry, once typed, contains an API
  key.
- The annotated log file is exported as prototext and re-encoded to the
  original bytes, byte-exact. The report goes to Bob.

The entire demo runs from a scripted teleprompt; no live typing is
required. Every claim is demonstrated in a terminal.

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
database used by the other tools for fast type lookup, deduplicated
via Hopcroft minimization across thousands of schemas.

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
corpus; as a data point, on a 12-CPU machine the googleapis descriptor
set (25 MB, ~8,000 `FileDescriptorProto` entries) is evaluated against
itself in under a second.

**protolens** is the interactive TUI version of prototext. Interactive
protobuf viewers exist (blackboxprotobuf's Burp extension, a handful of
schema-less GUI viewers), but to our knowledge none combines
schema-driven decode, wire-level anomaly classification, and live
corpus-based type inference in one interactive UI.
It displays a protobuf as a navigable tree, color-codes nodes
by anomaly severity, and overlays *heat cues* — per-field confidence
indicators from the live type-inference engine — to guide the analyst
toward the interesting types. The underlying wire bytes can be shown
side by side with the decoded text, down to individual tag, length,
and payload bytes. Type overrides are applied
interactively and saved; the annotated result is exportable as
prototext for reporting. The TUI stays fluid on documents and
descriptor corpora of tens of megabytes: indexes are pre-built and
memory-mapped, inference runs in parallel across cores, and the
renderer materializes only the visible viewport.

**Wire-level anomaly taxonomy.** A concrete output of this work is a
taxonomy of protobuf encoding anomalies and their forensic significance:

- *Shadowed scalar*: a singular field appears more than once at wire
  level. Standard decoders apply last-write-wins and silently discard
  all earlier values — a data exfiltration or smuggling vector invisible
  to the application layer, and a classic parser-differential primitive:
  two decoders that disagree on which occurrence wins see two different
  messages in the same bytes.
- *Overhanging bytes* (ohb): a varint padded beyond its minimal
  encoding. Forbidden by the spec, accepted by every parser. For the
  attacker, a covert channel (the padding bytes are ignored) and an
  evasion primitive against naive signature matching; for the defender,
  a fingerprint — no canonical encoder produces it, so its presence
  identifies a non-standard producer.
- *Non-canonical negative integers and NaNs*: values that decode
  identically to their canonical form but whose wire encoding a
  conformant re-encoder would not reproduce. They defeat
  canonicalization-based integrity checks: hash the re-encoded message
  and the tampering disappears.
- *Truncated message*: the binary ends mid-field. `protoc --decode`
  fails; prototext decodes as far as possible and annotates the
  boundary. In log forensics, truncation is timeline evidence — a
  process killed mid-write, and a precise marker of when.

#### Limitations

Type inference matches a blob against a corpus of recovered
descriptors, and the heat cues in protolens come from that corpus.
Without a corpus, prototext and protolens still work: they fall back
to best-effort discrimination between bytes, strings, and embedded
messages, and the analyst can still handcraft and apply type overrides
to make sense of the data — but the heat cues disappear.

#### On-site setup

Laptop + terminal only. No special hardware.

#### Expected duration

30 minutes (talk standard), including questions.

#### Prior and planned submissions

Presented at gRPConf 2026 (San Francisco, September 2026) — same tools,
engineering/practitioner framing rather than security framing. The
attached recording shows the same demo scenario (20-minute cut); the
GreHack version deepens the forensic and adversarial analysis.

---

### Notes (for the programme committee)

A 2-hour hands-on workshop ("Reverse-engineer these protobufs") is
proposed as an optional complement to the talk. Participants receive a
set of binaries, network captures, and log files with no schema, and
must answer escalating questions about their content — CTF-style. The
workshop is independent: the talk stands alone if no workshop slot is
available.

The gRPConf 2026 talk recording (20-minute version of the same scenario)
is attached as supporting material.

All tools are MIT-licensed. No proprietary technology. The demo scenario
uses entirely fictional data (no real captures, no real API keys).

---

### Tags

reverse engineering, forensics, protobuf, gRPC, covert channels,
binary analysis

---

### Author

Frederic Ruget
Reverse engineer — S3NS (Thales x Google joint venture, Cloud de confiance)
GitHub: @douzebis

---

### Video upload

Upload the gRPConf 2026 talk recording (20-minute version of the demo scenario).

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
- "Shadowed fields and wire-level anomalies" — covered in the accompanying talk

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
