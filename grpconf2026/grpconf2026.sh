clear && header "S3NS"
# \
# S3NS is a French Thales x Google joint venture that operates a GCP region    \
# as a Trusted Cloud service, for European customers.                          \

# \
# The platform is called PREMI3NS.                                             \

# \
# PREMI3NS is hosted on French data centers and operated by S3NS French        \
# personel, in autonomy.                                                       \

# \
# Google provides the software updates for the platform on a continuous basis. \
# Likewise, S3NS continuously inspects all new software updates before they    \
# can be approved for production:                                              \
# - Static analysis of the software and configuration packages                 \
# - Dynamic assessment of the service in a separate quarantine environment     \

# \
# As part of the inspection activities, we have developed tooling to help      \
# auditing protobufs, which are ubiquitous in the Google infrastructure.       \


clear && header "prototools"
# \
# prototools is 3 Command Line Interfaces and 1 Text-based User Interface      \
# for working with protobufs, including for reverse-engineering activities.    \

# \
# The CLIs:                                                                    \
# - protoscan: extractor of FileDescriptorProto descriptors                    \
# - reproto:   analysis and decompilation of FileDescriptor sets               \
# - prototext: lossless ser/deser tool with schema inference                   \

# \
# The TUI:                                                                     \
# - protolens: prototext "on steroids", the interactive version                \


# \
# We're going to demonstrate the prototools via a toy fictional scenario 🎬.   \

clear && header "The stage"

# \
# 👨 Bob downloaded an unknown executable and an associated log file.          \
# The executable seems to answer routing questions by calling some external    \
# service. Bob captured one of its network calls.                              \


ls -lh bob \
# Three files overall:                                                         \
# - bob/app       # the downloaded executable                                  \
# - bob/logfile   # the downloaded log file                                    \
# - bob/capture   # the capture Bob made                                       \


# \
# Bob suspects the calls to be gRPC, and the logs to be protobuf.              \


# \
# 👩 Alice is handed the lot for analysis 🚀🚀🚀                               \


clear && header "2. protoc falls short"

# Alice wants to have a look at the contents of the capture and the log file.
# She tries with protoc first, for decoding the capture file:
protoc --decode_raw < bob/capture   | view_textproto
# OK. This is protobuf indeed 🙂
# But we lack descriptors to make sense of it 🤔...

# Let's try with the logfile:
protoc --decode_raw < bob/logfile
# No luck here... 😭

header "3. If only we had descriptors"

# protoscan and reproto are our friends

protoscan bob/app   | view

# So bob/app contains reflected descriptors indeed 🥹
# Let's retrieve, decompile and index them into a schema database:
reproto --desc-root bob/app --schema-db-out alice/app.desc  # 🤞

ls -lhd alice/app.desc alice/app/* \
# A couple of files were produced 💪:                                          \
# - app.desc:           Set of descriptors found in app ✅                     \
# - app/hopcroft.rkyv:  Graph for type inference ✅                            \
# - app/index.rkyv:     Index for fast access and loading ✅                   \
# - app/proto/:         Decompiled descriptors ✅                              \


# Let's have a look at de decompiled descriptors:
tree alice/app/proto
# Let's have a look at places_service.proto for example:
view alice/app/proto/google/maps/places/v1/places_service.proto

header "4. Let's use our descriptors"

# prototext is our friend 🙏
# Given a descriptor set, it will try to infer the type of a protobuf:
prototext --descriptor-set alice/app.desc list-schemas bob/capture

# So prototext found one matching type for bob/capture 🏅
#   👉 google.maps.places.v1.SearchTextRequest
# (Even though with a negative score, we'll come back to that later.)
# Let's use this type with protoc:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture   | view_textproto
# Now protoc is happy 🍀🍀🍀

# Compare protoc's output with prototext's:
# (Actually, let's use protolens, the interactive version of prototext.)
protolens --descriptor-set alice/app.desc bob/capture   --script beats/capture

# protolens provided rich information about the protobuf.
# There was more to bob/capture than met the protoc eye 🕵


# Let's analyse the contents of the logfile against the same descriptor set:
protolens --descriptor-set alice/app.desc bob/logfile   --script beats/app.desc

# The googleapis is a large descriptor set:
ls -lh $PROTOTEXT_GOOGLEAPIS_SET
# Circa 25 megabytes and 8000 files
# Let's re-open the logfile against it
\
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET bob/logfile \
  --load-overrides alice/overrides \
  --script beats/googleapis.desc

# The exported prototext format keeps the details we just dug out:
view_textproto alice/logfile.all.node.pb

# It is also a faithful lossless representation of the original protobuf:
cmp bob/logfile <(prototext encode alice/logfile.all.node.pb)   && echo Identical 👍

# When Bob gets our finding, I bet he will stop using the app... 🤞

# \
# bob/capture and even bob/logfile are relatively small protobufs.             \
# protolens can happily deal with larger ones.                                 \
# For example, why not inspect googleapis.desc against itself 😎?              \

protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET

# \
# As we can see, navigation stays fluid, and the startup latency is short:
time protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET quit

# \
# In conclusion:                                                               \
# - A                                                                          \
# - B                                                                          \
# - C                                                                          \
# (TO BE FILLED)                                                               \


# Thank you for your attention 👋😊





# The End...