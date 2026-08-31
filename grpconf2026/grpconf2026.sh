clear && header "S3NS"
# \
# S3NS is a French Thales × Google joint venture that operates a GCP region    \
# as a Trusted Cloud service for European customers.                           \

# \
# The platform is called PREMI3NS.                                             \

# \
# PREMI3NS is hosted in French data centers and operated by S3NS French        \
# personnel, autonomously.                                                     \

# \
# Google supplies continuous software updates for the platform.                \
# S3NS inspects every update before it reaches production:                     \
# - Static analysis of software and configuration packages                     \
# - Dynamic assessment in an isolated quarantine environment                   \

# \
# As part of those inspection activities, we built tooling to audit protobufs, \
# which are ubiquitous in the Google infrastructure.                           \


clear && header "prototools"
# \
# prototools is 3 Command-Line Interfaces and 1 Text-based User Interface      \
# for working with protobufs, including for reverse-engineering.               \

# \
# The CLIs:                                                                    \
# - protoscan: extracts FileDescriptorProto descriptors from binaries          \
# - reproto:   decompiles and analyzes FileDescriptor sets                     \
# - prototext: lossless serialization / deserialization with schema inference  \

# \
# The TUI:                                                                     \
# - protolens: prototext "on steroids" — the interactive version               \


# \
# We will demonstrate prototools through a short fictional scenario 🎬.        \

clear && header "The stage"

# \
# 👨 Bob downloaded an unknown executable and an associated log file.          \
# The executable answers routing questions by calling an external service.     \
# Bob captured one of its network calls.                                       \

[[Consider naming the external service "Google Maps" explicitly here,
  since that is what the audience will recognize once descriptors appear.
  Or keep it vague for dramatic effect — your call.]]

ls -lh bob \
# Three files:                                                                 \
# - bob/app       the downloaded executable                                    \
# - bob/logfile   the associated log file                                      \
# - bob/capture   the network capture Bob made                                 \


# \
# Bob suspects gRPC calls and protobuf logs.                                   \


# \
# 👩 Alice is handed the lot for analysis. Let's go 🚀                         \


clear && header "2. protoc falls short"

# Alice's first reflex: protoc --decode_raw.
protoc --decode_raw < bob/capture   | view_textproto
# OK — this is protobuf 🙂
# But field numbers without names mean nothing 🤔

# Let's try the logfile:
protoc --decode_raw < bob/logfile
# Outright failure 😭
# [[If protoc actually prints an error message here, quote the key line
#   aloud — it lands better than a silent failure.]]

header "3. If only we had descriptors"

# Can we find descriptors in the binary itself?
protoscan bob/app   | view

# bob/app contains reflected descriptors 🥹
# Let's extract, decompile, and index them:
reproto --desc-root bob/app --schema-db-out alice/app.desc  # 🤞

ls -lhd alice/app.desc alice/app/* \
# reproto produced:                                                            \
# - app.desc:           descriptor set ✅                                      \
# - app/hopcroft.rkyv:  type-inference graph ✅                                \
# - app/index.rkyv:     fast-access index ✅                                   \
# - app/proto/:         decompiled .proto files ✅                             \

# Let's peek at the decompiled sources:
tree alice/app/proto
view alice/app/proto/google/maps/places/v1/places_service.proto
# [[Point out the service name — "Places" — it tells the audience
#   what the app is actually calling.]]

header "4. Let's use our descriptors"

# prototext can infer the type of an unknown protobuf:
prototext --descriptor-set alice/app.desc list-schemas bob/capture

# One match: google.maps.places.v1.SearchTextRequest 🏅
# (Score is negative — we will come back to that.)
# For now, let's decode with protoc:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture   | view_textproto
# protoc is happy 🍀

# But protolens shows more:
protolens --descriptor-set alice/app.desc bob/capture   --script beats/capture

# There was more to bob/capture than protoc let on 🕵️


# Now the logfile, same descriptor set:
protolens --descriptor-set alice/app.desc bob/logfile   --script beats/app.desc

# app.desc only gets us so far.
# The googleapis descriptor set covers the full Google API surface:
ls -lh $PROTOTEXT_GOOGLEAPIS_SET
# ~25 MB, ~8 000 files.
# Let's reopen the logfile against it:
\
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET bob/logfile \
  --load-overrides alice/overrides \
  --script beats/googleapis.desc

# The exported prototext preserves everything we uncovered:
view_textproto alice/logfile.all.node.pb

# And it round-trips faithfully to the original binary:
cmp bob/logfile <(prototext encode alice/logfile.all.node.pb)   && echo Identical 👍

# When Bob sees this report, I suspect he will uninstall the app 😬

# \
# bob/capture and bob/logfile are small protobufs.                             \
# protolens handles large ones just as well.                                   \
# Let's throw googleapis.desc at itself:                                       \

protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET

# \
# Navigation stays fluid. Startup latency:                                     \
time protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET quit

# \
# In conclusion:                                                               \
# [[Fill in 3 punchy bullet points — suggested starting points:]]              \
# - Descriptors are often hiding in the binary itself                          \
# - protobuf is not opaque if you have the right tools                         \
# - prototools is open source — pull requests welcome 🙂                      \


# Thank you 👋


clear && header "B. reproto deep-dive"

# \
# reproto decompiles FileDescriptorProto blobs back to .proto source.         \
# It is designed to work under real-world constraints:                         \
# incomplete imports, missing types, legacy syntaxes.                          \

# \
# B.1. Full decompile — seeded to google.maps.places.v1.                      \
# Input: googleapis.pb (21 MB, ~8 000 FDPs in one blob).                      \
# Seed: every FDP whose name matches google/maps/places/v1/*.proto.            \

reproto \
    -I $PROTOTEXT_GOOGLEAPIS_PBS \
    --seed 'file:google/maps/places/v1/*.proto' \
    --use-variant all \
    --emit-binary \
    --proto-out grpconf2026/alice/places-full

# reproto extracted 34 files: the 18 places.v1 files plus their transitive
# deps (google/api, google/type, google/geo/type, google/protobuf WKTs).

tree grpconf2026/alice/places-full
view grpconf2026/alice/places-full/google/maps/places/v1/places_service.proto


# \
# B.2. Missing imports.                                                        \
# Same 18 places.v1 .pb files, but without the surrounding googleapis corpus. \
# reproto works — but marks unresolvable types clearly.                        \

reproto \
    --use-variant all \
    --proto-out grpconf2026/alice/places-missing \
    $(ls grpconf2026/alice/places-full/google/maps/places/v1/*.pb \
        | grep -v '/routing_preference\.pb$')

# Files that import RoutingPreference render it as an unresolvable dotted FQDN.

diff \
    grpconf2026/alice/places-full/google/maps/places/v1/places_service.proto \
    grpconf2026/alice/places-missing/google/maps/places/v1/places_service.proto \
    | view


# \
# B.3. Incomplete input — TravelMode enum removed from travel_mode.pb.        \
# reproto can prune at the symbol level, not just at the file level.          \

reproto \
    --use-variant all \
    -I grpconf2026/alice/places-full \
    --prune 'enum:google.maps.places.v1.TravelMode' \
    --proto-out grpconf2026/alice/places-incomplete \
    grpconf2026/alice/places-full/google/maps/places/v1/*.pb

grep 'TravelMode\|travel_mode' \
    grpconf2026/alice/places-incomplete/google/maps/places/v1/places_service.proto \
    | view


# \
# B.4. proto3 → proto2 translation.                                           \
# --force-proto2-output rewrites proto3 files to proto2 syntax.               \
# In proto2, every singular field must carry an explicit label.                \

reproto \
    --use-variant all \
    --force-proto2-output \
    --emit-binary \
    -I grpconf2026/alice/places \
    --seed 'file:google/maps/places/v1/*.proto' \
    --proto-out grpconf2026/alice/places-proto2 \
    grpconf2026/alice/places/google/maps/places/v1/*.pb

# Every field gained an explicit 'optional' label:
diff \
    grpconf2026/alice/places/google/maps/places/v1/place.proto \
    grpconf2026/alice/places-proto2/google/maps/places/v1/place.proto \
    | view

# \
# The wire encoding is unchanged — same binary, different schema syntax.      \


clear && header "C. Anomaly taxonomy"

# \
# Not all protobuf anomalies are accidental.                                   \
# Some are fingerprints. Some are covert channels.                             \
# Some hide data below the application layer.                                  \

# \
# prototext detects and annotates every category.                              \
# Here is the complete vocabulary, ordered from most to least interesting:     \

protolens --descriptor-set $PROTOTEXT_WKT_SET \
    --type google.protobuf.FileDescriptorProto \
    grpconf2026/anomalies.pb \
    --script grpconf2026/anomalies.script



# The End...