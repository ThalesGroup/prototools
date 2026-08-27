<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# What's really in that `.pb` file?

**gRPConf 2026 North America — 20 minutes, live terminal.**
Synopsis, draft 3, **written 2026-08-17 against the built artifacts**.
Not yet rehearsed; timings are budgets, not measurements. Every score
quoted below was measured on the real files on that date, not estimated.

This document has two readers. The first half is what a participant
gets handed before the talk: the story, the artifacts, and what each
beat is meant to prove. The second half is the implementation
reference — which layer drives which keystroke, and what has to be
built before any of it runs.

Related: `docs/grpconf2026-abstract.md` (what was advertised),
`docs/grpconf2026-demo-plan.md` (the framing decisions this synopsis
is written against — read it before changing a beat),
`grpconf/artifacts.md` (how the artifacts get built).

**Draft 3 changed the spine.** Draft 2 escalated from a single recovered
schema database to the full googleapis corpus, and googleapis carried
three payoffs. It now carries none of them. Bob sends **two builds** of
his app — the one he had, and the newer one he grabbed when it started
misbehaving — and the escalation runs between *those two*. Everything the
demo reads is derived from files Bob sent. googleapis survives as a
one-minute epilogue about scale, which is the only claim it was ever
uniquely qualified to make.

Draft 3 also had to absorb a measured fact that killed a beat: since
spec 0314 the tooling **can** name boblog's root. Draft 2's beat 9 opened
on `<raw / no type>` and that no longer happens. See "What changed from
draft 2" at the end.

---

## Executive summary

**The room is handed somebody else's problem, which is the point.**
Bob downloaded an executable off the net. It answers questions about
driving distances between cities, and it clearly does so by calling
some external service. He played with it, kept a log file it wrote, and
took a capture of one of its calls. At some point it started behaving
oddly, so he downloaded a newer build too. He thinks there are protobufs
in there somewhere. He does not have a `.proto`, a type name, or any
idea what the thing is talking to. He hands **four files** to Alice, and
Alice is the person the talk is addressed to.

**The first three beats are all failures, and they are cheap.** The
standard decoder wants a schema path, a schema file and a type name;
Bob supplied none of them, and asked anyway it does not say "wrong
type", it says nothing useful at all. A raw wire dump then supplies
the opposite failure: every byte accounted for, every field numbered,
not one name. `prototext --raw` adds a third shade — the same
structure, but now with the *non-canonical* bytes called out, which is
the first hint that this producer is not a stock encoder. Two minutes
buys the premise, and the premise is that the bytes were never the
hard part.

**The schema was inside the executables the whole time.** An
application that speaks protobuf carries descriptors, because it needs
them at runtime. So an executable is not a dead end, it is a source:
protoscan lists 41 `.proto` files sitting in the old build — and the
names alone answer Bob's question, because they say
`google/maps/routing/v2/`. Bob's app calls the Google Routes API. Then
the same command on the newer build lists **77**, and a three-line diff
of the two listings is the whole second half of the talk in advance:
the newer build learned Places v1, Routes **v1**, and
`google/rpc/error_details.proto`. Then reproto reads each binary
*directly*, with no extraction step, and emits compilable `.proto`
source plus an indexed scoring database in one command. Recovery does
not stop at a descriptor either: what comes back is edition 2023, which
the room's own toolchain cannot compile, so the same command re-emits
it as proto2 — same wire format, syntax that builds today.

**Now the capture reads, and the score shows its work.** Against a
database built from the old binary sixty seconds ago, protolens names
bobshark's type without being told anything —
`google.maps.routing.v2.ComputeRoutesRequest` at **−16**, with the
runner-up at **−55**. What is worth more than the name is the breakdown
beside it: `unknown: 1`, `non_canonical: 1`. Two of the four anomalies
are *counted* before the file has been opened, and they are why the
winning score is still negative. The tool is not claiming a fit. It is
saying this is the best type it has and it does not completely explain
these bytes.

**The log opens named, and says out loud that it is broken.** boblog is
not one message, it is Bob's app's own log format — and the database
recovered from Bob's app names it: `bobapp.v1.Log`, **+19**, 24 fields
matched, nothing unknown, nothing mismatched, and one more line:
`truncated: true`. The last record was cut 1 024 bytes short because the
process died mid-write, and the tool reports that as a *property of the
answer* rather than as a refusal. That is the whole argument of the talk
in one line of YAML. Inside, the split: the two Routes exchanges open in
the clear — a request at −16 and a 9 868-byte reply at **+651** — and the
two Places exchanges are junk, scoring −26 and −50 against a schema that
has never heard of them. And they say so themselves, one line up:
`method: "/google.maps.places.v1.Places/SearchText"`. **The document
names the schema it is missing.**

**Which is the newer build's file list, and the room already saw it.**
Same log, same tool, one command apart: under the database recovered
from bobapp2 the Places request is `SearchTextRequest` at −8 and its
reply is **+45**, and the debug blob nobody could place is
`google.rpc.Status` at **+8** — sole, where the smaller database could
only offer a three-way tie at +3. The five points are not a rounding
difference: they are the five fields the tool can now read *inside* the
`google.protobuf.Any` that Status carries, because the newer build
embedded `error_details.proto`. **More schema, more the tool can say,
measured to the point.** And inside that Any is a
`metadata { key: "x-goog-api-key" }` — Bob's own credential, written by
Bob's own app into Bob's own log.

**And the bigger dictionary does not only help.** The newer build
carries *both* published versions of the Routes API, so the route
request that was unambiguous a minute ago now **ties**, at −16, between
`routes.v1` and `routing.v2`. Those two render it identically, field for
field; v1's field numbers are a strict subset of v2's, so no payload can
separate them. What separates them is a string in the envelope one line
up. The presenter reads it and picks by hand, out loud, because a
flawless demo reads as a recording and work shown is proof. Inference
ranks. Then it defers.

**Four things are on that wire that standard tooling will not show
you,** and in this frame they are not curiosities, they are the
finding. A singular field that occurs twice — legal wire,
last-one-wins, so a decoder shows the second value and silently drops
the first, and the first one is the debug Status carrying Bob's API key.
A varint padded past minimal length — same number, different bytes, and
nothing downstream notices. A field the recovered schema does not
declare, read by shape because the wire type is in the tag. And a record
truncated at the tail, which is the input the standard decoder refuses
outright. This is the section that is deliberately slow: the display
takes 200 ms and nobody absorbs it in 200 ms.

**The finale is what Alice sends back to Bob.** The annotated document
is written out as prototext — a plain text file, readable in any
editor, with every anomaly called out inline. Then it is re-encoded
and compared against the log Bob handed over: identical. Not a
checksum for its own sake. The leaked key survived the round trip. It
would not have survived `protoc --decode`.

**Then one minute of epilogue, because somebody is going to ask.** The
same log against the whole of googleapis — 7 771 files, 58 777 types,
25.6 MB, indexed once. It opens in 39 ms instead of 6, which answers
*does this scale*. And it reads **less** of the file than the 127 kB
database did, because Google has never heard of Bob's app. Two hundred
times the dictionary, and the envelope goes dark. More schema is not
more knowledge; the *right* schema is, and Bob was carrying it.

**What the demo argues, in one line.** A message you cannot name, in a
format you cannot see all of, produced by a program whose source you
do not have, is still fully readable — and reading it honestly means
showing what is there rather than what a schema expected.

## Executive summary, spoken

Here is what you are going to see.

A friend of mine — call him Bob — downloaded a program off the
internet. It tells you how far it is to drive between two cities, and
it is obviously asking someone else, over the network, to work that
out. Bob poked at it for an afternoon. He kept the log file it writes.
He took a packet capture of one of its calls. And when it started
misbehaving he downloaded the newer build as well and kept that too.
Then he sent me four files and a sentence: *I think this thing speaks
protobuf, can you tell me what it's doing?*

No `.proto`. No type name. No documentation. No idea who it is
talking to. Everything else that appears on this screen in the next
twenty minutes, I am going to derive from those four files, in front
of you.

We start where you would start, and we fail three times. The standard
decoder wants three things from me: where the schema lives, which file
it is in, and what type this message is. Bob gave me none of them. And
when I guess a type name anyway, it does not tell me the type was
wrong — it tells me nothing at all. So we drop to the bytes, and the
bytes are perfect: every field numbered, every length accounted for,
nothing missing. And completely meaningless. And then one more look at
the same bytes, which tells me something small and interesting:
whoever wrote this was not using a stock encoder.

That is the shape of the problem. The bytes were never the hard part.

Now — the schema was never actually missing. Bob's program has to
build these messages, so it has to carry descriptors. They are sitting
inside the executable. So I do not extract them and hope the pieces
fit: I read the binary as if it were a schema directory. Forty-one
`.proto` files come out of the old build, and the file *names* already
answer Bob's question, because they all start `google/maps/routing/v2`.
Bob's mystery app is calling the Google Routes API.

Then I run the same command on the newer build, and I get seventy-seven.
Hold on to the difference, because it is the second half of this talk:
the newer one learned three things, and one of them is a second Google
service Bob never mentioned.

One command turns either of those into compilable source and an indexed
schema database. And I do not stop at a descriptor, because a descriptor
is not something you can build against. What comes back is edition 2023,
and if your toolchain is like mine, it cannot compile that. So the same
command emits proto2 instead: identical on the wire, and it builds
today.

Then Bob's capture. I still do not know what it is, so I ask the
database we just built: which of your types does this look like? It
answers immediately, and it opens on the winner. And the winner is
right — nothing else is close.

But look at the number next to it, because it is negative. The tool has
docked its own best answer twice: once for a field that no schema
declares, and once for a number written in more bytes than it needs. It
is telling me, before I have opened the file, that the best type it has
does not completely explain these bytes. Hold on to that. It is two of
the four things I came here to show you, and they arrived on their own.

Now the log, which is the file Bob was actually worried about. This is
not one message; it is Bob's app's own log format — and the app's own
descriptors name it. Nineteen points, twenty-four fields matched,
nothing mismatched. And one more line: *truncated*. The end of the file
is chopped off, the process died mid-write, and the tool tells me that
as part of the answer instead of throwing the answer away. That
distinction is most of what I want you to take home.

Half the log opens. The routes are there in the clear — one reply scores
six hundred and fifty-one, which is what an easy answer looks like. The
other half is junk: minus twenty-six, minus fifty, a schema guessing at
bytes it has never seen. And the log tells me why, one line above the
junk. It says the method name. It says `places.v1`. This database has
never heard of Places.

But we know who has. It was in the file listing from the newer build,
ten minutes ago. So I swap the database — same file, same tool, one
command apart — and the junk turns into a place-search request and a
reply with five coffee shops in it. Bob's app does not just route. It
geocodes, and it sends a place name and a coordinate to a second Google
service on every run.

And then the thing I actually came here for. There is a field in that
request declared as a single string, and it occurs twice. That is legal.
Last one wins. Your decoder shows you the second value and silently
drops the first. The second value is `coffee in Grenoble`. The first
value is a hundred and sixty-four bytes that are not a string at all,
and the tool says so: it is a `google.rpc.Status`, and inside it is an
`Any`, and inside *that* — because the newer build happened to embed one
more file — is a key called `x-goog-api-key` and Bob's API key next to
it. This program writes his credential into his own log, in a place a
normal decoder will never print.

I want to point at one number while that is on screen. Under the older
database that blob scored three, in a three-way tie, and it stayed
opaque. Under the newer one it scores eight, on its own. The five points
are the five fields the tool can now read inside that `Any`. That is
what a schema buys you, priced.

The bigger dictionary also takes something away, and I want you to see
that too. The newer build ships two versions of the Routes API, so the
request that was unambiguous a minute ago now has two answers with the
identical score, and they render these bytes identically, field for
field. That is not the tool being confused. Those two schemas genuinely
say the same thing here, and nothing about the wire will separate them.
What separates them is a string one line up in the log. So I read it,
and I pick, by hand, while you watch. That is the point. It ranks. Then
it defers to me.

Now I put the raw bytes underneath every line, and we go through all
four things slowly. The duplicate field and the key. A number encoded
in more bytes than it needs; it decodes to the same value, re-encodes
to different bytes, and nothing downstream ever notices. A field the
schema does not declare at all, which we can still read, by shape,
because the wire type is in the tag. And the record at the end that is
cut off mid-length, where I will show you why the length prefix keeps
the damage down to one entry.

I will go slowly there. The screen draws in a fifth of a second, and
nobody reads it in a fifth of a second.

Then I write the whole thing back out as text — an ordinary file, with
every one of those findings annotated inline, which is what I would
actually send back to Bob. And then I re-encode that text and compare
it to the file Bob gave me. They are identical. Not as a party trick —
I want you to notice what survived that round trip. The leaked key is
still there. It would not have been, going through the decoder we
started with.

One last thing, because somebody always asks whether this works at real
scale. Here is the same log against every public Google API — seven
thousand seven hundred files, fifty-eight thousand types, twenty-five
megabytes. It opens in thirty-nine milliseconds instead of six. And it
reads *less* of this file than the hundred-and-twenty-kilobyte database
did, because Google has never heard of Bob's app. Two hundred times the
dictionary and the envelope goes dark. More schema is not more
knowledge. The right schema is — and Bob had been carrying it around in
his Downloads folder the whole time.

So: a log you cannot name, in a schema you do not have, written by a
program whose source nobody has. It is still completely readable. And
reading it honestly means showing you what is on the wire — not what a
schema expected to find there.

## The story, in one paragraph

Bob downloads an executable off the net. It answers questions about
distances between cities by calling an external service, and he thinks
it speaks protobuf. He keeps the log it writes, takes a packet capture
of one call, and — when the thing starts misbehaving — downloads the
newer build as well. He hands all four to Alice with no `.proto`, no
type name and no documentation. Over twenty minutes, everything else
in the demo is derived from those four files on stage: the descriptors
are recovered from both executables, `.proto` source and two indexed
schema databases are reconstructed from them, the capture's type is
inferred, the log's opaque payloads are identified by escalating from
the older schema to the newer one (and one ambiguity the newer one
introduces is settled by hand), and the whole thing is read down to the
byte — including four things `protoc --decode` will not show you, one of
which is Bob's own API key.

That is a situation most of the room has been in. The last part is the
one they have been in without knowing.

## The four artifacts

Everything starts from exactly four files, and they are named
distinctly on screen throughout. Conflating them is the failure mode:
if a payload were itself a descriptor, its type would already be known
and there would be nothing to infer.

| On screen | What it is | Size | Which promise it serves |
|---|---|---|---|
| `bobapp1` | The build Bob had. A real gRPC client of the Google Routes API; descriptors embedded uncompressed. No source, no `.proto`. | 6 339 248 B, **41** embedded `.proto` | descriptor archaeology |
| `bobapp2` | The build he downloaded when it started misbehaving. Same app; three more APIs compiled in. | 6 392 496 B, **77** embedded `.proto` | the escalation |
| `bobshark` | One request body, lifted out of Bob's capture. Type unknown. | 84 B | schema inference |
| `boblog` | The log the app wrote. An envelope the recovered schema describes, holding payloads the *older* one mostly does not. Carries the four anomalies and a truncated tail. | 20 243 B, 4 entries | lossless decoding, heat cues, the escalation |

The split of labor between the two payloads is deliberate:

- **bobshark is one message.** It exists so inference has a clean,
  single-type target, uncomplicated by an envelope.
- **boblog is a container.** It exists so the demo can show a document
  that is *partly* readable, which is the honest steady state of this
  kind of work and the only way to motivate the escalation.

`boblog`'s four entries are two Places exchanges then two Routes
exchanges. Measured field sizes, which the beats quote:

| entry | method | request | response |
|---|---|---|---|
| 1 | `places.v1.Places/SearchText` | 240 B | 508 B |
| 2 | `places.v1.Places/SearchText` | 238 B | 484 B |
| 3 | `routing.v2.Routes/ComputeRoutes` | 84 B — byte-identical to `bobshark` | 9 868 B |
| 4 | `routing.v2.Routes/ComputeRoutes` | 84 B | cut: `TRUNCATED_MESSAGE; MISSING: 1024` |

It carries four deliberate anomalies, chosen because each one is
invisible to standard tooling in a different way, and because each one
falls out of the story rather than being sprinkled on:

1. **A singular field that occurs twice, the first value being a whole
   message hiding in a `string`.** Legal wire; last-one-wins.
   `protoc --decode` shows you the second `text_query`, the place name
   that was really searched for, and stops. The first is a debug trace
   the app left in front of it — a `google.rpc.Status` carrying the
   `x-goog-api-key` it authenticated with, which it logs and should
   not. It reads, in full, as:

   ```
   code: 13
   message: "debug: outbound call authenticated"
   details {
    type_url: "type.googleapis.com/google.rpc.ErrorInfo"
    value {
     reason: "DEBUG_TRACE"
     domain: "bobapp"
     metadata { key: "x-goog-api-key"  value: "AIzaSyB0b5REK…" }
    }
   }
   ```

2. **A non-minimal varint** (`val_ohb`). `20 81 80 80 80 00` decodes to
   the same number a single byte would, re-encodes to different bytes,
   and nothing downstream notices.
3. **A field the recovered schema does not declare** — field 99,
   `"bobapp/0.9.3-rc2"`. Nothing is lost; the wire type is in the tag.
4. **A truncated length-delimited record at the tail.** The app was
   killed mid-write. The input `protoc` refuses.

Four is the budget. The full vocabulary — thirty annotation tokens,
every one of them — lives in `tests/fixtures/anomalies.pb`, which is a
repo pointer and a Q&A artifact, not a beat.

### Who writes them, and where they land

**bobapp writes them itself**, and the demo never explains how. Bob's
app is a little weird — that is why he sent it to Alice — and the room
needs to believe only that the bytes are real, not that they were
authored. The mechanism is a prototext round trip inside the encoder:
the message is encoded canonically, rendered to `#@ prototext` text,
patched *as text*, and encoded back. Nothing hand-rolls a varint, and
the patch reads as the anomaly it is (`val_ohb: 4`, a repeated line, a
bare field number). It sits between `item.encode` and `record_request`
in `codec.rs`, exactly where that file's doc comment already said such a
step would go.

Which anomaly lands where is fixed by what each beat has to show:

| # | Anomaly | Lands in | Why there |
|---|---|---|---|
| 2 | non-minimal varint (`val_ohb`) | the **route request** — so also in `bobshark` | beat 4 reads bobshark with `prototext` and needs `ohb` visible before protolens exists; beat 8's scorer then prices it |
| 3 | undeclared field 99 | the **route request** — so also in `bobshark` | same beat, and it is the one anomaly whose severity tier differs from 2's |
| 1 | duplicated singular field carrying the key | the **Places request**, entries 1 and 2 | the escalation's payoff: it is unreadable under `bobapp1.desc` and fully named under `bobapp2.desc` |
| 4 | truncated tail | the **log**, entry 4 | only a *file* can be cut short mid-write; a request that egressed was complete |

Anomaly 1's key is a **synthetic** string that looks like a Google API
key and is not one. The real key never reaches a committed artifact, and
the framing is carelessness — a downloaded utility logging a credential
where its user will not look — not exfiltration. That is both truer to
Bob and a smaller claim to have to defend from the stage.

## The two schema databases (and the epilogue's third)

The escalation is the spine of the second half, so the databases need
to stay as distinct on screen as the artifacts they came from. **Both
are built on stage, in one command each, out of a binary Bob sent.**

| On screen | Built from | Size | What it can name |
|---|---|---|---|
| `bobapp1.desc` | `reproto -I bobapp1 --schema-db-out` | 64 816 B, 41 files | boblog's envelope, and the Routes v2 traffic. Nothing Places. One version of the Routes API, so nothing in it is ambiguous. |
| `bobapp2.desc` | `reproto -I bobapp2 --schema-db-out` | 130 044 B, 77 files | all of the above, **plus** the Places traffic and the leaked `google.rpc.Status` down to the key inside its `Any`. Holds *both* Routes versions, so it is ambiguous where the small one was not. |
| `googleapis.desc` | the repo's own googleapis corpus, as CI builds it | 25 660 332 B, 7 771 files, 58 777 types | the Google traffic — and **not** the envelope. Epilogue only. |

**There is no merged set, and that is the point.** protolens takes a
single `--descriptor-set`, so "both" was never a mode it had. What would
have been a limitation is the demo's best teaching moment instead:

- Beat 9 opens the log under `bobapp1.desc`. The envelope is named
  outright, `bobapp.v1.Log` at +19. Inside, the Routes exchanges read
  and the Places exchanges are junk — and the `method:` string one line
  above the junk names the service the database is missing.
- Beat 10 opens the same log under `bobapp2.desc`. Everything reads.
  And the route request, unambiguous a minute ago, now ties.

**The escalation is not a strict improvement, and beat 10 says so.**
The bigger database buys names for the Places payloads and the leaked
Status, and simultaneously *introduces* a tie on Routes traffic that the
smaller one answered outright. That asymmetry is the most interesting
thing the two databases do, and it was discovered by measuring rather
than designed.

The epilogue pushes the same lever one notch too far on purpose:
googleapis buys nothing at all here and loses the envelope. Three data
points make a curve; two make a slogan.

### The measured escalation

Everything the beats claim, in one table. Scores are `prototext
list-schemas` on the real bytes, 2026-08-17.

| payload | `bobapp1.desc` | `bobapp2.desc` | `googleapis.desc` |
|---|---|---|---|
| boblog root, 20 243 B | **`bobapp.v1.Log` +19** (24 matched, `truncated: true`) | same, +19 | dark — a crowd at −2 |
| `bobshark` / route request, 84 B | **`routing.v2.ComputeRoutesRequest` −16**, next −55 | **tie −16**, `routes.v1` vs `routing.v2` | same tie |
| route response, 9 868 B | **`routing.v2.ComputeRoutesResponse` +651** | same | same |
| Places request, 240 B | `GeneratedCodeInfo.Annotation` **−26** — junk | **`places.v1.SearchTextRequest` −8** | same, −8 |
| Places response, 508 B | three-way junk at **−50** | **tie +45**, `SearchTextResponse` / `SearchNearbyResponse` | same tie |
| the leaked Status, 164 B | **+3, three-way tie** — `Status`, `ExtensionRangeOptions.Declaration`, `SourceCodeInfo.Location` | **`google.rpc.Status` +8, sole**; the `Any` expands | same, +8 |

The +3 → +8 step is the single most quotable number in the demo, and it
is exactly attributable: `prototext score --no-expand-any -t
google.rpc.Status` gives **3 matches, score 3**; with expansion it gives
**8 matches, score 8**. The five points are the five fields inside the
`google.protobuf.Any` — `reason`, `domain`, `metadata`, and the entry's
`key` and `value` — which only become readable because `bobapp2` embeds
`google/rpc/error_details.proto` and `bobapp1` does not. *More schema,
more the tool can say*, priced.

Open time on `boblog` (20 243 B), `protolens … quit`, wall clock with
process spawn included, mean of ten: **17 ms** under `bobapp1.desc`,
**17 ms** under `bobapp2.desc`, **82 ms** under `googleapis.desc`
(`protolens --version` alone costs 4 ms of that). That is the
epilogue's whole numeric content: two hundred times the dictionary
costs five times the wait, and the wait is a tenth of a second.

## The cast

Five tools, four visual vocabularies, exactly one new screen. That
constraint is what justifies a dense twenty minutes, and it is the
first thing to protect if a beat has to be rewritten.

| Surface | Appears | Orientation cost |
|---|---|---|
| the shell (`grpconf/prompt`) | throughout | none — it is a shell |
| `protoc`, `protoscope` | beats 2–3 | none, deliberately |
| `prototext` | beat 4, then the finale | none — flat text |
| `protoscan` | beat 5 | none — it prints a list of names |
| `reproto` | beat 6 | one sentence |
| neovim | beat 7, then again from inside protolens | none *if* set up first |
| **protolens** | beats 8–12 | **the one new screen** |

Two rules hold the lineup together: every tool appears in one
contiguous block, and no tool is returned to after it is left. The two
apparent exceptions are both deliberate, and both are payoffs rather
than violations — see "Two returns, on purpose" below.

---

## Beat by beat

Budgets total 18:20 against 20:00, which is 1:40 of slack — spent
entirely on beat 10, which is the one that will overrun. If more has to
go, beat 4 is the cheapest cut, beat 12 the next (it is an epilogue and
it is designed to be droppable), and beat 3 after that.

### 1 — The problem (slide) · 1:20

*This slide is the presenter's to write.* What it has to end on: Bob,
his four files, and the promise that everything else is derived from
them live. What it opens on — why we got interested in this, and why
the tools were open-sourced — is deliberately left blank here.

The presenter types the first shell command **by hand**, before the
prompter is engaged, so the room sees a real terminal.

```
ls -l bobapp1 bobapp2 boblog bobshark
```

The two binaries differ by 53 KB and nothing else visible. Say the
sentence that sets up beat 5 and then drop it: *I do not yet know what
the second one is for.*

### 2 — `protoc --decode` falls short · 0:40

```
protoc --decode ??? --proto_path ??? ??? < bobshark
```

There is nothing to put in any of the three placeholders. That is the
premise, and ten seconds of an error message buys it. Then, with a
type name guessed wrong, the second failure mode: it does not say
"wrong type", it says nothing useful at all.

**Claim:** the standard tool needs three things Bob does not have.

### 3 — protoscope: correct, complete, meaningless · 0:40

```
protoscope bobshark
protoscope boblog | head -30
```

Field numbers, wire types, values. Every byte accounted for. Not one
name. Run on both payloads, because the room needs to see that the log
is *also* protobuf and *also* opaque — that is what makes beat 9 a
payoff instead of a surprise.

**Do not explain this screen.** It is the one place where partial
comprehension is on-message: the beat exists to show that correct
bytes are not enough. Show it, name the problem, move.

**Claim:** the bytes were never the hard part.

### 4 — The notation, flat · 0:30

```
prototext decode --raw bobshark
```

Text, `#@` annotations, no panes, no caret, no colors to learn. The
same structure protoscope just printed, plus something protoscope did
not say: a couple of these bytes are **non-canonical**. No stock
encoder emits them. First hint that Bob's app is not a stock anything.

This beat exists so protolens has **one** new thing to teach instead
of two. It also makes an argument the finale will need: the format is
a plain text artifact that exists independently of its viewer.

**Claim:** this is a file format, not a UI.

### 5 — protoscan: the schema is in the executables · 1:00

```
protoscan bobapp1 | wc -l          # 41
protoscan bobapp1 | head -20
```

Forty-one `.proto` file names. bobapp1 has no symbols and no source,
but it has to *build* these messages at runtime, so the descriptors are
in there.

And then the beat's first payoff, which is free: **read the names.**
They say `google/maps/routing/v2/`. Bob asked what his app was talking
to, and the file listing already answered him, before a single byte was
decoded. One name is not Google's at all — `bobapp/v1/log.proto` — and
that is the format of the file he is worried about.

Then the second payoff, which is the whole second half of the talk
delivered in ten seconds:

```
protoscan bobapp2 | wc -l          # 77
diff <(protoscan bobapp1 | sort) <(protoscan bobapp2 | sort)
```

Thirty-six new files, and three of them are the story:
`google/maps/places/v1/places_service.proto`,
`google/maps/routes/v1/route_service.proto`, and
`google/rpc/error_details.proto`. Do not explain them yet. Say only:
*the newer build learned three things, and we are going to find out
why it needed each one.* The room will get there before Alice does,
which is the best kind of foreshadowing.

This beat **only reveals**. Nothing is extracted, no directory is
written, no file is opened.

Verified: protoscan prints **41** and **77**, i.e. every file of each
embedded set. On the `gh` binary (55 MB, stripped, modern Go) it prints
250 in 0.47 s; on `googleapis.desc` it finds all 7 771.

**Claim:** the schema was in the executable the whole time — and its
table of contents was the answer to Bob's question.

### 6 — reproto: the binary *is* the input · 2:00

No extraction step. reproto reads the executables themselves:

```
reproto -I bobapp1 -O src1/ --schema-db-out bobapp1.desc \
        google/maps/routing/v2/routes_service.proto \
        bobapp/v1/log.proto

reproto -I bobapp2 -O src2/ --schema-db-out bobapp2.desc \
        google/maps/routing/v2/routes_service.proto \
        google/maps/places/v1/places_service.proto \
        google/maps/routes/v1/route_service.proto \
        google/rpc/error_details.proto \
        bobapp/v1/log.proto
```

`-I` takes a **blob** and stands for the descriptors embedded in it —
one member per `FileDescriptorProto` found, which is exactly what
protoscan just listed (spec 0243). Imports resolve out of the same
binary. One command per build: stripped executable to compilable
`.proto` source *and* an indexed scoring database, no intermediate
directory, no temp files to explain.

Say the first point out loud: the usual pipeline here is *extract, then
hope the pieces fit together*. There is no extract.

The entry-point lists are the second command's whole difference, and
they are read off beat 5's diff. Do not narrate them line by line; say
*the three new ones, plus the two we already had* and move.

Then the part that earns the beat the rest of its time: the recovered
descriptor is **edition 2023**, and the room's Rust toolchain — prost,
prost-reflect — cannot compile editions syntax.

```
reproto -I bobapp1 -O proto2/ --force-proto2-output \
        google/maps/routing/v2/routes_service.proto
diff -u src1/.../routes_service.proto proto2/.../routes_service.proto
```

Same wire format, proto2 syntax, compiles today.

**Claim:** archaeology that ends at a descriptor is half a result —
and the descriptor never had to become a file.

### 7 — The editor (setup) · 0:40

```
nvim src1/google/maps/routing/v2/routes_service.proto
```

An ordinary `.proto` file. Any editor opens it — which is a real claim
about reproto's output, and worth saying out loud.

This beat is **setup, not inspection**, and its payoff is deferred all
the way to beat 10. protolens launches this same editor via `v`;
opening it here means the nested launch lands on a screen the room
already recognizes and reads as navigation rather than as "something
else opened". Same editor, same colorscheme, same font.

Do not spend the beat admiring the source. Spend it establishing that
*this file is now available to us*, because in beat 10 it stops being
a trophy and becomes the reference Alice consults before accepting a
type.

### 8 — protolens: inference, over a database that did not exist two minutes ago · 1:30

```
protolens --descriptor-set bobapp1.desc -I src1/ bobshark
```

No `--type`, no path, nothing told to it. protolens scores every type
in a database that was inside the executable two minutes ago, against
Bob's captured bytes, and opens on the winner. Bob's capture is
readable.

**It is right, and it is not close.**

| candidate | score |
|---|---|
| `google.maps.routing.v2.ComputeRoutesRequest` | **−16** |
| `google.maps.routing.v2.RouteMatrixDestination` | −55 |

Thirty-nine points of daylight. Say what that number is not: not a
percentage and not a confidence. It is a score, and the pane shows what
it is made of — which is the beat's second payoff, already on screen and
costing nothing:

```
matched: 14
unknown: 1          ← a field no schema declares
non_canonical: 1    ← a varint spelled in more bytes than it needs
```

**Two of the four anomalies, counted before the file has been opened.**
Beat 4 noticed in passing that a couple of these bytes were
non-canonical; here the scorer has not merely noticed, it has *priced*
it. Those two deductions are why the winning score is −16 rather than
positive: the tool is saying that the best type it has does not
completely fit these bytes, and it is right about that too.

**Why there is no ambiguity in this beat.** `bobapp1.desc` holds one
version of the Routes API, because that is what Bob's older build was
compiled against — so there is nothing here to be ambiguous *between*.
Ambiguity is a property of a bigger dictionary, not of a hard payload.
That is an argument for beat 10, and it is why the demo's manual detour
lives there rather than here.

**Claims:** inference ranks rather than guesses; the ranking is legible;
and two of the findings arrive before anyone has looked at the document.

### 9 — protolens: the log, named and half-read · 2:00

```
protolens --descriptor-set bobapp1.desc -I src1/ boblog
```

A different shape of document, and the first honest one. It opens
**named**:

```
bobapp.v1.Log     score: 19
  matched: 24   unknown: 0   out_of_range: 0
  non_canonical: 0   mismatches: 0   truncated: true
```

Stop on the last line, because it is the thesis. The end of this file is
missing — the process was killed 1 024 bytes into a record — and the
tool has not thrown the answer away over it. It named the type, counted
twenty-four matched fields, and reported the damage as a *property of
the answer*. A decoder that refuses truncated input gives you nothing
here; a decoder that ignores the truncation lies to you about the last
entry. This does neither.

And the damage is localized, in place, in the document — entry 4 carries
its own reason:

```
entry {  #@ repeated Entry = 1; TRUNCATED_MESSAGE; MISSING: 1024
```

**The tool does not merely fail; it says how far short the file falls.**
The length prefix is what makes that number knowable, and it is why the
damage stops at one entry out of four.

Now the split the beat exists for. Inside the entries:

- Entries 3 and 4 are Routes, and they open in the clear —
  `ComputeRoutesRequest` at −16 (the same 84 bytes as bobshark, which is
  worth pointing out) and a 9 868-byte reply at **+651**, which is what
  an easy answer looks like.
- Entries 1 and 2 are junk. Their requests score −26 against
  `google.protobuf.GeneratedCodeInfo.Annotation`, their replies −50 in a
  three-way tie. That is the tool saying *there is a message in here and
  I do not have it* — and its cue is lit but dim.

Then the move the whole second half turns on. **Read the line above the
junk:**

```
method: "/google.maps.places.v1.Places/SearchText"
```

The document names the schema it is missing. And the room has already
seen where that schema is: it was in beat 5's diff, in the newer build,
ten minutes ago.

That is the cliffhanger. Do not resolve it in this beat; end on the
`method:` line and change the command.

**Claim:** a partial answer, honestly marked, is worth more than a
confident wrong one — and an honest tool tells you what it is missing.

### 10 — protolens: the newer build · 4:30 · **headline + differentiator**

```
protolens --descriptor-set bobapp2.desc -I src2/ boblog
```

Same file, same tool, one command apart — and the database is the one
built from the binary Bob downloaded second. Then four moves, in this
order.

#### 1. The opaque halves have names now

| the same bytes | under `bobapp1.desc` (beat 9) | under `bobapp2.desc` (now) |
|---|---|---|
| Places request, 240 B | `GeneratedCodeInfo.Annotation` −26 | **`places.v1.SearchTextRequest` −8** |
| Places response, 508 B | three-way junk, −50 | **+45**, `SearchTextResponse` tied with `SearchNearbyResponse` |

A flat blob becomes a tree — `text_query: "coffee in Grenoble"`, a
`location_bias.circle` around a lat/lng, and a reply with five Grenoble
coffee shops and their coordinates in it.

**Bob's app does more than he thought.** It does not just route; it
geocodes, and it sends a place name and a coordinate to a second Google
service on every run.

#### 2. The field that occurs twice, and what is in front of it

`SearchTextRequest` declares `string text_query = 1` — singular. It
appears **twice** in this request. Legal wire, last-one-wins: `protoc
--decode` prints the second one and drops the first without a word.

The second one is 18 bytes and says `coffee in Grenoble`. The first one
is **164 bytes and is not a string at all** — and the tool says so, and
now it can say what it is:

```
google.rpc.Status     score: 8   (sole)
```

One override, and it opens all the way down:

```
code: 13
message: "debug: outbound call authenticated"
details {
 type_url: "type.googleapis.com/google.rpc.ErrorInfo"
 value {                                   #@ ErrorInfo = 2
  reason: "DEBUG_TRACE"
  domain: "bobapp"
  metadata { key: "x-goog-api-key"  value: "AIzaSyB0b5REK…" }
 }
}
```

**This program writes Bob's credential into Bob's own log, in a field a
normal decoder never prints.** That is the finding, and it is the reason
the talk exists.

Then point at the score, because it is the cheapest quantitative claim
in the demo. Under `bobapp1.desc` those same 164 bytes were **+3, in a
three-way tie**, and the `Any` inside stayed an opaque escaped string.
Under `bobapp2.desc` they are **+8, sole**, and the `Any` expands. The
five points are the five fields inside it, readable only because the
newer build happened to embed `google/rpc/error_details.proto` — the
third file in beat 5's diff, and now we know what it was for.

*More schema, more the tool can say.* Not a slogan; a subtraction.

#### 3. The tie — where the tool stops and defers

Now the route request in the same log, 84 bytes — the one beat 8 answered
outright:

| candidate | score |
|---|---|
| `google.maps.routes.v1.ComputeRoutesRequest` | **−16** |
| `google.maps.routing.v2.ComputeRoutesRequest` | **−16** |

Two candidates, the same score — and **they render the payload
identically, field for field.** That is a stronger thing to show than a
wrong guess: the tool is not confused, and it did not rank badly. On
these bytes the two published schemas genuinely say the same thing.
`routes.v1`'s field numbers are a strict subset of `routing.v2`'s, so v1
can never *outrank* v2; a tie is the ceiling, and a tie is what a
scoring function that can only see wire shape is entitled to reach.

What breaks it is not in the payload at all. It is two lines up, in the
envelope:

```
method: "/google.maps.routing.v2.Routes/ComputeRoutes"
```

**The tie is broken by a string, in the same file, that no scoring
function is allowed to read.** Alice reads it out and picks v2 by hand.

This is the demo's one **deliberate manual detour**: the script prefills
the `:override` command line and stops there; the presenter presses
Enter. Announce it — a flawless instantaneous demo reads as a recording,
and work shown is proof.

Two things worth saying while it is up. The tie **only exists under the
newer database** — `bobapp1.desc` never heard of `routes.v1`, which is
why beat 8 was unambiguous. And the escalation therefore bought two
opposite things in one command: names for the payloads and the key, and
a genuine ambiguity about traffic that was never in doubt before. Bigger
dictionaries do not monotonically help. Beat 12 pushes that one notch
further.

Note the rhyme, and say it once: three times now — the `method:` line in
beat 9, the `type_url` inside the Status, and the `method:` line here —
**the file has told us something the schema could not.** That is not a
coincidence; producers put names on the wire.

#### 4. The reconstructed source earns its keep, then the wire

Before accepting a candidate for one of these subnodes, `v` on the field
opens its definition in the `.proto` reproto wrote in beat 6. This is
the argument beat 7 was set up for: judging whether a type really fits a
subnode is not something a score can do for you, and the thing you want
in front of you while you do it is the schema, as source, in your own
editor. Binary → descriptors → source → and back into the source from
the payload.

Then `w`, and the wire bytes go under each rendered line in three
severity tiers. The four anomalies, in order, each held on screen long
enough to actually be read:

- **The duplicate field.** Both occurrences, in wire order. Then the
  sentence the talk is built around:

  > `protoc --decode` shows you one value. prototext shows you both,
  > because both are on the wire. The first one is Bob's API key.

- **The over-long varint.** `20 81 80 80 80 00`, where two bytes would
  do. The `w` row is where "non-canonical" stops being an abstraction —
  and it is the payoff for the small odd thing beat 4 noticed in
  passing, and for one of the two deductions beat 8 priced.
- **The undeclared field.** Field 99, `"bobapp/0.9.3-rc2"`, named by
  shape rather than by name. Nothing lost. This is the other deduction
  from beat 8.
- **The truncated record.** Where `protoc` gives up, and how the
  length prefix bounds the damage to one entry out of four.

This beat carries **deliberate stillness**. Automation can deliver a
three-tier wire display in 200 ms; nobody absorbs it in 200 ms. The
script's commentary lands *before* each move or *after* it, never
during.

### 11 — What Alice sends back · 1:30 · finale

```
:export boblog.prototext
prototext encode boblog.prototext -o roundtrip.pb
cmp boblog roundtrip.pb && echo identical
```

Two claims in one pipeline, and the order matters.

First, the artifact: an ordinary text file, with every finding
annotated inline, that Bob can open in any editor without installing
anything. That is the deliverable — the demo has a *recipient*, and it
is a better ending than a checksum.

Second, the proof: that text re-encodes to the file Bob handed over,
byte for byte — including the 1 024 bytes that are missing from the end
of it, which the annotation records rather than invents.

> The leaked key survived. It would not have survived
> `protoc --decode`.

All three advertised promises close in one sentence.

### 12 — Epilogue: two hundred times the dictionary · 1:00

```
protolens --descriptor-set googleapis.desc -I src2/ boblog
```

Somebody is going to ask whether this works against a real corpus, so
answer it before the Q&A and get two things for the price of one.

**It scales.** 7 771 files, 58 777 types, 25.6 MB, indexed once. The
same 20 KB log opens in **82 ms** against it, against 17 ms for the
130 kB database — two hundred times the dictionary for five times the
wait, because startup scales with the payload, not with the descriptor
set.

**And it reads less of the file.** The envelope goes dark: the best
googleapis can offer for `bobapp.v1.Log` is a crowd of candidates at −2,
because Google has never heard of Bob's app. Everything below it is
numbered unknowns — the probe still walks all the way down to
`/1/4/1/3/2/3/1`, which is the key, so the *shapes* survive intact and
only the names are gone. That is the sharper version of the point, and
it is free.

> Two hundred times the dictionary, and it knows less about this file
> than the one I built out of Bob's Downloads folder. More schema is not
> more knowledge. The *right* schema is.

This beat is designed to be **droppable**. If beat 10 overran, cut it and
say the sentence from the podium instead.

### 13 — Close (slide) · 1:00

MIT, `github.com/ThalesGroup/prototools`, `cargo install`. The session
is **left loaded** rather than exited — with instant manual
interaction available, questions get answered on the tool.

---

## Two returns, on purpose

The no-return rule is about *screens*, because re-orientation is paid
by the audience. Two beats look like violations and are not:

- **neovim, beat 7 then beat 10.** The whole point of beat 7 is to
  make beat 10's nested launch free. Cutting beat 7 does not save the
  return; it makes the return expensive.
- **prototext, beat 4 then the finale.** The finale is a *pipeline*,
  not a screen — `encode`, `cmp`, and one line of output. It
  introduces no visual vocabulary. If it did, it would have to move
  inside protolens.

protoscan and reproto each run **twice inside their own beat**, once per
binary. That is not a return: it is one screen, one idiom, and the
second invocation is the beat's payoff.

## What the participant will not see

Stated so the synopsis is honest about its own edges:

- **No scoring-graph visualization.** The HTML graphs in the one-hour
  tutorial are a beat of their own and there is no room. Repo pointer.
- **No full-googleapis decompile on stage.** It is too slow. The real
  command is shown and `googleapis.desc` is declared pre-built.
- **No packet-level unwrapping.** bobshark arrives as a message body.
  Whether the pcap and the tshark one-liner appear at all is an open
  question — see below.
- **No walk through the full anomaly vocabulary.** Four of thirty.
  `tests/fixtures/anomalies.pb` and its guided script are the pointer.
- **No `protoc` rehabilitation.** The talk is not against `protoc`; it
  is about the cases `protoc` was not built for. Say so once, in beat
  2, and do not litigate it.
- **No explanation of why bobapp2 exists.** Bob downloaded it because
  the app misbehaved; the demo never says what the bug was, and does not
  need to.

---

# Implementation reference

## Three nested layers

The demo is a script inside a script inside an editor. Which layer
owns a keystroke decides who has to be built.

```
grpconf/prompt <presentation.sh>   outer: one shell command per ENTER
  └── protolens --script <s>       inner: one view per ;
        └── nvim (via `v`)         nested: owns the terminal until :q
```

**The outer layer** is `grpconf/prompt`, unchanged. It walks a
pre-recorded command list, re-colors each line, and `eval`s it. Its
history is editable live (arrows, F2/F3, Ctrl-S), which is the
presenter's escape hatch.

**The handoff interlock the demo plan asked for is free here.** The
prompter does not advance on a timer — it blocks in `read -e` until
ENTER, and `eval` returns only when protolens exits. A scripted
keystroke can therefore never leak into a child process. This was
listed as a requirement for script mode; it is satisfied by
construction and needs no code.

**The inner layer** is protolens's script mode (spec 0271). A step
*declares a view* — `fold`, `unfold`, `node`, `wire_line`/`wire_lines`/
`wire_node`, plus the commentary `text` — and stepping is `;`/`,`.
There is no undo stack because a step is a reset plus a re-derivation.
`space` turns navigation off at any moment, from wherever the step left
the caret; that is the "instant fall-through to manual" requirement, and
it already works.

**The step keys are punctuation, not arrows, and that is deliberate**
(0271, amended 2026-08-14). On stage a hand reaches for an arrow key
without deciding to — to nudge the caret, to pan a wide row. If that
changed the slide there would be no way back to the view that was on
screen, because a step is re-derived rather than undone. So `,`/`;`
step, `?`/`.` scroll, and every arrow key belongs to the document
whether navigation is on or off. The one binding the script really
takes is `?` (backward search), and `space` hands it straight back.

**The `command:` key is the manual-detour mechanism, already built.**
It prefills a command line and **never executes it**.
Both override moments — beat 10's naming of the leaked Status, and beat
10's tie-break — are exactly this: the script sets it up, the presenter
presses Enter. Nothing new is needed.

Draft 2 needed a third override, to name boblog's envelope in beat 9.
That one is gone: since spec 0314 the envelope is inferred outright at
+19, so beat 9 has no override at all and beat 10 has two.

**Node addressing in the scripts is by search string, not positional
path** — see the header comment in `beats/infer.script`. A search that
misses is a visible, recoverable miss; a positional path that misses
silently lands on the wrong node. Measured positional paths, for
re-pinning only: under `bobapp1.desc`, `/1` is the first `entry {`;
under `googleapis.desc`, `/1/4/1/3/1` is the `type_url` inside the
leaked Status.

**The nested editor** is invoked by protolens's `v`, which resolves a
field's definition against `--proto-root`. So beat 10 requires
`-I src2/` — the directory reproto wrote for the newer build in beat 6.

## The protolens invocations

Four sessions, each one shell step. Re-entering protolens is not a
"return to a tool" — it is the same screen, and it is how a different
payload or a different descriptor set gets in.

| # | Beat | Invocation | Script |
|---|---|---|---|
| 1 | 8 | `protolens --descriptor-set bobapp1.desc -I src1/ --script beats/infer.script bobshark` | inference, the score breakdown, the two counted anomalies |
| 2 | 9 | `protolens --descriptor-set bobapp1.desc -I src1/ --script beats/log-v1.script boblog` | the named envelope, `truncated: true`, the Routes/Places split, the `method:` line |
| 3 | 10 + 11 | `protolens --descriptor-set bobapp2.desc -I src2/ --script beats/log-v2.script boblog` | Places, the duplicate field and the key, the tie-break, `v`, the four anomalies, the export |
| 4 | 12 | `protolens --descriptor-set googleapis.desc -I src2/ --script beats/scale.script boblog` | 39 ms, and the envelope going dark |

**The escalation makes session boundaries cheap.** Sessions 2, 3 and 4
are separated by *the thing the beat is about* — a different
`--descriptor-set` on the command line — so every boundary is motivated
on screen rather than being an artifact of the tooling. Sessions 3 and 4
are the only pair where nothing carries over, and nothing needs to: beat
12 sets no overrides and reads nothing forward.

Beat 11's export lives at the end of `log-v2.script`, so the overrides
the presenter set in beat 10 are still in force for it. The `cmp` stays
a shell step in `presentation.sh`.

## What has to be built

`grpconf/artifacts.md` is the build plan. In rough order of risk:

1. ~~**`bobapp1` and `bobapp2`.**~~ **Done 2026-08-17.** One Rust/tonic
   source tree, built twice from `demo/bobapp/default.nix` with a
   `variant` argument; `pname` and the two env vars are deliberately out
   of `commonArgs` so both variants share one dependency cache. Nix
   attributes are `bobapp1`, `bobapp2`, `bobapp1-desc`, `bobapp2-desc`;
   `bobapp` and `bobapp-desc` no longer exist. `grpconf-demo` stages
   `bin/bobapp1` and `bin/bobapp2`.
2. ~~**`boblog`.**~~ **Minted 2026-08-15.** 20 243 bytes, four entries,
   the four anomalies, and a tail cut 1 024 bytes inside the last
   record.
3. ~~**`bobshark`.**~~ **Done 2026-08-15.** 84 bytes, the `request`
   field of route entry 3, lifted verbatim.
4. **Nothing for `googleapis.desc`.** It is the corpus the repo's CI
   already builds — not made for this talk, and the better for it.
5. ~~**The four `.script` files and the outer step list.**~~ **Done
   2026-08-17.** `infer.script` survived draft 3 intact.
   `log-partial.script` and `log-full.script` were deleted and rewritten
   as `log-v1.script` (5 steps) and `log-v2.script` (18 steps, five
   prefilled overrides, the export last); `scale.script` is new and has
   3. `export.script` stays a stub. `presentation.sh` was re-pinned
   wholesale onto `BOBAPP1`/`BOBAPP2`, `DESC1`/`DESC2`, `SRC1`/`SRC2`
   and `PROTO2`, and now opens with the protoscan 41/77 gate.

   Every script is re-pinned by driving the binary headlessly —
   `protolens … script | grep 'error:'` — and every one prints nothing
   except `log-v2.script`, which prints `no match for "val_ohb"` twice
   and is documented in its own header as doing so. That is not drift:
   the headless transcript never executes a prefill, so the route
   request is still an escaped byte string at that step, and the
   over-long varint is the one anomaly with no ASCII to anchor a search
   on. Every other search in that file is spelled to match in *both*
   states.
6. **A `demo/header` title per beat**, as `demo/01-tutorial.sh` does.

## Verified so far

Facts established on this machine rather than assumed, so that the
beats above are not resting on hope. Everything below was re-measured
2026-08-17 unless dated otherwise.

- The API key at `~/.config/bobapp/api-key` answers **HTTP 200** from
  the Routes API and, since 2026-08-15, from Places as well.
- **Both embedded sets are complete and were dumped to check it**:
  `bobapp1 --dump-descriptor` writes 51 111 bytes / **41** files,
  `bobapp2` writes 103 430 bytes / **77**. In both, the *last*
  `FileDescriptorProto` is `bobapp/v1/log.proto` — see the trap below.
- **protoscan finds every one of them**, 41 and 77, given a build that
  carries the spec 0313 fix.
- **boblog's root is named, not vetoed** — `bobapp.v1.Log`, score 19,
  24 matched, 0 unknown, 0 mismatched, `truncated: true`, under both
  bobapp databases. This is new since spec 0314 and it is what retired
  draft 2's beat 9.
- **The Places escalation is real**: 240-byte request −26 → **−8**,
  508-byte response −50 → **+45**.
- **The leaked Status escalation is real and attributable**: +3 in a
  three-way tie → **+8 sole**, and `--no-expand-any` reproduces the +3
  exactly (3 matches vs 8), so the five points are demonstrably the
  expanded `Any`. Measured on the staged `bobapp2.desc` — 130 044 B,
  77 files, carrying *both* `google/rpc/error_details.proto` and
  `bobapp/v1/log.proto`, so the envelope and the key are named by one
  database.
- **The v1/v2 collision is real, and it is a tie.** On the route
  request under `bobapp2.desc`, `routes.v1.ComputeRoutesRequest` and
  `routing.v2.ComputeRoutesRequest` both score −16 and render the
  payload identically. Under `bobapp1.desc` there is no collision at all
  (−16 against −55). v1 can never *outrank* v2: its field numbers are a
  strict subset.
- **The scorer counts two anomalies unprompted** — `unknown: 1` and
  `non_canonical: 1` — on bobshark, before the file is opened.
- **Open times on boblog**: 17 ms / 17 ms / 82 ms against
  `bobapp1.desc` / `bobapp2.desc` / `googleapis.desc`, wall clock over
  `protolens … quit`, mean of ten. Re-measured 2026-08-17 against the
  rebuilt stage; an earlier draft quoted 7 / 6 / 39 ms, which no longer
  reproduces. The interactive numbers will be larger; re-time on the
  projector.

## Traps

- **`bobapp/v1/log.proto` is the last descriptor in both embedded
  sets, and a stale scanner drops the last one.** The spec 0313 bug
  discarded the final `FileDescriptorProto` whenever any byte followed
  it. On a shell holding a pre-0313 `fdp_scan_lib`, `protoscan` prints
  40 and 76 instead of 41 and 77, and — much worse — `reproto` emits
  `Warning: missing dependency file:bobapp/v1/log.proto` and silently
  builds a database **without the envelope**, which kills beats 9
  through 11. Observed on 2026-08-17 with a nix-store `fdp_scan_lib`
  older than the fix. Check with `protoscan bobapp1 | wc -l` before
  every rehearsal; the answer must be 41.
- The nix store is read-only: never `cp` without `--no-preserve=mode`
  when the destination has to be writable.
- `grpconf/stage/` is gitignored. Do not `git add` it.

## Rehearsal checklist

- **`protoscan bobapp1 | wc -l` must print 41.** See the trap above.
  This is the single check that can silently gut the second half.
- Pin `COLUMNS`/`LINES` and the color profile. protolens's layout and
  the wire rows are width-sensitive; a projector is not the laptop.
  **Truecolor is not cosmetic here** — see open question 3: without it,
  the cues beats 9 and 10 turn on may not be drawn at all.
- Record an asciinema fallback of the full run, one keystroke away.
- Rehearse the nested `v` → edit → return → keep-driving cycle twice
  on the actual projector. Nested apps and the Kitty
  keyboard-enhancement push/pop have historically leaked key events
  here.
- Time every beat on the real hardware. `docs/demo-timing.md` is the
  precedent format.
- Pre-load two more detours besides the two overrides — a second
  candidate, one skipped anomaly — so improvisation is really recall.
- **The demo must not need the network.** bobapp calls a live service
  to *mint* the artifacts; the artifacts are then committed and the
  talk replays them. Rehearse in airplane mode at least once.

## Open questions

1. **Does bobshark arrive as a `.pb` or as a `.pcap`?** Currently
   planned as a `.pb` — Bob extracted it. A real capture means TLS,
   which means `SSLKEYLOGFILE` plus tshark, and tshark is not
   installed here. Worth 30 seconds only if it is genuinely one
   command; otherwise it is a second tool for no new claim.
2. **Is beat 10's fourth entry-point defensible from the stage?** Beat 6
   asks reproto for `google/rpc/error_details.proto`, which is not a
   service Bob's app calls — it is reachable from the binary but not
   from the three service graphs. A sharp viewer may ask why Alice
   thought to name it. The honest answer is that beat 5's diff put it on
   screen and it was one of exactly three new files; the presenter
   should say that rather than skate past it.
3. **Are the cues visible on the projector?** The only thing on this
   list that can silently kill a beat. A cue's brightness is a function
   of the winning score, and on a terminal without truecolor protolens
   draws **nothing at all** for `best_score <= 3`. The leaked Status is
   +3 under `bobapp1.desc` and the v1/v2 tie is −16, so on a 16-color
   terminal those would be invisible while the `+651` response cue
   blazes. Confirm `COLORTERM=truecolor` reaches protolens through
   tmux/ssh before trusting beats 9 and 10.
4. **Does beat 12 survive contact with the clock?** It is the only beat
   written to be cut, and if beat 10 runs long it should be. Decide at
   rehearsal, not on stage.

## What changed from draft 2

Recorded so the diff is reviewable rather than archaeological.

| | Draft 2 | Draft 3 |
|---|---|---|
| Artifacts | three: `bobapp`, `bobshark`, `boblog` | **four**: `bobapp1`, `bobapp2`, `bobshark`, `boblog` |
| The escalation | one recovered database → the googleapis corpus | **two recovered databases**, both built on stage from binaries Bob sent |
| googleapis | carried three payoffs — the Places names, the tie, the ambiguity lesson | carries **none**; a droppable 1-minute epilogue about scale, and the counter-example that keeps the escalation honest |
| Beat 5 | one protoscan run, 39 names | **two runs and a diff**; the diff is the second half of the talk, pre-announced |
| Beat 6 | one reproto run | **two**, with entry-point lists read off beat 5's diff |
| Beat 9's premise | boblog opens `<raw / no type>`; the truncated tail vetoes every candidate | **dead.** Since spec 0314 the root is named at +19 with `truncated: true`. The beat is now about a *named* answer that reports its own damage |
| Beat 9's override | one, to name the envelope | **none.** Two overrides remain, both in beat 10 |
| The leaked key | named only under googleapis, as a repo-scale flex | named under `bobapp2.desc`, and its **+3 → +8** score step is the demo's one quantified claim about what a schema buys |
| The tie | needed googleapis | needs only `bobapp2`, because the newer build ships both Routes versions |
| Anomaly 3's framing | asserted | still asserted — field 99 is undeclared in *both* builds. The two-binary frame does **not** demonstrate it, and the beat should not claim it does |
| Beat count / budget | 12 beats, 18:40 | **13 beats, 18:20** |
| Largest open question | whether the cue is visible on the projector | unchanged — and joined by whether a stale scanner has silently removed the envelope |

Two things were considered and **kept**, unchanged from draft 2:

- **Beat 2, `protoc --decode` failing.** The abstract's title is
  *Beyond `protoc --decode`*, and the demo plan decided explicitly to
  open on it. 40 seconds; kept.
- **The manual detour.** The demo plan called the wrong-guess moment
  "the highest-value 45 seconds available". It is spent on the v1/v2
  tie-break, which is a stronger claim than a wrong guess: not *the tool
  ranked wrong* but *two published schemas say the same thing about
  these bytes, and a string in the envelope is what decides.*

One thing was considered and **rejected**: withholding
`google/rpc/error_details.proto` from beat 6, so that the leaked Status
could only be named under googleapis and the epilogue would have a
payoff. Measured both ways — it works, `googleapis.desc` names the
Status at +8 with the `Any` expanded exactly as `bobapp2.desc` does. It
was rejected because it breaks the demo's central promise: *everything
you see is derived from the files Bob sent.* Trading that for one minute
of epilogue is a bad trade, and the epilogue is stronger as an honest
negative result anyway.
