<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0241 — a real call leaves bytes worth opening

Status: draft
App: ringer
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

## Goals

- **G1.** `ringer` makes a real unary gRPC call to a live Google API over
  TLS, authenticated by an API key, and exits with the server's status.
- **G2.** Every googleapis message it builds or reads is handled
  **reflectively**, from descriptors. No `tonic-build`, no generated Rust
  type for any googleapis message.
- **G3.** The descriptor set is embedded in the executable. ringer needs
  no file on disk to know what it is sending.
- **G4.** The exact bytes ringer put on the wire are written to a file
  that `protolens` opens **without `--type`**, so the inference sweep
  names the message.
- **G5.** ringer's response is logged the same way. Same codec, opposite
  trait; and `ComputeRoutesResponse` is the larger and more interesting
  of the two messages, so the demo gets its best artifact for one extra
  `impl`.
- **G6.** Adding ringer does not change what the rest of the workspace
  compiles, and does not add a single derivation to `nix-build -A ci`.

## Non-goals

- **N1.** Streaming RPCs. Unary only; framing a stream is a second
  problem and teaches nothing about protolens.
- **N2.** OAuth, ADC, service accounts. An API key in an environment
  variable is the shortest credible path to a live response.
- **N3.** A general-purpose `grpcurl`. The method is fixed and the request
  is built from a handful of flags. Arbitrary request authoring would
  make ringer a tool, and a tool needs a spec of its own.
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

- **S1.** `demo/ringer/` is its own Cargo project with its own
  `Cargo.lock`, named in the root `Cargo.toml`'s `[workspace] exclude`.

  tonic, hyper, rustls and tokio appear nowhere in the workspace
  dependency graph today. As a workspace member, ringer would pull all
  four into `depsCache` and into every `--workspace` derivation, because
  cargo unifies features across the whole workspace — the trap spec 0239
  hit from a single leaf-crate dependency. Exclusion keeps
  `nix-build -A ci` byte-identical to what it builds today.

- **S2.** `workspaceSrc` subtracts `demo/ringer` by `lib.fileset.difference`.
  `crane.fileset.commonCargoSources ./.` admits any `.rs`/`.toml` it finds,
  `[workspace] exclude` notwithstanding, so without the subtraction every
  edit to ringer would change `workspaceSrc`'s hash and rebuild the entire
  Rust world.

- **S3.** `publish = false`.

### The embedded descriptors (G3)

- **S4.** `build.rs` reads `RINGER_DESCRIPTOR_SET`, copies the file to
  `$OUT_DIR/ringer.desc`, and emits `cargo::rerun-if-env-changed` plus
  `cargo::rerun-if-changed` for it. The binary does `include_bytes!`.

- **S5.** The variable is `RINGER_DESCRIPTOR_SET`, **not**
  `PROTOTEXT_DESCRIPTOR_SET`. After spec 0228 both shells export the
  latter and point it at the well-known types; reusing it would mean that
  `cargo build` inside a dev-shell silently produced a ringer that cannot
  name `google.maps.routing.v2.Routes`, and the failure would surface as a
  runtime "type not found" a long way from its cause.

- **S6.** If the variable is unset, the build **fails**, naming
  `nix-build -A googleapis-db --no-out-link` in the error. No fallback to
  the WKTs: a ringer that cannot describe the service it calls is not a
  ringer, and a demo that half-works is worse than one that refuses to
  build.

- **S7.** Embedded raw, not compressed. The 25 MB really is in the
  executable, which is half of what the demo is showing; a decompression
  step would hide it and would add startup cost to the program whose
  slowest phase is already the pool build. Revisit only if binary size
  becomes a distribution problem.

- **S8.** The `DescriptorPool` is built once, lazily, behind a `OnceLock`.
  `DescriptorPool::decode` over 25 MB is the single largest cost in the
  process — larger, most likely, than the network round trip. Measure it
  and record the number in *Measured outcome*.

### The call (G1, G2)

- **S9.** A `DynamicCodec` implements `tonic::codec::Codec` over
  `prost_reflect::DynamicMessage`, parameterized by the request and
  response `MessageDescriptor`s taken from the pool, and is driven through
  `tonic::client::Grpc::unary`. This is the entirety of the reflection
  story: the method path is a string, the messages are dynamic, and
  nothing about googleapis is known at compile time.

- **S10.** Endpoint `https://routes.googleapis.com`, method path
  `/google.maps.routing.v2.Routes/ComputeRoutes`. TLS through rustls with
  the platform root store.

- **S11.** Request metadata carries `x-goog-api-key` from `RINGER_API_KEY`
  and `x-goog-fieldmask` (Routes rejects a call without a field mask).
  The key is read from the environment and from nowhere else — never a
  flag, so it cannot reach shell history or `/proc/<pid>/cmdline`.

- **S12.** CLI:
  `ringer --origin <address> --destination <address> [--travel-mode DRIVE] [--depart-in <duration>] [--log-dir <dir>]`.

  `--origin`/`--destination` fill `Waypoint.address`, one arm of that
  message's `location_type` oneof. That is the shortest path to a request
  that is still structurally rich: two nested submessages each containing
  a oneof, three enums (`RouteTravelMode`, `RoutingPreference`, `Units`),
  and — with `--depart-in` — a `google.protobuf.Timestamp`. All of it
  visible in protolens, and none of it requiring the viewer to know the
  API.

- **S13.** Fields are set through `DynamicMessage::set_field_by_name`, so
  a wrong name is a run-time error rather than a compile error. That is
  not a shortcoming to apologize for in a comment; it is the property
  being demonstrated.

### The log (G4, G5)

- **S14.** `--log-dir DIR` writes `DIR/request.pb` and `DIR/response.pb`:
  the message bytes exactly as the codec produced and consumed them, with
  the five-byte gRPC frame header (compression flag plus big-endian
  length) stripped. protolens reads messages, not frames; left in place,
  the leading zero byte would be decoded as a field-0 tag and the blob
  would open as garbage.

- **S15.** The bytes are captured **inside** the codec, not by re-encoding
  a copy of the request afterwards. A second encode may legitimately
  differ from the first in field order and default elision, and the demo's
  whole claim is that these are the bytes that went out.

- **S16.** On success ringer prints the command that reads them back:

  ```
  protolens --descriptor-set <the path build.rs was given> <dir>/request.pb
  ```

  with no `--type`. Naming the message from the bytes is what the
  inference sweep is for, and a 58 777-type descriptor set is the case
  that makes it worth having. The path is the one compiled in, so the
  descriptor set protolens loads is the same file whose bytes are in the
  executable.

### Nix (G6)

- **S17.** A crane derivation with
  `RINGER_DESCRIPTOR_SET = "${python.googleapisDb}/googleapis.desc"`,
  exposed as `nix-build -A ringer` and added to `full-tests`. **Not**
  added to `ci`: `googleapisDb` is a `full-tests` input precisely because
  it is expensive, and `ci` must not acquire that dependency.

- **S18.** Neither shell exports `RINGER_DESCRIPTOR_SET` — doing so would
  make entering the dev-shell build the googleapis corpus.
  `demo/ringer/README.md` gives the one-liner instead:

  ```
  RINGER_DESCRIPTOR_SET=$(nix-build -A googleapis-db --no-out-link)/googleapis.desc \
    cargo build --release --manifest-path demo/ringer/Cargo.toml
  ```

## Alternatives considered

**ringer as a workspace member.** Rejected: cargo unifies features across
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

**Reusing `prototext-schema`'s lazy index instead of
`DescriptorPool::decode`.** Faster to start, and rejected for the same
reason, plus it would couple a demo to an internal crate's layout. If S8's
measurement turns out to be intolerable, compressing (S7) is the cheaper
lever.

**grpcurl, or a shell script around it.** Rejected: it does not embed
descriptors, and "the app carries its own schema" is half of what makes
the logged bytes interesting.

**Logging the framed stream rather than the message.** Rejected: see S14.

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
   `ComputeRoutesResponse`. Establishes that `build.rs` was given a real
   googleapis set and not the well-known types.
2. `request_round_trips_through_the_codec` — build the request from CLI
   arguments, encode it with `DynamicCodec`, decode it back through the
   same descriptor, assert the field values match. Exercises S13/S15's
   encode path with no socket.
3. `logged_bytes_carry_no_frame_header` — the first byte of the captured
   request parses as a protobuf tag, not as a compression flag.
4. `missing_api_key_fails_before_connecting` — with `RINGER_API_KEY`
   unset, ringer exits non-zero and opens no connection.
5. `protolens_names_the_request` — run protolens's batch export over a
   committed golden `request.pb` with `--descriptor-set` but no `--type`,
   and assert the inferred root type is
   `google.maps.routing.v2.ComputeRoutesRequest`. This is G4 stated as an
   assertion. It needs the googleapis DB, so it lives with `full-tests`.
6. Manual, with a real key: `ringer --origin … --destination … --log-dir /tmp/r`
   returns a route, writes both files, and the command it prints opens
   `request.pb` with the message correctly named.

## Measured outcome

Filled in at implementation. Record: `DescriptorPool::decode` time for the
25 MB set (S8), the stripped binary size (S7), and whether the inference
sweep in test 5 names the request type on the first candidate or needs the
full sweep.
