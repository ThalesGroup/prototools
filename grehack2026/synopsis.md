<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# GreHack 2026 — demo synopsis

Running order for the live demo that follows the talk: what happens on screen, in which window, and the point each step makes. The teleprompt deck `grehack2026.sh` is the source of truth; this file follows it section by section. If the two disagree, the deck wins and this file is out of date.

The demo app is the crate in `game/`: a Game of Life client and server talking cleartext gRPC (`life-client`, `life-server`), plus a network tap (`life-tap`). The tools under the lens are `protoscan`, `protoback`, `prototext` and `protolens`.

## Setup

Three windows, all opened beforehand. In each, `cd` to the window's directory (below), then run `nix-shell`: each directory's `shell.nix` gives the demo's own shell (spec 0394), with every tool built by Nix from committed sources, as in the workshop image — the prototools, the three `life-*` binaries, `dumpcap`/`tshark`, `teleprompt`, `protoc` and `hexdump`:

| Window | Directory | Runs |
|---|---|---|
| Eve 👩 | `grehack2026/eve/` | `life-server`; its stdout shows what Eve's covert channel brings back |
| Bob 🙂 | `grehack2026/bob/` | `life-client`, the Game of Life TUI |
| Alice 🕵️ | `grehack2026/` | `teleprompt grehack2026.sh`, the control tower |

The shell prompt names each window's directory, which tells the audience whose window is whose.

The deck runs in Alice's window. The `#` lines are on-screen narration: teleprompt shows them in blue, and the audience reads them. Real commands run as written. Lines starting with 👉 say what to do in another window, with the exact keys or command. `header "…"` draws section banners, `view` / `view_textproto` page a file or a decoded protobuf, and `protolens --script beats/<name>` loads a reminder pane for the presenter. Those reminder panes do not drive protolens; the presenter quits it with `:q`.

`grehack2026.init` runs when teleprompt starts:
- It wipes what earlier runs generated: `capture/`, `life.desc`, `life/` and `eve/server.log`.
- It copies the client binary to `grehack2026/life-client`, which is "the client binary Alice has". On a Nix PATH, `life-client` is a wrapper script, and the copy is the real binary behind it.

The tap runs in Alice's window, in the background, started with `life-tap -q --detach --out capture`, the same line natively and in the workshop image (spec 0403). Natively, it asks for the sudo password on stage if sudo has none cached; only `dumpcap` then runs as root, through `sudo -n`, while the tap and `tshark` run as Alice (spec 0393). In the container the shell is root already, and nothing is asked. `-q` leaves out the tap's startup block of commands. The tap prints one line per message and saves each message as `capture/NNNNNN-request.pb` / `-response.pb`. Because it runs in the background, the 👉 hints for the other windows come after it has started. `life-tap --stop` stops it and returns once its files are written; `tail -n 3 capture/tap.log` then shows the closing summary.

Every capture starts the same way. Bob's client is not running (Bob quits it with Ctrl-C), the deck runs `rm -rf capture` and starts the tap, and Bob then starts a paused client, `life-client --paused`, and plays single steps with `n`. A capture therefore always starts at `000001`, and the deck can name its files in advance. The client is a new process, so its connection opens while the tap is watching, and the tap sees it from the start. Without `--paused`, the client runs from launch, as it does in section 0.

## Why prototools

- At S3NS we operate a Trusted Partner Cloud. Google ships us software update packages, and we audit each one before it reaches production.
- Inside them, protobuf is everywhere: it is how Google's infrastructure serializes configuration, RPCs, logs and stored data. On the wire it is opaque.
- So we built prototools, to dissect protobufs even when nobody hands us the schema. The demo is a made-up investigation; the tools and their features are real.

## 0. The cast

- **Bob** 🙂 plays the Game of Life, and **Eve** 👩 runs the server that computes each generation. In their windows, Eve starts `life-server` and Bob starts `life-client`. A diagram shows the two exchanging `StepRequest` / `StepResponse` over cleartext gRPC.
- **Alice** 🕵️ taps the wire between them. She has the capture and the client binary, nothing else. A second diagram adds her window, where `life-tap` feeds `capture/*.pb` to prototools.
- Goal: take an unknown protobuf stream and, with no help from either endpoint, work out its structure, read it, then notice what it is not telling us.

## 1. On the wire

- Capture: Bob quits the client, the tap starts, Bob runs `life-client --paused`, presses `n` three times and quits it, then `life-tap --stop` and `tail -n 3 capture/tap.log`.
- `hexdump -v -C capture/000001-request.pb`: opaque. Protobuf is self-describing only up to field numbers and wire types; reading values needs a schema, that is, a descriptor set and a root type.
- `protoscan life-client`: the client carries its own schema. protoscan lists the embedded `FileDescriptorProto`s: the game's `grehack/life/v1/life.proto`, and the well-known `google/protobuf/descriptor.proto`. `--proto_out DIR` would extract them.
- `protoback -I life-client --schema-db-out life.desc` turns those descriptors into a schema database.

## 1b. Schema DB

- `ls -lhd life.desc life/*`:
  - `life.desc` is the descriptor set, the reusable schema DB;
  - `life/hopcroft.rkyv` is the type-inference scoring graph;
  - `life/proto/` holds the decompiled `.proto` sources;
  - `life/index.rkyv` is the fast-access index.
- `view life/proto/grehack/life/v1/life.proto`: a faithful `.proto` rebuilt from binary descriptors, with no access to the source. Everything protoback needed came from the binary.
- `prototext --descriptor-set life.desc list-schemas capture/000001-response.pb` infers the capture's type by scoring it against the DB.
- `protoc --decode=grehack.life.v1.StepRequest` on `000001-request.pb` works, but it is spartan.
- `protolens --descriptor-set life.desc capture/000001-response.pb --script beats/capture` gives the better view: annotations, the inferred type, and the wire level one keystroke (`w`) away. On a field whose type is a named message or enum, `v` opens Neovim at that type's declaration in the reconstructed `.proto`. This works because of the `SourceCodeInfo` protoback synthesized, and the `life/proto/` stub beside `life.desc`. Schema recovery is not just a list of types: we go from a byte on the wire to the line of source that defines it, and back.

## 2. Eve is spying

Two parts (spec 0395). First the capability, live, with no tap; then one controlled capture for protolens to dissect. Bob's hints never name the `whoami`/`experiment` payload — that is the reveal in 2b.

- Part 1 — the capability, on screen:
  1. Bob runs `life-client` (running, no `--paused`).
  2. In her window, Eve types shell commands on the server's stdin — e.g. `ls ~/.ssh`, `id` — and a few Life steps later their output appears on her screen. Those commands ran on Bob's machine: the "Life server" is a remote shell.
  3. Bob quits the client (Ctrl-C).

## 2b. Hidden bits

A single capture (spec 0396). Alice does not control Eve, so which message carries contraband is not knowable in advance — she captures, then finds the odd one.

- One capture: Bob resumes the game for a few steps and pauses it (Eve may or may not be typing); `life-tap --stop`; `tail -n 3 capture/tap.log`.
- `prototext is-canonical capture/*.pb` — no schema needed. Most files are `canonical`; one or two are `anomalous`, with `overhanging bytes in values: N` (spurious varint padding a normal encoder never emits).
- The presenter opens one flagged capture in protolens, editing the `NNNNNN` placeholder in the preloaded `protolens … capture/NNNNNN-request.pb --script beats/smuggle` to that file.
- At wire level (`w`), a VARINT is base-128, little-endian 7-bit groups, the high bit "another byte follows". The trick is a spurious continuation byte: one hidden bit per field, value unchanged.
- The hidden bits group into bytes, ASCII: `experiment` on the request side (the client trims the output's ends, so no newline), `whoami` on the response side. The reveal lands here — recovered from the wire alone, with no schema.
- Spec 0393's second capture, `life-client --paused` and the fixed `000001`/`000002` numbering are gone (spec 0396 G4).

## 3. No schema

- Eve's server has been writing `eve/server.log` since she started it (`life-server` logs to `server.log` by default; `--no-log` turns that off). The deck does not say what the file is: the audience finds out.
- `ls -lh eve/server.log`, then `protoc --decode_raw < eve/server.log`: protoc gives up on the whole file.
- `protolens --descriptor-set life.desc eve/server.log --script beats/logfile` opens it, but no known root type matches, and the tail is flagged as truncated. That is the reveal: a protobuf after all, Eve's traffic log, cut off mid-field (by construction, spec 0386). Its type is not in the client, and the server does not embed its descriptor.
- Every entry is the same field (42) and has the same shape: a `Capture` (spec 0391), one per message the server saw. Its type is in no schema we hold, but the heat cues recognize the game's own `StepRequest` or `StepResponse` inside each entry, and overrides pin those types to rebuild the record, truncated tail and all.
- One field of each entry, 666, is a string no schema we hold declares. Where it is set, it reads `whoami` (in the entry of the response that smuggled it) or `experiment` (in the entry of the request that brought it back): Eve's log records her own contraband.

## 4. Anomalies

- `protolens --type google.protobuf.FileDescriptorSet anomalies.pb --script beats/anomalies` walks through one example of every encoding anomaly prototext reports, from bytes every parser accepts to bytes none can read.
- `anomalies.pb` and `beats/anomalies` are copies of `grpconf2026/anomalies.pb` and `grpconf2026/anomalies.script`, kept here so the demo stands alone. The root type is `FileDescriptorSet`. The demo shell's `PROTOTEXT_DESCRIPTOR_SET` supplies the well-known types.

## 5. Takeaways

1. Descriptors are usually hiding in the binary itself: protoscan finds them, and protoback gives the `.proto` back.
2. A corpus can type a message it has never seen, piece by piece, because that message is built from messages it does know. That is what protolens's heat cues are for.
3. What your decoder normalizes away is evidence: a shadowed value, a spurious continuation bit, a truncated tail. prototools surfaces it, all the way back to the original bytes.

Then a pointer to <https://github.com/ThalesGroup/prototools>.

## Specs behind the demo

- `docs/specs/0375-a-game-of-life-to-spy-on.md`: the client, the server, and the tap (then called `life-spy`).
- `docs/specs/0381-a-step-survives-the-connection-renewal.md`: connection renewal, and why a tap started late misses traffic.
- `docs/specs/0384-smuggle-through-plain-varint-fields.md`: the covert channel rides plain VARINT values.
- `docs/specs/0385-the-smuggled-payload-is-exactly-the-bytes.md`: the command and its output, exactly, with no prefix or special number.
- `docs/specs/0386-the-server-writes-a-truncated-protobuf-log.md`: the server's traffic log (on by default since spec 0388 S14).
- `docs/specs/0387-a-request-and-a-response-score-apart.md`: the log's two old entry types, replaced by spec 0391.
- `docs/specs/0388-the-demo-runs-from-grehack2026.md`: this layout, the tap's rename, and the fixed captures.
- `docs/specs/0391-every-log-entry-is-a-capture.md`: one log entry type, `Capture`, with the contraband field.
- `docs/specs/0393-only-dumpcap-runs-as-root.md`: the tap in the background, only `dumpcap` as root, `life-tap -q`, and `life-client --paused`.
- `docs/specs/0395-the-teleprompt-deck-reads-more-easily.md`: splash banners, spacing, `| view` on the hexdump, syntax-colored `view`, and the two-part Eve section.
- `docs/specs/0396-prototext-is-canonical.md`: `prototext is-canonical`, and the single-capture section 2.
