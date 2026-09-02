clear && header "S3NS"
# \
# S3NS is a French Thales × Google joint venture that operates a GCP region    \
# as a Trusted Cloud platform for European customers.                          \

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
#                                                                              \
#           https://github.com/ThalesGroup/prototools                          \
#                                                                              \
#           MIT License                                                        \
#                                                                              \

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
# We will demonstrate prototools through a toy fictional scenario 🎬.          \


clear && header "The stage"

# \
# 👨 Bob downloaded an unknown executable and an associated log file.          \
# The executable answers routing questions by calling an external service.     \
# Bob captured one of its network calls.                                       \

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
# This is protobuf indeed 🙂
# But field numbers without names mean nothing 🤔

# Let's try the logfile:
protoc --decode_raw < bob/logfile
# Outright failure 😭

clear && header "3. If only we had descriptors"

# Can we find descriptors in the binary itself?
protoscan bob/app

# \
# So bob/app contains reflected descriptors 🥹                                 \
# Interesting: they look like a subset of the Google APIs 💡.                  \

# Let's extract, decompile, and index them:
reproto --desc-root bob/app --schema-db-out alice/app.desc  # 🤞

ls -lhd alice/app.desc alice/app/* \
# reproto delivered 💪:                                                        \
# - app.desc:           descriptor set                                         \
# - app/hopcroft.rkyv:  type-inference graph                                   \
# - app/index.rkyv:     fast-access index                                      \
# - app/proto/:         decompiled .proto files                                \


# What's inside the decompiled places_service.proto?
view alice/app/proto/google/maps/places/v1/places_service.proto
# Confirmed: this is from the Google maps API.

clear && header "4. Descriptors to the help"

# Now prototext can infer the type of an unknown protobuf:
prototext --descriptor-set alice/app.desc list-schemas bob/capture \
  | bat -l yaml --style=plain

# One match: google.maps.places.v1.SearchTextRequest 🏅
# (Score is negative — we will come back to that.)

# Given the type, protoc will now happily decode the protobuf 🍀:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture   | view_textproto

# But protolens shows more:
protolens --descriptor-set alice/app.desc bob/capture   --script beats/capture

clear && header "5. On to the log file"

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

clear && header "6. Reporting to Bob"

# The exported prototext preserves everything we uncovered:
view_textproto alice/logfile.all.node.pb

# And it round-trips faithfully to the original binary:
cmp bob/logfile <(prototext encode alice/logfile.all.node.pb)   && echo Identical 👍

# When Bob sees this report, I suspect he will uninstall the app 😬

clear && header "7. Conclusion"

# \
# - Descriptors are often hiding in the binary itself                          \
# - protobuf is not opaque if you have the right tools                         \
# - prototools is open source — pull requests welcome 🙂                       \


# Thank you 👋


clear && header "8. Annexes"

# \
# A. Performance and scaling                                                   \
# B. reproto deep-dive                                                         \
# C. Anomaly taxonomy                                                          \


clear && header "A. Performance and scaling"

# \
# bob/capture and bob/logfile are small protobufs.                             \
# protolens handles large ones just as well.                                   \
# Let's throw googleapis.desc at itself (25MB descriptor set)                  \


# The startup remains fast:
time protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET quit

# And the navigation remains fluid
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET

clear && header "B. reproto deep-dive"

# \
# reproto decompiles FileDescriptorProto blobs back to .proto source.          \
# It is designed to be flexible and forgiving.                                 \

# \
# 1. We can ask reproto to decompile only a subset of an FDS.                  \

# Here, let's decompile only the "places v1" subset of the google APIs:
reproto \
    --desc-root $PROTOTEXT_GOOGLEAPIS_SET \
    --proto-out alice/places \
    --emit-binary \
    --seed 'file:google/maps/places/v1/*.proto'

# reproto extracted 34 files: the 18 places.v1 files plus their transitive deps:
tree -P "*.pb" alice/places
# and decompiled them to equivalent .proto files:
tree -P "*.proto" alice/places
# What's inside places_service.proto again?
view alice/places/google/maps/places/v1/places_service.proto

# \
# 2. We can also have reproto work on an incomplete FDS.                       \

# For example, let's make a copy the binary descriptors we just extracted:
rsync -a --include="*/" --include="*.pb" --exclude="*" \
  alice/places/ alice/places-incomplete/
# Now remove polyline.pb from the copy:
rm alice/places-incomplete/google/maps/places/v1/polyline.pb
# And remove the TravelMode definition from travel_mode.pb:
reproto \
    --desc-root alice/places-incomplete \
    --proto-out alice/places-incomplete \
    --emit-binary \
    --use-variant all \
    google/maps/places/v1/travel_mode.pb \
    --prune enum:google.maps.places.v1.TravelMode

# Finally, re-run the initial reproto command on the pruned set
reproto \
    --desc-root alice/places-incomplete \
    --proto-out alice/places-incomplete \
    --emit-binary \
    --seed 'file:google/maps/places/v1/*.proto'
# reproto noticed the missing dependencies and worked around them.

# How did it manage? Let's have a look at a diff:
view -d -R \
    alice/places/google/maps/places/v1/places_service.proto \
    alice/places-incomplete/google/maps/places/v1/places_service.proto

# \
# 3. We can ask reproto to transcribe to proto2 syntax, while decompiling      \
#    a FDP, even if the original syntax was proto3 or editions.                \
#    This can be handy if your compile tools don't support (say) editions.     \

# The relevant reproto option is --force-proto2-output:
reproto \
    --desc-root alice/places \
    --proto-out alice/places-proto2 \
    --force-proto2-output

# Let's have a look at the differences.
view -d -R \
    alice/places/google/maps/places/v1/place.proto \
    alice/places-proto2/google/maps/places/v1/place.proto
# Implicit proto3 optional labels have become explicit.
# Explicit proto3 optional labels have been translated into oneof constructs.


clear && header "C. Anomaly taxonomy"

# \
# Not all protobuf anomalies are accidental.                                   \
# Some are fingerprints. Some are covert channels.                             \
# Some hide data below the application layer.                                  \

# \
# prototext detects and annotates every category.                              \
# Here is the complete vocabulary:                                             \

protolens --descriptor-set $PROTOTEXT_WKT_SET \
    --type google.protobuf.FileDescriptorSet \
    anomalies.pb



# The End...
