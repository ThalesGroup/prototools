<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

Demo plan — gRPConf 2026 North America
======================================

Framing and design decisions for the 20-minute talk advertised in
`grpconf2026-abstract.md`. Records what was decided and why, so the
detailed script can be written against it without relitigating the
choices.

Status: framing agreed; synopsis written, then **re-framed**. Read
"Revision — the Bob and Alice frame" at the end before trusting the
"The artifacts" and "The beats, with budget" sections above, both of
which are superseded.


The constraints
---------------

- **20 minutes**, one intro slide, then a live terminal demo, then one
  closing slide. Q&A on top.
- **Three promises are advertised and must land**: schema inference,
  lossless decoding, descriptor archaeology. Everything else in the demo
  is cuttable.
- **An audience remembers about three things.** Density buys *evidence*
  for those three, not additional messages.
- The demo is driven by `demo/prompt` (a bash wrapper that walks a
  pre-recorded command history) plus a protolens "script mode" that is
  **not yet specified**. See "What script mode has to provide" below.


The artifacts
-------------

The story runs on **two** files, and they must be named distinctly on
screen. Conflating them is the failure mode: if the payload is itself a
descriptor, its type is already known and there is nothing to infer.

1. **The executable** — a stripped binary / shared library / container
   layer with `FileDescriptorProto` blobs embedded in it. This is what
   `protoscan` digs through, and it is the archaeology beat.
2. **The captured payload** — one gRPC message of unknown type. This is
   what protolens identifies, browses, and round-trips.

Both are **still to be crafted**. Neither is `grpconf/anomalies.pb`.

`grpconf/anomalies.pb` is typed `google.protobuf.FileDescriptorProto`
(see `grpconf/README.md`), so it cannot serve as the captured payload
without breaking the story. The anomaly payload must be a *second*
fixture typed as the application's own message; `prototext-core/tests/
anomaly_fixture.rs` and spec 0226 make that cheap to build and keep
honest.

Framing to use: the developer has a container image (or a stripped
binary) and one captured payload. **Every other file in the demo is
derived from those two on stage.** That is a situation the room has
been in.


The beats, with budget
----------------------

| # | Beat | Budget |
|---|------|--------|
| 1 | Intro slide | 1:30 |
| 2 | `protoc --decode` fails | 0:45 |
| 3 | protoscope: raw, correct, meaningless | 0:45 |
| 4 | protoscan extracts FDPs from the executable | 1:30 |
| 5 | reproto: one file, editions | 2:00 |
| 6 | protolens: inference + scoring **(headline)** | 5:00 |
| 7 | navigation | 1:30 |
| 8 | anomalies + wire rows **(differentiator)** | 5:00 |
| 9 | lossless round-trip (finale) | 2:00 |
| 10 | close slide | 1:00 |

Total 21:00 against 20:00 with no slack — so one beat is expected to go.
Beat 7 folds into beat 6 if needed; beat 3 is the next cheapest cut.

The budget assumes the prompter removes execution overhead (see below).
It does **not** assume the audience reads faster, so beats 6 and 8 carry
deliberate stillness rather than more content.


Decisions
---------

**Open on `protoc --decode`, not on protoscope.** The abstract names
`protoc --decode` as the thing that falls short, so the demo must show
it falling short. Ten seconds of an error message buys the premise.
protoscope then plays its proper role — correct, complete output that
means nothing.

**Do not explain protoscope.** It is the one screen where partial
audience comprehension is on-message: the beat exists to show that
correct bytes are not enough. Show it, name the problem, move.

**Editions belongs in the reproto beat.** The abstract advertises "an
editions descriptor a downstream toolchain cannot consume" and an
earlier draft of the synopsis never showed editions at all. Decompiling
descriptors is a nod, not a reaction; editions is the reason to look at
reproto. This closes the gap and earns the beat its two minutes.

**Show the `#@` prototext format *before* protolens opens.** protolens
is the only genuinely new screen, and it otherwise carries two new
things at once — a UI *and* a notation. Learning both simultaneously is
where an audience quietly falls behind. So: one flat `prototext decode`
on screen first, no panes, no caret, no color semantics. Thirty seconds.
Then protolens has one new thing to teach instead of two.

This also strengthens the argument rather than merely easing it: it
shows the format is a plain text artifact that exists independently of
its viewer — which is exactly the claim the round-trip finale makes.

**Include a moment where the tool guesses wrong and the human
overrides it.** A demo in which every inference is correct reads as
staged. protolens already has the override pane; a plausible-but-wrong
top candidate, retyped by hand, shows a tool that respects judgment.
Highest-value 45 seconds available.

**Run the `reproto`-on-full-googleapis cut on camera.** Full googleapis
is too slow to decompile live, so a subset is decompiled on stage and a
pre-built indexed `googleapis.desc` is used thereafter. Show the real
command, then say the artifact was built earlier. Audiences forgive a
pre-built artifact and resent a hidden one. Honest numbers to put on
screen: 25.6 MB, 7 771 files, **58 777 types**, indexed once.

**Make the round-trip the payoff, not a checksum.** "Extract, re-encode,
diff" is a correctness proof and reads as an epilogue. Tie it to the
anomaly instead: re-encode, compare hashes, identical — *the smuggled
bytes survived, and they would not have survived `protoc --decode`*. One
command, one comparison, and all three advertised promises close in one
sentence.

**"Repeated singular fields leaking data" has no annotation to point
at.** Checked: the renderer's vocabulary has no duplicate-singular
token, and correctly so — repeated occurrences of a singular field are
legal wire, last-one-wins. Do not build one for the talk. Rephrase:

> `protoc --decode` shows you one value. prototext shows you both,
> because both are on the wire. The second one is the channel.

That is the stronger point anyway — the covert channel is invisible
*precisely because* canonical tooling is lossy — and it sets up the
round-trip finale.


The screen budget
-----------------

Every tool switch makes the room re-learn a screen. That cost is paid by
the audience, so no amount of automation touches it. The current lineup:

| Surface | Orientation cost |
|---|---|
| `demo/prompt` | none — it is a shell |
| protoscope output | none, deliberately (see above) |
| `.proto` source in the editor | none — everyone reads `.proto` |
| reproto's *command* | one sentence |
| protolens | **the one new screen** |
| neovim, launched from protolens via `v` | none *if* set up first |

Four visual vocabularies, exactly one new. That is what justifies the
denser 20 minutes, and it is a constraint to protect when the detailed
synopsis lands.

Two rules to hold: **every tool appears in one contiguous block**, and
**no tool is returned to after it is left.** If the detailed synopsis
violates either, reorder rather than trim.

**The editor beat is setup, not inspection.** Because protolens launches
neovim via `v`, the editor appears twice. Opening a `.proto` in the same
editor earlier means the nested launch lands on a screen the audience
already recognizes, and reads as navigation rather than as "something
else opened". Same editor, same colorscheme. If that beat is instead
done with `bat`/`less`, the setup is lost and the `v` jump costs a fresh
orientation at the worst possible moment.

If the beat is kept, say its claim out loud — *this is an ordinary file
any editor opens* — which is a real claim about reproto's output.
Otherwise it is the cheapest beat to cut when over time.


The prompter: what it buys, and what it does not
------------------------------------------------

**Refunded:** typing latency, typos, wrong-directory moments, hunting
for a key, the presenter reading their own notes — and, decisively for
a TUI, caret navigation. Locating a node in a large document live is
otherwise a guaranteed disaster.

**Not refunded:**

- **Tool wall-clock.** Unchanged, and *more* conspicuous once everything
  around it is instant: the only remaining pauses are the machine's.
- **Audience re-orientation.** Paid by the audience, not the presenter.
- **Reading time.** A wire-row display with three severity tiers needs
  seconds of near-silence. Automation can deliver it in 200 ms; nobody
  absorbs it in 200 ms.

Consequence: **go deeper per artifact, not wider across tools.** A sixth
tool costs a fresh orientation tax; three more minutes inside protolens
costs nothing, because the audience already paid for that screen.


### Risks the prompter introduces

**It can look canned.** A flawless, instantaneous demo reads as a
recording. Two antidotes: take the manual detour *deliberately and
visibly* once, announcing it; and do not hide real timings. Work shown
is proof; a cached instant answer is proof of nothing.

**Desync hurts more, not less.** With the sequence outsourced to a
script the presenter is not reading, a failed command or a wrong path
leaves them more lost than they would be typing. Keep the step list
visible to the presenter and make each step independently re-runnable.

**The script pane competes for attention.** Takeaway text landing while
the caret moves and rows re-render gives the audience three things
asking to be looked at. Land text *before* the action or *after* it,
never during.

**The escape hatch needs rehearsing.** "Not bound to follow the script"
is only true for a presenter who has practiced leaving it. Pre-load two
or three specific detours — the override case, a second candidate, one
skipped anomaly — so improvisation is really recall.


### What script mode has to provide

Design input, recorded while the mode is still unspecified:

1. **A handoff interlock.** protolens launches neovim (`v`), so the
   prompter must know when a child process owns the terminal. If the
   driver advances on a timer, the next scripted keystroke goes into
   neovim — which, depending on the key, is an edit to a real source
   file in front of the room. Block on an explicit handed-off /
   regained-control signal. **Do not rely on timing.**
2. **Visible caret motion.** A caret that teleports is invisible: the
   audience sees the screen change and never sees *where it went*, which
   defeats the point of automating navigation at all. Step the move, or
   give the landing an emphasis that outlives the jump. Same for pane
   opens.
3. **A "hold here" marker distinct from "next step"**, so reading pauses
   live in the script rather than in the presenter's head.
4. **Instant fall-through to manual.** The stated advantage of the whole
   approach; it is what makes the deliberate detour possible.


### Rehearsal checklist

- Pin `COLUMNS`/`LINES` and the color profile. protolens's layout and
  the wire rows are width-sensitive; a projector is not the laptop.
- Record an asciinema fallback of the full run, one keystroke away.
- Rehearse the nested `v` → edit → return → keep-driving cycle twice on
  the actual projector. Nested apps and the Kitty keyboard-enhancement
  push/pop have historically leaked key events in this codebase.
- Type the first command **manually**, before engaging the prompter, so
  the audience sees a real shell.
- Keep the final state loaded for Q&A instead of exiting — with instant
  manual interaction, questions can be answered on the tool.


Measured facts
--------------

**protolens startup against full googleapis is not a problem** —
measured 2026-08-03:

```
$ time protolens --descriptor-set .../googleapis-db/googleapis.desc /tmp/sim quit
protolens: inferring root type (459 bytes) on 12 threads...
protolens: rendering root node as google.bigtable.v2.ReadModifyWriteRowRequest (459 bytes)...
protolens: indexing 10 lines...
real 0m0.050s
```

50 ms wall, full 25.6 MB descriptor set, 58 777 types, correct
inference. The descriptor load is lazy and inference is index-driven, so
**startup scales with the payload, not with the descriptor set.** An
earlier assumption to the contrary — that a big `.desc` implies a
multi-second startup regardless of payload size — is wrong.

Multi-second startup figures recorded elsewhere in this repo belong to
*large-payload* scenarios (full-corpus inference sweeps), not to this
one. Still worth timing the actual demo payload against the actual
shipped `.desc` before the talk, but there is no reason to design
narration around a stall.


Open items
----------

- Craft the executable and the captured payload.
- Build the anomaly fixture typed as the application's message.
- Write the detailed synopsis; verify it respects the contiguous-block
  and no-return rules.
- Specify protolens script mode against the four requirements above.
- Time every beat on the real hardware once the synopsis exists;
  `demo-timing.md` is the precedent for that format.


Revision — the Bob and Alice frame
----------------------------------

Recorded 2026-08-14. `grpconf/synopsis.md` draft 2 re-frames the demo.
Everything in "Decisions" and "The prompter" above still holds. Two
sections above do not, and are superseded here rather than edited in
place, so the reasoning that produced them stays readable.

**The story has people in it now.** Bob downloads an executable off
the net, plays with it, and sends it to Alice with a log it wrote and
a capture he took. This is not decoration. It supplies a *motive* for
each beat — Alice is answering Bob's question, not touring a feature
list — and it gives the finale a recipient, which "extract, re-encode,
diff" never had.

**Two artifacts became three.** "The artifacts" above is superseded:

| | Was | Is |
|---|---|---|
| the executable | `ordersvc`, a fictional order service | `bobapp`, a real client of the Google Routes API |
| the payload | `capture.pb` | `bobshark`, one request body |
| — | — | `boblog`, the log bobapp wrote |

Splitting the payload in two is what buys the second half. bobshark is
*one message*, so inference has a clean single-type target and the
wrong-guess beat has somewhere uncluttered to happen. boblog is a
*container*, so the demo can show a document that is **partly**
readable — which is the honest steady state of this kind of work, and
the only thing that motivates escalating to the full corpus.

**The escalation is the new spine.** There are now two schema
databases, and moving between them is a beat rather than a setup
detail: `bobapp.desc` (39 files, built on stage out of the binary)
reads the envelope and some entries; the pre-built `merged.desc`
(58 777 types) reads the rest. In between, the unresolved payloads sit
there wearing heat cues — protolens saying *there is a message here
and I cannot name it*. That cliffhanger did not exist before and it is
the strongest thing in the revision.

**The anomalies stopped being curiosities.** In draft 1 they were
fixture content the demo detoured to admire. Here bobapp is a program
of unknown provenance, so a non-canonical encoder is in character and
the anomalies are *the finding* — the reason Alice was called. One
consequence worth recording: anomaly 1's smuggled value is now **Bob's
API key**, logged by his own downloaded app, replacing draft 1's
"somebody's exfiltration channel". The exfiltration framing invited a
threat model the demo never substantiated. A utility that carelessly
logs your credential is more common, more checkable, and lands harder.

**The editor beat's justification changed.** Above, it exists so the
nested `v` launch is free and so the loop binary → descriptors →
source closes on stage. Both still true, but the *argument* is now
better: in beat 10 Alice reads a recovered `.proto` to decide whether
a candidate type really fits a subnode. Judging relevance is not
something a score can do for you, and what you want in front of you
while you do it is the schema, as source, in your own editor. The
setup stays early; the payoff moves late.

**The beat list is superseded** by `grpconf/synopsis.md` draft 2:
twelve beats, 18:40 against 20:00. `protoc --decode` failing (beat 2)
and the wrong-guess override (beat 8) were both proposed for deletion
during the re-frame and were both kept — the first because the
abstract's title is *Beyond `protoc --decode`*, the second because
this document called it "the highest-value 45 seconds available" and
nothing about the new frame makes that less true. It now shares the
demo with a *second* override in beat 10 that makes a different claim.

**Open items settled by the revision:** the executable is
`demo/ringer/` renamed to bobapp — it already makes a real live gRPC
call and already embeds its descriptors; `protoscan` was verified to
find all 39 of them in the Rust binary, which retired the question of
rewriting it in Go. Script mode was specified and implemented as spec
0271. The remaining build work is in `grpconf/artifacts.md`.

**Open item created by the revision:** beat 8 asserts that
`google.maps.routes.v1` outranks `google.maps.routing.v2` on
bobshark's bytes. Both types are in the corpus; the collision itself
is unmeasured, and it is now the demo's largest single risk.
