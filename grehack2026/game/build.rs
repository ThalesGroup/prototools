// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Compiles `life.proto` (and, for the server, `log.proto`) and writes
//! `life.proto`'s `FileDescriptorProto` for the binaries to embed (spec 0375
//! S3).
//!
//! The embedded blob is the one `FileDescriptorProto` of `life.proto`, not a
//! `FileDescriptorSet`: that is what `protoscan` looks for in a binary.
//!
//! `log.proto` (spec 0386) is compiled so the server has generated code to
//! encode the traffic log, but its FDP is deliberately **not** embedded (spec
//! 0386 G4): the demo's step 3 depends on the audience having no schema for the
//! log type. Because `log.proto` imports `life.proto`, the descriptor set now
//! holds more than one file, so the life FDP is selected by name rather than
//! by being the only entry. The client never pulls in the generated `log`
//! module (spec 0386 G3), so it does not link the log types either.
//!
//! `descriptor.proto`'s FDP is written too, for the binaries to embed beside
//! life's (spec 0388 S13), which reproto needed in its input set; protoback
//! fills it in from its built-in copy, so it is now one more file protoscan
//! finds. It comes from protoc's own include directory.

use prost::Message;
use prost_types::FileDescriptorSet;
use std::{env, fs, path::PathBuf};

const LIFE_PROTO: &str = "proto/grehack/life/v1/life.proto";
const LOG_PROTO: &str = "proto/grehack/life/v1/log.proto";
const DESCRIPTOR_PROTO: &str = "google/protobuf/descriptor.proto";

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let set_path = out.join("life.fds");

    tonic_prost_build::configure()
        .file_descriptor_set_path(&set_path)
        // The service reads a request's values before decoding it (spec 0377
        // S2): the generated code builds this codec in place of the default
        // ProstCodec, in both the client and the server.
        .codec_path("crate::codec::TagReadingCodec")
        .compile_protos(&[LIFE_PROTO, LOG_PROTO, DESCRIPTOR_PROTO], &["proto"])
        .unwrap_or_else(|e| panic!("compiling the protos: {e}"));

    // Embed ONLY life.proto's FDP (spec 0386 S4, G4). The set compiled above
    // also holds log.proto (and its dependency ordering), so pick life.proto by
    // name; embedding the whole set would leak the log type into the binary.
    let set =
        FileDescriptorSet::decode(&fs::read(&set_path).expect("reading the descriptor set")[..])
            .expect("decoding the descriptor set");
    let life = set
        .file
        .iter()
        .find(|f| f.name() == "grehack/life/v1/life.proto")
        .expect("life.proto is in the compiled descriptor set");
    fs::write(out.join("life.fdp"), life.encode_to_vec()).expect("writing life.fdp");

    let descriptor = set
        .file
        .iter()
        .find(|f| f.name() == DESCRIPTOR_PROTO)
        .expect("descriptor.proto is in the compiled descriptor set");
    fs::write(out.join("descriptor.fdp"), descriptor.encode_to_vec())
        .expect("writing descriptor.fdp");

    println!("cargo::rerun-if-changed={LIFE_PROTO}");
    println!("cargo::rerun-if-changed={LOG_PROTO}");
}
