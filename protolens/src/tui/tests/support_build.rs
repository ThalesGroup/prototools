// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! How a fixture is assembled, rather than what any one of them holds.
//!
//! Every schema-backed fixture in the `support_*` siblings is the same
//! three steps — write a descriptor set to a temporary file and load it,
//! decode a byte literal under a named root type, wrap the result in an
//! `App` — differing only in the schema, the root name and the bytes.
//! Those three steps live here once.

use super::super::*;
use prost::Message as _;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet};

use crate::decode::{decode, DescriptorContext, RootType};

/// `<label> <ty> <name> = <number>;` for a primitive `ty`.
pub(super) fn field(name: &str, number: i32, label: Label, ty: Type) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.to_string()),
        number: Some(number),
        label: Some(label as i32),
        r#type: Some(ty as i32),
        ..Default::default()
    }
}

/// [`field`] for a `ty` that names something — a message, a group or an
/// enum. `type_name` is fully qualified, leading dot and all, exactly as
/// `protoc` writes it.
pub(super) fn field_of(
    name: &str,
    number: i32,
    label: Label,
    ty: Type,
    type_name: &str,
) -> FieldDescriptorProto {
    FieldDescriptorProto {
        type_name: Some(type_name.to_string()),
        ..field(name, number, label, ty)
    }
}

/// `message <name> { <fields> }`.
pub(super) fn message(name: &str, fields: Vec<FieldDescriptorProto>) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.to_string()),
        field: fields,
        ..Default::default()
    }
}

/// A one-file, `proto3` descriptor set under `package` — what a fixture
/// wants when its schema has no dependency between files and no reason
/// to be `proto2`.
pub(super) fn proto3_fds_in(
    package: &str,
    file_name: &str,
    messages: Vec<DescriptorProto>,
) -> FileDescriptorSet {
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(file_name.to_string()),
            package: Some(package.to_string()),
            message_type: messages,
            syntax: Some("proto3".to_string()),
            ..Default::default()
        }],
    }
}

/// [`proto3_fds`]'s `proto2` counterpart, for the one thing proto3
/// cannot express: a `required` field.
pub(super) fn proto2_fds(file_name: &str, messages: Vec<DescriptorProto>) -> FileDescriptorSet {
    FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some(file_name.to_string()),
            package: Some("test".to_string()),
            message_type: messages,
            syntax: Some("proto2".to_string()),
            ..Default::default()
        }],
    }
}

/// [`proto3_fds_in`] under package `test`, which is what most fixtures
/// use — the package name is only observable when a test quotes a fully
/// qualified name, and `test` reads as "no particular package".
pub(super) fn proto3_fds(file_name: &str, messages: Vec<DescriptorProto>) -> FileDescriptorSet {
    proto3_fds_in("test", file_name, messages)
}

/// `message <name> { optional <type_name> <field_name> = 1; }` — one
/// level of burial, for the fixtures that need a plain message between
/// the root and the node under test.
pub(super) fn wrapper_message(name: &str, field_name: &str, type_name: &str) -> DescriptorProto {
    message(
        name,
        vec![field_of(
            field_name,
            1,
            Label::Optional,
            Type::Message,
            type_name,
        )],
    )
}

/// `google/protobuf/any.proto`, as `protoc` writes it: `Any { string
/// type_url = 1; bytes value = 2; }`.
///
/// Declared by hand rather than read from the real file because a
/// `DescriptorContext` is loaded from a descriptor set, not compiled
/// from sources — a fixture that wants `Any` has to carry it.
pub(super) fn any_proto_file() -> FileDescriptorProto {
    FileDescriptorProto {
        name: Some("google/protobuf/any.proto".to_string()),
        syntax: Some("proto3".to_string()),
        package: Some("google.protobuf".to_string()),
        message_type: vec![message(
            "Any",
            vec![
                field("type_url", 1, Label::Optional, Type::String),
                field("value", 2, Label::Optional, Type::Bytes),
            ],
        )],
        ..Default::default()
    }
}

/// An `Any`'s body: `type_url` at field 1, then `value` — an
/// already-encoded message — at field 2. Same one-varint-byte length
/// limit as [`wrap_len_field_1`].
pub(super) fn any_body(type_url: &str, value: &[u8]) -> Vec<u8> {
    assert!(type_url.len() < 0x80 && value.len() < 0x80);
    let mut out = vec![0x0Au8, type_url.len() as u8];
    out.extend_from_slice(type_url.as_bytes());
    out.push(0x12);
    out.push(value.len() as u8);
    out.extend_from_slice(value);
    out
}

/// `<bytes>` prefixed with the tag of field 1, `WT_LEN`, and its own
/// one-byte length — one more level of LEN nesting, matching one more
/// [`wrapper_message`]. Panics above 127 bytes, where the length would
/// need a two-byte varint the callers' hand-written layouts do not
/// account for.
pub(super) fn wrap_len_field_1(bytes: Vec<u8>) -> Vec<u8> {
    assert!(bytes.len() < 0x80, "length must fit one varint byte");
    let mut out = vec![0x0Au8, bytes.len() as u8];
    out.extend_from_slice(&bytes);
    out
}

/// A uniquely named path under the system temp directory, whose file is
/// removed when this value is dropped — whether or not one was ever
/// written there.
///
/// Both halves earn their keep. The unique name is what lets the whole
/// suite run concurrently, since two tests naming the same fixture would
/// otherwise race on each other's writes and deletes. The `Drop` is what
/// makes the delete safe: the hand-written form this replaced ended in
/// `remove_file(..).unwrap()`, which a failing assertion above it skips,
/// so every red test run leaked a file into the temp directory.
pub(super) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A path named after `name` — which should carry its own extension
    /// — and unique to this call. Nothing is written; the file need not
    /// ever exist, which is what a test of "save, then read back" wants.
    ///
    /// The counter is shared by every caller rather than one per
    /// fixture, so that two fixtures given the same `name` by accident
    /// do not collide either.
    pub(super) fn reserved(name: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        TempFile {
            path: std::env::temp_dir().join(format!("protolens-tui-{n}-{name}")),
        }
    }

    /// [`TempFile::reserved`] with `bytes` already written there.
    pub(super) fn written(name: &str, bytes: &[u8]) -> Self {
        let file = TempFile::reserved(name);
        std::fs::write(&file.path, bytes).unwrap();
        file
    }

    /// `fds` written out under `tag`, ready for `DescriptorContext::load`
    /// — which takes a path, not bytes, so a fixture with a schema has
    /// to put one on disk.
    pub(super) fn descriptor(tag: &str, fds: &FileDescriptorSet) -> Self {
        TempFile::written(&format!("{tag}-descriptor.pb"), &fds.encode_to_vec())
    }

    pub(super) fn load(&self) -> DescriptorContext {
        DescriptorContext::load(&self.path).unwrap()
    }

    pub(super) fn as_str(&self) -> &str {
        self.path.to_str().expect("temp path must be valid UTF-8")
    }

    /// Leaves the file on disk and yields its path.
    ///
    /// For the fixtures whose tests re-read the descriptor set later —
    /// `descriptor_sha256` hashes it from disk on demand (spec 0197
    /// §S6), and that read is an error rather than an empty hash if the
    /// file is gone.
    pub(super) fn keep(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A `DescriptorContext` over `fds`, with nothing left on disk.
pub(super) fn ctx_from_fds(tag: &str, fds: &FileDescriptorSet) -> DescriptorContext {
    TempFile::descriptor(tag, fds).load()
}

/// The `App` a fixture hands back: the tests' fixed indent of 2, the
/// dark theme, no scoring graph, and a document named `file` — which
/// varies only because the status line quotes it.
pub(super) fn app_named(decoded: Decoded, ctx: DescriptorContext, file: &str) -> App {
    App::new(
        decoded,
        file,
        PathBuf::from(file),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    )
}

/// [`app_named`] as `test.pb`, past the splash screen and wide enough
/// that nothing truncates — a fixture that is ready to be driven.
pub(super) fn fixture_app(decoded: Decoded, ctx: DescriptorContext) -> App {
    let mut app = app_named(decoded, ctx, "test.pb");
    app.splash = false;
    app.term_width = 120;
    app
}

/// A whole fixture from its three parts: the schema, the root type to
/// read `blob` under, and `blob` itself.
///
/// `tag` names the temporary descriptor file, so it should say which
/// fixture this is when one is left behind by a crash.
pub(super) fn fixture_under(tag: &str, fds: &FileDescriptorSet, root: &str, blob: &[u8]) -> App {
    let mut ctx = ctx_from_fds(tag, fds);
    let decoded = decode(wrapped(blob), &mut ctx, RootType::Named(root), 2).unwrap();
    fixture_app(decoded, ctx)
}
