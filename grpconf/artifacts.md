<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Building bobapp, bobshark and boblog

The build plan for the three artifacts `grpconf/synopsis.md` runs on.
Read the synopsis first — this document only says how the files get
made, not why they are shaped the way they are.

Status: **every step but 7 is done** (2026-08-14/15); step 7 is plan.
Step 6's measurement came back with two answers, neither of them the one
beat 8 is written against — see "Step 6" below, and the synopsis's open
question 1. What is left for the artifacts is the *narration*: the
synopsis's beats 8 and 10 have not caught up with steps 3 and 6.

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
  string note     = 3;   // a diagnostic — and anomaly 1's carrier (step 4)
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
sets `rank_preference`, `open_now`, `min_rating` and a
`location_bias.circle` around the leg's endpoint, which is both what a
real search looks like and enough shape to be recognized:

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
| 1 | A singular field written twice | **log** | `Entry.note` | The app writes a diagnostic while the call is in flight — which is the outgoing `x-goog-api-key` header — then overwrites it with `ok` when the call lands. Last-one-wins hides the first. |
| 4 | A record truncated at the tail | **log** | the last `Entry` | The process was killed mid-write. |

### What was built, and what it measures

`demo/bobapp/src/anomaly.rs` is the whole of it: `rewrite_request` and
`rewrite_log` are the two text round trips, `cut_short` is the short
write. The request rewrite sits in `codec.rs` between `item.encode` and
`record_request`, the log rewrite at the end of `Recorder::encode_log`,
and the cut in `main.rs`'s `write_log`.

**The carrier of anomaly 1 is `Entry.note`, not `Entry.method`.** Two
reasons. `method` is what tells one call from another in a multi-entry
log, and corrupting it costs the demo a landmark it needs. And a
free-text diagnostic that gets overwritten is the *ordinary* shape of
this bug — debug logging left in — which is exactly the small,
defensible claim the synopsis asks for. `note` is numbered **3**, ahead
of `request` (4) and `response` (5), so that anomaly 4's tail cut
cannot reach it: with one entry in the log, all four anomalies are on
one screen, in reading order.

Verified against a live Grenoble→Lyon call (2026-08-15):

| Anomaly | On the wire | |
|---|---|---|
| 2 | `20 81 80 80 80 00` | field 4, value 1, spelled in five bytes |
| 3 | `9a 06 10 "bobapp/0.9.3-rc2"` | field 99, LEN, undeclared anywhere |
| 1 | `1a 37 "x-goog-api-key: AIza…"` then `1a 02 "ok"` | field 3 twice |
| 4 | 1 024 bytes short | `protoc --decode_raw` exits 1, "Failed to parse input" |

Two facts worth having on stage. **Google accepted the request** — the
padded varint and the undeclared field both went out and a full route
came back, which is the point of saying they are legal. And the fake
key is 39 characters beginning `AIza`, asserted by a test, because
that shape is what the room recognizes in the half-second it is up.

Twelve tests in `demo/bobapp` cover it, including that the patch **fails
loudly** rather than silently doing nothing when its target line is
absent.

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
`at`, `method`, **both `note` lines**, and a `request`; the route ones
carry the padded varint and field 99, and the last entry is a bare `1:`
holding the bytes that survived the cut. `protoc --decode_raw` still
refuses the file.

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

### Consequence for beat 8, which is **not yet resolved**

Beat 8 as written runs `protolens --descriptor-set bobapp.desc bobshark`
— and against `bobapp.desc` the answer is unambiguous and *correct*, 39
points clear. The collision only exists against `googleapis.desc`, which
beat 10 is what introduces. So the beat's manual-override detour has no
home where it currently sits. See the synopsis's open question 1.

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

Beat 6's `bobapp.desc` is built on stage, in one command, from the
binary. `googleapis.desc` is the repo's own corpus, shipped by CI and
not made for this talk.

## Step 8 — mint the artifacts — **done 2026-08-15**

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
calls succeeded; **Google accepted the padded varint and the undeclared
field** and returned a full route both ways. `log.pb` is 20 198 bytes,
four entries:

| # | Method | Request | Response |
|---|---|---|---|
| 0 | `Places/SearchText` | 75 | 532 |
| 1 | `Places/SearchText` | 73 | 518 |
| 2 | `Routes/ComputeRoutes` | 84 | 9 868 |
| 3 | `Routes/ComputeRoutes` | 84 | 9 480, of which **8 456 are on disk** |

Entry 3 claims 9 690 bytes and 8 666 remain — the 1 024-byte cut lands
inside its response, which is where anomaly 4 has to land.

The demo itself must not touch the network. Rehearse it in airplane
mode at least once.

## Step 9 — read the artifacts back — **works, as designed**

Measured on the minted `log.pb` against the on-stage `bobapp.desc`
database.

### The root has no type, and that is the beat

```
protolens: inferring root type (18 KB) on 8 threads...
protolens: rendering root node as <raw / no type> (18 KB)...
```

`prototext list-schemas` returns an **empty candidate list** for the
whole file. The cause is anomaly 4: the cut record makes every candidate
walk off the end, and an incomplete walk is a veto.

This is not a failure to route around. It is what beat 9 is *for*. A
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
for, while the true type is docked for the doubled `note`. This is a
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
- Whether spec 0241 is amended or superseded. Leaning superseded.
- Sizes and timings for the synopsis's "honest numbers on screen"
  line, once boblog exists.
