// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A descriptor.proto the descriptor pool cannot build is refused.
//!
//! The variant's descriptor.proto is what every option is read against: one
//! the pool refuses (here, a scalar field claiming a type name) ends the run
//! with an error, exit status 1; not a traceback, and not a run that writes
//! every file without its options.
//!
//! Ported from `test_descriptor_proto_refused.py`. Its inputs, compiled by
//! protoc then modified with Python's protobuf (descriptor.proto's
//! `FileDescriptorProto.name` given a `type_name`), are those bytes,
//! committed as the `#@prototext` fixtures `descriptor_proto_refused/`.

use crate::common::{encode_fixture, protoback, tmp};

const USER_SHA256: &str = "7cfb540bf7227788eb81fff2f36298ef0639b1b5c0be831452a76303c5eba858";
const DESCRIPTOR_SHA256: &str = "68bfae848dbad60d3e319a9e07ba6ca2c98939f28e9af32ad7b231e58de59d5e";

#[test]
fn test_an_unbuildable_descriptor_proto_is_refused() {
    let dir = tmp("test_an_unbuildable_descriptor_proto_is_refused");
    let inputs = dir.join("in");
    encode_fixture(
        "descriptor_proto_refused/user.pb",
        USER_SHA256,
        &inputs.join("user.pb"),
    );
    encode_fixture(
        "descriptor_proto_refused/google/protobuf/descriptor.pb",
        DESCRIPTOR_SHA256,
        &inputs.join("google/protobuf/descriptor.pb"),
    );
    let run = protoback([
        format!("-I{}", inputs.display()).as_ref(),
        "-O".as_ref(),
        dir.join("out").as_os_str(),
        "user.pb".as_ref(),
        "google/protobuf/descriptor.pb".as_ref(),
    ]);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert!(!run.stderr.contains("Traceback"));
    assert!(
        run.stderr.trim_end().ends_with(
            "Error: descriptor.proto could not be built into a descriptor pool; \
             it is not a valid descriptor.proto"
        ),
        "{}",
        run.stderr
    );
}
