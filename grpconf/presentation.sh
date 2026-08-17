# \
# What's really in that .pb file?                                               \
# gRPConf 2026 North America — 20 minutes, live terminal.                       \
#
# Run from the repo root:  grpconf/prompt grpconf/presentation.sh               \
# ENTER runs the next command.  Up/Down browse and edit.                        \
# F2 duplicates a line, F3 deletes one, Ctrl-S saves.                           \
#
# Pinned against grpconf/synopsis.md draft 3 (2026-08-17): two builds of        \
# Bob's app, two databases built on stage, googleapis as a droppable epilogue.  \

\
export STAGE=grpconf/stage BOBAPP1=grpconf/stage/bin/bobapp1 BOBAPP2=grpconf/stage/bin/bobapp2 BOBSHARK=grpconf/stage/bobshark BOBLOG=grpconf/stage/boblog GOOGLEAPIS=grpconf/stage/googleapis.desc DESC1=grpconf/stage/bobapp1.desc DESC2=grpconf/stage/bobapp2.desc SRC1=grpconf/stage/src1 SRC2=grpconf/stage/src2 PROTO2=grpconf/stage/proto2 SVC=google/maps/routing/v2/routes_service.proto
\
for f in $BOBAPP1 $BOBAPP2 $BOBSHARK $BOBLOG $GOOGLEAPIS; do [ -e "$f" ] && true || echo "MISSING: $f"; done

# \
# REHEARSAL GATE.  A pre-spec-0313 fdp_scan_lib drops the LAST descriptor of   \
# an embedded set, and in both binaries that is bobapp/v1/log.proto — which     \
# silently guts beats 9 through 11.  These two must print 41 and 77.            \
#
\
protoscan $BOBAPP1 | wc -l ; protoscan $BOBAPP2 | wc -l
\
rm -rf $SRC1 $SRC2 $PROTO2 $DESC1 $DESC2 ${DESC1%.desc} ${DESC2%.desc} $STAGE/boblog.prototext $STAGE/roundtrip.pb

\
demo/header "1. The problem"

# \
# Bob downloaded an executable, twice — an older build and a newer one.        \
# It answers routing questions by calling some external service.  He logged    \
# its traffic and captured one of its calls.                                   \
#                                                                               \
# Four files.  No .proto.  No type name.  No source code.                      \
# Alice is handed the problem.                                                  \
#                                                                               \
# "The two binaries differ by 53 KB and nothing else visible.                   \
#  I do not yet know what the second one is for."                               \
#
\
ls -l $BOBAPP1 $BOBAPP2 $BOBSHARK $BOBLOG

\
demo/header "2. protoc falls short"

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
