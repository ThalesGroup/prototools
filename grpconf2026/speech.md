<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Speech text — grpconf 2026 demo (20 min)

Read this aloud before the talk to warm up and calibrate pace.
It is not meant to be read verbatim on stage — treat it as a rehearsal script.

Pronunciation notes are marked **[PRON: ...]** inline.
Timing is cumulative wall-clock from the start of the talk.

---

## Pronunciation quick-reference

Words that trip up French native speakers:

| Word | Sounds like | Trap for French speakers |
|------|-------------|--------------------------|
| **binary** | BY-nuh-ree | not "bih-NAY-ree" |
| **canonical** | kuh-NON-ih-kul | not "ca-NO-ni-CAL" |
| **heuristic** | hyoo-RIS-tik | not "oo-ris-TEEK" |
| **schema** | SKEE-muh | not "shay-MAH" |
| **glyph** | glif (rhymes with "cliff") | not "gleef" |
| **caret** | KAIR-et | not "ca-RAY" (that's "carré") |
| **varint** | VAIR-int | not "var-AHN" |
| **descriptor** | dih-SCRIP-ter | stress the second syllable |
| **infer / inference** | IN-fer / IN-fuh-runss | not "een-FAIR" |
| **routing** | ROO-ting | not "roo-TAHNG" |
| **quarantine** | KWOR-un-teen | not "ka-ran-TEEN" |
| **autonomous** | aw-TON-uh-muss | not "oh-to-NO-m" |
| **ubiquitous** | yoo-BIK-wih-tuss | not "u-bi-KWI-toose" |
| **protobufs** | PRO-toh-buffs | stress on "PRO" |
| **Places** | PLAY-siz | not "plah-SESS" |
| **decompile** | dee-kum-PYL | not "de-kom-PEEL" |
| **opaque** | oh-PAYK | not "o-PAK" |

---

## Section 1 — S3NS  `[00:00 – 01:00]`

*(Screen shows: `header "S3NS"`)*

Good morning everyone. I'm going to show you something that lives at the
intersection of security, infrastructure, and open-source tooling.

A bit of context first. S3NS **[PRON: "ess-trois-enn-ess"]** is a joint venture
between Thales and Google. We operate a Google Cloud region as a Trusted Cloud
service for European customers — our platform is called PREMI3NS
**[PRON: "pray-mee-trois-enn-ess"]**.

The platform is hosted in French data centers and operated entirely by French
personnel, autonomously **[PRON: aw-TON-uh-muss-lee]** — which means Google
software updates reach us, but they don't deploy themselves. We inspect every
update before it reaches production: static analysis, dynamic assessment in an
isolated quarantine **[PRON: KWOR-un-teen]** environment.

As part of those inspection activities, we built tooling to audit protobufs
**[PRON: PRO-toh-buffs]**, which are ubiquitous **[PRON: yoo-BIK-wih-tuss]**
in the Google infrastructure.

---

## Section 2 — prototools  `[01:00 – 02:00]`

*(Screen shows: `header "prototools"`)*

The result is prototools — three command-line interfaces and one text-based
user interface for working with protobufs, including for reverse-engineering.

The CLIs:
- **protoscan** extracts FileDescriptorProto **[PRON: "file-descriptor-proto"]**
  descriptors from binaries **[PRON: BY-nuh-reez]**.
- **reproto** decompiles and analyzes descriptor sets.
- **prototext** does lossless serialization and deserialization
  **[PRON: dee-see-ree-uh-lih-ZAY-shun]** with schema **[PRON: SKEE-muh]**
  inference **[PRON: IN-fuh-runss]**.

And the TUI — **protolens** — which is prototext on steroids: the interactive
version.

It's all open source, MIT license, on GitHub under ThalesGroup.

We'll demonstrate prototools through a short fictional scenario.

---

## Section 3 — The stage  `[02:00 – 03:00]`

*(Screen shows: `header "The stage"`, then `ls -lh bob`)*

Bob downloaded an unknown executable and an associated log file. The
executable answers routing **[PRON: ROO-ting]** questions by calling some
external service. Bob captured one of its network calls.

Three files: the executable, the log file, and the network capture.

Bob suspects gRPC calls and protobuf logs — he hands the whole lot to Alice
for analysis. Let's go.

---

## Section 4 — protoc falls short  `[03:00 – 04:00]`

*(Screen shows: `header "2. protoc falls short"`)*

Alice's first reflex: `protoc --decode_raw`. This is the standard tool everyone
reaches for.

*(runs the command on bob/capture)*

This is protobuf, alright — the structure is there. But field numbers without
names mean nothing. We can see that field 1 holds some text, field 7 holds a
submessage — but what do they represent? We don't know yet.

Let's try the log file:

*(runs on bob/logfile)*

Outright failure. protoc can't even parse it. Not a great start.

---

## Section 5 — Descriptors to the rescue  `[04:00 – 05:30]`

*(Screen shows: `header "3. If only we had descriptors"`)*

Can we find descriptors **[PRON: dih-SCRIP-terz]** in the binary **[PRON: BY-nuh-ree]** itself?

*(runs protoscan bob/app)*

Yes. bob/app contains reflected descriptors — and they look like a subset of
the Google APIs. Interesting.

Let's extract, decompile **[PRON: dee-kum-PYL]**, and index them with reproto.

*(runs reproto)*

reproto delivered. We now have a descriptor set, a type-inference
**[PRON: IN-fuh-runss]** graph, a fast-access index, and decompiled `.proto`
source files.

Let's glance at the decompiled places_service.proto — confirmed, this is from
the Google Maps API.

---

## Section 6 — Type inference on bob/capture  `[05:30 – 09:30]`

*(Screen shows: `header "4. Descriptors to the help"`)*

Now prototext can infer the type of the capture. One match:
`google.maps.places.v1.SearchTextRequest`. The score is negative — we'll come
back to that.

Given the type, protoc will now happily decode the capture. But protolens
shows more.

*(launches protolens on bob/capture with the capture script)*

---

### capture — step 1  `[05:50]`

protolens opened the capture without being given a type name, and found one
anyway: SearchTextRequest. What you see is prototext format — textproto
enriched with `#@` annotations.

---

### capture — step 2  `[06:30]`

Hover over "SearchTextRequest". A score breakdown tooltip appears. Note the
twenty-point penalty for "non-canonical **[PRON: kuh-NON-ih-kul]** encoding" —
the binary doesn't quite conform to the protobuf specification.

---

### capture — step 3  `[07:00]`

The culprit: `text_query`. The schema **[PRON: SKEE-muh]** declares it
singular — it should appear at most once. But it appears twice at wire level.
The first instance is shadowed by the second. Most tools, including protoc,
silently drop it.

---

### capture — step 4  `[07:20]`

protolens flags the repeated instance with an amber diamond and a
`repeated_singular` annotation.

---

### capture — step 5 & 6  `[07:40]`

The dropped instance is also flagged — with a hollow amber diamond, and a
`shadowed_scalar` annotation. Nothing is hidden.

---

### capture — step 7  `[08:00]`

A quick note on protolens basics before we move on: navigate with arrow keys,
hover over any text for more information, fold and unfold nodes by clicking
their triangle.

Fold `location_bias` to step — I'll do that now.

---

### capture — step 8  `[08:30]`

So bob/capture is a SearchTextRequest with a non-canonical encoding. Let's
move on to the log file.

*(presses Enter, protolens quits)*

---

## Section 7 — The log file against app.desc  `[09:30 – 13:30]`

*(Screen shows: `header "5. On to the log file"`, then protolens launches)*

---

### app.desc — step 1  `[09:50]`

The log file, opened against the same descriptor set. Larger than the capture.
The root type is unknown — app.desc has no schema **[PRON: SKEE-muh]** for the
envelope. What we see is raw field numbers.

Let me fold to get a cleaner view.

---

### app.desc — step 2  `[10:10]`

Better. Note the `TRUNCATED_MESSAGE` annotation — the logging process was
probably killed mid-write, with over a kilobyte missing. That's why protoc
bailed out entirely.

Each top-level field holds a similar submessage — these look like log entries,
one per RPC call.

---

### app.desc — step 3  `[10:30]`

The prefilled command below declares a synthetic type "Entry", marks field 1
as repeated, and names it "entries". I'll commit that now.

*(presses Enter)*

---

### app.desc — step 4  `[10:50]`

Entry is in place. Now let's enable heat cues. protolens will highlight
fields where it recognizes known types — the brighter the cue, the more
confident.

*(presses `i`)*

---

### app.desc — step 5  `[11:10]`

Field 5 of the first entry has a bright heat cue. Hovering over it gives two
candidates — SearchTextResponse matches what we saw in the capture.

I'll double-click the heat cue, then double-click SearchTextResponse to lock
it in.

---

### app.desc — step 6  `[11:35]`

SearchTextResponse locked in. Field 4 has an amber cue — one candidate:
SearchTextRequest. Makes sense, a request-response pair. I'll commit that.

---

### app.desc — step 7  `[11:55]`

Field 2 is a string that looks like an RPC method name. Let's just rename it
"rpc". Committing.

---

### app.desc — step 8  `[12:10]`

Field 1: hovering the heat cue suggests Timestamp — fits a log entry perfectly.
I'll press `t`, choose Timestamp, and commit.

---

### app.desc — step 9  `[12:30]`

Now look at the third entry — fields 6 and 7. Field 6 scores deeply negative
and the heat cues give nothing convincing. app.desc only knows the Places API.
Fields 6 and 7 look like a different service entirely. We'll need a broader
descriptor set.

---

### app.desc — step 10  `[12:50]`

Before going further, let's save our overrides. We don't want to redo this
work.

*(presses Enter to save)*

---

### app.desc — step 11  `[13:10]`

Done. Quitting.

*(screen returns to shell)*

The googleapis descriptor set covers the full Google API surface — about
twenty-five megabytes, eight thousand files. Let's reopen against it.

---

## Section 8 — The log file against googleapis  `[13:30 – 17:30]`

*(launches protolens with googleapis + saved overrides)*

---

### googleapis — step 1  `[13:50]`

The log file again — this time against the full googleapis descriptor set. Our
overrides are back. Let me unfold to see what changes.

---

### googleapis — step 2  `[14:10]`

Fields 6 and 7 now have bright cues — googleapis knows them. Field 7:
hovering suggests ComputeRoutesResponse. The app also calls the Routes API.
I'll commit that.

---

### googleapis — step 3  `[14:30]`

Field 6: ComputeRoutesRequest. Symmetric. Committing.

---

### googleapis — step 4  `[14:50]`

The log is fully typed. But those amber cues are still there — something is
wrong with the encoding. Let me unfold `compute_routes_request` to look closer.

---

### googleapis — step 5  `[15:10]`

`travel_mode`: amber diamond plus an `ohb` annotation. `ohb` stands for
OverHangingBytes **[PRON: OH-ver-HANG-ing bytes]** — the DRIVE value,
which is just the integer 1, is encoded with too many bytes. Five bytes where
one would do.

Weird.

---

### googleapis — step 6  `[15:30]`

I'll press `w` to open the wire-level detail. Five bytes for the value 1.
The canonical **[PRON: kuh-NON-ih-kul]** encoding would be a single byte,
`0x01`. This is almost certainly not accidental.

Closing the wire view.

---

### googleapis — step 7  `[15:50]`

One more thing. Remember the shadowed field in the SearchTextRequest? Let me
go back to the very first search_text_request at the top of the document and
unfold it.

---

### googleapis — step 8  `[16:10]`

Good. Now I'll hover the heat cue at the left edge of the shadowed node.
protolens thinks this binary blob might be a `google.rpc.Status` message.
I'll commit that override.

---

### googleapis — step 9  `[16:35]`

The shadowed field is a `google.rpc.Status` — and it contains an API key.

That's not something you want to see in a network capture.

---

### googleapis — step 10  `[16:55]`

Let's navigate back to root and export the annotated log file as prototext, so
we can send our findings to Bob.

---

### googleapis — step 11  `[17:10]`

`x`, `p` — export as prototext. Saving under `alice/`.

---

### googleapis — step 12  `[17:25]`

Done. Quitting.

---

## Section 9 — Reporting to Bob  `[17:30 – 18:30]`

*(Screen shows: `header "6. Reporting to Bob"`)*

The exported prototext preserves everything we uncovered — the shadowed field,
the overhanging bytes, all the annotations. Nothing is lost.

And it round-trips **[PRON: ROWND-trips]** faithfully: we can re-encode it and
compare byte-for-byte with the original log file.

*(runs cmp command, prints "Identical")*

When Bob sees this report, I suspect he will uninstall the app.

---

## Section 10 — Conclusion  `[18:30 – 19:30]`

*(Screen shows: `header "7. Conclusion"`)*

Three takeaways:

First — descriptors are often hiding in the binary **[PRON: BY-nuh-ree]**
itself. Don't assume a protobuf is opaque **[PRON: oh-PAYK]** just because you
don't have the `.proto` files.

Second — protobuf is not opaque if you have the right tools.

Third — prototools is open source, MIT license. Pull requests welcome.

Thank you.

*(bow, pause for applause)*

---

## Appendix A — Scaling  `[only if time permits, ~19:30+]`

*(Screen shows: `header "A. Performance and scaling"`)*

If we have a moment — bob/capture and bob/logfile are small protobufs.
protolens handles large ones just as well. Here's googleapis.desc opened
against itself: twenty-five megabytes of descriptor data, fully navigable.
Navigation stays fluid and startup latency is short.

---

## Timing summary

| Wall clock | Event |
|------------|-------|
| 0:00 | Start — S3NS intro |
| 1:00 | prototools overview |
| 2:00 | The stage — Bob & Alice setup |
| 3:00 | protoc --decode_raw |
| 4:00 | protoscan / reproto |
| 5:30 | Launch protolens on capture |
| 5:50 | capture step 1 — SearchTextRequest |
| 6:30 | capture step 2 — score tooltip |
| 7:00 | capture step 3 — shadowed field |
| 7:20 | capture step 4 — repeated_singular |
| 7:40 | capture step 5/6 — shadowed_scalar |
| 8:00 | capture step 7 — basics / fold |
| 8:30 | capture step 8 — done |
| 9:30 | Launch protolens on logfile (app.desc) |
| 9:50 | app.desc step 1 — raw field numbers |
| 10:10 | app.desc step 2 — TRUNCATED_MESSAGE |
| 10:30 | app.desc step 3 — declare Entry type |
| 10:50 | app.desc step 4 — enable heat cues |
| 11:10 | app.desc step 5 — SearchTextResponse |
| 11:35 | app.desc step 6 — SearchTextRequest |
| 11:55 | app.desc step 7 — rename rpc |
| 12:10 | app.desc step 8 — Timestamp |
| 12:30 | app.desc step 9 — fields 6/7 unknown |
| 12:50 | app.desc step 10 — save overrides |
| 13:10 | app.desc step 11 — quit |
| 13:30 | Launch protolens on logfile (googleapis) |
| 13:50 | googleapis step 1 — unfold |
| 14:10 | googleapis step 2 — ComputeRoutesResponse |
| 14:30 | googleapis step 3 — ComputeRoutesRequest |
| 14:50 | googleapis step 4 — amber cues remain |
| 15:10 | googleapis step 5 — ohb annotation |
| 15:30 | googleapis step 6 — wire view |
| 15:50 | googleapis step 7 — navigate to shadowed field |
| 16:10 | googleapis step 8 — google.rpc.Status |
| 16:35 | googleapis step 9 — API key exposed |
| 16:55 | googleapis step 10 — navigate to root |
| 17:10 | googleapis step 11 — export prototext |
| 17:25 | googleapis step 12 — quit |
| 17:30 | Reporting — round-trip check |
| 18:30 | Conclusion — three takeaways |
| 19:30 | Thank you / applause |
| 19:30+ | Appendix A (scaling) if time permits |
