clear && header "S3NS"
# \
# S3NS is a French company, a joint venture between Thales, a global           \
# technology leader in aerospace, defense and security, and Google.            \
# We provide a dedicated GCP region for European customers,                    \
# as a Trusted Cloud platform (SecNumCloud qualified).                         \

# \
# The platform is hosted in French data centers and operated by our own        \
# French personnel.                                                            \

# \
# Google supplies software updates on a continuous basis, and we inspect       \
# them — also on a continuous basis — before they reach our production.        \

# \
# As part of that inspection work, we developed prototools: a software         \
# suite for auditing protobufs, which are ubiquitous in Google's               \
# infrastructure.                                                              \


clear && header "prototools"
# \
#                                                                              \
#           https://github.com/ThalesGroup/prototools                          \
#                                                                              \

# \
# We have open-sourced prototools under the MIT license, in the hope that      \
# others will find it useful.                                                  \

# \
# prototools has three command-line interfaces:                                \
# - protoscan, for scanning any blob for binary protobuf schema descriptors    \
# - reproto, for extracting, indexing, and decompiling schema descriptors      \
#   back to .proto source files                                                \
# - prototext, for converting protobufs between binary and text format —       \
#   and guaranteeing a byte-for-byte round trip. And if you give it a corpus   \
#   of schema descriptors, prototext will infer the type of a protobuf         \
#   automatically as it converts it to text.                                   \

# \
# and one text-based user interface:                                           \
# - protolens, the interactive version of prototext. protolens will help you   \
#   recover the type of an unknown protobuf even when your corpus of schema    \
#   descriptors is incomplete.                                                 \

# \
# We will demonstrate prototools through a toy fictional scenario 🎬.          \


clear && header "1. Setting the stage"

# \
# The scenario goes as follows:                                                \

# \
# 👨 Bob downloaded an unknown executable and an associated log file.          \
# The executable answers routing questions by calling an external service.     \
# Bob captured one of its network calls.                                       \

ls -lh bob \
# So we have three files of interest:                                          \
# - bob/app       the downloaded executable                                    \
# - bob/logfile   the associated log file                                      \
# - bob/capture   the network capture Bob made                                 \

# \
# Bob believes the app communicates over gRPC, using protobufs.                \


# \
# 👩 Alice is handed all three for analysis. Let's go 🚀                       \


clear && header "2. protoc falls short"

# Alice attempts to decode the network capture with protoc:
protoc --decode_raw < bob/capture \
  | view_textproto
# This is protobuf indeed 🙂
# But field numbers without names are difficult to interpret 🤔

# Then Alice tries the same with the downloaded log file:
protoc --decode_raw < bob/logfile
# This time protoc did not deliver. 😭

clear && header "3. If only we had descriptors"

# \
# Together with Alice, we wish we had schema descriptors to interpret the      \
# network capture. Maybe we can find some embedded in the downloaded app?      \

# Let's check with our first prototool: protoscan.
protoscan bob/app
# Yes! It worked 🥹.
# It looks like the app embeds a subset of the Google APIs descriptor set 💡.

# \
# Let's process those descriptors with our second prototool, reproto, so that  \
# we can take a look at them in the clear and enable type inference.           \

reproto --desc-root bob/app --schema-db-out alice/app.desc  # 🤞

ls -lhd alice/app.desc alice/app/* \
# reproto delivered 💪:                                                        \
# - app.desc:           extracted descriptor set                               \
# - app/proto/:         decompiled .proto files                                \
# - app/hopcroft.rkyv:  type-inference graph                                   \
# - app/index.rkyv:     fast-access index                                      \

# For instance what's inside the decompiled places_service.proto?
view alice/app/proto/google/maps/places/v1/places_service.proto
# Confirmed: this is from the Google maps API indeed.

clear && header "4. Descriptors to the help"

# \
# Now that we have a corpus of schema descriptors, we can use it to infer      \
# the type of the network capture, with our third prototool: prototext.        \

prototext --descriptor-set alice/app.desc list-schemas bob/capture \
  | bat -l yaml --style=plain
# \
# Yes! prototext found one type match for the network capture:                 \
#                                                                              \
#       👉 google.maps.places.v1.SearchTextRequest 🏅                          \

# Let's use that with protoc:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture \
  | view_textproto
# Good! Now we have field names instead of numbers 🍀.

# Our fourth prototool, protolens, will give more detailed information:
protolens --descriptor-set alice/app.desc bob/capture \
  --script beats/capture

clear && header "5. On to the log file"

# Now the logfile, same descriptor set:
protolens --descriptor-set alice/app.desc bob/logfile \
  --script beats/app.desc

# We happen to have the Google APIs descriptor set handy.
# It is a big one:
ls -lh $PROTOTEXT_GOOGLEAPIS_SET # ~25 MB, ~8 000 files.

# Let's reopen the log file against it:
\
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET bob/logfile \
  --load-overrides alice/overrides \
  --script beats/googleapis.desc

clear && header "6. Reporting to Bob"

# The exported prototext preserves everything we uncovered:
view_textproto alice/logfile

# And it round-trips faithfully to the original binary:
cmp bob/logfile <(prototext encode alice/logfile) \
  && echo Identical 👍

# When Bob sees this report, I suspect he will uninstall the app 😬

clear && header "7. Three takeaways"

# \
# 1. Descriptors are usually hiding in the binary itself. You rarely have to   \
#    guess a schema — reproto gives you the .proto back.                       \

# \
# 2. A corpus can type a message it has never seen, node by node, because      \
#    that message is built out of messages you already know. That is what      \
#    protolens's heat cues are for.                                            \

# \
# 3. What your decoder normalizes away is evidence. The enriched prototext     \
#    format preserves it, all the way back to the original bytes.              \


# https://github.com/ThalesGroup/prototools — pull requests welcome 🙂

# Thank you 👋



clear && header "Annexes"

# \
# A. Performance                                                               \
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
