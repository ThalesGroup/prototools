<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0241 — a real call leaves bytes worth opening

Status: draft
App: bobapp
Refs: docs/specs/0226-a-fixture-shows-every-anomaly.md (the demo blob this
        one complements), docs/specs/0228-the-well-known-types-are-the-default-descriptor-set.md
        (why the build variable is not `PROTOTEXT_DESCRIPTOR_SET`),
        docs/specs/0239-the-schema-says-where-a-descriptor-ends.md (the
        feature-unification trap a new workspace member would re-enter)

## Background

protolens has one demo blob, `grpconf/anomalies.pb` (spec 0226). It is a
fixture: every field exists to exhibit an annotation token, and it is
opened with `--type` because it must be — it is a
`FileDescriptorProto` wearing a costume. It answers "what does protolens
show when the bytes are wrong". It does not answer the question every
viewer actually has, which is "what does protolens show me about *my*
traffic".

Two capabilities have no demo at all as a result:

- **Inference.** The sweep that names a message from bytes alone
  (specs 0217/0218) is the machinery protolens is largely built around,
  and nothing in the repo exercises it in front of an audience. Against
  the googleapis set — 7 771 files, 58 777 types — naming an unlabeled
  blob is a genuinely surprising trick. Against a hand-made fixture with
  a `--type` flag on the command line it is invisible.
- **Scale.** The 25 MB `googleapis.desc` and its sidecars exist only
  inside `nix-build -A full-tests`. A viewer never sees the tool open a
  blob against a descriptor set that large.

There is also nothing in the repo that produces bytes from a live wire.
`demo/01-tutorial.sh` drives committed fixtures end to end; the wire is
always something we wrote.

The two capabilities are **two demonstrations, and this spec is only the
first**. Inspecting the logged bytes against a small, freshly built schema
DB is a self-contained story: the application carries its own descriptors,
`reproto` turns them into a scoring DB on the spot, protolens names the
message. Re-running the same inspection against the full precomputed
`googleapis.desc` is the second story, and it shows more precisely
*because* the first one showed less. Building one artifact that serves
both would blur them.

## Goals

- **G1.** `bobapp` makes a real unary gRPC call to a live Google API over
  TLS, authenticated by an API key, and exits with the server's status.
- **G2.** Every googleapis message it builds or reads is handled
  **reflectively**, from descriptors. No `tonic-build`, no generated Rust
  type for any googleapis message.
- **G3.** The descriptor set is embedded in the executable, and holds
  **only what bobapp actually calls** — the transitive closure of one
  service file, not all of googleapis. That is what a real application
  ships, and it keeps the binary and the pool honest.
- **G3b.** `bobapp --dump-descriptor <path>` writes that embedded set back
  out, so `reproto --schema-db-out` can build a scoring DB from it at demo
  time. The schema the audience inspects with is then provably the schema
  the application was built against — extracted from the binary in front
  of them rather than prepared earlier.
- **G4.** The exact bytes bobapp put on the wire are written to a file
  that `protolens` opens **without `--type`**, so the inference sweep
  names the message.
- **G5.** bobapp's response is logged the same way. Same codec, opposite
  trait; and `ComputeRoutesResponse` is the larger and more interesting
  of the two messages, so the demo gets its best artifact for one extra
  `impl`.
- **G6.** Adding bobapp does not change what the rest of the workspace
  compiles, and does not add a single derivation to `nix-build -A ci`.

## Non-goals

- **N1.** Streaming RPCs. Unary only; framing a stream is a second
  problem and teaches nothing about protolens.
- **N2.** OAuth, ADC, service accounts. An API key in an environment
  variable is the shortest credible path to a live response.
- **N3.** A general-purpose `grpcurl`. The method is fixed and the request
  is built from a handful of flags. Arbitrary request authoring would
  make bobapp a tool, and a tool needs a spec of its own.
- **N4.** Any test that touches the network. A demo whose test suite needs
  a credential and a route to the internet is a demo that is broken half
  the time. Everything under test stops at the encoder.
- **N5.** Using `prototext-core`, `prototext-schema` or any other crate
  from this workspace. Deliberate — see *Alternatives considered*: the
  claim being demonstrated is that an *ordinary* gRPC application's bytes
  are readable, and an application that already speaks prototext proves
  nothing.
- **N6.** Publishing to crates.io.

## Specification

### Where it lives (G6)

- **S1.** `demo/bobapp/` is its own Cargo project with its own
  `Cargo.lock`, named in the root `Cargo.toml`'s `[workspace] exclude`.

  tonic, hyper, rustls and tokio appear nowhere in the workspace
  dependency graph today. As a workspace member, bobapp would pull all
  four into `depsCache` and into every `--workspace` derivation, because
  cargo unifies features across the whole workspace — the trap spec 0239
  hit from a single leaf-crate dependency. Exclusion keeps
  `nix-build -A ci` byte-identical to what it builds today.

- **S2.** `workspaceSrc` subtracts `demo/bobapp` by `lib.fileset.difference`.
  `crane.fileset.commonCargoSources ./.` admits any `.rs`/`.toml` it finds,
  `[workspace] exclude` notwithstanding, so without the subtraction every
  edit to bobapp would change `workspaceSrc`'s hash and rebuild the entire
  Rust world.

- **S3.** `publish = false`.

### The embedded descriptors (G3, G3b)

- **S4.** A `bobappDesc` derivation runs `protoc --include_imports` over
  the single file `google/maps/routing/v2/routes_service.proto` from the
  pinned googleapis corpus, producing a `FileDescriptorSet` holding that
  file and its transitive imports and nothing else. This is the same
  `protoc` invocation `googleapisPbs` makes, narrowed from the whole
  corpus to one entry point.

- **S5.** `build.rs` reads `BOBAPP_DESCRIPTOR_SET`, copies the file to
  `$OUT_DIR/bobapp.desc`, and emits `cargo::rerun-if-env-changed` plus
  `cargo::rerun-if-changed` for it. The binary does `include_bytes!`.

- **S6.** The variable is `BOBAPP_DESCRIPTOR_SET`, **not**
  `PROTOTEXT_DESCRIPTOR_SET`. Spec 0228 makes the latter point at the
  well-known types in both shells, so reusing it would mean that
  `cargo build` inside a dev-shell silently produced a bobapp that cannot
  name `google.maps.routing.v2.Routes`, and the failure would surface as a
  runtime "type not found" a long way from its cause.

  It is not `PROTOTEXT_GOOGLEAPIS_SET` either (the dev-shell's path to the
  full corpus DB): bobapp embeds its own narrow set, and reaching for the
  7 771-file one would be exactly the mistake S4 exists to avoid.

- **S7.** If the variable is unset, the build **fails**, naming
  `nix-build -A bobapp-desc --no-out-link` in the error. No fallback to
  the WKTs: a bobapp that cannot describe the service it calls is not a
  bobapp, and a demo that half-works is worse than one that refuses to
  build.

- **S8.** Embedded raw, not compressed, and the `DescriptorPool` is built
  once behind a `OnceLock`.

  On the narrow set this is cheap. On the full corpus it is not, which is
  the measurement that settled S4: `DescriptorPool::decode` over the
  25.6 MB `googleapis.desc` takes **1.34 s cold / 0.76 s warm** and peaks
  at **720 MB RSS** — about 28x the file — for 7 771 files and 49 255
  messages. That is a defensible cost for protolens, which exists to open
  large schema sets. It is not what an application that calls one method
  would ship, and a demo whose whole claim is "this is an ordinary app"
  cannot open by contradicting itself.

- **S9.** `bobapp --dump-descriptor <path>` writes the embedded set to
  `<path>` and exits, before any network setup. That is what makes the
  demo self-contained: the audience extracts the schema from the binary,
  runs `reproto --schema-db-out` on it to get `hopcroft.rkyv`,
  `index.rkyv` and a decompiled `proto/` tree, and inspects the logged
  request against a DB built in front of them from bytes that were inside
  the executable a minute earlier.

### The call (G1, G2)

- **S10.** A `DynamicCodec` implements `tonic::codec::Codec` over
  `prost_reflect::DynamicMessage`, parameterized by the request and
  response `MessageDescriptor`s taken from the pool, and is driven through
  `tonic::client::Grpc::unary`. This is the entirety of the reflection
  story: the method path is a string, the messages are dynamic, and
  nothing about googleapis is known at compile time.

- **S11.** Endpoint `https://routes.googleapis.com`, method path
  `/google.maps.routing.v2.Routes/ComputeRoutes`. TLS through rustls with
  the platform root store.

- **S12.** Request metadata carries `x-goog-api-key` from `BOBAPP_API_KEY`
  and `x-goog-fieldmask` (Routes rejects a call without a field mask).
  The key is read from the environment and from nowhere else — never a
  flag, so it cannot reach shell history or `/proc/<pid>/cmdline`.

- **S13.** CLI:
  `bobapp --origin <address> --destination <address> [--travel-mode DRIVE] [--depart-in <duration>] [--log-dir <dir>]`.

  `--origin`/`--destination` fill `Waypoint.address`, one arm of that
  message's `location_type` oneof. That is the shortest path to a request
  that is still structurally rich: two nested submessages each containing
  a oneof, three enums (`RouteTravelMode`, `RoutingPreference`, `Units`),
  and — with `--depart-in` — a `google.protobuf.Timestamp`. All of it
  visible in protolens, and none of it requiring the viewer to know the
  API.

- **S14.** Fields are set through `DynamicMessage::set_field_by_name`, so
  a wrong name is a run-time error rather than a compile error. That is
  not a shortcoming to apologize for in a comment; it is the property
  being demonstrated.

### The log (G4, G5)

- **S15.** `--log-dir DIR` writes `DIR/request.pb` and `DIR/response.pb`:
  the message bytes exactly as the codec produced and consumed them, with
  the five-byte gRPC frame header (compression flag plus big-endian
  length) stripped. protolens reads messages, not frames; left in place,
  the leading zero byte would be decoded as a field-0 tag and the blob
  would open as garbage.

- **S16.** The bytes are captured **inside** the codec, not by re-encoding
  a copy of the request afterwards. A second encode may legitimately
  differ from the first in field order and default elision, and the demo's
  whole claim is that these are the bytes that went out.

- **S17.** On success bobapp prints the command that reads them back:

  ```
  protolens --descriptor-set <the path build.rs was given> <dir>/request.pb
  ```

  with no `--type` — naming the message from the bytes is what the
  inference sweep is for. The schema DB named there is the one
  `reproto --schema-db-out` built from S9's dump, so nothing in the
  printed command was prepared before the demo started.

### Nix (G6)

- **S18.** `bobappDesc` (S4) is exposed as `nix-build -A bobapp-desc`, and
  a crane derivation building bobapp against it as `nix-build -A bobapp`.
  Both are cheap — `bobappDesc` is one `protoc` run over one file — so
  both belong in `ci`. Neither pulls in `googleapisDb`, which is what kept
  the earlier draft's version of this out of `ci`.

- **S19.** Neither shell exports `BOBAPP_DESCRIPTOR_SET`: it is a
  build-time input to one excluded Cargo project, not a property of the
  environment. `demo/bobapp/README.md` gives the one-liner instead:

  ```
  BOBAPP_DESCRIPTOR_SET=$(nix-build -A bobapp-desc --no-out-link)/bobapp.desc \
    cargo build --release --manifest-path demo/bobapp/Cargo.toml
  ```

### The second demonstration

- **S20.** Out of scope here, and recorded so that the first demo is not
  grown to accommodate it: the same `request.pb` is re-opened against the
  full `$PROTOTEXT_GOOGLEAPIS_SET`. Nothing in bobapp changes for it — the
  log is just bytes, and which descriptor set protolens is pointed at is a
  command-line argument.

## Alternatives considered

**bobapp as a workspace member.** Rejected: cargo unifies features across
a workspace, so tonic/hyper/rustls/tokio would enter every
`--workspace` derivation and `depsCache`. Spec 0239 records what one
leaf-crate dependency already cost here.

**`PROTOTEXT_DESCRIPTOR_SET` as the build variable.** Rejected: spec 0228
makes it point at the well-known types in both shells, so the common case
— `cargo build` inside a dev-shell — would produce a silently useless
binary.

**Building the request with `prototext encode` from a prototext file.**
Rejected as circular. The demo asserts that an ordinary application's
egress is inspectable; an application that authors its requests in
prototext has assumed the conclusion.

**Embedding the whole 25.6 MB `googleapis.desc`.** This is what the first
draft specified, on the reasoning that one artifact could serve both the
inference demo and the scale demo. Rejected on two counts. The cost was
measured and is not incidental — 1.34 s cold to build the pool and 720 MB
peak RSS, for an application that calls one method (see S8). And it is not
what a real application ships, so it would undercut the exact claim the
demo exists to make. Scale is now a second demonstration against
`$PROTOTEXT_GOOGLEAPIS_SET` (S20), which costs bobapp nothing because the
log is just bytes.

**Reusing `prototext-schema`'s lazy index instead of
`DescriptorPool::decode`.** Faster to start, and rejected for the same
reason as the previous entry plus one more: it would couple a demo to an
internal crate's layout. Moot now that S4 keeps the set small.

**grpcurl, or a shell script around it.** Rejected: it does not embed
descriptors, and "the app carries its own schema" is half of what makes
the logged bytes interesting.

**Logging the framed stream rather than the message.** Rejected: see S15.

**`google.cloud.language.v1.AnalyzeEntities` as the method.** Present in
the pinned corpus and simpler to call, but its request is one string and
two enums — nothing to look at in protolens. Its *response* is rich, which
is an argument for a second method someday, not for this one.

**Committing a recorded response fixture instead of calling live.**
Rejected: the recording would need a key to refresh, and would go stale
silently. The live call is the point; the committed golden blob in the
test plan is a request, which needs no credential to produce.

## Test plan

None of these touch the network (N4).

1. `pool_resolves_compute_routes` — the embedded set resolves
   `google.maps.routing.v2.ComputeRoutesRequest` and
   `ComputeRoutesResponse`, and `pool.files().len()` is in the tens, not
   the thousands. Establishes both that `build.rs` was given a real
   descriptor set and that S4 narrowed it.
2. `request_round_trips_through_the_codec` — build the request from CLI
   arguments, encode it with `DynamicCodec`, decode it back through the
   same descriptor, assert the field values match. Exercises S14/S16's
   encode path with no socket.
3. `logged_bytes_carry_no_frame_header` — the first byte of the captured
   request parses as a protobuf tag, not as a compression flag.
4. `missing_api_key_fails_before_connecting` — with `BOBAPP_API_KEY`
   unset, bobapp exits non-zero and opens no connection.
5. `dumped_descriptor_is_the_embedded_one` — `--dump-descriptor` writes
   bytes equal to the embedded slice, and `reproto --schema-db-out` on the
   result produces `hopcroft.rkyv`, `index.rkyv` and a `proto/` tree. This
   is G3b, and it is what makes the demo self-contained.
6. `protolens_names_the_request` — protolens's batch export over a
   committed golden `request.pb`, against the schema DB from test 5, with
   no `--type`; assert the inferred root type is
   `google.maps.routing.v2.ComputeRoutesRequest`. G4 stated as an
   assertion. Needs no googleapis DB now, so it can live in `ci`.
7. Manual, with a real key: `bobapp --origin … --destination … --log-dir /tmp/r`
   returns a route, writes both files, and the command it prints opens
   `request.pb` with the message correctly named.

## Measured outcome

Filled in at implementation. Record: the narrowed set's file count and
byte size against the full corpus's 7 771 / 25.6 MB, `DescriptorPool::decode`
time for it, the stripped binary size, and whether the inference sweep in
test 6 names the request type on the first candidate or needs the full
sweep.

Already measured, and the reason S4 exists (2026-08-04, this machine):
`DescriptorPool::decode` over the full `googleapis.desc` — 25 660 332
bytes, 7 771 files, 49 255 messages — takes **1.34 s cold, 0.76 s warm**
and peaks at **720 MB RSS**.
