<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0350 — bobapp: SearchText-only embedding, typed log fields, SearchText capture

Status: implemented
Implemented in: 2026-08-24
App: grpconf2026 demo (demo/bobapp)
Refs: grpconf2026/artifacts.md (build record); grpconf2026/synopsis.md

## Background

The current bobapp embeds descriptors for both
`google/maps/routing/v2/routes_service.proto` and
`google/maps/places/v1/places_service.proto`, plus `bobapp/v1/log.proto`.
Three things are wrong:

1. **The demo's central claim** is that `protoscan` finds only the
   schemas the app compiled in, and that the log is only half-readable
   under that schema.  Embedding both services means `protoscan` names
   both, and beat 10's "bigger dictionary" move has nothing to unlock.

2. **`log.proto` fields 4 and 5 are `bytes`**, giving no type
   information about the payloads.  Typed fields let protolens label
   and partially render them.

3. **`bob/capture` holds a `ComputeRoutesRequest`** — a type not in the
   proposed embedded set.  The capture should come from the embedded
   service (Places/SearchText) for consistency.

## Goals

- **G1.** The embedded `FileDescriptorSet` contains **only** the
  transitive closure of `google/maps/places/v1/places_service.proto`.
  `log.proto`'s own FDP is **not** embedded.  `protoscan bobapp` lists
  exactly the Places files.  The log envelope (`bobapp.v1.Log`,
  `bobapp.v1.Entry`) is not nameable from `bobapp.desc` — just as the
  Routes payload is not.

- **G2.** `log.proto` declares typed fields:
  - field 4: `google.maps.places.v1.SearchTextRequest`   (type in embedded set)
  - field 5: `google.maps.places.v1.SearchTextResponse`  (type in embedded set)
  - field 6: `google.maps.routing.v2.ComputeRoutesRequest`  (type NOT embedded)
  - field 7: `google.maps.routing.v2.ComputeRoutesResponse` (type NOT embedded)

  `log.proto` imports both `places_service.proto` and `routes_service.proto`
  so protoc can resolve the field types at compile time.  Neither
  `log.proto`'s own FDP nor the Routes FDPs appear in the embedded set.

- **G3.** `method` (field 2) stores only the last path segment:
  `"SearchText"` or `"ComputeRoutes"`.

- **G4.** Log entry order is unchanged from today:
  1. SearchText "coffee in Grenoble" (whole)
  2. SearchText "bouchon lyonnais" (whole)
  3. ComputeRoutes Grenoble → Lyon (whole)
  4. ComputeRoutes Lyon → Grenoble (truncated — anomaly 4)

- **G5.** `bob/capture` is replaced with a `SearchTextRequest` body,
  lifted verbatim from the first log entry's `places_request` field
  (the exact bytes bobapp puts on the wire, before the debug trace
  is prepended — anomaly 1 still applies, so the captured request
  carries the doubled `text_query`).

## Non-goals

- **N1.** No change to anomalies 1, 2, 3, 4.
- **N2.** No change to the `note` field or its values.
- **N3.** No change to the nix `grpconfDemo` derivation's output layout.
- **N4.** No change to log entry order.

## Specification

### log.proto (S1)

```proto
syntax = "proto3";
package bobapp.v1;

import "google/protobuf/timestamp.proto";
import "google/maps/places/v1/places_service.proto";
import "google/maps/routing/v2/routes_service.proto";

// One round trip: when it happened, which method, and the bytes in
// each direction typed against their service schema.
//
// Fields 4/5 (Places) are in the embedded descriptor set; fields 6/7
// (Routes) are not.  Under bobapp.desc alone, fields 6/7 appear as
// unknown typed fields.  Under a Routes-aware descriptor set they open
// fully — which is beat 10's payoff.
//
// bobapp.v1.Log and bobapp.v1.Entry are themselves NOT in the embedded
// set, so the log envelope is also opaque until a descriptor set that
// includes log.proto is provided.
message Entry {
  google.protobuf.Timestamp at = 1;

  // Last segment of the gRPC method path: "SearchText" or "ComputeRoutes".
  string method = 2;

  // Diagnostic written when the call goes out ("sent") and updated
  // when it lands ("ok").  Singular: last-one-wins gives the reader
  // the final state.
  string note = 3;

  // Places/SearchText pair — types present in the embedded set.
  google.maps.places.v1.SearchTextRequest  places_request  = 4;
  google.maps.places.v1.SearchTextResponse places_response = 5;

  // Routes/ComputeRoutes pair — types NOT in the embedded set.
  google.maps.routing.v2.ComputeRoutesRequest  routes_request  = 6;
  google.maps.routing.v2.ComputeRoutesResponse routes_response = 7;
}

message Log {
  repeated Entry entry = 1;
}
```

### method string (S2)

`Recorder::record_request` derives the short name:

```rust
let method_name = method.rsplit('/').find(|s| !s.is_empty())
    .unwrap_or(method)
    .to_owned();
```

### Descriptor sets (S3)

Two env vars govern descriptor inputs:

| Env var | Content | Used for |
|---|---|---|
| `BOBAPP_DESCRIPTOR_SET` | Places transitive closure only | Embedded in binary via `include_bytes!`; what `protoscan` finds |
| `BOBAPP_EXTRA_DESCRIPTOR_SET` | Routes transitive closure + `log.proto` | Runtime pool for encoding all log entries and building the Log/Entry envelope |

`build.rs` is unchanged: it copies `BOBAPP_DESCRIPTOR_SET` to
`OUT_DIR/bobapp.desc`.

`BOBAPP_EXTRA_DESCRIPTOR_SET` is read at runtime (in `main.rs`, after
the `--dump-descriptor` branch) and decoded into a second
`DescriptorPool`.  This pool knows `Log`, `Entry`, `ComputeRoutesRequest`,
`ComputeRoutesResponse`, `SearchTextRequest`, and `SearchTextResponse`.
All log encoding happens against this pool.

The embedded pool (from `BOBAPP_DESCRIPTOR_SET`) is no longer used for
log encoding — only for building the ComputeRoutes *request* (the
actual API call) and resolving the response type.  It still serves
`request.rs` and `codec.rs` unchanged.

The nix `bobappDescOf` function gains a second invocation:

```nix
# Embedded — Places only; what protoscan finds in the binary.
bobappDesc = bobappDescOf "bobapp" [
  "google/maps/places/v1/places_service.proto"
];

# Extra — Routes + log.proto; runtime encoding pool, not embedded.
bobappExtraDesc = bobappDescOf "bobapp-extra" [
  "google/maps/routing/v2/routes_service.proto"
  "bobapp/v1/log.proto"
];
```

`demo/bobapp/default.nix` gains an `extraDesc` input and passes it as
`BOBAPP_EXTRA_DESCRIPTOR_SET`.

### Entry encoding (S4)

`Entry` in `log.rs` gains a `kind` field:

```rust
enum EntryKind { Places, Routes }

struct Entry {
    at: SystemTime,
    method: String,
    note: &'static str,
    kind: EntryKind,
    request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}
```

`Recorder` is constructed with the extra pool (passed in from
`main.rs`).  `Entry::encode` selects field names based on `kind`:

- `Places` → `places_request` / `places_response`
- `Routes` → `routes_request` / `routes_response`

All encoding uses the extra pool, which knows all types.

`record_request` determines `kind` from the method name:

```rust
let kind = if method.contains("Places") || method.contains("SearchText") {
    EntryKind::Places
} else {
    EntryKind::Routes
};
```

### capture file (S5)

`grpconf2026/fixtures/bobshark` is replaced with a `SearchTextRequest`
body — the first lookup request, lifted verbatim from the log (with
anomaly 1 applied: the doubled `text_query` with the debug trace
prepended).  Extracted the same way the original bobshark was: the
exact pre-frame bytes from inside the codec.

### Fixture re-mint (S6)

Re-mint `grpconf2026/fixtures/boblog` and `grpconf2026/fixtures/bobshark`
with a live API call using the updated binary.  Verification checklist:

- 4 entries in the order G4 specifies.
- `protoc --decode_raw` exits 1 (truncated tail).
- Under `bobapp.desc`: entries 1–2 show `places_request`/`places_response`
  as properly-typed fields; entries 3–4 show fields 6/7 as unknown;
  the root (`bobapp.v1.Log`) is also unknown.
- Under `bobapp-extra.desc` (or a combined set): all four entries
  fully named, root named.
- Entry 4 partially rendered (anomaly 4).
- `bobshark` decodes cleanly as `SearchTextRequest` against a
  Places-aware descriptor set.

## Alternatives considered

### Keep log.proto in the embedded set

Makes the log envelope nameable from `bobapp.desc`.  Rejected by the
user: `protoscan` should find only SearchText APIs, and `log.proto`
is not one of them.

### Keep `bytes` fields in log.proto

No descriptor-set complexity.  Rejected: typed fields are the
user-visible requirement.  With typed fields, protolens can label
`places_request` by name and show its internal structure, rather than
treating it as an opaque blob.

### `google.protobuf.Any`

Resolves directly in protolens; opacity disappears.  Ruled out in
artifacts.md and spec 0241.
