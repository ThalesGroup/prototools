// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0248: an extension field renders the same on both descriptor
//! branches.
//!
//! The file that *declares* an extension is in nobody's dependency closure —
//! an extendee has never heard of what extends it — so the lazy branch has no
//! reason to load it while resolving the root. Only `ext_to_file` knows where
//! it is, and until spec 0248 nothing consulted it during ordinary rendering.
//!
//! This is the user-visible reproduction reduced to a fixture: the same
//! descriptor set, decoded with and without its `index.rkyv` sidecar, must
//! produce byte-identical text.

use std::path::{Path, PathBuf};
use std::process::Command;

use prost::Message;
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};

/// `leaf.proto` / `root.proto` / `ext.proto`. proto2 throughout: proto3 has
/// no extension ranges.
fn fixture_files() -> Vec<FileDescriptorProto> {
    use prost_reflect::prost_types::descriptor_proto::ExtensionRange;

    let leaf = FileDescriptorProto {
        name: Some("leaf.proto".to_string()),
        package: Some("t".to_string()),
        message_type: vec![DescriptorProto {
            name: Some("Leaf".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("id".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Int32 as i32),
                ..Default::default()
            }],
            extension_range: vec![ExtensionRange {
                start: Some(100),
                end: Some(200),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    let root = FileDescriptorProto {
        name: Some("root.proto".to_string()),
        package: Some("t".to_string()),
        dependency: vec!["leaf.proto".to_string()],
        message_type: vec![DescriptorProto {
            name: Some("Root".to_string()),
            field: vec![FieldDescriptorProto {
                name: Some("leaf".to_string()),
                number: Some(1),
                label: Some(Label::Optional as i32),
                r#type: Some(Type::Message as i32),
                type_name: Some(".t.Leaf".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    let ext = FileDescriptorProto {
        name: Some("ext.proto".to_string()),
        package: Some("t".to_string()),
        dependency: vec!["leaf.proto".to_string()],
        extension: vec![FieldDescriptorProto {
            name: Some("tag".to_string()),
            number: Some(100),
            label: Some(Label::Optional as i32),
            r#type: Some(Type::Int32 as i32),
            extendee: Some(".t.Leaf".to_string()),
            ..Default::default()
        }],
        syntax: Some("proto2".to_string()),
        ..Default::default()
    };

    vec![leaf, root, ext]
}

/// Each file's `(start, end)` byte span within an encoded FDS, keyed by file
/// name.
type FdsSpans = Vec<(String, (u64, u64))>;

/// The FDS wire encoding plus each file's byte span within it. Laid out by
/// hand rather than through `FileDescriptorSet::encode`, because the spans
/// exist only while the records are being written and they are exactly what
/// `LazyPool` slices FDPs out of.
fn encode_fds(files: &[FileDescriptorProto]) -> (Vec<u8>, FdsSpans) {
    let mut buf = Vec::new();
    let mut spans = Vec::new();
    for file in files {
        let body = file.encode_to_vec();
        buf.push(0x0a); // field 1, LEN
        let mut len = body.len() as u64;
        loop {
            let byte = (len & 0x7f) as u8;
            len >>= 7;
            buf.push(if len == 0 { byte } else { byte | 0x80 });
            if len == 0 {
                break;
            }
        }
        let start = buf.len() as u64;
        buf.extend_from_slice(&body);
        spans.push((file.name().to_owned(), (start, buf.len() as u64)));
    }
    (buf, spans)
}

/// The sidecar `reproto` would have written for `files`.
fn write_index(files: &[FileDescriptorProto], spans: FdsSpans, path: &Path) {
    use prototext_graph::fds_index::{canonical_map, FdsIndex};

    let mut types = Vec::new();
    let mut deps = Vec::new();
    let mut exts = Vec::new();
    for file in files {
        let fname = file.name().to_owned();
        let prefix = format!("{}.", file.package());
        for msg in &file.message_type {
            types.push((format!("{prefix}{}", msg.name()), fname.clone()));
        }
        deps.push((fname.clone(), file.dependency.clone()));
        for x in &file.extension {
            let extendee = x.extendee().trim_start_matches('.');
            exts.push((format!("{extendee}/{}", x.number()), fname.clone()));
        }
    }

    let index = FdsIndex {
        type_to_file: canonical_map(types),
        file_to_span: canonical_map(spans),
        dep_graph: canonical_map(deps),
        ext_to_file: canonical_map(exts),
    };
    prototext_graph::fds_index::write(&index, path).unwrap();
}

/// `<dir>/schema.pb` plus the `<dir>/schema/` sidecar directory `load_pool`
/// derives from `path.with_extension("")`. The sidecar is written only when
/// `lazy` is set; without it the whole descriptor set is decoded up front.
fn fixture_dir(tag: &str, lazy: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prototext-0248-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("schema")).unwrap();
    let files = fixture_files();
    let (bytes, spans) = encode_fds(&files);
    std::fs::write(dir.join("schema.pb"), &bytes).unwrap();
    if lazy {
        write_index(&files, spans, &dir.join("schema").join("index.rkyv"));
    }
    dir
}

/// `Root { leaf { id: 5, [t.tag]: 42 } }` — field 100 is the extension.
const BLOB: &[u8] = &[0x0a, 0x05, 0x08, 0x05, 0xa0, 0x06, 0x2a];

fn decode_with(dir: &Path, blob: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_prototext"))
        // Spec 0228 S8: the dev-shell exports these and `nix-build` does not,
        // and this test always passes `--descriptor-set` explicitly.
        .env_remove("PROTOTEXT_DESCRIPTOR_SET")
        .env_remove("PROTOTEXT_DEFAULT_DESCRIPTOR")
        .arg("--descriptor-set")
        .arg(dir.join("schema.pb"))
        .arg("decode")
        .arg("--type")
        .arg("t.Root")
        .arg(blob)
        .output()
        .expect("prototext must run");
    assert!(
        out.status.success(),
        "decode failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("output is UTF-8")
}

#[test]
fn an_extension_renders_the_same_on_both_descriptor_branches() {
    let lazy_dir = fixture_dir("lazy", true);
    let eager_dir = fixture_dir("eager", false);
    let blob = std::env::temp_dir().join("prototext-0248-blob.pb");
    std::fs::write(&blob, BLOB).unwrap();

    let lazy = decode_with(&lazy_dir, &blob);
    let eager = decode_with(&eager_dir, &blob);

    assert!(
        eager.contains("[t.tag]: 42"),
        "the eager branch is the reference, and it resolves the extension: {eager}"
    );
    assert_eq!(lazy, eager, "the descriptor branch must not be observable");

    let _ = std::fs::remove_dir_all(&lazy_dir);
    let _ = std::fs::remove_dir_all(&eager_dir);
    let _ = std::fs::remove_file(&blob);
}
