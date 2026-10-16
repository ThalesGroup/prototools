# Remove the artifacts of a previous run (skip this step to keep them):
[ -e capture/tap.pid ] && life-tap --out capture --stop; rm -rf capture life.desc life eve/server.log eve/server.yaml

clear && header "Why prototools"
# \
#                                                                              \
#                                                                              \
# At S3NS we run a Trusted Partner Cloud: we audit Google's software           \
# updates before they reach production.                                        \
#                                                                              \
# Inside them, protobuf is everywhere — and it is opaque: the bytes alone      \
# don't say what they mean.                                                    \
#                                                                              \
# So we built prototools, to dissect protobufs even when nobody hands us       \
# the schema.                                                                  \
#                                                                              \
# First, what a protobuf does — and doesn't — tell you.                        \
#                                                                              \


clear && header "Protobuf in a nutshell"
# \
#                                                                              \
# • Compact binary encoding of structured data — Google's lingua franca:       \
#   RPCs (gRPC), configs, logs, archives.                                      \
#                                                                              \
# • Not self-describing: the bytes carry field numbers and wire types,         \
#   nothing more. No names, no types — a length-delimited field may be a       \
#   string, raw bytes, or a nested message.                                    \
#                                                                              \
# • Decoding needs the schema: a descriptor set (compiled .proto files)        \
#   plus the root message type. Neither travels with the data.                 \
#                                                                              \
# • Generated code usually embeds its descriptors, for reflection — but        \
#   lite builds strip them. Nothing guarantees they are there.                 \
#                                                                              \
# • Decoders are lenient: many encodings decode to the same value, and         \
#   unknown fields pass through silently.                                      \
#                                                                              \
# To read a protobuf you didn't write: recover its schema, and watch what      \
# the decoder lets through. Here is prototools doing both, in a toy            \
# cyber-investigation.                                                         \
#                                                                              \


clear && header "0. The cast" && picture images/characters.jpeg 22
# \
#                                                                              \
# Bob 🙂 — the player. Thinks he's just playing.                               \
#                                                                              \
# Eve 👩 — the server admin. Keep an eye on her.                               \
#                                                                              \
# Alice 🕵️ — Bob's friend, savvy with cyber security.                          \
#                                                                              \

clear && picture images/playing.jpeg 22
# \
#                                                                              \
# Bob plays the Game of Life with software Eve provides: his client asks       \
# her server for each next generation over gRPC — one pair of protobuf         \
# messages per step.                                                           \
#                                                                              \
#    ┌──────────────┐        StepRequest         ┌──────────────┐              \
#    │     Bob      │ ─────────────────────────▶ │     Eve      │              \
#    │ life-client  │ ◀───────────────────────── │ life-server  │              \
#    └──────────────┘        StepResponse        └──────────────┘              \
#                           cleartext gRPC                                     \
#                                                                              \
#                                                                              \


# \
#                                                                              \
# 👉 In Eve's window (eve/) — Eve starts her server:                           \
#        life-server                                                           \
#                                                                              \
# 👉 In Bob's window (bob/) — Bob starts his client:                           \
#        life-client                                                           \
#                                                                              \


clear && header "1. On the wire"
# \
#                                                                              \
# Bob, idly curious whether his game only plays Life, asks Alice to audit      \
# the traffic. She has his client binary and her own tap of the wire —         \
# nothing from the server.                                                     \
#                                                                              \

clear && picture images/tapping.jpeg 22
# \
#                                                                              \
#    ┌──────────────┐        StepRequest         ┌──────────────┐              \
#    │     Bob      │ ───────────┬─────────────▶ │     Eve      │              \
#    │ life-client  │ ◀──────────┼────────────── │ life-server  │              \
#    └──────────────┘            │ StepResponse  └──────────────┘              \
#                                ▼                                             \
#              ┌───────────────────────────────────────────────┐               \
#              │ Alice — this window                           │               \
#              │   life-tap ──▶ capture/*.pb ──▶ prototools    │               \
#              └───────────────────────────────────────────────┘               \
#                                                                              \


# \
#                                                                              \
# Alice starts her tap in the background.                                      \
# One capture per message; each saved in capture/.                             \


life-tap -q --detach --out capture

# Look at the first captured request as raw bytes:
hexdump -v -C capture/000001-request.pb | view
# \
#                                                                              \
# Opaque, as promised: field numbers and wire types, nothing more.             \
#                                                                              \


# \
#                                                                              \
# ######################################################################       \
# #                          Enters protoscan                          #       \
# ######################################################################       \
#                                                                              \


# The client likely embeds its descriptors. protoscan finds them:
protoscan life-client
# \
#                                                                              \
# protoscan found the game's own schema, life.proto, inside the client.        \


# \
#                                                                              \
# ######################################################################       \
# #                           Enters reproto                           #       \
# ######################################################################       \
#                                                                              \


# reproto extracts it into a reusable schema DB:
reproto -I life-client --schema-db-out life.desc

ls -lhd life.desc life/* \
#                                                                              \
# Not just extraction — reproto delivered 💪:                                  \
# - life.desc:          the descriptor set, ready to reuse                     \
# - life/hopcroft.rkyv: the scoring graph, for type inference                  \
# - life/index.rkyv:    the fast-access index                                  \
# - life/proto/:        the decompiled .proto sources                          \


# Browse one decompiled .proto — it reads like hand-written source:
view life/proto/grehack/life/v1/life.proto


# \
#                                                                              \
# ######################################################################       \
# #                          Enters protolens                          #       \
# ######################################################################       \
#                                                                              \


# We have the descriptors, not the root type — protolens infers it:
protolens --descriptor-set life.desc capture/000001-request.pb \
    --script beats/capture


# \
#                                                                              \
# Alice can now read every message in the clear. But is the server only        \
# playing Life?                                                                \


clear && header "2. Eve is spying" && picture images/spying.jpeg 22

# \
#                                                                              \
# A look behind the curtain — what Alice cannot see. Watch Eve's window:       \
# the grid plays on, but she's about to do more.                               \

# \
#                                                                              \
# 👉 In Bob's window — resume the game                                         \
#                                                                              \
# 👉 In Eve's window — Eve types shell commands on her server's stdin, and     \
#    their output appears back on her screen:                                  \
#        pwd                                                                   \
#        whoami                                                                \
#                                                                              \

# \
#                                                                              \
# Those commands ran on Bob's machine: Eve's "Life server" is a remote         \
# shell, hidden in an ordinary game. Alice saw none of this — she has only     \
# the wire. Can she find it there?                                             \


clear && header "3. Hidden bits" && picture images/dissecting.jpeg 22

# \
#                                                                              \
# She can. A covert channel has to bend the encoding — so Alice                \
# hunts non-canonical protobufs.                                               \


# \
#                                                                              \
# ######################################################################       \
# #                          Enters prototext                          #       \
# ######################################################################       \
#                                                                              \


# prototext is-canonical checks every capture's encoding:
prototext is-canonical capture/*.pb || red Some messages have anomalies

# Which ones carry anomalies:
prototext is-canonical capture/*.pb | grep anomalous

# protoc decodes the flagged messages cleanly — blind to the anomalies:
protoc --descriptor_set_in=life.desc \
       --decode=grehack.life.v1.StepResponse < capture/000000-response.pb \
  | view_textproto

# A closer look, with protolens:
protolens --descriptor-set life.desc capture/000000-response.pb \
  --script beats/smuggle

protolens --descriptor-set life.desc capture/000000-request.pb \
  --script beats/smuggle2

# \
#                                                                              \
# Alice has it, from the wire alone: Eve asked "whoami", Bob answered          \
# "experiment". Commands ride the responses, answers the requests.             \


clear && header "4. Eve's own log" && picture images/seizing.jpeg 22

# \
#                                                                              \
# With the wire evidence in hand, Eve's server is pulled. No trace of the      \
# life-server binary — but the server kept a log. What is it?                  \

ls -lh eve/server.log

# \
#                                                                              \
# No server binary, so no server descriptors to recover. All we hold is the    \
# client's descriptor set — try it as an ersatz against the blob:              \


protolens --descriptor-set life.desc eve/server.log \
  --script beats/logfile


clear && header "5. Takeaways" && picture images/takeaways.jpeg 22
# \
#                                                                              \
# No schema at hand  → descriptors usually hide in the binary: protoscan       \
#                      finds them, reproto gives the .proto back.              \
#                                                                              \
# No root type       → a corpus types even a message it has never seen,        \
#                      piece by piece: protolens's heat cues.                  \
#                                                                              \
# Lenient decoders   → what a decoder normalizes away is evidence — a          \
#                      padding bit, a shadowed value. is-canonical and         \
#                      protolens surface it.                                   \
#                                                                              \
# Damaged blob       → protolens reads up to the damage; protoc rejects        \
#                      the whole file.                                         \
#                                                                              \


clear && picture images/goodbye.jpeg 22
# \
# https://github.com/ThalesGroup/prototools — pull requests welcome 🙂         \
# https://search.nixos.org/packages?channel=unstable&query=prototext           \
#                                                                              \
# Thank you 👋                                                                 \




clear && header "Annex: anomalies"

# \
#                                                                              \
# Not every anomaly is accidental: fingerprints, covert channels, data         \
# below the app layer. protolens annotates every category:                     \

protolens --type google.protobuf.FileDescriptorSet anomalies.pb \
  --script beats/anomalies
