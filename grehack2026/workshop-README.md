<!--
SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# prototools — GreHack 2026 workshop

This container holds prototools and everything the workshop needs. The full
guide, including how to start the container, is `SETUP.md`:
<https://github.com/douzebis/prototools/blob/main/grehack2026/SETUP.md>.

## The tools

- `prototext` decodes a protobuf to text and back, byte for byte, with or
  without its schema; given a schema database, it infers the message type.
- `protolens` browses a protobuf interactively: types, heat cues, the raw
  bytes behind every line. `F1` lists its keys.
- `protoscan` finds the protobuf descriptors embedded in a binary.
- `protoback` turns descriptors back into `.proto` files and schema databases.

`$PROTOTEXT_DESCRIPTOR_SET` (the well-known types) is the default schema
database; `$PROTOTEXT_GOOGLEAPIS_SET` holds all of googleapis, for
`--descriptor-set`.

## Where things are

- `/workshop` — this README and the material below. Writable, but lost when
  the container stops.
- `/work` — your directory on the laptop, when the container was started
  with `-v "$PWD":/work`. Save anything you want to keep here.

## Start here

`anomalies.pb` holds one example of every encoding anomaly prototools
reports. Open it with its guided walk:

    protolens --type google.protobuf.FileDescriptorProto anomalies.pb \
        --script anomalies.script

`anomalies.md` explains each anomaly.

## The game of life, in three terminals

A Game of Life client and server talking gRPC, and a tap that captures
their traffic as `.pb` files (`SETUP.md`, section 4):

1. this terminal: `life-client`
2. `podman exec -it workshop bash`, then `life-server`
3. `podman exec -it workshop life-tap`, which writes `/work/capture`

With Docker: `docker exec`, and `-u 0` for the tap.

## The talk, and a stretch goal

- `grehack2026/`: the GreHack 2026 talk, replayed with
  `cd grehack2026 && teleprompt grehack2026.sh` (`SETUP.md`, section 5).
- `grpconf2026/`: the gRPConf 2026 talk, on the googleapis corpus
  (`SETUP.md`, section 6).
