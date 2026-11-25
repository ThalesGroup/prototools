// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What `life-server` and `life-client` share: the generated gRPC code and
//! the embedded descriptor. The game logic is not here — it is the
//! server's alone (spec 0375 N4).

pub mod codec;
pub mod tags;

/// The generated messages and service of `grehack.life.v1`.
///
/// The service stubs use [`codec::TagReadingCodec`] (spec 0377 S2), whose
/// decoder and encoder hand each message's raw bytes to the callbacks the
/// server and client install for the tag channel (spec 0379). A side that
/// installs neither gets a plain prost pass-through.
pub mod pb {
    tonic::include_proto!("grehack.life.v1");

    /// The traffic-log types of `grehack.life.v1.log` (spec 0386), compiled for
    /// the server's `--log-file` encoding. Nested under `pb` so the generated
    /// code's `super::StepRequest`/`super::StepResponse` resolve to the game
    /// types above. **Not** reflected: `log.proto`'s `FileDescriptorProto` is
    /// deliberately excluded from [`DESCRIPTOR`] (spec 0386 G4, build.rs), so
    /// `protoscan` and a client-derived `life.desc` never surface these types
    /// (G3). Only the server uses this module; the client leaves it unused.
    pub mod log {
        tonic::include_proto!("grehack.life.v1.log");
    }
}

/// The serialized `FileDescriptorProto` of `life.proto`, which `protoscan`
/// finds in each binary (spec 0375 S3). The service is not reflected: this
/// blob is the only way to the schema.
pub static DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/life.fdp"));

/// The serialized `FileDescriptorProto` of the well-known
/// `google/protobuf/descriptor.proto`, embedded beside [`DESCRIPTOR`]
/// (spec 0388 S13), which reproto needed in its input set; protoback fills
/// it in from its built-in copy, so it is now one more file protoscan finds.
pub static DESCRIPTOR_PROTO: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.fdp"));

/// The fully qualified name of the one method, read from [`DESCRIPTOR`]:
/// `/grehack.life.v1.Life/Step`.
///
/// Reading the descriptor at startup, rather than only holding it, is what
/// guarantees the linker keeps it. [`DESCRIPTOR_PROTO`] is never read, so it
/// goes through `black_box`, which the linker cannot see through either.
pub fn step_path() -> String {
    use prost::Message;
    std::hint::black_box(DESCRIPTOR_PROTO);
    let file = prost_types::FileDescriptorProto::decode(DESCRIPTOR)
        .expect("the embedded descriptor is written by build.rs");
    let service = &file.service[0];
    format!(
        "/{}.{}/{}",
        file.package(),
        service.name(),
        service.method[0].name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_descriptor_names_the_method() {
        assert_eq!(step_path(), "/grehack.life.v1.Life/Step");
    }
}
