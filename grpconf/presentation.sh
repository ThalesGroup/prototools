# \
#                                                                                \
# What's really in that .pb file?                                                \
#                                                                                \
# gRPConf 2026 North America — 20 minutes, live terminal.                        \
# The beat structure, the budgets and every claim below come from                \
# grpconf/synopsis.md (draft 2).  Read that first; this file is only its         \
# outer layer.  grpconf/artifacts.md says how the three files get built.         \
#                                                                                \
# Run from the repo root:  grpconf/prompt grpconf/presentation.sh                \
#                                                                                \
# ENTER runs the next command.  Up/Down browse and edit, F2 duplicates a         \
# line, F3 deletes one, Ctrl-S saves — that is the escape hatch if a beat        \
# has to be improvised.                                                          \
#                                                                                \
# The prompter blocks in `read -e` and `eval` returns only when the child        \
# exits, so a keystroke meant for protolens can never leak back here.            \
# That is the handoff interlock, and it costs no code.                           \
#
\
demo/header "0. Stage"

# \
#                                                                                \
# THE STORY.  Bob downloaded an executable off the net.  It answers              \
# questions about driving distances between cities by calling some external      \
# service.  He kept the log it wrote and captured one of its calls, and he       \
# sent all three to Alice with no .proto, no type name and no idea what the      \
# thing is talking to.  Alice is who this talk is addressed to.                  \
#                                                                                \
# Three files, built ahead of time (grpconf/artifacts.md).  The dev-shell        \
# populates grpconf/stage/ from `nix-build -A grpconf-demo`; everything          \
# below is a writable copy of that read-only nix store path:                     \
#                                                                                \
#   bobapp     the executable Bob downloaded.  Real gRPC client, descriptors     \
#              embedded UNCOMPRESSED.  No symbols, no source.                    \
#   bobshark   one request body, lifted out of Bob's capture.  Type unknown.     \
#   boblog     the log bobapp wrote.  Envelope + opaque payloads + four          \
#              anomalies + a tail cut mid-record.                                \
#                                                                                \
# And one pre-built database.  bobapp.desc is NOT here: it is built on stage     \
# in beat 6, out of the binary, which is the whole point of that beat.           \
#                                                                                \
# The binary lives at grpconf/stage/bin/bobapp so that bobapp.desc's stem        \
# directory (grpconf/stage/bobapp/) does not collide with it.                    \
#                                                                                \
#   googleapis.desc  the repo corpus: 7 771 files, 58 777 types, indexed once      \
#                                                                                \
# Until they exist this beat is the only one that fails, and it fails            \
# loudly rather than four beats in.                                              \
#
\
export STAGE=grpconf/stage
\
export BOBAPP=$STAGE/bin/bobapp BOBSHARK=$STAGE/bobshark BOBLOG=$STAGE/boblog
\
export GOOGLEAPIS=$STAGE/googleapis.desc BOBDESC=$STAGE/bobapp.desc
\
export SRC=$STAGE/src SRC2=$STAGE/src2 SVC=google/maps/routing/v2/routes_service.proto
\
for f in $BOBAPP $BOBSHARK $BOBLOG $GOOGLEAPIS; do [ -e $f ] || echo "MISSING: $f"; done

# \
# Clean everything beat 6 derives, so a rerun is honest.
\
rm -rf $SRC $SRC2 $BOBDESC ${BOBDESC%.desc} $STAGE/boblog.prototext $STAGE/roundtrip.pb

\
demo/header "1. The problem"

# \
#                                                                                \
# THE INTRO SLIDE IS THE PRESENTER'S TO WRITE — why we got interested in         \
# this, and why the tools were open-sourced.  What it has to END on: Bob,        \
# his three files, and the promise that everything else is derived from          \
# them live.                                                                     \
#                                                                                \
# The presenter types the next command BY HAND, before engaging the              \
# prompter, so the room sees a real terminal.                                    \
#
\
ls -l $BOBAPP $BOBSHARK $BOBLOG

\
demo/header "2. protoc falls short"

# \
#                                                                                \
# There is nothing to put in any of the three placeholders.  Ten seconds of      \
# an error message buys the premise.                                             \
#
\
protoc --decode ??? --proto_path ??? ??? < $BOBSHARK

# \
#                                                                                \
# Now guess a type name.  The second failure mode: protoc does not say           \
# "wrong type", it says nothing useful at all.                                   \
#                                                                                \
# Say once that this is not an argument against protoc, and do not litigate      \
# it again.                                                                      \
#                                                                                \
# CLAIM: the standard tool needs three things Bob does not have.                 \
#
\
protoc --decode google.protobuf.Timestamp --proto_path . ??? < $BOBSHARK

\
demo/header "3. Correct, complete, meaningless"

# \
#                                                                                \
# Field numbers, wire types, values.  Every byte accounted for.  Not one name.   \
#                                                                                \
# Run it on BOTH payloads.  The room needs to see that the log is also           \
# protobuf and also opaque — that is what makes beat 9 a payoff instead of       \
# a surprise.                                                                    \
#                                                                                \
# DO NOT EXPLAIN THIS SCREEN.  It is the one place where partial                 \
# comprehension is on-message.  Show it, name the problem, move.                 \
#                                                                                \
# CLAIM: the bytes were never the hard part.                                     \
#
\
protoscope $BOBSHARK

\
protoscope $BOBLOG | head -30

\
demo/header "4. The notation, flat"

# \
#                                                                                \
# The same structure protoscope just printed, plus something protoscope did      \
# not say: a couple of these bytes are NON-CANONICAL.  No stock encoder emits    \
# them.  First hint that Bob's app is not a stock anything — and beat 10 pays    \
# it off on the w row.                                                           \
#                                                                                \
# Text, #@ annotations, no panes, no caret, no colors to learn.  This exists     \
# so that protolens has ONE new thing to teach instead of two.                   \
#                                                                                \
# CLAIM: this is a file format, not a UI.                                        \
#
\
prototext decode --raw $BOBSHARK

\
demo/header "5. The schema is in the executable"

# \
#                                                                                \
# bobapp has no symbols and no source, but it has to BUILD these messages at     \
# runtime, so the descriptors are in there.  39 of them.                         \
#                                                                                \
# THEN READ THE NAMES OUT LOUD.  They say google/maps/routing/v2/.  Bob asked    \
# what his app was talking to, and the file listing already answered him,        \
# before a single byte was decoded.  That payoff is free and it is the best      \
# thing in this beat.                                                            \
#                                                                                \
# This beat ONLY REVEALS.  Nothing is extracted, no directory is written, no     \
# file is opened.  Measured: 39 names out of bobapp; 250 out of gh (55 MB,       \
# stripped, modern Go) in 0.47 s; all 7 771 out of googleapis.desc.              \
#                                                                                \
# CLAIM: the schema was in the executable the whole time — and its table of      \
# contents was the answer to Bob's question.                                     \
#
\
protoscan $BOBAPP

\
demo/header "6. The binary IS the input"

# \
#                                                                                \
# No extraction step.  -I takes a BLOB and stands for the descriptors embedded   \
# in it — one member per FileDescriptorProto found, which is exactly what        \
# protoscan just listed (spec 0243).  Imports resolve out of the same binary.    \
#                                                                                \
# Say the first point out loud: the usual pipeline here is extract, then hope    \
# the pieces fit together.  There is no extract.                                 \
#                                                                                \
# ONE command gives both the source AND the indexed scoring database that        \
# beats 8 and 9 open with.  bobapp.desc did not exist ten seconds ago.           \
#
\
reproto -I $BOBAPP -O $SRC --schema-db-out $BOBDESC $SVC

\
cat $SRC/$SVC

# \
#                                                                                \
# Now the part that earns the beat its time: what came back is EDITION 2023,     \
# and the room's Rust toolchain — prost, prost-reflect — cannot compile          \
# editions syntax.  Same wire format, proto2 syntax, builds today.               \
#                                                                                \
# CLAIM: archaeology that ends at a descriptor is half a result — and the        \
# descriptor never had to become a file.                                         \
#
\
reproto -I $BOBAPP -O $SRC2 --force-proto2-output $SVC

\
diff -u $SRC/$SVC $SRC2/$SVC

\
demo/header "7. The editor"

# \
#                                                                                \
# An ordinary .proto file.  Any editor opens it — a real claim about reproto's   \
# output, and worth saying out loud.                                             \
#                                                                                \
# This beat is SETUP, NOT INSPECTION, and its payoff is deferred all the way     \
# to beat 10.  Do not spend it admiring the source.  Spend it establishing       \
# that THIS FILE IS NOW AVAILABLE TO US, because in beat 10 it stops being a     \
# trophy and becomes the reference Alice reads before accepting a type for a     \
# subnode.                                                                       \
#                                                                                \
# It is also what makes protolens's nested v launch free: same editor, same      \
# colorscheme, same font, a screen the room already recognizes.                  \
#                                                                                \
# Leave with :q.                                                                 \
#
\
nvim $SRC/$SVC

\
demo/header "8. Inference"

# \
#                                                                                \
# The database is sixty seconds old and was inside the executable two minutes    \
# ago.  protolens scores every type in it against Bob's captured bytes and       \
# opens on the winner.  Bob's capture is readable.                               \
#                                                                                \
# IT IS RIGHT AND NOT CLOSE: ComputeRoutesRequest at -16, next at -55.           \
# bobapp.desc holds one version of the Routes API so nothing is ambiguous.       \
# The v1/v2 collision is real but needs googleapis.desc to happen — it is        \
# beat 10's moment, not beat 8's.  (Measured 2026-08-15; synopsis beat 8.)       \
#                                                                                \
# Score breakdown already on screen: unknown: 1, non_canonical: 1.              \
# Two of the four anomalies counted before the file has been opened.             \
#                                                                                \
# The script prefills the :override command line and stops there — the           \
# presenter presses Enter.  ANNOUNCE THAT.  A flawless instantaneous demo        \
# reads as a recording; work shown is proof.                                     \
#                                                                                \
# ,/; step, ?/. scroll.  space turns navigation off at any moment.               \
#                                                                                \
# CLAIMS: inference ranks rather than guesses; the ranking is legible;           \
# two of the findings arrive before anyone has looked at the document.           \
#
\
protolens --descriptor-set $BOBDESC -I $SRC --script $STAGE/beats/infer.script $BOBSHARK

\
demo/header "9. The log, half-read"

# \
#                                                                                \
# A different shape of document, and the first honest one.  The envelope is      \
# bobapp's own message type, so the recovered schema names it.  Inside, the      \
# entries do NOT all resolve: some are Routes v2 traffic and open in the         \
# clear, the rest are bytes fields that stay bytes.                              \
#                                                                                \
# And the HEAT CUES mark them anyway — [...] beside a field declared bytes       \
# that does not look like bytes at all.  protolens is saying: there is a         \
# message in here and I do not know which one.  It can say that without a        \
# schema, from wire shape alone.                                                 \
#                                                                                \
# LEAVE IT HANGING.  Do not resolve it in this beat.                             \
#                                                                                \
# CLAIM: a partial answer, honestly marked, is worth more than a confident       \
# wrong one.                                                                     \
#
\
protolens --descriptor-set $BOBDESC -I $SRC --script $STAGE/beats/log-partial.script $BOBLOG

\
demo/header "10. The full corpus"

# \
#                                                                                \
# Same file, bigger dictionary: 7 771 files, 58 777 types, 25.6 MB, indexed      \
# once — and it still opens in about 50 ms, because startup scales with the      \
# payload, not with the descriptor set.                                          \
#                                                                                \
# Three things land here, in order:                                              \
#                                                                                \
#  1. THE REST OF THE LOG RESOLVES.  Those opaque entries were calls to two      \
#     other Google services.  Bob's app does more than he thought.               \
#  2. THE BYTES FIELDS BECOME MESSAGES.  Override each one to the type the       \
#     heat cue was pointing at.  Different claim from beat 8's: not "the tool    \
#     ranked wrong" but "the schema was hiding structure, and the wire           \
#     disagreed with it".                                                        \
#  3. THE RECONSTRUCTED SOURCE EARNS ITS KEEP.  v on the field opens its         \
#     definition in the .proto reproto wrote in beat 6.  Judging whether a       \
#     type really fits a subnode is not something a score can do for you, and    \
#     what you want in front of you while you do it is the schema, as source,    \
#     in your own editor.                                                        \
#                                                                                \
# Then w, and the four anomalies, each held on screen long enough to be read:    \
# the duplicate field (Bob's API KEY, which his own app logs), the over-long     \
# varint (the beat-4 payoff), the undeclared field, the truncated tail.          \
#                                                                                \
# DELIBERATE STILLNESS: automation delivers a three-tier wire display in         \
# 200 ms and nobody absorbs it in 200 ms.  Commentary lands before each move     \
# or after it, never during.                                                     \
#                                                                                \
# "protoc --decode shows you one value.  prototext shows you both, because       \
#  both are on the wire.  The first one is Bob's API key."                       \
#
\
protolens --descriptor-set $GOOGLEAPIS -I $SRC --script $STAGE/beats/log-full.script $BOBLOG

\
demo/header "11. What Alice sends back"

# \
#                                                                                \
# Beat 11 is the last two steps of log-full.script (synopsis open question 3    \
# resolved: merge).  The overrides set in beat 10 are still in force.           \
# The protolens session has already exited; boblog.prototext is on disk.         \
#                                                                                \
# Two claims in one pipeline, and the order matters.                             \
#                                                                                \
# FIRST, THE ARTIFACT.  An ordinary text file, every finding annotated           \
# inline, that Bob can open in any editor without installing anything.  That     \
# is the deliverable — this demo has a recipient.                                \
#
\
head -30 $STAGE/boblog.prototext

# \
#                                                                                \
# SECOND, THE PROOF.  That text re-encodes to the file Bob handed over, byte     \
# for byte.                                                                      \
#
\
prototext encode $STAGE/boblog.prototext -o $STAGE/roundtrip.pb

\
cmp $BOBLOG $STAGE/roundtrip.pb && echo identical

# \
#                                                                                \
# "The leaked key survived.  It would not have survived protoc --decode."        \
#                                                                                \
# All three advertised promises close in one sentence.                           \
#
\
demo/header "12. Close"

# \
#                                                                                \
# MIT.  github.com/ThalesGroup/prototools.  cargo install prototools.            \
#                                                                                \
# The session is LEFT LOADED rather than exited — with instant manual            \
# interaction available, questions get answered on the tool.                     \
#                                                                                \
# The full anomaly vocabulary, all thirty tokens, is the repo pointer:           \
#   protolens --type google.protobuf.FileDescriptorProto grpconf/anomalies.pb    \
#
\
protolens --descriptor-set $GOOGLEAPIS -I $SRC $BOBLOG
