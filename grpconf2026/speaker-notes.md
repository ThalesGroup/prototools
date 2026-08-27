<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Speaker's notes — gRPConf 2026

## General advice

**Slow down on technical terms.** The audience will follow the story,
but technical terms go by fast. When you say `FileDescriptorProto`,
`SearchTextRequest`, or `ComputeRoutesResponse`, give the audience
half a beat to read it on screen before you move on.

**Let the tool do the talking.** During protolens demos, resist the
urge to narrate every keypress. Say the high-level intent ("now I am
naming this structure"), press the key, then let the screen update
before explaining what changed.

**Pause at the two climaxes.** There are two emotional beats where
silence is more effective than words:
1. When `repeated_singular` / `shadowed_scalar` appear in the capture
   demo — this is the moment the audience realises protoc was lying.
2. When the API key appears inside the google.rpc.Status message —
   say nothing for two seconds. Let people read it.

**Have a fallback for the live demo.** If anything misbehaves, the
script beats are your safety net: they restore exact tool state. Know
in advance which beat to re-enter if you lose your place.

---

## Section by section

### S3NS / PREMI3NS intro

Keep this brisk — the audience came for protobuf, not company history.
Two or three sentences maximum per slide. If you feel the need to
elaborate, do it in Q&A.

"PREMI3NS" is spelled with a 3, which stands for the French word
"Premier" (first-class, premium). Say it aloud as **"Prémiens"**
(preh-mee-YEN), not letter-by-letter.

### The stage

Decide in advance whether to name Google Maps explicitly when
introducing Bob's app. Naming it earlier makes the SearchTextRequest
inference more satisfying when it appears. Keeping it vague preserves
suspense — pick one and commit.

### Section 2 — protoc falls short

When protoc fails on the logfile, quote the error message aloud if
one appears. A concrete error message ("unexpected end of group") is
funnier and more memorable than "protoc failed".

### Section 3 — descriptors

When you show the decompiled `.proto` files, point to the service name
in `places_service.proto`. Say: "So bob/app is calling the Google Maps
Places API." This is the first moment the audience understands what
the app actually is — give it a beat.

### Section 4 — the score

When `list-schemas` returns a negative score, acknowledge it directly:
"A negative score means the encoding is suspicious. We will come back
to that." Then come back to it in the capture demo.

### The capture demo

The "non-canonical encoding" penalty in the tooltip is the link back
to the negative score. Say explicitly: "This is intentional — the app
is deliberately encoding data in a non-standard way to evade detection."

### The logfile + googleapis demo

After typing ComputeRoutesRequest and ComputeRoutesResponse, pause and
say: "So the app calls both the Places API and the Routes API." This
contextualises the credential leak that follows.

The `ohb` annotation is technical. Say: "ohb stands for
OverHangingBytes — this value is encoded with five bytes where one
would suffice. Standard protobuf parsers accept it silently. That is
the point."

After the API key appears: stop talking. Let the audience read. Then:
"That key was hidden inside a field that protoc would have silently
dropped."

### Conclusion

The conclusion bullet points are marked TO BE FILLED. Suggested
framings — pick what fits your delivery style:

- Factual: "Descriptors are often present in the binary. Schema
  inference makes them useful. Non-canonical encoding is a real attack
  surface."
- Narrative: "Bob's app was calling Google Maps, logging credentials
  in a shadowed field, and using non-canonical encoding to confuse
  parsers. protobuf is not opaque — if you have the right tools."
- Call to action: end with the GitHub URL and an invitation to
  contribute.

---

## Pronunciation guide for French-native speakers

The following words appear in the script and have traps for
French speakers.

| Word | Wrong (French reflex) | Right (English) | Notes |
|------|----------------------|-----------------|-------|
| **ubiquitous** | oo-bee-kee-TOOSS | yoo-BIK-wih-tuss | stress on second syllable; the qu sounds like kw |
| **schema** | shay-MA | SKEE-muh | hard sk, not sh; stress on first syllable |
| **inference** | an-fay-RANCE | IN-fer-ence | stress on first syllable; three syllables only |
| **lossless** | loss-LESS | LOSS-less | stress on first syllable |
| **canonical** | ca-no-NI-cal | kuh-NON-ih-kul | stress on second syllable |
| **executable** | ex-eh-CU-ta-ble | ek-ZEK-yuh-tuh-bul | stress on second syllable; five syllables |
| **descriptor** | deh-SCRIP-tor | dih-SKRIP-ter | stress on second syllable |
| **binary** | bee-NAI-ree | BY-nuh-ree | stress on first syllable; three syllables |
| **reverse-engineering** | ruh-VERSE | rih-VURSS | the vowel is a schwa, not "air" |
| **serialization** | seh-ri-ah-lee-ZA-sion | seer-ee-ul-ih-ZAY-shun | six syllables; -tion = shun |
| **decompile** | deh-com-PEEL | dee-kum-PYL | rhymes with "mile", not "peel" |
| **assert** | AH-sert | uh-SERT | stress on second syllable |
| **truncated** | TRON-catted | TRUNK-ay-tid | three syllables; stress on first |
| **shadowed** | sha-DOED | SHA-dode | stress on first syllable; one syllable "dode" |
| **credential** | creh-den-CIAL | kruh-DEN-shul | -tial = shul, not see-AL |
| **latency** | lah-TEN-see | LAY-ten-see | first syllable rhymes with "day" |
| **prototext** | pro-to-TEKST | PRO-toh-tekst | stress on first syllable |
| **protolens** | pro-to-LENZ | PRO-toh-lenz | stress on first syllable |
| **protoscan** | pro-to-SKAN | PRO-toh-skan | stress on first syllable |

### A few more specific to the demo

**gRPC** — say "gee-ar-pee-see", four letters, not a word.

**API** — say "ay-pee-eye", not "ah-pee". Both are heard but the
three-letter spelling is universal.

**Places API** — say "PLAY-sez ay-pee-eye". The s in Places is voiced
(z sound), not silent.

**Routes API** — say "ROOTS ay-pee-eye" (not "routs"). In American
English, "routes" rhymes with "boots".

**FileDescriptorProto** — say it as one flowing compound:
"file-dih-SKRIP-ter-PRO-toh". Do not pause between the parts.
Saying it confidently and quickly signals familiarity.

**ComputeRoutesRequest / ComputeRoutesResponse** — similarly, say
these as one fluid compound. Practice them before the talk.

---

## Timing hints

Two versions of the script exist: `grpconf2026.sh` (full, ~26 min) and
`grpconf2026-20min.sh` (20-minute cut). The three cuts in the short
version are marked with `[CUT 1/2/3]` comments in the script.

### Full version (~26 min)

| Section | Target duration |
|---------|----------------|
| S3NS intro | 2 min |
| prototools overview | 2 min |
| The stage (scenario setup) | 1 min |
| protoc falls short | 2 min |
| protoscan + reproto | 3 min |
| prototext + protolens/capture | 4 min |
| protolens/logfile (app.desc) | 5 min |
| protolens/logfile (googleapis) | 5 min |
| Scale demo + conclusion | 2 min |
| **Total** | **~26 min** |

### Short version (~20 min) — `grpconf2026-20min.sh`

| Section | Cut? | Target duration |
|---------|------|----------------|
| S3NS intro | — | 2 min |
| prototools overview | — | 2 min |
| The stage (scenario setup) | — | 1 min |
| protoc falls short | — | 2 min |
| protoscan + reproto | Cut 2: no tree/file view | 1.5 min |
| prototext + protolens/capture | Cut 3: no intermediate protoc decode | 3 min |
| protolens/logfile (app.desc) | — | 5 min |
| protolens/logfile (googleapis) | — | 5 min |
| Scale demo | Cut 1: removed | — |
| Conclusion | — | 0.5 min |
| **Total** | | **~22 min** |

The remaining 2-minute margin gives room for a slow start, an
unexpected question, or a tool hiccup. Do not try to spend it.

If you are still running long after the googleapis beat, cut the
round-trip `cmp` check and say it from the podium instead.

Leave buffer for questions during the demo — audience members at
gRPConf will ask about schema inference and the scoring algorithm.
Prepare a one-sentence answer for each: "inference uses a Hopcroft
bisimulation graph built from the descriptor set" and "score is a
sum of matched-field weights minus penalties for encoding anomalies".

---

## Anticipated questions and proposed answers

### On the tool and the approach

**Q: Is this open source? Where can I get it?**
Yes. MIT license. `github.com/ThalesGroup/prototools`.
`cargo install prototools` gets you the CLIs; protolens is in the same
crate.

**Q: Does it work without a descriptor set at all?**
Partially. `prototext decode --raw` and `protoscan` need no schema —
they operate purely on wire structure. Schema inference (`list-schemas`,
heat cues, type scoring) needs a descriptor set to score against.
Without one you get field numbers, wire types, and values — more than
`protoc --decode_raw` because non-canonical encodings are flagged, but
no field names.

**Q: How long does it take to build a schema database from a binary?**
In the demo: a few seconds for a ~10 MB binary. The bottleneck is
protoscan (scanning for embedded descriptors) and reproto (decompilation
and graph construction). Both are single-pass. A 50 MB binary stays
under a minute on a laptop.

**Q: Can it handle binaries that have been stripped or obfuscated?**
Stripping removes debug symbols but not protobuf descriptors — those
are embedded as data, not symbols. Light obfuscation (string
encryption, packing) will defeat protoscan if the descriptors are not
present in plaintext. In practice, most Go and Java gRPC binaries
embed descriptors in plaintext for reflection support.

**Q: What about binaries that use gRPC reflection instead of embedded descriptors?**
protoscan works on the binary file itself, not on a live service.
For a live service that exposes the gRPC reflection API, you would use
`grpc_cli` or `grpcurl --list` to retrieve the descriptors — then feed
the resulting descriptor set to prototext or protolens. reproto is
not needed in that case.

---

### On protobuf internals

**Q: What exactly is a non-canonical encoding? Is it part of the spec?**
The protobuf wire format allows multiple encodings of the same value —
most notably, varints can be padded with extra continuation bytes
(e.g., the value 1 encoded as `0x81 0x80 0x80 0x80 0x00` instead of
`0x01`). This is legal per the spec: decoders must accept it. It is
just unusual in production — stock encoders never produce it. So
seeing it is a signal that the encoder was modified deliberately.

**Q: Why does protoc silently drop the shadowed field? Is that a bug?**
No — it is specified behaviour. The protobuf spec says that for a
singular field appearing multiple times on the wire, the last value
wins. protoc's `--decode` implements the spec. The information loss
is real but intentional from the spec's perspective. protolens shows
both instances because it reads the wire directly without applying the
singular-last-wins rule.

**Q: Could the shadowed field be accidental — a client retry, for example?**
Possible in theory. A retry that reuses the same stream could produce
duplicate field tags. In this demo the shadowed field contains a
`google.rpc.Status` with an API key — that content makes the accidental
explanation very unlikely. The combination of non-canonical varint
encoding elsewhere and a credential tucked into a normally-invisible
field suggests deliberate construction.

**Q: How does the scoring work? What is the Hopcroft graph?**
The score is a weighted sum over matched fields, minus penalties for
encoding anomalies (non-canonical varints, overhanging bytes, unknown
fields, wire-type mismatches). The Hopcroft graph is a bisimulation
structure built from the descriptor set: it encodes which message types
are structurally compatible with each other, so the scorer can
efficiently navigate through nested types without trying every
combination. The graph is built once by reproto and stored in
`hopcroft.rkyv`.

**Q: What does a negative score mean? Is the type wrong?**
Not necessarily. The score measures how well the bytes fit the schema,
penalising every anomaly. A negative score means the penalties
outweigh the matched-field evidence. In the demo, `SearchTextRequest`
scores −8 because of the repeated singular field and the non-canonical
encoding. The type is still correct — those anomalies are in the data,
not in the inference.

---

### On the scenario

**Q: Is the scenario realistic? Do real apps actually do this?**
The specific combination — credential in a shadowed field, non-canonical
encoding — is fictional and constructed for the demo. The underlying
techniques are individually documented: varint padding has been used to
evade length-based detection, and shadowed fields are a known blind
spot of standard parsers. Whether a real app uses them in combination
is an open question.

**Q: Could this be used offensively?**
The tools are read-only analysers — they decode and annotate, they
do not forge or inject. The knowledge they surface (non-canonical
encoding is possible, shadowed fields are invisible to standard parsers)
is already public and in the protobuf spec. The defensive value of
being able to detect these patterns outweighs the marginal uplift to
an attacker who already knows the spec.

**Q: Why not just use Wireshark / gRPCurl / Postman?**
Those tools require either a live service (gRPCurl, Postman) or a
known schema in advance (Wireshark with a protobuf dissector). The
scenario starts with no schema, no live service, and a binary blob.
prototools works from the artifact itself.

---

### On S3NS and the broader context

**Q: Is prototools used in production at S3NS?**
Yes — it is part of our binary inspection pipeline for software updates
arriving from Google. The audit workflow shown in the demo is a
simplified version of what runs in practice.

**Q: Will you publish the schema databases you build for Google's binaries?**
No. The decompiled `.proto` files and schema databases are internal
work product. The tools are open source; the corpora are not.
