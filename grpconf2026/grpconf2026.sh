clear && header "S3NS"
# \
# S3NS is Thales x Google joint venture that operates a GCP region as a        \
# Trusted Cloud service, for European customers.                               \
# The service is hosted on French data centers and operated by French          \
# personel, in autonomy.                                                       \
#                                                                              \
# However Google provides all software updates, which are inspected and tested \
# in a quarantine environment before being deployed to production.             \
#                                                                              \
# As part of the inspection activities, we have developed tooling to help      \
# auditing protobufs, which are ubiquitous in the Google Cloud infrastructure. \


clear && header "prototools"
# \
# prototools is 3 CLIs :                                                       \
#                                                                              \
# - protoscan: extraction of FileDescriptorProto descriptors from executables  \
# - reproto:   analysis and decompilation of FileDescriptor sets               \
# - prototext: lossless ser/deser or protobufs, using schemas                  \

# \
# and 1 TUI:                                                                   \
#                                                                              \
# - protolens: interactive TUI for analysing binary protobufs                  \


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

# Let's try protoc for decoding the capture file:
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

# Let's retrieve, decompile and index the descriptors 🤞
reproto --desc-root bob/app --schema-db-out alice/app.desc

ls -lhd alice/app.desc alice/app/* \
# A couple of files were produced 💪:                                          \
# - app.desc:           Set of descriptors found in app ✅                     \
# - app/hopcroft.rkyv:  Graph for type inference ✅                            \
# - app/index.rkyv:     Index for fast access and loading ✅                   \
# - app/proto/:         Decompiled descriptors ✅                              \


tree alice/app/proto   | view
# Let's look at one decompiled .proto file:
view alice/app/proto/google/maps/places/v1/places_service.proto

header "4. Let's use our descriptors"

# prototext is our friend 🙏
# Given a descriptor set, it will try to infer the type of a protobuf:
prototext --descriptor-set alice/app.desc decode bob/capture   | view_textproto

# prototext was able to infer capture's protobuf type 🏅
#   👉 google.maps.places.v1.SearchTextRequest
# Let's use this with protoc:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture   | view_textproto
# Now protoc is happy 🍀🍀🍀

# Compare again with the prototext output.
# (This time, we'll use protolens, the interactive version of prototext.)
protolens --descriptor-set alice/app.desc bob/capture   --script beats/capture
# Notice that protolens adds annotations as comments
# There was more to bob/capture than met the protoc eye 🕵


# Let's try with the logfile:
protolens --descriptor-set alice/app.desc bob/logfile   --script beats/app.desc

# Let's re-open the logfile against googleapis.desc (25-megabyte descriptor set)
\
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET bob/logfile \
  --load-overrides alice/overrides \
  --script beats/googleapis.desc

# The exported prototext format keeps the details we just dug out:
view_textproto alice/logfile.all.node.pb

# It is also a faithful lossless representation of the original protobuf:
cmp bob/logfile <(prototext encode alice/logfile.all.node.pb) \
  && echo Identical 👍

# Bob will probably decide to stop using this suspicious app... 😠

# \
# Before the demo ends up, let me show you how protolens scales to larger      \
# protobufs.                                                                   \
# For example we can inspect googleapis.desc against itself, why not?          \

protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET   --script beats/scaling

# \
# As you can see, navigations stays fluid, and the startup time is short:
time protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET $PROTOTEXT_GOOGLEAPIS_SET quit

# Thank you for your attention 🙂





# The End...