# \
# What's really in that .pb file?                                               \
# gRPConf 2026 North America — 20 minutes, live terminal.                       \
#
# Run from the repo root:  grpconf/prompt grpconf/presentation.sh               \
# ENTER runs the next command.  Up/Down browse and edit.                        \
# F2 duplicates a line, F3 deletes one, Ctrl-S saves.                           \

\
export STAGE=grpconf/stage BOBAPP=grpconf/stage/bin/bobapp BOBSHARK=grpconf/stage/bobshark BOBLOG=grpconf/stage/boblog GOOGLEAPIS=grpconf/stage/googleapis.desc BOBDESC=grpconf/stage/bobapp.desc SRC=grpconf/stage/src SRC2=grpconf/stage/src2 SVC=google/maps/routing/v2/routes_service.proto
\
for f in $BOBAPP $BOBSHARK $BOBLOG $GOOGLEAPIS; do [ -e "$f" ] && true || echo "MISSING: $f"; done
\
rm -rf $SRC $SRC2 $BOBDESC ${BOBDESC%.desc} $STAGE/boblog.prototext $STAGE/roundtrip.pb

\
demo/header "1. The problem"

# \
#                                                                                \
# Bob downloaded an executable.  It answers routing questions by calling some   \
# external service.  He logged its traffic and captured one of its calls.       \
#                                                                                \
# Three files.  No .proto.  No type name.  No source code.                      \
# Alice is handed the problem.                                                   \
#
\
ls -l $BOBAPP $BOBSHARK $BOBLOG

\
demo/header "2. protoc falls short"

# \
# The standard tool needs three things Bob does not have.                        \
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
#
\
protoscope $BOBSHARK

# \
# The log is also protobuf.  Also opaque.                                        \
#
\
protoscope $BOBLOG | head -30

\
demo/header "4. The notation, flat"

# \
# Same structure protoscope just printed — plus something protoscope did not     \
# say: a couple of these bytes are NON-CANONICAL.                               \
# First hint that this producer is not a stock encoder.                         \
#
\
prototext decode --raw $BOBSHARK

\
demo/header "5. The schema is in the executable"

# \
# bobapp has no symbols and no source, but it has to BUILD these messages at    \
# runtime — so the descriptors are in there.                                    \
#
\
protoscan $BOBAPP

# \
# READ THE NAMES OUT LOUD.                                                       \
# They say google/maps/routing/v2/.                                              \
# Bob's question is already answered.                                            \

\
demo/header "6. The binary IS the input"

# \
# No extraction step.  -I takes the binary directly.                            \
# ONE command: .proto source AND an indexed scoring database.                   \
#
\
reproto -I $BOBAPP -O $SRC --schema-db-out $BOBDESC $SVC

\
cat $SRC/$SVC

# \
# What came back is EDITION 2023.                                                \
# The room's Rust toolchain cannot compile editions syntax.                      \
# Same wire format, proto2 syntax, builds today.                                 \
#
\
reproto -I $BOBAPP -O $SRC2 --force-proto2-output $SVC

\
diff -u $SRC/$SVC $SRC2/$SVC

\
demo/header "7. The editor"

# \
# An ordinary .proto file.  Any editor opens it.                                 \
# This file is now available to us.                                              \
# In beat 10 it stops being a trophy and becomes the reference.                 \
#
\
nvim $SRC/$SVC

\
demo/header "8. Inference"

# \
# The database is sixty seconds old and was inside the executable two minutes   \
# ago.  protolens scores every type in it against Bob's captured bytes.         \
#
\
protolens --descriptor-set $BOBDESC -I $SRC --script $STAGE/beats/infer.script $BOBSHARK

\
demo/header "9. The log, half-read"

# \
# A different shape of document, and the first honest one.                       \
# The envelope is bobapp's own type — the recovered schema names it.            \
# Inside, some entries resolve.  Others do not.                                 \
# The heat cues mark them anyway.                                               \
#
\
protolens --descriptor-set $BOBDESC -I $SRC --script $STAGE/beats/log-partial.script $BOBLOG

\
demo/header "10. The full corpus"

# \
# Same file.  7 771 files, 58 777 types, 25.6 MB — indexed once.               \
# Still opens in about 50 ms.                                                   \
#
\
protolens --descriptor-set $GOOGLEAPIS -I $SRC --script $STAGE/beats/log-full.script $BOBLOG

\
demo/header "11. What Alice sends back"

# \
# An ordinary text file.  Every finding annotated inline.                       \
# Bob can open it in any editor with nothing installed.                         \
#
\
head -30 $STAGE/boblog.prototext

# \
# That text re-encodes to the file Bob handed over, byte for byte.              \
#
\
prototext encode $STAGE/boblog.prototext -o $STAGE/roundtrip.pb

\
cmp $BOBLOG $STAGE/roundtrip.pb && echo identical

# \
# "The leaked key survived.                                                      \
#  It would not have survived protoc --decode."                                  \
#

\
demo/header "12. Close"

# \
# MIT.  github.com/ThalesGroup/prototools.  cargo install prototools.           \
#
\
protolens --descriptor-set $GOOGLEAPIS -I $SRC $BOBLOG
