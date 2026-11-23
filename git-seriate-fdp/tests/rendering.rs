// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The FDP renderer's version holds its text (spec 0464 S5): the files of
//! a frozen descriptor set, rendered against that set, compared with a
//! committed copy. A history commit records the version, and `settle`
//! trusts what a commit of the same version wrote: a change of text with
//! the same version would split one version of a file in two.
//!
//! When this fails, prototext writes other text: refresh the copy
//! (`GIT_SERIATE_REFRESH_RENDERING=1 cargo test -p git-seriate-fdp --test
//! rendering`) and raise `FdpRenderer::version`'s `prototext`, together.

use git_seriate::plugins::Renderer;
use git_seriate_fdp::{FdpRenderer, Schema, FDP};

/// Frozen: a test fixture, not a copy of the release (spec 0460 N3).
const SET: &[u8] = include_bytes!("../../prototext-core/fixtures/descriptor.pb");

/// The set's files, each a `FileDescriptorProto`'s bytes (field 1).
fn files(mut b: &[u8]) -> Vec<&[u8]> {
    fn varint(b: &mut &[u8]) -> usize {
        let (mut v, mut shift) = (0usize, 0);
        loop {
            let byte = b[0];
            *b = &b[1..];
            v |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte < 0x80 {
                return v;
            }
        }
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        assert_eq!(varint(&mut b), 0x0a, "field 1, length-delimited");
        let n = varint(&mut b);
        out.push(&b[..n]);
        b = &b[n..];
    }
    out
}

#[test]
fn the_renderer_version_holds_its_text() {
    let renderer = FdpRenderer::new(Schema::from_descriptor_set(SET, FDP).unwrap());
    let mut text = Vec::new();
    for (i, f) in files(SET).iter().enumerate() {
        text.extend(format!("=== file {i}\n").into_bytes());
        text.extend(renderer.render("x.pb", f).unwrap());
    }
    let expected =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rendering/descriptor.txt");
    if std::env::var_os("GIT_SERIATE_REFRESH_RENDERING").is_some() {
        std::fs::write(&expected, &text).unwrap();
    }
    let committed = std::fs::read(&expected).unwrap();
    assert!(
        text == committed,
        "prototext renders {} otherwise: raise FdpRenderer's prototext version and refresh the copy \
         (see this file's doc)",
        expected.display()
    );
}
