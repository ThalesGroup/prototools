<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# What's really in that `.pb` file?

**gRPConf 2026 North America — 20 minutes, live terminal.**
Synopsis, draft 2, **revised 2026-08-15 against the built artifacts**.
Not yet rehearsed; timings are budgets, not measurements. Every score
quoted below is measured on the real files, not estimated.

This document has two readers. The first half is what a participant
gets handed before the talk: the story, the artifacts, and what each
beat is meant to prove. The second half is the implementation
reference — which layer drives which keystroke, and what has to be
built before any of it runs.

Related: `docs/grpconf2026-abstract.md` (what was advertised),
`docs/grpconf2026-demo-plan.md` (the framing decisions this synopsis
is written against — read it before changing a beat),
`grpconf/artifacts.md` (how bobapp, boblog and bobshark get built).

**Draft 2 changed the frame.** Draft 1 was an unattributed second
person: *you are handed a binary and a capture*. It is now Bob and
Alice, three artifacts instead of two, and a schema escalation in the
middle that draft 1 did not have. What survived unchanged: the beat
grammar, the no-return rule, the wrong-guess override, and the
round-trip finale. See "What changed from draft 1" at the end.

---

## Executive summary

**The room is handed somebody else's problem, which is the point.**
Bob downloaded an executable off the net. It answers questions about
driving distances between cities, and it clearly does so by calling
some external service. He has played with it, kept a log file it
wrote, and taken a capture of one of its calls. He thinks there are
protobufs in there somewhere. He does not have a `.proto`, a type
name, or any idea what the thing is talking to. He hands all three
files to Alice, and Alice is the person the talk is addressed to.

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

**The schema was inside the executable the whole time.** An
application that speaks protobuf carries descriptors, because it needs
them at runtime. So the executable is not a dead end, it is a source:
protoscan lists thirty-nine `.proto` files sitting in it — and the
names alone answer Bob's question, because they say
`google/maps/routing/v2/`. Bob's app calls the Google Routes API.
Then reproto reads the binary *directly*, with no extraction step, and
emits compilable `.proto` source plus an indexed scoring database in
one command. Recovery does not stop at a descriptor either: what comes
back is edition 2023, which the room's own toolchain cannot compile,
so the same command re-emits it as proto2 — same wire format, syntax
that builds today.

**Now the capture reads, and the score shows its work.** Against a
database built from the binary sixty seconds ago, protolens names
bobshark's type without being told anything —
`google.maps.routing.v2.ComputeRoutesRequest`, thirty-nine points clear
of the runner-up. What is worth more than the name is the breakdown
beside it: `unknown: 1`, `non_canonical: 1`. Two of the four anomalies
are *counted* before the file has been opened, and they are why the
winning score is still negative. The tool is not claiming a fit. It is
saying this is the best type it has and it does not completely explain
these bytes.

**The log is where the talk earns its title, and it starts by
failing.** boblog is not one message, it is Bob's app's own log format —
and it opens as `<raw / no type>`, because the truncated record at its
tail makes every candidate walk off the end of the file. That is not a
dead end; it is the case heat cues exist for. A cue on an entry names
`bobapp.v1.Entry` from wire shape alone, thirty-five points clear, and
one override clears the whole log. Then the split: the Routes payloads
open in the clear, and the rest sit there as opaque `bytes` whose cue is
lit but dim — *there is a message in here, and I do not have it.* That
is the cliffhanger, and it is resolved by pointing the same tool at the
full googleapis corpus — 7 771 files, 58 777 types, 25.6 MB, indexed
once — whereupon those blobs turn out to be a second Google service Bob
never knew his app called. This is also where the reconstructed `.proto`
source stops being a trophy and becomes a tool: before accepting a
candidate for a subnode, Alice reads its definition.

**And the bigger dictionary does not only help.** Under googleapis the
envelope goes dark — Google has never heard of Bob's app — and the route
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
the first, and the first one is **Bob's API key**, which this
downloaded application writes into its own log. A varint padded past
minimal length — same number, different bytes, and nothing downstream
notices. A field the recovered schema does not declare, read by shape
because the wire type is in the tag. And a record truncated at the
tail, where the app was killed mid-write, which is the input the
standard decoder refuses outright. This is the section that is
deliberately slow: the display takes 200 ms and nobody absorbs it in
200 ms.

**The finale is what Alice sends back to Bob.** The annotated document
is written out as prototext — a plain text file, readable in any
editor, with every anomaly called out inline. Then it is re-encoded
and compared against the log Bob handed over: identical. Not a
checksum for its own sake. The leaked key survived the round trip. It
would not have survived `protoc --decode`.

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
He took a packet capture of one of its calls. And then he sent me
three files and a sentence: *I think this thing speaks protobuf, can
you tell me what it's doing?*

No `.proto`. No type name. No documentation. No idea who it is
talking to. Everything else that appears on this screen in the next
twenty minutes, I am going to derive from those three files, in front
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
fit: I read the binary as if it were a schema directory. Thirty-nine
`.proto` files come out, and the file *names* already answer Bob's
question, because they all start `google/maps/routing/v2`. Bob's
mystery app is calling the Google Routes API.

One command turns that into compilable source and an indexed schema
database. And I do not stop at a descriptor, because a descriptor is
not something you can build against. What comes back is edition 2023,
and if your toolchain is like mine, it cannot compile that. So the
same command emits proto2 instead: identical on the wire, and it
builds today.

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
not one message; it is Bob's app's own log format. And it does not open
at all — no type, nothing. The end of the file is chopped off, so every
candidate the tool tries runs off the end and disqualifies itself.

That is where this gets interesting, because the tool has a second thing
it can do. It cannot name the document, but it can look at any piece of
it and say *this shape is somebody's message* — and it points at an
entry and names Bob's own log format, from the bytes alone, with nothing
to go on. One correction from me and the whole log opens. Half of it,
anyway. The routes are there in the clear. The rest are just `bytes`,
and the tool marks them and says: *there is a message in here, I cannot
tell you which one.*

So I give it a bigger dictionary. The whole of googleapis — seven
thousand seven hundred files, fifty-eight thousand types — and those
blobs fall open. Bob's app is calling a second Google service that he
never mentioned, looking up place names. And this is where the source
code we reconstructed ten minutes ago stops being a souvenir: before I
accept a type for one of these, I open its definition and read it.

The bigger dictionary also takes something away, and I want you to see
that too. Bob's own envelope goes dark — Google has never heard of Bob's
app. And the request that was unambiguous a minute ago now has two
answers with the identical score, from two versions of the same Google
API, which render these bytes identically, field for field. That is not
the tool being confused. Those two schemas genuinely say the same thing
here, and nothing about the wire will separate them. What separates them
is a string one line up in the log. So I read it, and I pick, by hand,
while you watch. That is the point. It ranks. Then it defers to me.

And now the part I actually came here for. I am going to put the raw
bytes underneath every line, and show you four things that are on this
wire that nothing you normally run will show you.

There is a field that is declared singular, and it occurs twice. That
is legal. Last one wins. Your decoder shows you the second value and
silently drops the first — and the first one is Bob's API key. This
program writes his credential into its own log file, in a place a
normal decoder will never print. There is a number encoded in more
bytes than it needs; it decodes to the same value, it re-encodes to
different bytes, and nothing downstream ever notices. There is a field
the schema does not declare at all, and we can still read it, by
shape, because the wire type is in the tag. And there is a record at
the end that is cut off mid-length, because the process died
mid-write; that is the input the standard decoder refuses outright,
and I will show you why the length prefix keeps the damage down to one
entry.

I will go slowly there. The screen draws in a fifth of a second, and
nobody reads it in a fifth of a second.

Then I write the whole thing back out as text — an ordinary file, with
every one of those findings annotated inline, which is what I would
actually send back to Bob. And then I re-encode that text and compare
it to the file Bob gave me. They are identical. Not as a party trick —
I want you to notice what survived that round trip. The leaked key is
still there. It would not have been, going through the decoder we
started with.

So: a log you cannot name, in a schema you do not have, written by a
program whose source nobody has. It is still completely readable. And
reading it honestly means showing you what is on the wire — not what a
schema expected to find there.

## The story, in one paragraph

Bob downloads an executable off the net. It answers questions about
distances between cities by calling an external service, and he thinks
it speaks protobuf. He keeps the log it writes and takes a packet
capture of one call, and hands all three to Alice with no `.proto`, no
type name and no documentation. Over twenty minutes, everything else
in the demo is derived from those three files on stage: the
descriptors are recovered from the executable, `.proto` source and an
indexed schema database are reconstructed from the descriptors, the
capture's type is inferred, the log's opaque payloads are identified
against a corpus of sixty thousand candidates (and one ambiguity in
them is settled by hand), and the whole thing is read down to the byte
— including four things `protoc --decode` will not show you, one of
which is Bob's own API key.

That is a situation most of the room has been in. The last part is the
one they have been in without knowing.

## The three artifacts

Everything starts from exactly three files, and they are named
distinctly on screen throughout. Conflating them is the failure mode:
if a payload were itself a descriptor, its type would already be known
and there would be nothing to infer.

| On screen | What it is | Which promise it serves |
|---|---|---|
| `bobapp` | The executable Bob downloaded. A real gRPC client that calls the Google Routes API; descriptors embedded uncompressed. No source, no `.proto`. | descriptor archaeology |
| `bobshark` | One request body, lifted out of Bob's capture. 84 bytes. Type unknown. | schema inference |
| `boblog` | The log `bobapp` wrote. An envelope the recovered schema describes, holding payloads it mostly does not. Carries the four anomalies and a truncated tail. | lossless decoding, heat cues, the escalation |

The split of labor between the two payloads is deliberate and is what
draft 1 did not have:

- **bobshark is one message.** It exists so inference has a clean,
  single-type target — and so the wrong-guess beat has somewhere to
  happen that is not tangled up with an envelope.
- **boblog is a container.** It exists so the demo can show a document
  that is *partly* readable, which is the honest steady state of this
  kind of work and the only way to motivate the escalation to the full
  corpus.

`boblog` carries four deliberate anomalies, chosen because each one is
invisible to standard tooling in a different way, and because each one
falls out of the story rather than being sprinkled on:

1. **A singular field that occurs twice, the first value being a whole
   message hiding in a `string`.** Legal wire; last-one-wins.
   `protoc --decode` shows you the second `text_query`, the place name
   that was really searched for, and stops. The first is a debug trace
   the app left in front of it — a `google.rpc.Status` carrying the
   `x-goog-api-key` it authenticated with, which it logs and should
   not. Under googleapis those 164 bytes have exactly one candidate, so
   the heat cue does not merely say "this is not a string": it names
   the message, and the key is one keystroke away.
2. **A non-minimal varint** (`val_ohb`). Decodes to the same number,
   re-encodes to different bytes. Nothing downstream notices.
3. **A field the recovered schema does not declare.** A newer producer
   against an older schema — nothing is lost, the wire type is in the
   tag.
4. **A truncated length-delimited record at the tail.** The app was
   killed mid-write. The input `protoc` refuses.

Four is the budget. The full vocabulary — thirty annotation tokens,
every one of them — lives in `grpconf/anomalies.pb`, which is a repo
pointer and a Q&A artifact, not a beat.

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

Which anomaly lands in which artifact is fixed by what each beat has to
show:

| # | Anomaly | Lands in | Why there |
|---|---|---|---|
| 2 | non-minimal varint (`val_ohb`) | the **request** | beat 4 reads the request with `prototext` and needs `ohb` visible before protolens exists |
| 3 | undeclared field | the **request** | same beat, and it is the one anomaly whose severity tier differs from 2's |
| 1 | duplicated singular field carrying the key | the **log** | the escalation's payoff: Bob's own app writes his credential to his own disk |
| 4 | truncated tail | the **log** | only a *file* can be cut short mid-write; a request that egressed was complete |

Anomaly 1's key is a **synthetic** string that looks like a Google API
key and is not one. The real key never reaches a committed artifact, and
the framing is carelessness — a downloaded utility logging a credential
where its user will not look — not exfiltration. That is both truer to
Bob and a smaller claim to have to defend from the stage.

## The two schema databases

The escalation is the spine of the second half, so the two databases
need to stay as distinct on screen as the three artifacts.

| On screen | Built from | Size | What it can name |
|---|---|---|---|
| `bobapp.desc` | `reproto -I bobapp --schema-db-out` | 41 files, 51 111 bytes | the Routes v2 traffic, **and boblog's envelope** — and only one version of the Routes API, so nothing in it is ambiguous |
| `googleapis.desc` | the repo's own googleapis corpus, as CI builds it | 7 771 files, 58 777 types, 25.6 MB | the Places traffic too — but *not* the envelope, and it holds both Routes versions, so it can be ambiguous where the small one was not |

`bobapp.desc` is built **on stage, in one command, from the binary**.
`googleapis.desc` is the stock corpus, shipped, not made for this talk.

**There is no merged set, and that is the point.** An earlier draft had
a third, pre-built `merged.desc` holding both — which meant one artifact
on stage that the audience had to take on trust, and protolens takes a
single `--descriptor-set` anyway, so "both" was never a mode it had.
Dropping it turns what looked like a regression into the demo's best
teaching moment:

- Beat 9 opens the log under `bobapp.desc`. The envelope can be
  *named* — `bobapp.v1.Entry`, with `at`, `method`, `request` — though
  not for free: the truncated tail vetoes the root, so a heat cue
  proposes the type and one override accepts it. Inside, `bobapp.desc`
  only knows Routes v2, so the Places payloads stay opaque bytes.
- Beat 10 opens the same log under `googleapis.desc`. Now every payload
  can be named — and the envelope cannot, because Google has never heard
  of Bob's app.

Neither schema reads the whole file, and no third schema is fetched to
make the problem go away. What closes the gap is a `path:field`
override aimed at an exact positional path — which is the feature beat
10 exists to show, arriving because the document asked for it rather
than because the script did.

**The escalation is not a strict improvement, and beat 10 says so.**
The big dictionary buys names for the Places payloads and simultaneously
*loses* the envelope and *introduces* a tie on Routes traffic that the
small dictionary answered outright. That asymmetry is the most
interesting thing the two databases do, and it was discovered by
measuring rather than designed.

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
| **protolens** | beats 8–11 | **the one new screen** |

Two rules hold the lineup together: every tool appears in one
contiguous block, and no tool is returned to after it is left. The two
apparent exceptions are both deliberate, and both are payoffs rather
than violations — see "Two returns, on purpose" below.

---

## Beat by beat

Budgets total 18:40 against 20:00, which is 1:20 of slack — spent
entirely on beats 9 and 10, which are the two that will overrun. If
more has to go, beat 4 is the cheapest cut and beat 3 the next.

**Revised 2026-08-15**: beat 8 gave a minute to beat 10 when the
override detour moved there. The total is unchanged.

### 1 — The problem (slide) · 1:30

*This slide is the presenter's to write.* What it has to end on: Bob,
his three files, and the promise that everything else is derived from
them live. What it opens on — why we got interested in this, and why
the tools were open-sourced — is deliberately left blank here.

The presenter types the first shell command **by hand**, before the
prompter is engaged, so the room sees a real terminal.

```
ls -l bobapp boblog bobshark
```

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

### 5 — protoscan: the schema is in the executable · 0:50

```
protoscan bobapp
```

Thirty-nine `.proto` file names scroll past. bobapp has no symbols and
no source, but it has to *build* these messages at runtime, so the
descriptors are in there.

And then the beat's real payoff, which is free and which draft 1 did
not have: **read the names.** They say `google/maps/routing/v2/`. Bob
asked what his app was talking to, and the file listing already
answered him, before a single byte was decoded.

This beat **only reveals**. Nothing is extracted, no directory is
written, no file is opened.

Verified on the real artifact: protoscan prints **39** names from the
bobapp binary. On the `gh` binary (55 MB, stripped, modern Go) it
prints 250 in 0.47 s; on `googleapis.desc` it finds all 7 771.

**Claim:** the schema was in the executable the whole time — and its
table of contents was the answer to Bob's question.

### 6 — reproto: the binary *is* the input · 2:00

No extraction step. reproto reads the executable itself:

```
reproto -I bobapp -O src/ --schema-db-out bobapp.desc \
        google/maps/routing/v2/routes_service.proto
```

`-I` takes a **blob** and stands for the descriptors embedded in it —
one member per `FileDescriptorProto` found, which is exactly what
protoscan just listed (spec 0243). Imports resolve out of the same
binary. One command: stripped executable to compilable `.proto` source
*and* an indexed scoring database, no intermediate directory, no temp
files to explain.

Say the first point out loud: the usual pipeline here is *extract,
then hope the pieces fit together*. There is no extract.

Then the part that earns the beat the rest of its time: the recovered
descriptor is **edition 2023**, and the room's Rust toolchain — prost,
prost-reflect — cannot compile editions syntax.

```
reproto -I bobapp -O src2/ --force-proto2-output \
        google/maps/routing/v2/routes_service.proto
diff -u src/.../routes_service.proto src2/.../routes_service.proto
```

Same wire format, proto2 syntax, compiles today.

**Claim:** archaeology that ends at a descriptor is half a result —
and the descriptor never had to become a file.

### 7 — The editor (setup) · 0:40

```
nvim src/google/maps/routing/v2/routes_service.proto
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

### 8 — protolens: inference, over a database that did not exist two minutes ago · 2:00

```
protolens --descriptor-set bobapp.desc -I src/ bobshark
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
unknown: 1          ← a field no schema declares
non_canonical: 1    ← a varint spelled in more bytes than it needs
```

**Two of the four anomalies, counted before the file has been opened.**
Beat 4 noticed in passing that a couple of these bytes were
non-canonical; here the scorer has not merely noticed, it has *priced*
it. Those two deductions are why the winning score is −16 rather than
positive: the tool is saying that the best type it has does not
completely fit these bytes, and it is right about that too.

**Why there is no wrong guess in this beat.** `bobapp.desc` holds one
version of the Routes API, because that is what Bob's app was built
against — so there is nothing here to be ambiguous *between*.
Ambiguity is a property of a big dictionary, not of a hard payload. That
is an argument for beat 10, and it is why the demo's manual detour lives
there rather than here.

**Claims:** inference ranks rather than guesses; the ranking is legible;
and two of the findings arrive before anyone has looked at the document.

> **Measured 2026-08-15 on the real bobshark.** Draft 2 asserted a wrong
> guess here — `routes.v1.ComputeRouteMatrixRequest` beating
> `routing.v2` — and that was wrong twice over. The method is
> `ComputeRoutes`, not `ComputeRouteMatrix`; and v1's field numbers are
> a **strict subset** of v2's, so v1 can never outrank v2 on any
> payload. The collision is real but it is a *tie*, it needs
> `googleapis.desc` to happen at all, and it has moved to beat 10. See
> `grpconf/artifacts.md` step 6.

### 9 — protolens: the log, half-read · 2:00

```
protolens --descriptor-set bobapp.desc -I src/ boblog
```

A different shape of document, and the first honest one. It opens with
**no type at all**:

```
protolens: rendering root node as <raw / no type> (20 KB)...
```

Beat 8's file was named in 50 ms; this one defeats the same machinery
completely. The cause is anomaly 4 — the truncated tail makes every
candidate walk off the end of the file, and an incomplete walk is a
veto, so the candidate list comes back *empty*. Do not route around
this. **A document nothing can name is exactly the document the heat
cues have something to say about**, and this is the beat where they say
it.

The cue on the first entry:

| node | suggestion | score | runner-up |
|---|---|---|---|
| an entry | `bobapp.v1.Entry` | −12 | −47 |

Thirty-five points, from wire shape alone, on a document with no type.
One override names it — and because every entry is field 1 of the root,
**that single override clears the whole log**. Bob's app's own format,
recovered from Bob's app.

Now the split the beat exists for. Inside the entries:

- The Routes payloads open in the clear — `ComputeRoutesRequest` at
  −16, and its 9 868-byte reply at **+651**, which is what an easy
  answer looks like.
- The others are `bytes` and stay `bytes`. Their cue is *lit but dim*:
  the best thing `bobapp.desc` can offer is
  `google.protobuf.GeneratedCodeInfo.Annotation` at −37, which is the
  tool saying *there is a message in here, and I do not have it.*

And the last entry does not render at all. It is the cut tail, so it
comes back as one opaque run with the reason attached:

```
#@ TRUNCATED_BYTES; MISSING: 1024
```

**The tool does not merely fail; it says how far short the file falls.**
Anomaly 4 turns up in place, in the document, rather than being
announced later — and the length prefix is what makes that number
knowable, which is the point beat 10 returns to.

That is the cliffhanger, and it should be left hanging. Do not resolve
it in this beat.

**Claim:** a partial answer, honestly marked, is worth more than a
confident wrong one.

### 10 — The full corpus, and bytes that are messages · 5:00 · **headline + differentiator**

```
protolens --descriptor-set googleapis.desc -I src/ boblog
```

Same file, bigger dictionary: 7 771 files, 58 777 types, 25.6 MB,
indexed once, and it still opens in about 50 ms because startup scales
with the payload, not with the descriptor set.

**Open by saying what was lost.** In beat 9 the entries could be named
`bobapp.v1.Entry`. Here they cannot: Google has never heard of Bob's
app, and the cue that named them a minute ago has no candidate left to
offer. Nothing is merged to paper over that. It is the honest state of
the world — **neither schema reads the whole file** — and it is what
makes the next four minutes a piece of work rather than a lookup.

Then three moves, in this order.

#### 1. The opaque bytes have names now

The same 75 bytes the cue shrugged at in beat 9:

| scored against | best candidate | score | runner-up |
|---|---|---|---|
| `bobapp.desc` (beat 9) | `google.protobuf.GeneratedCodeInfo.Annotation` | −37 | −48 |
| `googleapis.desc` (now) | **`google.maps.places.v1.SearchTextRequest`** | **+12** | −25 |

Same bytes, same tool, one command apart: junk under one database and
named outright under the other. The presenter overrides the field and a
flat blob becomes a tree — `text_query: "coffee in Grenoble"`, a
`location_bias.circle` around a lat/lng. The reply comes with it, at
**+45**.

**Bob's app does more than he thought.** It does not just route; it
geocodes, and it sends a place name and a coordinate to a second Google
service on every run.

The override is a **`path:field`** one, aimed at a positional path —
`/1:4` is the first entry's field 4. It works *while the parent is still
raw*, so the envelope never has to be named first, and the two kinds of
payload in this log get separate origins instead of fighting over one.
This is the feature the beat exists to show, and it arrived because the
document asked for it.

#### 2. The tie — where the tool stops and defers

Now the route request in the same log, 84 bytes:

| candidate | score |
|---|---|
| `google.maps.routes.v1.ComputeRoutesRequest` | **−16** |
| `google.maps.routing.v2.ComputeRoutesRequest` | **−16** |
| next | −37 |

Two candidates, the same score — and **they render the payload
identically, field for field.** That is a stronger thing to show than a
wrong guess: the tool is not confused, and it did not rank badly. On
these bytes the two published schemas genuinely say the same thing.
`routes.v1`'s field numbers are a strict subset of `routing.v2`'s, so v1
can never *outrank* v2; a tie is the ceiling, and a tie is what a
scoring function that can only see wire shape is entitled to reach.

What breaks it is not in the payload at all. It is two lines up, in the
envelope — and note *how* it is available. Under this database the
envelope has no name, so the field has no name either. It is just:

```
2: "/google.maps.routing.v2.Routes/ComputeRoutes"
```

An unnamed field, in a message nothing could identify, holding the
answer in plain text. **The tie is broken by the part of the file that
no schema described.** Alice reads it out and picks v2 by hand.

This is the demo's one **deliberate manual detour**: the script prefills
the `:override` command line and stops there; the presenter presses
Enter. Announce it — a flawless instantaneous demo reads as a recording,
and work shown is proof.

Two things worth saying while it is up. The tie **only exists under the
big dictionary** — `bobapp.desc` never heard of `routes.v1`, which is
why beat 8 was unambiguous. And the escalation therefore bought two
opposite things in one command: names for the payloads, and a genuine
ambiguity about traffic that was never in doubt before. Bigger
dictionaries do not monotonically help.

#### 3. The reconstructed source earns its keep

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

- **The over-long varint.** Same number, two more bytes. The `w` row
  is where "non-canonical" stops being an abstraction — and it is the
  payoff for the small odd thing beat 4 noticed in passing.
- **The undeclared field.** Named by shape rather than by name.
  Nothing lost.
- **The truncated record.** Where `protoc` gives up, and how the
  length prefix bounds the damage to one entry.

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
anything. That is the deliverable — the demo has a *recipient*, which
draft 1 did not, and it is a better ending than a checksum.

Second, the proof: that text re-encodes to the file Bob handed over,
byte for byte.

> The leaked key survived. It would not have survived
> `protoc --decode`.

All three advertised promises close in one sentence.

### 12 — Close (slide) · 1:00

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
  `grpconf/anomalies.pb` and its guided script are the pointer.
- **No `protoc` rehabilitation.** The talk is not against `protoc`; it
  is about the cases `protoc` was not built for. Say so once, in beat
  2, and do not litigate it.

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
*declares a view* — `fold`, `unfold`, `node`, `wire-line`/`wire-lines`/
`wire-node`, plus the commentary `text` — and stepping is `;`/`,`.
There is no undo stack because a step is a reset plus a re-derivation.
`space` turns navigation off at any moment, from wherever the step left
the caret; that is the "instant fall-through to manual" requirement, and
it already works.

**The step keys are punctuation, not arrows, and that is deliberate**
(0271 S7, amended 2026-08-14). On stage a hand reaches for an arrow key
without deciding to — to nudge the caret, to pan a wide row. If that
changed the slide there would be no way back to the view that was on
screen, because a step is re-derived rather than undone. So `,`/`;`
step, `?`/`.` scroll, and every arrow key belongs to the document
whether navigation is on or off. The one binding the script really
takes is `?` (backward search), and `space` hands it straight back.

**The `override:` key is the manual-detour mechanism, already built.**
It prefills a `:override …` command line and **never executes it**.
All three override moments — beat 9's naming of the envelope, beat 10's
bytes-that-are-messages, and beat 10's tie-break — are exactly this: the
script sets it up, the presenter presses Enter. Nothing new is needed.

The origin syntax the scripts need is `path:field` with a **positional**
path (`/1:4` is the *first child's* field 4, not field 1's). It resolves
against a parent that is still raw, which is what lets beat 9 name a
payload before the envelope has a type and lets beat 10 give the Places
and Routes payloads separate origins with no conflict to resolve.

**The nested editor** is invoked by protolens's `v`, which resolves a
field's definition against `--proto-root`. So beat 10 requires
`-I src/` — the directory reproto wrote in beat 6.

## The protolens invocations

Four sessions, each one shell step. Re-entering protolens is not a
"return to a tool" — it is the same screen, and it is how a different
payload or a different descriptor set gets in.

| # | Beat | Invocation | Script |
|---|---|---|---|
| 1 | 8 | `protolens --descriptor-set bobapp.desc -I src/ --script beats/infer.script bobshark` | inference, the score breakdown, the two counted anomalies |
| 2 | 9 | `protolens --descriptor-set bobapp.desc -I src/ --script beats/log-partial.script boblog` | the vetoed root, the cue that names the envelope, the entries that stay opaque |
| 3 | 10 + 11 | `protolens --descriptor-set googleapis.desc -I src/ --script beats/log-full.script boblog` | the corpus, the two overrides, the tie-break, `v`, the four anomalies, the export |

**The escalation makes session boundaries cheap, which is new in draft
2.** Draft 1's largest open question was how session 1's chosen
override survived into session 2. Here sessions 2 and 3 are separated
by *the thing the beat is about* — a different `--descriptor-set` on
the command line — so the boundary is motivated on screen rather than
being an artifact of the tooling.

~~That leaves the question alive in exactly one place: sessions 3 and 4,
where the overrides the presenter set in beat 10 must still be in
force for the export in beat 11.~~ **Resolved 2026-08-15: merged.**
`log-full.script` ends with the export step and the round-trip
narration; `export.script` is a stub explaining the merge. The `cmp`
stays a shell step in `presentation.sh`, which no longer invokes
`export.script`.

## What has to be built

`grpconf/artifacts.md` is the build plan. In rough order of risk:

1. ~~**`bobapp`.**~~ **Done 2026-08-15.** Renamed from `demo/ringer/`;
   a real Rust/tonic gRPC client calling `google.maps.routing.v2.Routes`
   **and** `google.maps.places.v1.Places` live with an API key, embedding
   a 41-file descriptor set uncompressed and dumping it back out. The
   anomaly writer and the truncated tail are in.
2. ~~**`boblog`.**~~ **Minted 2026-08-15.** 20 198 bytes, four entries —
   two Places pairs then two Routes pairs — the four anomalies, and a
   tail cut 1 024 bytes inside the last record.
3. ~~**`bobshark`.**~~ **Done 2026-08-15.** 84 bytes, the `request`
   field of a route entry, lifted verbatim.
4. **Nothing.** `googleapis.desc` is the corpus the repo's CI already
   builds — not made for this talk, and the better for it. Beat 10's
   collision is **measured and real** (a tie at −16); nothing was
   manufactured.
5. ~~**The four `.script` files** and the outer step list.~~ **Done
   2026-08-15.** `infer.script`, `log-partial.script`, `log-full.script`
   (beats 10 + 11 merged), and `export.script` (stub). `presentation.sh`
   updated to match.
6. **A `demo/header` title per beat**, as `demo/01-tutorial.sh` does.

## Verified so far

Facts established on this machine rather than assumed, so that the
beats above are not resting on hope:

- The API key at `~/.config/bobapp/api-key` answers **HTTP 200** from
  the Routes API and, since 2026-08-15, from Places as well — the
  project had to enable `places.googleapis.com` and the key's
  allow-list had to be extended to both.
- **`protoscan` finds all 39 descriptors in the existing Rust
  binary.** This was the one thing a Go binary would have given for
  free, and it was the reason to consider Go at all; it works, so
  bobapp stays Rust and no new toolchain enters the repo.
- **The v1/v2 collision is real, and it is a tie.** On the route
  request, under `googleapis.desc`, `routes.v1.ComputeRoutesRequest`
  and `routing.v2.ComputeRoutesRequest` both score −16 with the next
  candidate at −37, and they render the payload identically. Under
  `bobapp.desc` there is no collision at all (−16 against −55), which
  is why the beat moved from 8 to 10. v1 can never *outrank* v2: its
  field numbers are a strict subset.
- **The root of boblog is vetoed under both databases.** The truncated
  tail disqualifies every candidate, so the log opens `<raw / no type>`
  and beat 9 runs off heat cues. This is by design, not a defect.
- **The scorer counts two anomalies unprompted** — `unknown: 1` and
  `non_canonical: 1` — on bobshark, before the file is opened.
- protolens opens a 459-byte blob against the full 25.6 MB
  `googleapis.desc` in **50 ms**. Startup scales with the payload, not
  with the descriptor set.

## Rehearsal checklist

Carried forward from the demo plan, still open:

- Pin `COLUMNS`/`LINES` and the color profile. protolens's layout and
  the wire rows are width-sensitive; a projector is not the laptop.
  The sandbox-vs-terminal color difference is a known trap in this
  codebase. **Truecolor is not cosmetic here** — see open question 6:
  without it, the two cues beats 9 and 10 turn on are not drawn at all.
- Record an asciinema fallback of the full run, one keystroke away.
- Rehearse the nested `v` → edit → return → keep-driving cycle twice
  on the actual projector. Nested apps and the Kitty
  keyboard-enhancement push/pop have historically leaked key events
  here.
- Time every beat on the real hardware. `docs/demo-timing.md` is the
  precedent format.
- Pre-load two more detours besides the three overrides — a second
  candidate, one skipped anomaly — so improvisation is really recall.
- **The demo must not need the network.** bobapp calls a live service
  to *mint* the artifacts; the artifacts are then committed and the
  talk replays them. Rehearse in airplane mode at least once.

## Open questions

1. ~~**Does the v1/v2 pair actually collide on bobshark?**~~
   **Answered 2026-08-15: yes, but only under `googleapis.desc`, and as
   a tie.** The beat moved from 8 to 10 as a result, and is stronger for
   it — see both beats.
2. **Does bobshark arrive as a `.pb` or as a `.pcap`?** Currently
   planned as a `.pb` — Bob extracted it. A real capture means TLS,
   which means `SSLKEYLOGFILE` plus tshark, and tshark is not
   installed here. Worth 30 seconds only if it is genuinely one
   command; otherwise it is a second tool for no new claim.
3. ~~**Do sessions 3 and 4 merge?**~~ **Answered 2026-08-15: yes.**
   `log-full.script` ends with the export step; `presentation.sh` runs
   only one protolens invocation for beats 10 + 11.
4. ~~**Which two extra services go in the log?**~~ **Answered: one, and
   it is `google.maps.places.v1`.** `SearchTextRequest` has a
   free-format `string text_query = 1`, which is the field the demo
   wants to point at, and it is absent from bobapp's 41 embedded files.
   One service, twice, beats two services once each: the second entry
   costs no narration and it makes the pattern visible.
5. ~~**Does bobapp really call them?**~~ **Answered 2026-08-15: yes.**
   Both Places calls are real, and each is a request *and* a response.
   The credential surface did not grow — it is the same key, on the same
   project, with `places.googleapis.com` added to its allow-list.
6. **Is beat 9's cue visible on the projector?** New, and the only
   thing on this list that can silently kill a beat. The cue's
   brightness is a function of the winning score, and on a terminal
   without truecolor `protolens` draws **nothing at all** for
   `best_score <= 3`. Beat 9's entry cue is −12 and beat 10's tie is
   −16, so on a 16-color terminal both would be invisible while beat 9's
   `+651` response cue blazes. Confirm `COLORTERM=truecolor` reaches
   protolens through tmux/ssh on the actual projector before trusting
   beats 9 and 10.

## What changed from draft 1

Recorded so the diff is reviewable rather than archaeological.

| | Draft 1 | Draft 2 |
|---|---|---|
| Frame | unattributed second person | Bob hands three files to Alice |
| Artifacts | `ordersvc`, `capture.pb` | `bobapp`, `bobshark`, `boblog` |
| Schema DBs | one merged set | two, neither of which reads the whole file, and the escalation between them is a beat |
| Service | a fictional order service | the real Google Routes API |
| The anomalies | curiosities in a fixture | the reason Alice was called |
| Anomaly 1 | "somebody's exfiltration channel" | Bob's API key, logged by his own app |
| Heat cues | absent | beat 9's cliffhanger and beat 10's payoff |
| The editor | closes the loop | supplies the reference for judging a candidate |
| Finale | `:export --binary`, `cmp` | export **text** for Bob, then re-encode and `cmp` |
| Overrides | one | three, making three different claims: *name the envelope* (9), *these bytes are a message* (10), *choose between equals* (10) |
| Largest open question | how an override crosses a session boundary | ~~whether the v1/v2 collision is real~~ — settled 2026-08-15; now whether the cue is visible on the projector |

Two things were considered and **kept**, against the outline that
prompted this draft:

- **Beat 2, `protoc --decode` failing.** The outline dropped it. The
  abstract's title is *Beyond `protoc --decode`*, and the demo plan
  decided explicitly to open on it. 40 seconds; kept.
- **Beat 8's wrong guess.** The outline replaced it with beat 10's
  overrides. Both are kept, because they make different claims and the
  demo plan called the wrong-guess moment "the highest-value 45
  seconds available".

  **Revised 2026-08-15.** Measurement moved it rather than cutting it.
  There is no wrong guess to show under `bobapp.desc` — the answer there
  is right by 39 points — and the real collision needs the big
  dictionary, so the moment now sits inside beat 10. It also changed
  shape for the better: not *the tool ranked wrong* but *two published
  schemas say the same thing about these bytes, and a string in the
  envelope is what decides.* The demo plan's 45 seconds are still spent;
  they are spent later, and on a stronger claim.
