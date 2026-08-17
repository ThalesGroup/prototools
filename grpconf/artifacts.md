<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Building bobapp, bobshark and boblog

The build plan for the artifacts `grpconf/synopsis.md` runs on. Read the
synopsis first — this document only says how the files get made, not why
they are shaped the way they are.

Status: **every step is done** (2026-08-14 → 2026-08-17). Steps 1–9 are
records of work already landed and are kept as written; where a later
change invalidated one of their conclusions the section says so in
place rather than being quietly rewritten. Step 10 is the two-build
split that draft 3 of the synopsis turns on.

**Two things below were true when they were measured and are not true
now**, and both are called out where they sit:

- There is **one bobapp binary** in steps 1–9. There are two from step
  10 on, and no artifact named `bobapp.desc` exists any more —
  `bobapp1.desc` and `bobapp2.desc` do.
- Step 9 records that boblog's **root is vetoed**. Spec 0314 retired
  that: the root is named `bobapp.v1.Log` at +19 and reports
  `truncated: true`. Draft 2's beat 9 was built on the veto; draft 3's
  is built on the honest partial answer, which is a better beat.

## The starting point: ringer already is most of bobapp

`demo/ringer/` (spec 0241, draft) was written for a different demo and
turns out to be four fifths of what this one needs. What it already
does, verified on this machine:

- A real unary gRPC call to `google.maps.routing.v2.Routes/ComputeRoutes`
  at `routes.googleapis.com:443`, over TLS, authenticated by an API
  key, handled **reflectively** — no generated Rust type for any
  googleapis message.
- Embeds a `FileDescriptorSet` holding the transitive closure of
  `google/maps/routing/v2/routes_service.proto` and nothing else: **39
  files, 50 836 bytes**, against 7 771 files and 25.6 MB for the whole
  corpus. Embedded **raw, not compressed**, via `include_bytes!`.
- `--dump-descriptor <path>` writes that set back out.
- Captures the exact egress bytes **inside** the tonic codec, between
  serialization and the wire — not by re-encoding a copy, which may
  differ in field order and default elision.
- Writes a `Log`/`Entry` envelope whose payload fields are `bytes`,
  deliberately outside the reflected schema.

Two facts that were open and are now closed:

- **`protoscan bobapp` finds all 39 descriptors.** This was the one
  argument for rewriting bobapp in Go — protoc-gen-go embeds
  descriptors in every binary for free, and it was not obvious a Rust
  `include_bytes!` blob would be equally visible. It is. bobapp stays
  Rust, and no Go toolchain enters the repo.
- **The key still works.** `~/.config/bobapp/api-key` returns HTTP 200
  from `computeRouteMatrix` (Paris → Lyon, 465 590 m, 16 563 s). It is
  restricted to `routes.googleapis.com`; it will not authenticate
  anything else, which constrains open question 5 below.

## Step 1 — rename ringer to bobapp — **done 2026-08-14**

"ringer" names nothing the audience can see. Mechanical, and it should
land before anything else so no later work has to be renamed twice.

| From | To |
|---|---|
| `demo/ringer/` | `demo/bobapp/` |
| crate + binary `ringer` | `bobapp` |
| `RINGER_DESCRIPTOR_SET` | `BOBAPP_DESCRIPTOR_SET` |
| `RINGER_API_KEY` | `BOBAPP_API_KEY` |
| `nix-build -A ringer-desc` | `-A bobapp-desc` |
| `nix-build -A ringer` | `-A bobapp` |
| `~/.config/ringer/api-key` | `~/.config/bobapp/api-key` — **done** |

Also touches: the root `Cargo.toml` `[workspace] exclude` entry, the
`workspaceSrc` subtraction in `default.nix` (spec 0241 S2 — without it
every edit to bobapp rebuilds the entire Rust world), spec 0241
itself, and `demo/ringer/README.md`.

Spec 0241 is still `draft`. Rather than rewrite it in place, it should
be superseded by a new spec covering the whole artifact set, with 0241
marked superseded — its measured facts (the 720 MB `DescriptorPool`
figure, the API reconnaissance) stay valuable and should not be
rewritten out of existence.

## Step 2 — the log format — **done 2026-08-14**

boblog has to be an envelope the recovered schema *describes*, holding
payloads it mostly does not. That is what makes beat 9 half-readable
and beat 10 a payoff.

Today's shape (`demo/ringer/src/log.rs`), written by hand without a
`DescriptorPool`:

```proto
message Log   { repeated Entry entry = 2042; }
message Entry { bytes query = 42; bytes response = 43; }
```

The problem: those two messages are outside the embedded set, so
`bobapp.desc` cannot name the envelope either, and beat 9's "the
envelope resolves, the payloads do not" split does not exist — nothing
resolves.

**The fix is to move `Log`/`Entry` inside bobapp's own schema.** Add a
`bobapp/v1/log.proto` to the descriptor set bobapp embeds, so that:

- `bobapp.desc` names `bobapp.v1.Log` and `bobapp.v1.Entry` — the
  envelope opens in beat 9.
- The `bytes` payload fields stay `bytes` in the schema, so the heat
  cues have something to disagree with. This is the mechanism behind
  "some bytes/string fields are in fact hidden submessages".

Target shape:

```proto
package bobapp.v1;

message Entry {
  google.protobuf.Timestamp at = 1;
  string method   = 2;   // "/google.maps.routing.v2.Routes/ComputeRoutes"
  string note     = 3;   // a diagnostic: "sent" while in flight, "ok" after
  bytes  request  = 4;   // opaque on purpose
  bytes  response = 5;   // opaque on purpose
}
message Log { repeated Entry entry = 1; }
```

`method` matters: it is a *string field that names the type of an
adjacent bytes field*, which is exactly the human breadcrumb Alice
follows when deciding what to override the payload to. It also means
the demo never has to pretend the override was guessed from nothing.

**Not** `google.protobuf.Any`. Any carries a `type_url`, which
protolens resolves directly — the payload would just open, and beats 9
and 10 would both evaporate. Opaque `bytes` is the whole point.

Odd field numbers (2042, 42, 43) can go; they were a different demo's
joke and they cost credibility here.

**Built and verified.** `demo/bobapp/proto/bobapp/v1/log.proto` is
compiled into the embedded set alongside the 39 googleapis files, so it
is **40 files, 51 091 bytes**. `log.rs` no longer hand-rolls a varint:
it builds a `DynamicMessage` against the pool, by field name, the same
way `request.rs` builds the request. A live Grenoble→Lyon call writes a
10 045-byte `log.pb` that `prototext --descriptor-set <the embedded set>
decode --type bobapp.v1.Log` reads back with `at`, `method`, `request`
and `response` all named. Four unit tests run against the *real*
embedded descriptor set, not a stand-in, and one of them pins the
property the log exists for: a deliberately padded varint handed to the
recorder comes back byte-identical.

One bug fixed on the way: `build.rs` used `fs::copy`, which carried the
nix store's 0444 mode onto the copy in `OUT_DIR`, so a *second* build
could not overwrite it. Changing the descriptor set needed a
`cargo clean` first. It now reads and writes the bytes.

## Step 3 — the entries — **done 2026-08-15**

Beat 9 needs entries that resolve against `bobapp.desc`. Beat 10 needs
entries that do **not**, and that resolve against `googleapis.desc`.

The log now holds four, in this order:

| # | Method | Payload resolves with |
|---|---|---|
| 0 | `/google.maps.places.v1.Places/SearchText` | `googleapis.desc` only |
| 1 | `/google.maps.places.v1.Places/SearchText` | `googleapis.desc` only |
| 2 | `/google.maps.routing.v2.Routes/ComputeRoutes` | either |
| 3 | `/google.maps.routing.v2.Routes/ComputeRoutes` | either |

`google.maps.places.v1` is the service to pick because
`SearchTextRequest` has `string text_query = 1`, and a free-format
string is the field the demo wants to be able to point at. It is in
googleapis and it is **not** among bobapp's 41 embedded files.

### The lookups are real calls

bobapp resolves place names before it routes between them, so the
lookups come first in the log. They are as real as the routing ones —
same codec, same recorder, same key — so each is a request **and** a
response, and nothing about these entries is a pretence.

They very nearly were not. The key was allow-listed for
`routes.googleapis.com` only, and the Places API was not enabled on the
project, so an early probe came back 403 `API_KEY_SERVICE_BLOCKED`. Both
were fixed on the project (2026-08-15): `places.googleapis.com` enabled,
and the key's allow-list extended to `routes` **and** `places`. Note
that `gcloud services api-keys update --api-target` *replaces* the whole
allow-list — re-list routes or minting breaks.

Two consequences worth keeping:

- **They must come first, not last.** Anomaly 4 cuts 1 024 bytes off
  the tail, and the cut has to land *inside* the last record. A lookup
  entry is ~720 bytes; a route entry carrying its response is ~10 000.
  Put the lookups last and the cut swallows both of them and reaches
  back into a route entry.
- **Only the route request is rewritten.** `anomaly::patch_request`
  names `travel_mode` and stamps field 99 — both written in terms of
  `ComputeRoutesRequest` — so `rewrite_request` returns anything else
  untouched. The log therefore shows two pairs of entries, one pair odd
  and one pair ordinary, which is a truer picture than four identical
  ones.

### The live key can never reach the file

`main.rs`'s `refuse_the_live_key` scans the encoded log for the value of
`BOBAPP_API_KEY` and refuses to write if it finds it. It should never
fire — the key travels as an `x-goog-api-key` header and `Recorder` only
ever sees message bodies — but the artifact is committed to a public
repo, and "should never" is not "cannot". It runs **before** `cut_short`,
so a key sitting in the bytes about to be dropped is still an error
rather than a near miss nobody hears about. The synthetic key
`anomaly.rs` writes is a different string and is deliberately left
alone: it is the anomaly, not a leak.

### Where the descriptors come from

bobapp compiles in no Places service, so `--look-up` is gated behind
`--extra-descriptor-set <PATH>` and builds the request against a
descriptor set read off disk at run time. That is exactly why
`--dump-descriptor` cannot produce a schema that names these bytes:
they were never in the binary. It is also an ordinary way for a tool to
be wrong — a side-loaded proto that never made it into the build.

The lookup is deliberately more than one field. A three-field message
is too generic to score: with `text_query`, `language_code` and
`max_result_count` alone the top candidate under googleapis was
`google.ads.admanager.v1.ReportDefinition`. The shipped request also
sets `rank_preference`, `min_rating` and a `location_bias.circle` around
the leg's endpoint, which is both what a real search looks like and
enough shape to be recognized:

It deliberately does **not** set `open_now`. That field would make the
number of results — and so the size of the logged response — depend on
the hour the artifact was minted; a mint at midnight came back with one
result and then none.

| descriptor set | top candidate for the lookup payload | score | runner-up |
|---|---|---|---|
| `bobapp.desc` | `google.protobuf.GeneratedCodeInfo.Annotation` | −37 | −48 |
| `googleapis.desc` | **`google.maps.places.v1.SearchTextRequest`** | **+12** | −25 |

That pair of rows *is* beat 10: the same bytes, junk under one database
and named outright under the other.

The lookup's **response** does the same thing one notch louder — −50
under `bobapp.desc`, **+45** under googleapis — and lands on a genuine
tie between `SearchTextResponse` and `SearchNearbyResponse`, which the
sibling `method` field settles. Worth knowing about; probably not worth
stage time.

## Step 4 — the four anomalies — **done 2026-08-15**

The anomalies are not sprinkled on. In this story bobapp is a program
of unknown provenance that Bob downloaded, so a non-canonical,
slightly wrong encoder is *in character* — and it is the reason Alice
was called.

They go in the same place spec 0241 S16 already identified: the gap
inside the codec, between serializing and handing off. No other code
has to change for the log to stay truthful.

**How they are written: a prototext round trip.** The message is
encoded canonically, rendered to `#@ prototext` text, patched *as text*,
and encoded back — so the patch reads as the anomaly it is rather than
as byte surgery, and the vocabulary already supports all of it
(`val_ohb: N` for the padding, a repeated line for the duplicate, a
bare field number for the undeclared field). The truncation is not a
patch at all; it is a short write.

**Unconditional, not behind a flag.** An earlier draft leaned toward
`--legacy-encoder` to keep the canonical path testable. Rejected: this
is simply what the app *does*, and it is why Bob sent it to Alice. A
flag would put the demo's premise behind an opt-in that the audience
would reasonably ask about. The canonical path stays testable anyway,
because the patch is a function from bytes to bytes and can be tested as
one.

Which anomaly lands in which artifact:

| # | Anomaly | Lands in | Where exactly | In-story cause |
|---|---|---|---|---|
| 2 | A varint padded past minimal length | **request** | a scalar in the request | An encoder that reserves a fixed-width varint and never compacts it. |
| 3 | A field the schema does not declare | **request** | a tag inside the request | A newer build writing a field the recovered schema predates. |
| 1 | A singular field written twice, the first occurrence being a whole message hiding in a `string` | **lookup request** | `SearchTextRequest.text_query` | The app leaves a debug trace in the query it is about to send — a `google.rpc.Status` carrying the `x-goog-api-key` it authenticated with — then writes the real query over it. Last-one-wins hides the first from the server and from `protoc`. |
| 4 | A record truncated at the tail | **log** | the last `Entry` | The process was killed mid-write. |

### What was built, and what it measures

`demo/bobapp/src/anomaly.rs` is the whole of it: `rewrite_request` is
one text round trip per request type, `cut_short` is the short write.
The request rewrite sits in `codec.rs` between `item.encode` and
`record_request`, and the cut in `main.rs`'s `write_log`. Which of the
two patches runs is decided by the request's own type — the route call
gets 2 and 3, the place lookup gets 1 — so the log shows two pairs of
entries, odd in two different ways.

**The carrier of anomaly 1 is `SearchTextRequest.text_query`, and what
it carries is a message.** A duplicated diagnostic *string* is a
one-line observation; a duplicated field whose first value is a
serialized `google.rpc.Status` is one the tool has to earn. The trace
holds an `Any` that spells `google.rpc.ErrorInfo` out in its
`type_url`, and inside that an `ErrorInfo` filing the key under
`x-goog-api-key`, the name of the header the call really did travel in.
Scored against googleapis those 164 bytes have **exactly one candidate**
— `google.rpc.Status`, +8 — so protolens's heat cue does not merely say
"this is not a string", it names the message. The leak is not visible to
`protoc --decode`, which shows the second `text_query` and stops.

Every byte of the trace is ASCII, which is what lets it pass for the
`string` it is written into: a proto3 `string` has to be valid UTF-8 or
the server rejects the call, and Places accepted it. ASCII holds only
while every length varint inside the trace stays below 128, so
`debug_trace` checks the result and fails loudly if a constant grows
past that.

Verified against a live Grenoble→Lyon call (2026-08-16):

| Anomaly | On the wire | |
|---|---|---|
| 2 | `20 81 80 80 80 00` | field 4, value 1, spelled in five bytes |
| 3 | `9a 06 10 "bobapp/0.9.3-rc2"` | field 99, LEN, undeclared anywhere |
| 1 | `0a a4 01 <164-byte Status>` then `0a 12 "coffee in Grenoble"` | field 1 twice |
| 4 | 1 024 bytes short | `protoc --decode_raw` exits 1, "Failed to parse input" |

Two facts worth having on stage. **Google accepted the request** — the
padded varint and the undeclared field both went out and a full route
came back, which is the point of saying they are legal. And the fake
key is 39 characters beginning `AIza`, asserted by a test, because
that shape is what the room recognizes in the half-second it is up.

Fourteen tests in `demo/bobapp` cover it, including that each patch
**fails loudly** rather than silently doing nothing when its target line
is absent.

**Anomaly 4 forced bobapp to make two calls, and this is not
negotiable.** The first run wrote a one-entry log, and cutting its tail
made the *whole document* unreadable rather than just its end: a `Log`
is one LEN record per `Entry`, so with a single entry the cut lands in
the **outermost** record. `entry[0]` then claims 10 066 bytes with
9 042 remaining, and both prototext and protolens can only render the
file as one opaque `1: "…"`. Beat 9 would have had nothing to read.

bobapp now looks up both directions — the route asked for and the
return leg — which is in character for a round-trip planner and gives
the log entries that are whole. Verified: the first three render with
`at`, `method`, `note`, a `request` and a `response`; the two lookup
requests carry **both `text_query` lines**, the two route ones carry the
padded varint and field 99, and the last entry is truncated in place.
`protoc --decode_raw` still refuses the file.

The general rule this leaves behind: **a truncated tail needs something
in front of it to be the tail of.**

The split is forced by the beats, not chosen for balance. Beat 4 reads
the *request* with `prototext` before protolens has appeared, and the
outline needs `ohb` visible there — so 2 and 3 must be in the request.
And only a file can be cut short mid-write: a request that reached
Google was complete, so 4 can only be in the log. 1 goes with 4 because
its payoff is the escalation, and the escalation is about the log.

**Anomaly 1 is the one the talk is built on, and it should be the API
key.** Draft 1 called it "somebody's exfiltration channel", which
invites the audience to imagine an attacker and a threat model the
demo never substantiates. A downloaded utility that carelessly logs
your credential is more common, more checkable, and lands harder —
Bob's key is right there in Bob's log file, and `protoc --decode`
would have shown him a method name instead.

The key written into the fixture must be a **synthetic string of the
right shape**, never the real one from `~/.config/bobapp/api-key`.
The committed artifact goes in a public repo.

`prototext-core/tests/anomaly_fixture.rs` and spec 0226 are the
precedent for keeping a fixture like this honest: encode it, re-render
it, assert the re-encoding is byte-identical, and assert the set of
annotation tokens is exactly what was intended. boblog should get the
same treatment — four tokens expected, four found.

## Step 5 — bobshark

One request body, from a real call. bobapp already writes exactly this
(`--log-dir` captures the pre-frame message bytes, spec 0241 S15), so
bobshark is a copy of one `request.pb`.

Deliberately *not* a `.pcap` for now. A real capture of this traffic
is TLS, so it would need `SSLKEYLOGFILE` plus tshark to decrypt, and
tshark is not installed here. That is a second tool on screen for no
new claim. Recorded as synopsis open question 2; if it ever becomes
one command, it is worth 30 seconds.

**Done 2026-08-15.** bobshark is `ComputeRoutes`, not
`ComputeRouteMatrix`: step 6 measured the collision on the method bobapp
actually calls, so there is no reason to add a second one. It is **84
bytes**, the `request` field of the log's first entry, lifted out
verbatim.

The earlier note that "which method bobshark should be depends on the
collision below" is answered: the method was never the variable. The
*waypoint shape* was.

## Step 6 — beat 8's collision — **measured 2026-08-15**

This was the highest-risk item in the plan, because beat 8 is the
headline and asserted something unmeasured: that inference prefers
`google.maps.routes.v1` over the `routing.v2` type the producer sent.

**The collision is real, and it is a tie, not a wrong ranking.**

### Why v1 can never outrank v2

`routes.v1.ComputeRoutesRequest`'s field numbers are a **strict subset**
of `routing.v2.ComputeRoutesRequest`'s — v1 stops at 13, v2 continues
with 14, 15, 16, 18, 19, 20 — and the shared numbers carry the same
types. So no payload can make v1 score *higher*. A tie is the ceiling,
and the beat has to be written to that.

The field that decides whether the tie happens at all is in `Waypoint`:

```proto
// routing/v2/waypoint.proto        // routes/v1/waypoint.proto
Location location = 1;             Location location = 1;
string   place_id = 2;             string   place_id = 2;
string   address  = 7;             // ← v1 has no address
```

An address waypoint therefore names v2 uniquely. A **lat/lng** waypoint
is describable by both, identically.

### The three measurements

| bobshark's waypoints | `--descriptor-set` | outcome |
|---|---|---|
| `address` | googleapis | `routing.v2` alone, −22 |
| `location.lat_lng` | googleapis | **tie: `routes.v1` and `routing.v2`, both −16**; next candidate −37 |
| `location.lat_lng` | bobapp.desc | `routing.v2` alone, −16; next candidate −55 |

The tie survives the anomalies unchanged, and the two candidates render
the payload **identically, field for field** — which is a stronger thing
to show than a wrong guess: the tool is not confused, the two schemas
genuinely say the same thing about these bytes.

The score breakdown is a bonus. On screen, before anyone has opened the
file:

```
unknown: 1          ← field 99, the undeclared agent string
non_canonical: 1    ← the padded travel_mode varint
```

Two of the four anomalies, counted by the scorer.

### What was changed to get it

`request::waypoint` now takes whichever arm of the `location_type` oneof
the caller named the place with: `coordinates()` splits on a comma and
requires **both** halves to parse as `f64`, so `"Grenoble, France"` is
still an address and `"45.188529, 5.724524"` is a point. Nothing was
manufactured — the collision is a property of the two published schemas,
and bobapp sends coordinates because that is what a routing client that
has already geocoded sends.

`bobapp --origin "45.188529, 5.724524" --destination "45.764043, 4.835659"`
is now the minting command.

### Consequence for beat 8 — **resolved 2026-08-17 by step 10**

Beat 8 runs against the *older* build's database, and there the answer
is unambiguous and correct: `routing.v2` at −16, next candidate −55.
The collision needs a database holding both Routes versions, and
`bobapp1.desc` holds one.

When this was written the only such database was `googleapis.desc`, so
the manual-override detour had nowhere to live but the epilogue. Step 10
gave it a home: `bobapp2.desc` embeds `google/maps/routes/v1` as well,
so the tie appears in beat 10, one command after beat 9, out of Bob's
own second binary — and the tie is now *evidence for* the escalation
rather than a wart on it. The synopsis's old open question 1 is closed.

## Step 7 — googleapis.desc — **nothing to build**

An earlier draft had this step build a `merged.desc` holding googleapis
*plus* bobapp's recovered files. It is deleted. protolens takes a single
`--descriptor-set`, so "both at once" was never a mode it had, and the
merged set would have been one more artifact the audience had to take on
trust. The two databases stay disjoint, neither one reads the whole
file, and the gap between them is closed on stage by a `path:field`
override — which is beat 10's subject anyway. See the synopsis, "The two
schema databases".

So this step is only a lookup. The corpus lives at
`$PROTOTEXT_GOOGLEAPIS_SET`, whose store path **changes on rebuild** —
resolve it with `nix-build --no-out-link -A googleapis-db` and never
from a written-down path. Several stale `*-googleapis-db` paths coexist
in the store and an old one carries a v2 scoring graph the current
binary rejects.

Beat 6's databases — `bobapp1.desc` and `bobapp2.desc` since step 10 —
are built on stage, one command each, from the binaries.
`googleapis.desc` is the repo's own corpus, shipped by CI and not made
for this talk.

## Step 8 — mint the artifacts — **re-minted 2026-08-16**

One live run, with the key, producing files that are then **committed
and never re-minted during a talk**:

```sh
BOBAPP_API_KEY=$(cat ~/.config/bobapp/api-key) \
  bobapp --origin "45.188529, 5.724524" \
         --destination "45.764043, 4.835659" \
         --extra-descriptor-set "$GOOGLEAPIS_DB/googleapis.desc" \
         --look-up "coffee in Grenoble" \
         --look-up "bouchon lyonnais" \
         --log-dir grpconf/stage/
```

Grenoble ↔ Lyon, as coordinates, for the reason step 6 gives. All four
calls succeeded; **Google accepted the padded varint, the undeclared
field and the duplicated `text_query`** and returned a full route both
ways. `log.pb` is 20 243 bytes, four entries:

| # | Method | Request | Response |
|---|---|---|---|
| 0 | `Places/SearchText` | 240 | 508 |
| 1 | `Places/SearchText` | 238 | 484 |
| 2 | `Routes/ComputeRoutes` | 84 | 9 868 |
| 3 | `Routes/ComputeRoutes` | 84 | 9 633, of which **8 609 are on disk** |

Each lookup request is 164 bytes bigger than the query alone needs: that
is anomaly 1's trace, sitting in front of the real `text_query`.

Entry 3 claims 9 633 bytes and 8 609 remain — the 1 024-byte cut lands
inside its response, which is where anomaly 4 has to land.

Measured on the result, against `googleapis.desc`:

| node | top candidate | score | runner-up |
|---|---|---|---|
| the whole file | — | vetoed | — |
| a lookup request | `google.maps.places.v1.SearchTextRequest` | −8 | none |
| **its first `text_query`** | **`google.rpc.Status`** | **+8** | **none** |
| a lookup response | `SearchNearbyResponse` **tie** `SearchTextResponse` | +45 | — |

The lookup request used to score +12; the duplicated field is what costs
it the twenty points, and it is still the only candidate.

The demo itself must not touch the network. Rehearse it in airplane
mode at least once.

## Step 9 — read the artifacts back — **works, as designed**

Measured on the minted `log.pb` against the on-stage `bobapp.desc`
database.

### The root has no type, and that is the beat — **SUPERSEDED**

> **This section is a record, not a fact.** Spec 0310 stopped a cut tail
> from vetoing every candidate, and spec 0314 (2026-08-17) wired the
> same assertion into the `prototext` CLI, so both tools now agree. The
> root of boblog is named `bobapp.v1.Log`, **score 19**, 24 matched,
> 0 unknown, 0 mismatched, `truncated: true`, under both bobapp
> databases. Everything below this line describes the behavior before
> those two specs; the numbers in the cue table are still accurate,
> because the cues never depended on the root.
>
> The beat got better, not worse. "Nothing can name this file" was a
> demonstration of a limit; "here is the answer, and here is exactly how
> much of the file it does not cover" is a demonstration of the thesis.
> Draft 3's beat 9 is written to the second one and ends on the
> `method:` line instead of on a shrug.

```
protolens: inferring root type (18 KB) on 8 threads...
protolens: rendering root node as <raw / no type> (18 KB)...
```

`prototext list-schemas` returned an **empty candidate list** for the
whole file. The cause was anomaly 4: the cut record made every candidate
walk off the end, and an incomplete walk was a veto.

That was not a failure to route around; it was what beat 9 was *for*. A
document nothing can name is exactly the document whose **heat cues**
have something to say, and they do:

| node | top suggestion | score | runner-up |
|---|---|---|---|
| `/1` (a lookup entry) | `bobapp.v1.Entry` | −12 | −47 |
| `/3` (a route entry) | `bobapp.v1.Entry` | −12 | −47 |
| `/3:4` (its request) | `google.maps.routing.v2.ComputeRoutesRequest` | −16 | −55 |
| `/3:5` (its response) | `google.maps.routing.v2.ComputeRoutesResponse` | **+651** | +1 |

Every gap is decisive — 35, 35, 39 and 650 points — so the cue is
pointing, not shrugging. All four entries score alike, which is the
right answer: they *are* alike, and what differs is one level down.

**One `path:field` override at `/:1` puts the whole log in the clear**,
because all four entries are field 1 of the root. And the last one
renders *partially*, because it is the truncated tail: anomaly 4 becomes
visible in place, in the same view, instead of being announced. The cue
one level down is beat 10's escalation already queued up.

### An override path is positional, so it can name one entry

`OverrideOrigin::Path` is a `positional_path` — `/1` is the *first*
child, `/3` the third — and `PathField` is such a path plus a field
number. So `/:1` reaches all four entries while `/3:4` reaches exactly
one entry's `request`. The mixed log does not force a choice between
`ComputeRoutesRequest` and `SearchTextRequest` at a single origin; each
gets its own. `PathField` also works when the parent is raw, so none of
this needs the envelope to be named first.

bobapp's own closing advice — "no `--type`, the sweep names the message"
— is the one thing here that is wrong about the file bobapp just wrote,
and should be reworded.

### Undamaged, `BytesValue` outscores the real envelope

With the cut record removed cleanly (the first 10 081 bytes):

| type | score | matches | non-canonical |
|---|---|---|---|
| `google.protobuf.BytesValue` | **+1** | 1 | 0 |
| `bobapp.v1.Log` | −11 | 9 | 1 |

A one-field wrapper wins a document whose top level is one LEN field,
because it matches everything it sees and has nothing to be penalized
for, while the true type is docked for what it finds inside. This is a
scoring question, not a demo question — `docs/scoring-flaws.md` is where
it belongs. It costs the demo nothing, because the root is vetoed
anyway and the beat runs off the cue, not off root inference.

### Reading the DB back at the working tree's graph version

`bin/reproto --schema-db-out` loads the **nix-store**
`prototext_graph_lib` and therefore writes a v4 scoring graph, which
working-tree binaries reject. Shim the freshly built cdylib in front of
it rather than waiting on `nix-build`:

```sh
SHIM=/tmp/gshim/prototext_graph_lib && mkdir -p "$SHIM"
cp target/release/libprototext_graph_lib.so "$SHIM/prototext_graph_lib.so"
# __init__.py re-exports build_fds_index and build_graph
PYTHONPATH=/tmp/gshim ./bin/reproto --schema-db-out /tmp/bobapp-db/bobapp.desc bobapp.desc
```

## Step 10 — two builds, two databases — **done 2026-08-17**

The change draft 3 of the synopsis is built on. Bob downloaded bobapp
**twice**: an older build and a newer one. The escalation that used to
run bobapp → googleapis now runs bobapp1 → bobapp2, with googleapis
demoted to a droppable epilogue, so the demo's central move is made out
of two artifacts that are both Bob's.

**How it is built.** One Rust/tonic source tree, built twice from
`demo/bobapp/default.nix` with a `variant` argument. `pname` and the two
env vars are deliberately outside `commonArgs` so both variants share
one dependency cache. Nix attributes: `bobapp1`, `bobapp2`,
`bobapp1-desc`, `bobapp2-desc`. `bobapp` and `bobapp-desc` no longer
exist, and `grpconf-demo` stages `bin/bobapp1` and `bin/bobapp2`.

The only difference between the two is the entry-point list handed to
`protoc` when the embedded `FileDescriptorSet` is made:

| | embedded files | dumps | recovered database |
|---|---|---|---|
| `bobapp1` | 41 | 51 111 B | `bobapp1.desc`, 64 816 B |
| `bobapp2` | 77 | 103 430 B | `bobapp2.desc`, 130 044 B |

bobapp2 adds Places v1, Routes **v1**, and `google/rpc/error_details.proto`.
Those three are the whole second half of the talk: the first names the
lookup traffic, the second creates the tie, and the third is what makes
the leaked `google.rpc.Status` open all the way to the key inside its
`Any` (+3 in a three-way tie → **+8 sole**, and `--no-expand-any`
reproduces the +3 exactly, so the five points are demonstrably the
expanded `Any`).

**The trap that will cost a rehearsal.** In *both* embedded sets the
last `FileDescriptorProto` is `bobapp/v1/log.proto`, and the spec 0313
bug discarded the final descriptor whenever any byte followed it. On a
shell holding a pre-0313 `fdp_scan_lib`, `protoscan` prints 40 and 76
instead of 41 and 77 and — much worse — `reproto` emits

```
Warning: missing dependency file:bobapp/v1/log.proto
```

and **silently builds a database with no envelope in it**, which guts
beats 9 through 11. It is a warning rather than a refusal because that
is reproto's posture on partial input, and that posture is right; it is
simply invisible here. `presentation.sh` now opens with

```sh
protoscan $BOBAPP1 | wc -l ; protoscan $BOBAPP2 | wc -l
```

as a gate. The answers must be 41 and 77.

**Verified against the staged files, 2026-08-17.** Every score the
synopsis quotes for beats 9–12 was re-measured with `prototext
list-schemas` on the real bytes; the table lives in the synopsis under
"The measured escalation". The four `.script` files were re-pinned by
driving the binary headlessly.

## Open items

- ~~Whether bobapp *calls* the extra service or merely builds and logs
  its payload.~~ **Closed 2026-08-15: it calls.** The block was
  Google's, not a design choice — 403 `API_KEY_SERVICE_BLOCKED` on
  project `187176146673` — and it was lifted with two `gcloud` actions:
  `services enable places.googleapis.com`, and an `api-keys update`
  re-listing routes *and* places. `anomaly::rewrite_request` now returns
  anything that is not a `ComputeRoutesRequest` untouched, which is what
  it should have done anyway.
- ~~Whether the anomaly writer lives behind a flag.~~ **Closed
  2026-08-14: unconditional.** See step 4.
- ~~Whether beat 8's collision has a home.~~ **Closed 2026-08-17 by
  step 10:** it lives in beat 10, under `bobapp2.desc`.
- Whether spec 0241 is amended or superseded. Leaning superseded. It
  now also predates the two-variant split.
- ~~Sizes and timings for the synopsis's "honest numbers on screen"
  line, once boblog exists.~~ **Closed 2026-08-17.** boblog is
  20 243 B; open times are 17 / 17 / 82 ms against `bobapp1.desc` /
  `bobapp2.desc` / `googleapis.desc`.
