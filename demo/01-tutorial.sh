# \
#                                                                                \
# prototools live demo                                                           \
#                                                                                \
# Full tutorial:                                                                 \
#   github.com/ThalesGroup/prototools/blob/main/docs/tutorial.md                 \
#                                                                                \
# Run from the repo root:  ./demo/prompt --splash prototools demo/01-tutorial.sh \
# Generated files go under ./stash/ (gitignored).                                \
#

\
demo/header "1. Setup"

# \
# Meet our two tools.
\
prototext --version && reproto --version

# \
# Build the googleapis schema DB — we will use it throughout.
\
export GOOGLEAPIS_DB=$(nix-build -A googleapis-db --no-out-link)/googleapis.desc
\
export GOOGLEAPIS_PBS=$(dirname $GOOGLEAPIS_DB)/instances
\
export GOOGLEAPIS_DESCS=$(dirname $GOOGLEAPIS_DB)/googleapis/proto

# \
# Pre-clean stash output dirs to avoid stale files from previous runs.
\
rm -rf stash/*

\
demo/header "2. Protobufs are everywhere"

# \
#                                                                                \
# Protobuf is the wire format of choice for most cloud APIs and microservices.   \
# When you are doing security work — auditing, forensics, incident response —    \
# you often receive raw binary blobs with no schema, no documentation, and no    \
# source code.  Standard tools either refuse to decode them or silently discard  \
# wire-level details that may be security-relevant.  prototools fills that gap.  \
#
# \
#                                                                                \
# "I spent 2.5 years at Google, and most of what I did was pushing one protobuf  \
#  from one place to another."                                                   \
#   — Hacker News, 2018, anonymous ex-Googler (item 18189458)                    \
#
# \
#                                                                                \
# Google themselves agree:                                                       \
# "Protocol buffers are the most commonly-used data format at Google.  They are  \
#  used extensively in inter-server communications as well as for archival       \
#  storage of data on disk."                                                     \
#   — protobuf.dev/overview, Google                                              \
#
# \
#                                                                                \
# Protobufs are compact (binary), self-describing (with a schema), and           \
# language-neutral — the lingua franca of microservice communication.            \
#

\
demo/header "3. What's inside a protobuf?"

# \
# Our running example: a postal address, serialised as a protobuf.
\
ls -lh $GOOGLEAPIS_PBS/google/type/PostalAddress.pb

# \
# Raw bytes — this is what travels on the wire.
\
hexdump -C $GOOGLEAPIS_PBS/google/type/PostalAddress.pb

# \
# Let's decode it as a protobuf:
\
prototext decode --raw \
    $GOOGLEAPIS_PBS/google/type/PostalAddress.pb \
  2>&1 | bat --style=numbers,header-filename -l pbtxt
# \
# 👆 No schema yet: field numbers and wire types, but no names.

# \
# With the right schema: the message becomes readable.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode --type google.type.PostalAddress \
    $GOOGLEAPIS_PBS/google/type/PostalAddress.pb \
  | bat --style=numbers,header-filename -l pbtxt

# \
# Here is the schema that unlocked it — the .proto source.
\
bat --style=numbers,header-filename -l proto \
    $GOOGLEAPIS_DESCS/google/type/postal_address.proto

\
demo/header "4. Schema auto-inference"

# \
#                                                                                \
# The googleapis schema DB bundles thousands of compiled schemas —               \
# one per message type across the entire Google Cloud API surface.               \
# prototext scores every type in the DB against the binary and picks             \
# the best match, consolidating field coverage, wire type matches,               \
# and value plausibility.                                                        \
#

# \
# Watch prototext infer the schema with no hint from us.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode $GOOGLEAPIS_PBS/google/type/PostalAddress.pb \
  | bat --style=numbers,header-filename -l pbtxt -H 1 -H 2
# \
# 👆 Notice the score at the top of the output — the higher, the better the fit. \
# The googleapis DB contains thousands of types; prototext scores them all and   \
# picks the best candidate.                                                      \
#
# \
#                                                                                \
# Let's try another example.                                                     \
#
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $GOOGLEAPIS_PBS/google/cloud/compute/v1beta/UsableSubnetwork.pb \
  2>&1 | bat --style=header-filename -l yaml -H 4 -H 6
# \
# 👆 prototext finds a tie and asks us to be explicit.

# \
# With --type the ambiguity is resolved.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode --type google.cloud.compute.v1beta.UsableSubnetwork \
    $GOOGLEAPIS_PBS/google/cloud/compute/v1beta/UsableSubnetwork.pb \
  | bat --style=numbers,header-filename -l pbtxt

\
demo/header "5. Non-canonical protobufs"

# \
#                                                                                \
# The wire format allows encodings that are valid but non-canonical: repeated    \
# optional fields, over-long varints, unexpected field ordering.  Standard       \
# decoders silently normalise or discard them — a potential side channel.        \
# prototext decode exposes every anomaly; prototext encode preserves them.       \
#
# \
#                                                                                \
# --- Hidden field ---                                                           \
#                                                                                \
# proto3 last-write-wins: an earlier occurrence of an optional field is silently \
# discarded by standard decoders.  This is a real steganographic / exfiltration  \
# vector.  prototext decode preserves wire order and exposes all occurrences.    \
#
\
bat --style=numbers,header-filename -l proto -r 13:25 -H 24 \
    $GOOGLEAPIS_DESCS/google/type/postal_address.proto
# \
#                                                                                \
# We are going to slip a secret value before the real organization field,        \
# hidden in plain sight inside a valid protobuf.                                 \
#

# \
# Craft postal_hidden.pb: slip a secret organization field before the real one.
# \
# (The #@ annotations are prototext's wire-encoding hints — field number and type.)
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode $GOOGLEAPIS_PBS/google/type/PostalAddress.pb \
  | sed '/^organization: "S3NS"/i organization: "Entrance secret PIN code: 666*"  #@ string = 11' \
  | prototext encode > stash/postal_hidden.pb

# \
# Let's look at the resulting patched protobuf:
\
hexdump -C stash/postal_hidden.pb
# \
# 👆 The hidden field is right there in the binary.

# \
# Standard decoder (protoc): only shows the last occurrence — secret gone.
\
protoc \
    --proto_path $GOOGLEAPIS_DESCS \
    --decode google.type.PostalAddress \
    google/type/postal_address.proto \
    < stash/postal_hidden.pb \
  | bat --style=numbers,header-filename -l pbtxt

# \
# prototext decode: preserves all contents.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode stash/postal_hidden.pb \
  | bat --style=numbers,header-filename -l pbtxt -H 5
# \
# 👆 Both occurrences visible: the secret first, then the real value, all fields in wire order.
# \
#                                                                                \
# --- Over-long varint ---                                                       \
#                                                                                \
# A spurious continuation-byte on a varint does not change its decoded value,    \
# but makes the encoding non-minimal.  Standard decoders strip it silently.      \
# prototext decode flags it and preserves it through encode.                     \
#

# \
# Craft postal_patched.pb: inject an over-long varint on the revision field.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    $GOOGLEAPIS_PBS/google/type/PostalAddress.pb \
  | sed 's/#@ int32 = 1$/#@ int32 = 1; val_ohb: 1/' \
  | prototext encode > stash/postal_patched.pb

# \
# Let's compare BEFORE the patch...
\
hexdump -C $GOOGLEAPIS_PBS/google/type/PostalAddress.pb | head -1
# \
# ... with AFTER the patch.
\
hexdump -C stash/postal_patched.pb | head -1
# \
# 👆 Spot the difference: one extra byte — `01` has become `81 00`.

# \
# protoc is oblivious to the non-canonical serialization.
\
protoc \
    --proto_path $GOOGLEAPIS_DESCS \
    --decode google.type.PostalAddress \
    google/type/postal_address.proto \
    < stash/postal_patched.pb \
  | bat --style=numbers,header-filename -l pbtxt

# \
# prototext decode shows full anomaly annotations by default.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    stash/postal_patched.pb \
  | bat --style=numbers,header-filename -l pbtxt -H 5
# \
# 👆 prototext flags it: look for val_ohb on the revision field.                 \
# (val_ohb = over-hung byte — the extra byte that shouldn't be there.)           \
#

# \
#                                                                                \
# prototext decode | prototext encode achieves a bit-perfect round-trip even     \
# with non-canonical or anomalous protobufs.  Nothing is silently normalised.    \
#

# \
# prototext round-trip: the over-hung byte is preserved exactly.
\
prototext --descriptor-set $GOOGLEAPIS_DB \
    decode \
    stash/postal_patched.pb \
  | prototext encode \
  | diff - stash/postal_patched.pb \
  && echo "$(tput setaf 2)bit-perfect accuracy$(tput sgr0)"

\
demo/header "6. Building a scoring database"

# \
#                                                                                \
# Quick vocabulary recap:                                                        \
#   protobuf   — a binary-encoded message on the wire                            \
#   schema     — the .proto definition that names fields and assigns types       \
#   descriptor — a .proto compiled into a binary protobuf (FileDescriptorProto)  \
#
# \
#                                                                                \
# A .proto source file compiles into a descriptor — a FileDescriptorProto.       \
# postal_address.pb is exactly that: the PostalAddress schema, serialised as     \
# a protobuf.  Let's have a look at it:                                          \
#

# \
# Vernacular: the PostalAddress schema as a descriptor.
vim +'set ft=pbtxt' <( \
    prototext --descriptor-set $GOOGLEAPIS_DB \
        decode --type google.protobuf.FileDescriptorProto \
        $GOOGLEAPIS_DESCS/google/type/postal_address.pb)

# \
# Auto-referential! 🤯 — the descriptor schema, decoded using itself.
vim +'set ft=pbtxt' <( \
    prototext decode \
        prototext/fixtures/prebuilt/descriptor.pb)

# \
#                                                                                \
# From descriptors, reproto can build a scoring DB — a compiled schema with      \
# a scoring graph side-car.  --schema-db-out produces it from any seed.          \
#

# \
#                                                                                \
# Let's build a scoring graph for a specific slice of googleapis: every          \
# OperationMetadata variant across Google Cloud APIs.  Each team defines its     \
# own — most share the same 7 fields, but Cloud Functions v2 has extra fields    \
# for build stages, source token, build name, and operation type.                \
#

\
reproto \
    --schema-db-out stash/opmeta.desc \
    --scoring-html-out stash/opmeta.html \
    --use-variant descriptor \
    -I $GOOGLEAPIS_DESCS \
    --seed 'desc:.google.cloud.apigeeregistry.v1.OperationMetadata' \
    --seed 'desc:.google.cloud.apihub.v1.OperationMetadata' \
    --seed 'desc:.google.cloud.apphub.v1.OperationMetadata' \
    --seed 'desc:.google.cloud.auditmanager.v1.OperationMetadata' \
    --seed 'desc:.google.cloud.baremetalsolution.v2.OperationMetadata' \
    --seed 'desc:.google.cloud.batch.v1.OperationMetadata' \
    --seed 'desc:.google.cloud.batch.v1alpha.OperationMetadata' \
    --seed 'desc:.google.cloud.beyondcorp.appconnections.v1.AppConnectionOperationMetadata' \
    --seed 'desc:.google.cloud.functions.v2.OperationMetadata' \
    $GOOGLEAPIS_DB

# \
# Auto-infer a Cloud Functions deployment operation.
\
prototext --descriptor-set stash/opmeta.desc \
    decode $GOOGLEAPIS_PBS/google/cloud/functions/v2/OperationMetadata.pb \
  | bat --style=numbers,header-filename -l pbtxt -H 1 -H 2
# \
# 👆 Score 26, unique match.
# \
#                                                                                \
# The raw scoring graph encodes every distinct wire shape in the seed set.       \
# Open it: each seed type is its own amber node, even the wire-identical ones.   \
#
# \
# The scoring graph: open in browser.
\
xdg-open stash/opmeta.html
# \
#                                                                                \
# Graph legend:                                                                  \
#   nodes  — amber/gold = top-level message; 5 brightness steps (Fibonacci):     \
#               1 type → darkest  2 → dim  3-4 → mid  5-7 → bright  8+ → gold    \
#             blue      = internal sub-message (no top-level name)               \
#   edges  — blue       = optional field                                         \
#             light blue = repeated field                                        \
#             purple    = packed repeated field                                  \
#             red       = required field (proto2 only)                           \
#   label  — short type name; "+N" suffix if N extra types collapsed into it     \
#
# \
#                                                                                \
# Many nodes are structurally identical: 8 teams defined their own               \
# OperationMetadata with the same 7 fields.  They are separate nodes here,       \
# but they could be merged — a binary consistent with one is consistent with     \
# all.  Hopcroft deduplication collapses them automatically.                     \
#

# \
# Hopcroft graph: 8 OperationMetadata states collapsed into 1 amber+8 node.
\
xdg-open stash/opmeta-hopcroft.html
# \
#                                                                                \
# To see deduplication at work on a small example: IpRules (5 → 4 nodes).        \
# Allowed and Denied have opposite semantics but identical wire structure:       \
#   message Allowed { repeated IpRule ip_rules = 1; }                            \
#   message Denied  { repeated IpRule ip_rules = 1; }                            \
# Hopcroft merges them automatically.                                            \
#

# \
# IpRules toy example: 5 nodes raw, 4 nodes after Hopcroft.
\
reproto -q \
    --schema-db-out stash/iprules.desc \
    --scoring-html-out stash/iprules.html \
    --use-variant descriptor \
    -I $GOOGLEAPIS_DESCS \
    --seed 'desc:.google.cloud.securitycenter.v2.Allowed' \
    --seed 'desc:.google.cloud.securitycenter.v2.Denied' \
    --seed 'desc:.google.cloud.securitycenter.v2.IpRule' \
    --seed 'desc:.google.cloud.securitycenter.v2.IpRules' \
    google/cloud/securitycenter/v2/ip_rules.pb

\
xdg-open stash/iprules.html

\
xdg-open stash/iprules-hopcroft.html

\
demo/header "7. Decompiling descriptors"

# \
#                                                                                \
# Descriptors are often available — embedded as reflection data inside           \
# compiled binaries, shipped alongside gRPC services, or stored in schema        \
# registries — even when the original .proto source is not.                      \
#                                                                                \
# reproto turns a descriptor back into readable .proto source.  Useful when      \
# you need to write code that audits or processes the corresponding protobufs.   \
#

# \
#                                                                                \
# As an example, let's decompile the postal_address descriptor back into         \
# its .proto source form.                                                        \
#

# \
# This is again how postal_address looks as a descriptor — a binary FileDescriptorProto:
out=$( \
    prototext decode $GOOGLEAPIS_DESCS/google/type/postal_address.pb \
); vim +'set ft=pbtxt' <(echo "$out"); bat --style=numbers,header-filename -r 1:5 -l pbtxt <<< "$out"

# \
# Decompile it back to .proto source.
\
reproto -q \
    -O stash/reproto-out \
    --use-variant descriptor \
    $GOOGLEAPIS_DESCS/google/type/postal_address.pb
# \
# Let's see the result.
\
vim stash/reproto-out/google/type/postal_address.proto; \
bat --style=numbers,header-filename -l proto \
    stash/reproto-out/google/type/postal_address.proto
# \
# 👆 Human-readable .proto source, recovered from the binary descriptor.
# \
#                                                                                \
# reproto can decompile an entire schema DB — thousands of files at once.        \
#

# \
# Decompile the entire googleapis DB: thousands of .proto files reconstructed.
\
reproto -O stash/googleapis-out \
    --use-variant descriptor \
    -I $GOOGLEAPIS_DESCS .
# \
#                                                                                \
# Browse the reconstructed sources in VSCode.  The proto language server         \
# understands the import graph: Go to Definition navigates across files,         \
# find-all-references works, and the full type hierarchy is explorable.          \
#

# \
# Browse the decompiled googleapis sources.
\
code --reuse-window stash/googleapis-out/google/type/postal_address.proto
# \
#                                                                                \
# What if some descriptors are missing — imported files not available?           \
# reproto handles incomplete inputs gracefully: missing imports are noted        \
# as orphan comments (///) in the output, so nothing is silently dropped.        \
# The --prune flag makes this explicit and controlled.                           \
#

\
demo/header "8. Seeding and pruning"

# \
#                                                                                \
# The decompilation is thousands of files — but Simon's audit team does not      \
# need all of googleapis.  They only care about one message: AuditLog, the       \
# record of every Cloud API call.  Pass its descriptor as the seed: reproto      \
# pulls only its transitive closure.                                             \
#

# \
# Seed on AuditLog: thousands of files collapse to 8.
\
reproto -q \
    -O stash/audit-seed \
    --use-variant descriptor \
    -I $GOOGLEAPIS_DESCS \
    google/cloud/audit/audit_log.pb
\
find stash/audit-seed -name '*.proto' | sort | bat --style=numbers,header-filename
# \
# 👆 The transitive closure: every file AuditLog depends on, nothing more.
# \
# Let's browse the decompiled AuditLog source.
\
code --reuse-window stash/audit-seed/google/cloud/audit/audit_log.proto
# \
#                                                                                \
# But Simon's tool only decodes payloads — it never needs to interpret RPC       \
# error statuses.  Prune google/rpc/status.proto: reproto drops the file and     \
# orphans the field that referenced it, leaving a /// comment so nothing is      \
# silently lost.                                                                 \
#
\
reproto -q \
    -O stash/audit-pruned \
    --use-variant descriptor \
    -I $GOOGLEAPIS_DESCS \
    --prune 'file:google/rpc/status.proto' \
    google/cloud/audit/audit_log.pb
# \
# Let's look at what was produced:
\
find stash/audit-pruned -name '*.proto' | sort | bat --style=numbers,header-filename
# \
# 👆 Prune status.proto: 8 files become 7.

# \
# Let's look at the impact on audit_log.proto:
\
bat --style=numbers,header-filename -l proto -r 13:22 -H 17 -H 20 \
    stash/audit-pruned/google/cloud/audit/audit_log.proto
# \
# 👆 status.proto import and AuditLog.status become /// orphans.                 \
# They are not silently dropped.                                                 \
# Simon's team can see exactly what was cut and why.                             \
#
# \
# Let's browse the pruned source.
\
code --reuse-window stash/audit-pruned/google/cloud/audit/audit_log.proto

\
demo/header "Conclusion"

# \
#                                                                                \
# prototools gives you:                                                          \
#   — a forensic decoder that preserves wire anomalies standard tools hide       \
#   — a schema DB that infers protobuf types without knowing the .proto source   \
#   — a decompiler that recovers .proto sources from binary descriptors          \
#   — a schema DB builder that collapses structurally equivalent types           \
#   — a scalpel for extracting exactly the schema slice your tool needs          \
#

# \
#                                                                                \
# prototools is open source — MIT licence.                                       \
#   github.com/ThalesGroup/prototools                                            \
#                                                                                \
# ~34 000 lines of Rust and Python.                                              \
#

# \
# THE END




# \
# 👆 Intentionally adding a few newlines sentinel before exiting
