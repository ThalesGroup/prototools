// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The protoscan command line (spec 0407 S5): each test runs the binary on a
//! file it builds, and checks stdout, stderr, the exit status and the files
//! written. The cases are those of the former Python CLI's tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prototext_upb::descriptor::{Encode, FileDescriptorProto};

/// A minimal serialised FileDescriptorProto.
///
/// Sets `package` as well as `name`: a record with only its name is
/// indistinguishable from a Java package option, and the scanner rejects it
/// (spec 0313 S4).
fn fdp(name: &str) -> Vec<u8> {
    FileDescriptorProto {
        name: Some(name.to_string()),
        package: Some("test".to_string()),
        ..Default::default()
    }
    .encode_to_vec()
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("protoscan-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn protoscan(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_protoscan"))
        .args(args)
        .output()
        .expect("protoscan must run")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn an_empty_binary_prints_nothing() {
    let dir = scratch("empty");
    let bin = dir.join("empty.bin");
    std::fs::write(&bin, b"").unwrap();
    let out = protoscan(&[&bin]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_single_descriptor_is_printed() {
    let dir = scratch("single");
    let bin = dir.join("one.bin");
    std::fs::write(&bin, fdp("foo/bar.proto")).unwrap();
    let out = protoscan(&[&bin]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("foo/bar.proto"), "{}", stdout(&out));
}

#[test]
fn several_descriptors_are_all_printed() {
    let dir = scratch("multiple");
    let bin = dir.join("many.bin");
    let names = ["a/one.proto", "b/two.proto", "c/three.proto"];
    let mut data = Vec::new();
    for name in names {
        data.extend(fdp(name));
        data.push(0);
    }
    std::fs::write(&bin, data).unwrap();
    let out = protoscan(&[&bin]);
    assert!(out.status.success(), "{}", stderr(&out));
    for name in names {
        assert!(
            stdout(&out).contains(name),
            "{name} missing: {}",
            stdout(&out)
        );
    }
}

#[test]
fn proto_out_writes_the_descriptor_as_pb() {
    let dir = scratch("writes");
    let bin = dir.join("one.bin");
    let payload = fdp("pkg/thing.proto");
    std::fs::write(&bin, &payload).unwrap();
    let out_dir = dir.join("out");
    let out = protoscan(&[&bin, Path::new("--proto-out"), &out_dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    let expected = out_dir.join("pkg/thing.pb");
    assert_eq!(std::fs::read(&expected).unwrap(), payload);
}

#[test]
fn proto_out_creates_parent_directories() {
    let dir = scratch("parents");
    let bin = dir.join("deep.bin");
    std::fs::write(&bin, fdp("a/b/c/deep.proto")).unwrap();
    let out_dir = dir.join("out");
    let out = protoscan(&[&bin, Path::new("--proto-out"), &out_dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out_dir.join("a/b/c/deep.pb").exists());
}

#[test]
fn a_descriptor_after_noise_is_found() {
    let dir = scratch("noise");
    let bin = dir.join("noisy.bin");
    let mut data = b"\x7fELF some random binary noise \x00\x01\x02".to_vec();
    data.extend(fdp("embedded/thing.proto"));
    std::fs::write(&bin, data).unwrap();
    let out = protoscan(&[&bin]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("embedded/thing.proto"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_missing_file_is_an_error() {
    let dir = scratch("missing");
    let out = protoscan(&[&dir.join("does_not_exist.bin")]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("does_not_exist.bin"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn proto_out_writes_every_descriptor() {
    let dir = scratch("every");
    let bin = dir.join("two.bin");
    let mut data = fdp("pkg/alpha.proto");
    data.push(0);
    data.extend(fdp("pkg/beta.proto"));
    std::fs::write(&bin, data).unwrap();
    let out_dir = dir.join("out");
    let out = protoscan(&[&bin, Path::new("--proto-out"), &out_dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out_dir.join("pkg/alpha.pb").exists());
    assert!(out_dir.join("pkg/beta.pb").exists());
}

/// Spec 0407 S3: the former spelling still works, and says it is deprecated.
#[test]
fn the_deprecated_proto_out_still_works_and_warns() {
    let dir = scratch("deprecated");
    let bin = dir.join("one.bin");
    std::fs::write(&bin, fdp("pkg/old.proto")).unwrap();
    let out_dir = dir.join("out");
    let out = protoscan(&[&bin, Path::new("--proto_out"), &out_dir]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out_dir.join("pkg/old.pb").exists());
    assert!(
        stderr(&out).contains("--proto_out is deprecated, use --proto-out"),
        "{}",
        stderr(&out)
    );
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// A zip local header claiming an encrypted deflated member, and junk data.
fn encrypted_zip_member(name: &str) -> Vec<u8> {
    let mut b = b"PK\x03\x04".to_vec();
    b.extend_from_slice(&[20, 0, 1, 0, 8, 0, 0, 0, 0, 0]); // version, flags, method, time, date
    b.extend_from_slice(&[0; 4]); // crc
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&(name.len() as u16).to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(name.as_bytes());
    b.extend_from_slice(&[0xa5; 8]);
    b
}

/// `--max-member`: a member inflating past it is skipped, `member-limit`;
/// under the default, the same member is read.
#[test]
fn max_member_bounds_what_a_member_may_inflate_to() {
    let dir = scratch("max-member");
    let bin = dir.join("big.bin");
    let mut inflated = fdp("pkg/big.proto");
    inflated.extend(vec![0u8; 2 << 20]);
    let mut data = vec![0u8; 16];
    data.extend(gzip(&inflated));
    std::fs::write(&bin, &data).unwrap();

    let read = protoscan(&[&bin, Path::new("--inflate")]);
    assert!(read.status.success(), "{}", stderr(&read));
    assert_eq!(stdout(&read), "pkg/big.proto\n");

    let bounded = protoscan(&[
        &bin,
        Path::new("--inflate"),
        Path::new("--max-member"),
        Path::new("1"),
    ]);
    assert!(bounded.status.success(), "{}", stderr(&bounded));
    assert_eq!(stdout(&bounded), "");
    assert_eq!(stderr(&bounded), "skipped gzip@16: member-limit\n");
}

/// `--max-member` and `--max-total` bound --inflate, and need it.
#[test]
fn the_limits_require_inflate() {
    let dir = scratch("limits-require-inflate");
    let bin = dir.join("empty.bin");
    std::fs::write(&bin, b"").unwrap();
    for option in ["--max-member", "--max-total"] {
        let out = protoscan(&[&bin, Path::new(option), Path::new("1")]);
        assert!(
            !out.status.success(),
            "{option} without --inflate must be refused"
        );
    }
}

/// Spec 0480 S7: `--inflate` finds what a gzip member holds, `--origin`
/// says where, skipped members go to stderr, and a second descriptor of
/// one name is written `~2`.
#[test]
fn inflate_origin_and_variants() {
    let dir = scratch("inflate");
    let bin = dir.join("mixed.bin");
    let clear = fdp("pkg/a.proto");
    let other = FileDescriptorProto {
        name: Some("pkg/a.proto".to_string()),
        package: Some("other".to_string()),
        ..Default::default()
    }
    .encode_to_vec();
    let member = gzip(&other);
    let mut data = vec![0u8; 16];
    data.extend(&clear);
    data.extend([0u8; 16]);
    let gzip_at = data.len();
    data.extend(&member);
    data.extend([0u8; 16]);
    let zip_at = data.len();
    data.extend(encrypted_zip_member("e.pb"));
    data.extend([0u8; 16]);
    std::fs::write(&bin, &data).unwrap();

    let plain = protoscan(&[&bin, Path::new("--origin")]);
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert_eq!(
        stdout(&plain),
        format!("pkg/a.proto\t16..{}\n", 16 + clear.len())
    );

    let out_dir = dir.join("out");
    let out = protoscan(&[
        &bin,
        Path::new("--inflate"),
        Path::new("--origin"),
        Path::new("--proto-out"),
        &out_dir,
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "pkg/a.proto\t16..{}\npkg/a.proto\tgzip@{gzip_at}!0..{}\n",
            16 + clear.len(),
            other.len()
        )
    );
    assert_eq!(
        stderr(&out),
        format!("skipped zip@{zip_at}!e.pb: encrypted\n")
    );
    assert_eq!(std::fs::read(out_dir.join("pkg/a.pb")).unwrap(), clear);
    assert_eq!(std::fs::read(out_dir.join("pkg/a~2.pb")).unwrap(), other);
}
