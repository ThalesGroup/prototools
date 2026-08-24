<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# bobapp

A toy gRPC client that calls live Google APIs reflectively and logs the exact
bytes it put on the wire, so that protolens has something real to open.

Specs: `docs/specs/0241-a-real-call-leaves-bytes-worth-opening.md`,
`docs/specs/0350-bobapp-routes-only-embedding.md`.

bobapp is **not a member of the root Cargo workspace**. It has its own
`Cargo.lock` and is named in the root manifest's `[workspace] exclude`, because
cargo unifies features across a workspace and tonic/hyper/rustls/tokio appear
nowhere else in this repo. `nix-build -A ci` compiles exactly what it did
before bobapp existed.

## Descriptor sets (spec 0350)

bobapp uses two descriptor sets:

| Env var | Content | Purpose |
|---|---|---|
| `BOBAPP_DESCRIPTOR_SET` | Places/SearchText transitive closure only | **Embedded** in binary via `include_bytes!`; what `protoscan` finds |
| `BOBAPP_EXTRA_DESCRIPTOR_SET` | Routes + Places + log.proto + error_details | **Runtime** pool for all encoding — reflective calls, log |

The split is the demo's central claim: `protoscan` finds only the SearchText
APIs the binary compiled in. The ComputeRoutes types and the log envelope are
not embedded, so they are opaque under `bobapp.desc` alone — until beat 10
provides a broader descriptor set.

## Build

`BOBAPP_DESCRIPTOR_SET` must be set at build time (baked into the binary via
`include_bytes!`):

```sh
# Preferred — builds and runs in one step via the nix derivation.
nix-build -A bobapp
```

Or, to build the Cargo project directly:

```sh
BOBAPP_DESCRIPTOR_SET=$(nix-build -A bobapp-desc --no-out-link)/bobapp.desc \
  cargo build --manifest-path demo/bobapp/Cargo.toml
```

## Run

Both env vars must be set at runtime:

```sh
BOBAPP_API_KEY=$(cat ~/.config/bobapp/api-key) \
BOBAPP_EXTRA_DESCRIPTOR_SET=$(nix-build -A bobapp-extra-desc --no-out-link)/bobapp-extra.desc \
  bobapp --origin "Grenoble, France" \
         --destination "Lyon, France" \
         --look-up "coffee in Grenoble" \
         --look-up "bouchon lyonnais" \
         --log-dir /tmp/bobapplog
```

The API key is read from the environment and from nowhere else — never a CLI
flag, so it cannot reach shell history or `/proc/<pid>/cmdline`.

## The log

`--log-dir DIR` writes `DIR/log.pb`, a serialized `bobapp.v1.Log` message.

Log entry order (spec 0350 G4):

1. SearchText "coffee in Grenoble" — whole
2. SearchText "bouchon lyonnais" — whole
3. ComputeRoutes Grenoble → Lyon — whole
4. ComputeRoutes Lyon → Grenoble — **truncated** (anomaly 4: bobapp is killed
   before finishing)

Each `Entry` carries typed fields:

```proto
message Entry {
  google.protobuf.Timestamp at = 1;
  string method = 2;   // "SearchText" or "ComputeRoutes"
  string note   = 3;   // "sent" → "ok"

  // Places pair — types in the embedded set:
  google.maps.places.v1.SearchTextRequest  places_request  = 4;
  google.maps.places.v1.SearchTextResponse places_response = 5;

  // Routes pair — types NOT in the embedded set:
  google.maps.routing.v2.ComputeRoutesRequest  routes_request  = 6;
  google.maps.routing.v2.ComputeRoutesResponse routes_response = 7;
}
```

Under `bobapp.desc` (the embedded Places-only schema): entries 1–2 are fully
named (`places_request`, `places_response`); entries 3–4 show fields 6/7 as
unknown typed fields; and the root `bobapp.v1.Log` itself is also unknown
(log.proto is not embedded).

Under a Routes-aware descriptor set: all four entries open fully — which is
beat 10's payoff.

## Reading it back

```sh
bobapp --dump-descriptor /tmp/bobapp.desc
reproto --schema-db-out /tmp/bobapp-db /tmp/bobapp.desc
protolens /tmp/bobapplog/log.pb --descriptor-set /tmp/bobapp-db
```

## Re-minting the fixtures

The demo fixtures (`grpconf2026/fixtures/boblog` and `bobshark`) are minted
from a live API call. To re-mint them:

```sh
# From the repo root, inside nix-shell:
BOBAPP_API_KEY=$(cat ~/.config/bobapp/api-key) \
  grpconf2026/mint-fixtures.sh
```

The script builds the nix targets, runs bobapp, extracts the `places_request`
bytes from the first log entry as `bobshark`, and copies both files into
`grpconf2026/fixtures/`. See `grpconf2026/mint-fixtures.sh` for details.

After minting, verify and commit:

```sh
protoc --decode_raw < grpconf2026/fixtures/boblog  # must exit 1 (truncated)
git add grpconf2026/fixtures/boblog grpconf2026/fixtures/bobshark
git commit -m "chore(grpconf2026): re-mint boblog and bobshark fixtures"
```
