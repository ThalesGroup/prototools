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

1. **Talk (30 min standard)** — "Cracking protobufs: schema recovery,
   type inference, anomaly detection"
2. **Workshop (2h, proposed as an option)** — "Reverse-engineer these
   protobufs" — a hands-on CTF-style session

---

## Talk synopsis

### Title

**Cracking protobufs: schema recovery, type inference, anomaly detection**

---

### Abstract

Protocol Buffers are ubiquitous in modern infrastructure — gRPC services,
cloud platforms, mobile apps — yet they are routinely treated as opaque.
The standard tool, `protoc --decode`, requires the original `.proto` source
files and silently normalizes anything it does not understand, making
wire-level anomalies invisible.

This talk presents prototools, an open-source suite built during the
inspection of Google software updates at S3NS (a Thales–Google joint
venture operating a sovereign cloud for European customers). Through a
live terminal demo, we show how to extract embedded descriptors from an
unknown binary, infer the message type of an undocumented capture, and
surface the anomalies that `protoc` would have silently discarded:
shadowed fields carrying data the application never sees, non-canonical
varints used as fingerprints or covert channels, and truncated messages
that standard decoders reject entirely.

Attendees will leave with a clear mental model of the protobuf wire
format, a taxonomy of encoding anomalies and their forensic significance,
and a freely available toolset they can apply immediately.

---

### Description

#### Who I am

Frederic Ruget is a software engineer at S3NS, a Thales–Google joint
venture that operates PREMI3NS — a Google Cloud region hosted in French
data centers and operated autonomously by French personnel under the
"Cloud de confiance" program. He built prototools as part of the
inspection work S3NS performs on every Google software update before it
reaches production. prototools is MIT-licensed and available at
github.com/ThalesGroup/prototools.

#### Context

The Google infrastructure is saturated with protobufs. S3NS inspects
every software and configuration update that Google ships to PREMI3NS
before it reaches production, using static analysis and dynamic
assessment in an isolated quarantine environment. That work kept running
into the same wall: binary blobs with no accompanying schema, malformed
messages that standard tools reject, and wire-level encoding details that
`protoc` erases before the analyst can see them. prototools was built to
remove that wall.

#### State of the art

The standard tool is `protoc --decode` (or `--decode_raw`). Its hard
limits:

- It requires the original `.proto` source files and the exact message
  type name.
- It silently normalizes non-canonical encodings. Wire-level anomalies
  vanish without a trace.
- It fails entirely on malformed or truncated inputs.

Wireshark has a protobuf dissector but also requires a schema upfront and
offers no anomaly detection. There is no open-source tool that answers:
*"I have a binary blob and no schema — what is it, and what is wrong
with it?"*

#### Work and findings

prototools is a suite of four tools.

**protoscan** scans an arbitrary binary for embedded
`FileDescriptorProto` blobs. gRPC services compiled with server
reflection or certain logging frameworks embed their own descriptors
directly in the binary. protoscan finds them in seconds, scanning at
roughly 1 GiB/s, using the wire-level invariants of
`FileDescriptorProto` itself as a sieve.

**reproto** decompiles those blobs to compilable `.proto` source files.
It handles proto2, proto3, and the newer editions syntax; it operates on
incomplete descriptor sets (not every imported type needs to be present);
and it produces an indexed descriptor database used by the other tools
for fast type lookup. Deduplication uses Hopcroft minimization to merge
structurally identical subgraphs across thousands of schemas.

**prototext** is a lossless, bidirectional converter between binary
protobuf and human-readable text. "Lossless" is the key word: every
non-canonical byte is preserved as an inline annotation, and the encoded
output round-trips byte-exact — including for malformed or adversarial
inputs. Given an indexed descriptor database, prototext infers the
message type automatically, ranking all candidates in a single wire walk
and surfacing ties. With the googleapis descriptor set (~8,000 types),
inference runs in under a second.

**protolens** is the interactive TUI version of prototext. It displays a
protobuf as a navigable tree, color-codes nodes by anomaly severity, and
overlays per-field confidence indicators from the live type-inference
engine to guide the analyst. Type overrides can be applied interactively
and saved. The annotated result is exportable as prototext for reporting.

**Wire-level anomaly taxonomy.** A concrete output of this work is a
taxonomy of protobuf encoding anomalies and their forensic significance:

- *Shadowed scalar*: a singular field appears more than once at wire
  level. Standard decoders apply last-write-wins and silently discard
  all earlier values — a data exfiltration or smuggling vector invisible
  to the application layer.
- *Overhanging bytes* (ohb): a varint padded beyond its minimal
  encoding. Forbidden by the spec; its presence is a fingerprint or a
  covert channel.
- *Non-canonical negative integers and NaNs*: values that decode
  identically to their canonical form but whose wire encoding a
  conformant re-encoder would not reproduce.
- *Truncated message*: the binary ends mid-field. `protoc --decode`
  fails; prototext decodes as far as possible and annotates the
  boundary. In log forensics, truncation is evidence of a process killed
  mid-write.

**The demo scenario.** The talk is built around a live terminal demo:

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

#### On-site setup

Laptop + terminal only. No special hardware.

#### Expected duration

30 minutes (talk standard), including questions.

#### Prior and planned submissions

Presented at gRPConf 2026 (San Francisco, September 2026) — same tools,
engineering/practitioner framing rather than security framing. The
GreHack version focuses on the forensic and adversarial angle.

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

reverse engineering, forensics, protobuf, binary analysis, gRPC,
covert channels, schema recovery, type inference, anomaly detection,
open source

---

### Author

Frederic Ruget
Software engineer — S3NS (Thales x Google joint venture, Cloud de confiance)
GitHub: @douzebis

---

### Video upload

Upload the gRPConf 2026 talk recording (20-minute version of the demo scenario).

---

*(Old draft synopsis below — superseded by the sections above)*

**prototext** is a lossless, bidirectional converter between binary protobuf
and human-readable text. "Lossless" is the key word: it preserves every
non-canonical byte as an inline `#@` annotation, and the encoded output
round-trips byte-exact — including for malformed or adversarial inputs.
Given an indexed descriptor database, prototext automatically infers the
message type, ranking all candidates simultaneously in a single wire walk
(no schema tried twice) and surfacing ties rather than silently committing
to one result. With the googleapis descriptor set (~8,000 types), inference
runs in under a second on a laptop.

**protolens** is the interactive TUI version of prototext. It displays a
protobuf as a navigable tree, color-codes nodes by anomaly severity, and
overlays "heat cues" — per-field confidence indicators from the live type
inference engine — to guide the analyst toward where the interesting types
are. The analyst can apply type overrides interactively (pressing `t` to
assign a known type to an untyped field, double-clicking a heat cue to
accept a suggestion), save those overrides, and export the annotated result
as prototext for reporting.

**Wire-level anomaly taxonomy.** One concrete output of this work is a
taxonomy of protobuf encoding anomalies and their forensic significance:

- *Shadowed scalar* (`shadowed_scalar`): a singular field appears more than
  once at wire level. Standard decoders apply last-write-wins and silently
  discard all but the final value. The discarded values are invisible to
  the application layer — a potential data exfiltration or smuggling vector.
  prototools surfaces every instance.
- *Overhanging bytes* (`ohb`): a varint is padded beyond its minimal
  encoding. The canonical protobuf specification forbids this; its presence
  is a fingerprint. It can be accidental (some older encoders produce it)
  or deliberate (a covert channel in the encoding layer, or a
  canonicality-breaking obfuscation technique).
- *Truncated message* (`TRUNCATED_MESSAGE`): the binary ends mid-field.
  `protoc --decode` fails on this; prototext decodes as far as possible and
  annotates the boundary. In log forensics, truncation is evidence of a
  process killed mid-write.
- *Repeated singular* (`repeated_singular`): a proto3 singular field
  appears more than once. The spec says last-write-wins; the earlier values
  are accessible only at the wire level.

**The demo scenario.** The talk is built around a live terminal demo using
a pre-built scenario:

- *Bob* downloads an unknown executable and a truncated log file, and
  captures a network call. `protoc --decode_raw` gives field numbers, no
  semantics; the log file fails entirely.
- *Alice* runs `protoscan` on the executable: embedded descriptors found,
  a subset of the Google Maps API. `reproto` decompiles them. `prototext`
  infers the capture's type: `google.maps.places.v1.SearchTextRequest` --
  with a non-canonical encoding penalty. `protolens` opens the capture and
  shows what `protoc` missed: a shadowed `text_query` field containing data
  the application never saw.
- Against the log file, `protolens` guides the analyst through heat cues to
  identify `SearchTextResponse`, `SearchTextRequest`, `Timestamp` — then, on
  switching to the full googleapis descriptor set, `ComputeRoutesRequest` and
  `ComputeRoutesResponse` for a second service entirely. The overhanging bytes
  on `travel_mode` confirm non-canonical encoding is intentional. The shadowed
  field in the first log entry, once typed as `google.rpc.Status`, contains an
  API key.
- The annotated log file is exported as prototext and re-encoded to the
  original bytes, byte-exact. The report goes to Bob.

The entire demo runs from a scripted teleprompt system; no live typing is
required. Every claim is demonstrated in a terminal.

**Performance.** protolens opens the googleapis descriptor set (25 MiB,
~8,000 `FileDescriptorProto` entries) against itself as the document in
under a second. Navigation is immediate. The type inference graph is
computed in parallel across available CPU cores; indexes are pre-built and
memory-mapped. The rendering engine never materializes the full document:
it uses a rope-like cursor abstraction and renders only what is visible in
the current viewport.

---

### Expected talk duration

40 minutes (including questions).

---

### Support material

- **Video:** gRPConf 2026 talk recording (same scenario, 20-minute version,
  different audience framing — gRPC practitioners rather than security
  analysts). Available as a support document.
- **Repository:** github.com/ThalesGroup/prototools (MIT license)
- **Prior presentations:** gRPConf 2026 (San Francisco, September 2026) —
  same tools, tooling/engineering framing rather than security framing.

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
