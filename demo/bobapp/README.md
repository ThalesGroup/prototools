<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# bobapp

A toy gRPC client that calls a live Google API reflectively and logs the exact
bytes it put on the wire, so that protolens has something real to open.

Spec: `docs/specs/0241-a-real-call-leaves-bytes-worth-opening.md`.

bobapp is **not a member of the root Cargo workspace**. It has its own
`Cargo.lock` and is named in the root manifest's `[workspace] exclude`, because
cargo unifies features across a workspace and tonic/hyper/rustls/tokio appear
nowhere else in this repo. `nix-build -A ci` compiles exactly what it did
before bobapp existed.

## Build

The descriptor set is embedded at build time and there is no fallback, so
`BOBAPP_DESCRIPTOR_SET` must be set:

```sh
BOBAPP_DESCRIPTOR_SET=$(nix-build -A bobapp-desc --no-out-link)/bobapp.desc \
  cargo build --release --manifest-path demo/bobapp/Cargo.toml
```

`nix-build -A bobapp-desc` runs `protoc --include_imports` over the single file
`google/maps/routing/v2/routes_service.proto`. The result is **50 836 bytes and
40 files** — against 25 660 332 bytes and 7 771 files for the whole corpus.

## Run

The API key is read from the environment and from nowhere else, so it never
reaches shell history or `/proc/<pid>/cmdline`:

```sh
BOBAPP_API_KEY=$(cat ~/.config/bobapp/api-key) \
  bobapp --origin "Grenoble, France" \
         --destination "Lyon, France" \
         --log-dir /tmp/bobapplog
```

Every value that names something in the schema — `--travel-mode`,
`--routing-preference`, `--units` — is resolved by name at run time against the
embedded descriptors. A wrong one is an error that lists the alternatives.

## The log

`--log-dir DIR` writes `DIR/log.pb`, a serialized `Log` message:

```proto
message Log {
  repeated Entry entry = 2042;
}

message Entry {
  bytes query    = 42;
  bytes response = 43;
}
```

**These two messages are deliberately outside the reflected schema.** They are
not in `routes_service.proto`, so they are not in the embedded descriptor set,
and `src/log.rs` writes their wire form by hand without consulting a
`DescriptorPool`. That is the point of the demo: protolens knows
`ComputeRoutesRequest` and knows nothing at all about `Log`, so the envelope
has to be read as raw wire structure while the payloads inside it get named.

The query bytes are captured **inside the codec**, in
`DynamicEncoder::encode`, between serializing the message and handing it to
tonic — see `src/codec.rs`. Not by re-encoding a copy afterwards, which may
legitimately differ in field order and default elision. A later step that
rewrites the encoding into a non-canonical form goes in that same gap, and
needs no other change for the log to stay truthful.

`EncodeBuf` is the *message* buffer; tonic adds the five-byte gRPC frame header
around it afterwards. So the logged bytes carry no header and need no
stripping.

## Reading it back

The schema comes out of the binary, not from anything prepared earlier:

```sh
bobapp --dump-descriptor /tmp/bobapp.desc
mkdir -p /tmp/bobapp-db
reproto --schema-db-out /tmp/bobapp-db/bobapp.desc /tmp/bobapp.desc
protolens /tmp/bobapplog/log.pb --descriptor-set /tmp/bobapp-db/bobapp.desc
```

The root is `Log`, which is unknown, so inference declines it and the envelope
renders raw — fields `2042`, `42`, `43`. Name the payload with the override
pane, or pull it out and let inference work on it alone:

```sh
protolens /tmp/bobapplog/log.pb --descriptor-set /tmp/bobapp-db/bobapp.desc \
  export /1/1 --format binary -o /tmp/query.pb
protolens /tmp/query.pb --descriptor-set /tmp/bobapp-db/bobapp.desc
```

Against the full corpus (`$PROTOTEXT_GOOGLEAPIS_SET`) the same 49 bytes are
named `google.maps.routing.v2.ComputeRoutesRequest` in about 50 ms.
