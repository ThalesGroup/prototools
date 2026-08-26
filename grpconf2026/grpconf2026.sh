clear && header "S3NS"
# \
# S3NS is Thales x Google joint venture that operates a GCP region as a Trusted Cloud service, for European customers. The service is hosted on French data centers and operated by French personel, in autonomy. However Google provides all software updates, which are inspected and tested in a quarantine environment before being deployed to production. As part of the inspection activities, we have developed tooling to help audit protobufs, which are ubiquitous in the design of the Google Cloud platform.                                                                \


header "prototools"
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

header "3. Let's retrieve descriptors"

# protoscan and reproto are our friends

protoscan bob/app   | view
# So bob/app contains reflected descriptors indeed

# Let's retrieve, decompile and index the descriptors
reproto --desc-root bob/app --schema-db-out alice/app.desc

ls -lhd alice/app.desc alice/app/* \
# A couple of files were produced:                                             \
# - app.desc:           Set of descriptors found in app                        \
# - app/hopcroft.rkyv:  Graph for type inference                               \
# - app/index.rkyv:     Index for fast access and loading                      \
# - app/proto/:         Decompiled descriptors                                 \


tree alice/app/proto   | view
# Let's look at one decompiled .proto file:
view alice/app/proto/google/maps/places/v1/places_service.proto

header "4. Let's use the descriptors"

# prototext is our friend 🙏
prototext --descriptor-set alice/app.desc decode bob/capture   | view_textproto

# prototext was able to infer capture's protobuf type 💪💪💪
#   👉 google.maps.places.v1.SearchTextRequest
# Let's use this with protoc:
\
protoc --descriptor_set_in=alice/app.desc \
       --decode=google.maps.places.v1.SearchTextRequest \
    < bob/capture   | view_textproto
# Now protoc is happy 🍀🍀🍀

# Compare again with the prototext output.
# This time, let's use protolens, the interactive version of prototext:
protolens --descriptor-set alice/app.desc bob/capture   --script beats/capture
# Notice that protolens adds annotations as comments
# There is more to capture than met the protoc eye 🕵


# Let's try with the logfile:
protolens --descriptor-set alice/app.desc bob/logfile   --script beats/app.desc

# Let's re-open the logfile against googleapis.desc (25-megabyte descriptor set)
\
protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET bob/logfile \
  --load-overrides alice/overrides \
  --script beats/googleapis.desc

view_textproto alice/export
cmp bob/logfile <(prototext encode alice/logfile.0-20134.node.pb) && echo Identical 👍



# \
# The standard tool needs three things Bob does not have.                       \
#
\
protoc --decode ??? --proto_path ??? ??? < $BOBSHARK

# \
# Even with a guess: not "wrong type" — nothing useful at all.                  \
#
\
protoc --decode google.protobuf.Timestamp --proto_path . ??? < $BOBSHARK

\
demo/header "3. Correct, complete, meaningless"

# \
# Field numbers, wire types, values.  Every byte accounted for.  Not one name.  \
# DO NOT EXPLAIN THIS SCREEN.  Show it, name the problem, move.                 \
#
\
protoscope $BOBSHARK

# \
# The log is also protobuf.  Also opaque.                                       \
#
\
protoscope $BOBLOG | head -30

\
demo/header "4. The notation, flat"

# \
# Same structure protoscope just printed — plus something protoscope did not    \
# say: a couple of these bytes are NON-CANONICAL.                              \
# First hint that this producer is not a stock encoder.                         \
#
\
prototext decode --raw $BOBSHARK

\
demo/header "5. The schema is in the executables"

# \
# bobapp has no symbols and no source, but it has to BUILD these messages at    \
# runtime — so the descriptors are in there.                                   \
#
\
protoscan $BOBAPP1 | head -20

# \
# READ THE NAMES OUT LOUD.  They say google/maps/routing/v2/.                   \
# Bob's question is already answered, before a byte was decoded.                \
# And one name is not Google's at all: bobapp/v1/log.proto.                     \
#
\
diff <(protoscan $BOBAPP1 | sort) <(protoscan $BOBAPP2 | sort)

# \
# Thirty-six new files, and three of them are the story:                        \
#   google/maps/places/v1/places_service.proto                                  \
#   google/maps/routes/v1/route_service.proto                                   \
#   google/rpc/error_details.proto                                              \
#                                                                               \
# "The newer build learned three things, and we are going to find out why it    \
#  needed each one."  Do not explain them yet.                                  \
#
\
demo/header "6. The binary IS the input"

# \
# No extraction step.  -I takes the binary directly.                           \
# ONE command: .proto source AND an indexed scoring database.                  \
#
\
reproto -I $BOBAPP1 -O $SRC1 --schema-db-out $DESC1 $SVC bobapp/v1/log.proto

\
cat $SRC1/$SVC

# \
# The second build.  The entry-point list is the whole difference, and it is    \
# read straight off the diff above: the three new ones, plus the two we had.    \
#
\
reproto -I $BOBAPP2 -O $SRC2 --schema-db-out $DESC2 $SVC google/maps/places/v1/places_service.proto google/maps/routes/v1/route_service.proto google/rpc/error_details.proto bobapp/v1/log.proto

\
ls -l $DESC1 $DESC2

# \
# What came back is EDITION 2023.                                               \
# The room's Rust toolchain cannot compile editions syntax.                     \
# Same wire format, proto2 syntax, builds today.                                \
#
\
reproto -I $BOBAPP1 -O $PROTO2 --force-proto2-output $SVC

\
diff -u $SRC1/$SVC $PROTO2/$SVC

\
demo/header "7. The editor"

# \
# An ordinary .proto file.  Any editor opens it.                                \
# This file is now available to us.                                             \
# In beat 10 it stops being a trophy and becomes the reference.                 \
#
\
nvim $SRC1/$SVC

\
demo/header "8. Inference"

# \
# The database is sixty seconds old and was inside the executable two minutes   \
# ago.  protolens scores every type in it against Bob's captured bytes.         \
#
\
protolens --descriptor-set $DESC1 -I $SRC1 --script $STAGE/beats/infer.script $BOBSHARK

\
demo/header "9. The log, named and half-read"

# \
# It opens NAMED — bobapp.v1.Log at +19 — and says truncated: true.            \
# Inside, the Routes entries read and the Places entries are junk.             \
# The method: line above the junk names the schema we are missing.             \
#
\
protolens --descriptor-set $DESC1 -I $SRC1 --script $STAGE/beats/log-v1.script $BOBLOG

\
demo/header "10. The newer build"

# \
# Same file.  One command apart.  The only thing that changed is the           \
# --descriptor-set — and it is the one built from the SECOND binary.           \
#
\
protolens --descriptor-set $DESC2 -I $SRC2 --script $STAGE/beats/log-v2.script $BOBLOG

\
demo/header "11. What Alice sends back"

# \
# An ordinary text file.  Every finding annotated inline.                      \
# Bob can open it in any editor with nothing installed.                        \
#
\
head -30 $STAGE/boblog.prototext

# \
# That text re-encodes to the file Bob handed over, byte for byte —            \
# including the 1 024 bytes missing from the end of it.                        \
#
\
prototext encode $STAGE/boblog.prototext -o $STAGE/roundtrip.pb

\
cmp $BOBLOG $STAGE/roundtrip.pb && echo identical

# \
# "The leaked key survived.                                                     \
#  It would not have survived protoc --decode."                                 \
#
\
demo/header "12. Two hundred times the dictionary"

# \
# DROPPABLE.  If beat 10 overran, cut this and say the quote from the podium.  \
#                                                                               \
# 7 771 files, 58 777 types, 25.6 MB — indexed once.  82 ms against 17 ms.     \
# And it reads LESS of the file: the envelope goes dark.                       \
#
\
protolens --descriptor-set $GOOGLEAPIS -I $SRC2 --script $STAGE/beats/scale.script $BOBLOG

\
demo/header "13. Close"

# \
# MIT.  github.com/ThalesGroup/prototools.  cargo install prototools.          \
# Leave the session loaded — questions get answered on the tool.               \
#
\
protolens --descriptor-set $DESC2 -I $SRC2 $BOBLOG
