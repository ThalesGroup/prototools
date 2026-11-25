// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Descriptors inside zip and gzip members (spec 0480): the test plan's
//! scanner tests, on members built here.

use std::io::Write;
use std::path::Path;

use fdp_scan::{
    scan_explained, scan_inflating, InflateLimits, MemberKind, Origin, ScanOptions, SkipReason,
    Visit,
};
use flate2::write::{DeflateEncoder, GzEncoder};
use flate2::{Compression, Crc};

/// The smallest descriptor the scanner reports: a name and a package.
fn fdp(name: &str) -> Vec<u8> {
    let mut b = vec![0x0a, name.len() as u8];
    b.extend_from_slice(name.as_bytes());
    b.extend_from_slice(&[0x12, 1, b'p']);
    b
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn crc(bytes: &[u8]) -> u32 {
    let mut c = Crc::new();
    c.update(bytes);
    c.sum()
}

/// How a zip member is written.
#[derive(Clone, Copy, PartialEq)]
enum Zip {
    Stored,
    Deflated,
    /// Deflated, crc and sizes in a trailing data descriptor (bit 3).
    Trailing,
}

/// A zip local file header and its data: a member, without the archive's
/// central directory, as a carved archive shows it.
fn zip_member(name: &str, payload: &[u8], how: Zip) -> Vec<u8> {
    let data = if how == Zip::Stored {
        payload.to_vec()
    } else {
        deflate(payload)
    };
    let (flags, method): (u16, u16) = match how {
        Zip::Stored => (0, 0),
        Zip::Deflated => (0, 8),
        Zip::Trailing => (1 << 3, 8),
    };
    let sizes = |b: &mut Vec<u8>| {
        b.extend_from_slice(&crc(payload).to_le_bytes());
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    };
    let mut b = b"PK\x03\x04".to_vec();
    b.extend_from_slice(&20u16.to_le_bytes());
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&method.to_le_bytes());
    b.extend_from_slice(&[0; 4]); // time, date
    if how == Zip::Trailing {
        b.extend_from_slice(&[0; 12]);
    } else {
        sizes(&mut b);
    }
    b.extend_from_slice(&(name.len() as u16).to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(name.as_bytes());
    b.extend_from_slice(&data);
    if how == Zip::Trailing {
        b.extend_from_slice(b"PK\x07\x08");
        sizes(&mut b);
    }
    b
}

/// `bytes` between runs of zeros, as a linker might place them.
fn embedded(parts: &[&[u8]]) -> Vec<u8> {
    let mut b = vec![0; 64];
    for part in parts {
        b.extend_from_slice(part);
        b.extend_from_slice(&[0; 64]);
    }
    b
}

/// What the visitor saw, as text: `found <origin>` per descriptor,
/// `skipped <chain> <reason>` per member skipped, in order.
fn walk(data: &[u8], limits: &InflateLimits) -> Vec<String> {
    let mut seen = Vec::new();
    scan_inflating(data, &ScanOptions::default(), limits, |visit| match visit {
        Visit::Scanned {
            origin, explained, ..
        } => {
            for &(start, end) in &explained.found {
                seen.push(format!("found {}", origin.text(None, start, end)));
            }
        }
        Visit::Skipped { origin, reason } => {
            seen.push(format!(
                "skipped {} {}",
                reason.place(origin, None),
                reason.code()
            ));
        }
    });
    seen
}

#[test]
fn inflate_off_is_unchanged() {
    // Every corpus layout: the input's own visit is today's scan.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "bin") {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let mut first = None;
        scan_inflating(
            &data,
            &ScanOptions::default(),
            &InflateLimits::default(),
            |v| {
                if let (
                    None,
                    Visit::Scanned {
                        explained, origin, ..
                    },
                ) = (&first, v)
                {
                    assert_eq!(origin, &Origin::default());
                    first = Some(explained.clone());
                }
            },
        );
        assert_eq!(first.unwrap(), scan_explained(&data), "{}", path.display());
    }
}

#[test]
fn gzip_member_found() {
    let a = fdp("a.proto");
    let member = gzip(&a);
    let data = embedded(&[&member]);
    assert_eq!(
        walk(&data, &InflateLimits::default()),
        [format!("found gzip@64!0..{}", a.len())]
    );
}

#[test]
fn zip_members_stored_and_deflated() {
    let (a, b) = (fdp("a.proto"), fdp("b.proto"));
    let stored = zip_member("s.pb", &a, Zip::Stored);
    let deflated = zip_member("d.pb", &b, Zip::Deflated);
    let data = embedded(&[&stored, &deflated]);
    let second = 64 + stored.len() + 64;
    // The stored member's descriptor is reported in the member only, not
    // also in the clear in the input.
    assert_eq!(
        walk(&data, &InflateLimits::default()),
        [
            format!("found zip@64!s.pb!0..{}", a.len()),
            format!("found zip@{second}!d.pb!0..{}", b.len()),
        ]
    );
}

#[test]
fn bit3_data_descriptor() {
    let a = fdp("a.proto");
    let data = embedded(&[&zip_member("t.pb", &a, Zip::Trailing)]);
    assert_eq!(
        walk(&data, &InflateLimits::default()),
        [format!("found zip@64!t.pb!0..{}", a.len())]
    );
}

#[test]
fn false_signatures_dropped() {
    let a = fdp("a.proto");
    // A gzip magic and a zip signature followed by junk.
    let mut junk = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\xff\xff\xff".to_vec();
    junk.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x08\x00junkjunkjunkjunk");
    // A gzip member whose trailer crc is wrong.
    let mut bad = gzip(&a);
    let n = bad.len();
    bad[n - 8] ^= 0xff;
    // A stored member whose crc is wrong: its descriptor stays in the clear.
    let mut stored = zip_member("s.pb", &a, Zip::Stored);
    stored[14] ^= 0xff;
    let data = embedded(&[&junk, &bad, &stored]);
    let at = 64 + junk.len() + 64 + bad.len() + 64 + 30 + 4;
    assert_eq!(
        walk(&data, &InflateLimits::default()),
        [format!("found {at}..{}", at + a.len())]
    );
}

#[test]
fn limits() {
    let a = fdp("a.proto");
    // Nested 4 deep: zip in zip in zip in zip, the innermost one skipped.
    let mut nested = a.clone();
    for name in ["4.pb", "3.zip", "2.zip", "1.zip"] {
        nested = zip_member(name, &nested, Zip::Deflated);
    }
    let seen = walk(&embedded(&[&nested]), &InflateLimits::default());
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].starts_with("skipped zip@64!1.zip!zip@0!2.zip!zip@0!3.zip!zip@0!4.pb depth-limit"),
        "{seen:?}"
    );

    // A member larger than the per-member limit, and the input limit.
    let big = vec![0u8; 1 << 20];
    let data = embedded(&[&gzip(&big), &gzip(&a)]);
    let member = InflateLimits {
        max_member: 1 << 19,
        ..Default::default()
    };
    assert_eq!(
        walk(&data, &member)[0],
        "skipped gzip@64 member-limit".to_string()
    );
    assert_eq!(
        walk(&data, &member).len(),
        2,
        "the next member is still read"
    );
    let total = InflateLimits {
        max_total: 1 << 19,
        ..Default::default()
    };
    assert_eq!(
        walk(&data, &total),
        ["skipped gzip@64 input-limit".to_string()],
        "nothing is inflated after the input's budget is spent"
    );
}

#[test]
fn visit_order() {
    let (a, b, c) = (fdp("a.proto"), fdp("b.proto"), fdp("c.proto"));
    let inner = zip_member("b.pb", &b, Zip::Deflated);
    let outer = zip_member("in.zip", &embedded(&[&inner]), Zip::Deflated);
    let data = embedded(&[&a, &outer, &gzip(&c)]);
    let mut origins = Vec::new();
    scan_inflating(
        &data,
        &ScanOptions::default(),
        &InflateLimits::default(),
        |v| {
            if let Visit::Scanned { origin, .. } = v {
                origins.push(
                    origin
                        .layers
                        .iter()
                        .map(|l| match &l.kind {
                            MemberKind::Zip { name } => name.clone(),
                            MemberKind::Gzip => "gzip".into(),
                            MemberKind::Class => "class".into(),
                        })
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        },
    );
    // The input, then each member depth-first, its own members right after.
    assert_eq!(origins, ["", "in.zip", "in.zip/b.pb", "gzip"]);
}

#[test]
fn skip_reasons() {
    assert_eq!(SkipReason::Method(12).code(), "method 12");
    let a = fdp("a.proto");
    let mut encrypted = zip_member("e.pb", &a, Zip::Deflated);
    encrypted[6] |= 1;
    let mut bzip2 = zip_member("b.pb", &a, Zip::Deflated);
    bzip2[8] = 12;
    let seen = walk(&embedded(&[&encrypted, &bzip2]), &InflateLimits::default());
    let second = 64 + encrypted.len() + 64;
    assert_eq!(
        seen,
        [
            "skipped zip@64!e.pb encrypted".to_string(),
            format!("skipped zip@{second}!b.pb method 12"),
        ]
    );
}

// ── Java class files (spec 0481) ─────────────────────────────────────────────

/// A descriptor with bytes ≥ 0x80: a name, a package, and a dependency
/// of `len` bytes, long enough that its length takes two bytes.
fn java_fdp(len: usize) -> Vec<u8> {
    // A message `M` before the dependency: a record cut inside the
    // dependency still declares something (spec 0483 S1).
    let mut b = fdp("x/y.proto");
    b.extend_from_slice(&[0x22, 3, 0x0a, 1, b'M']);
    let dep = format!("{}.proto", "d".repeat(len - 6));
    b.push(0x1a);
    b.extend_from_slice(&[0x80 | (dep.len() as u8 & 0x7f), (dep.len() >> 7) as u8]);
    b.extend_from_slice(dep.as_bytes());
    b
}

/// `d` cut into parts of `size` bytes, as protoc cuts `descriptorData`.
fn parts(d: &[u8], size: usize) -> Vec<&[u8]> {
    d.chunks(size).collect()
}

/// A descriptor of `fields` × 40 bytes, every field 40 bytes long, so that
/// a 400- or 16 000-byte part ends where a field does: dropping its last
/// part leaves a descriptor that is still well formed.
fn aligned_fdp(fields: usize) -> Vec<u8> {
    let mut b = fdp(&format!("x/{}.proto", "a".repeat(27)));
    assert_eq!(b.len(), 40);
    for n in 1..fields {
        b.push(0x1a);
        b.push(38);
        b.extend_from_slice(format!("d/{n:030}.proto").as_bytes());
    }
    assert_eq!(b.len(), 40 * fields);
    b
}

/// Bytes as Java's modified UTF-8 stores a string of chars 0 to 255.
fn mutf8(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in bytes {
        if b != 0 && b < 0x80 {
            out.push(b);
        } else {
            out.extend_from_slice(&[0xc0 | (b >> 6), 0x80 | (b & 0x3f)]);
        }
    }
    out
}

/// A class file whose constant pool holds `strings` as String constants,
/// each with its Utf8, after a class name, as javac would pool them.
fn class_file(strings: &[&[u8]]) -> Vec<u8> {
    let mut pool: Vec<Vec<u8>> = vec![];
    let utf8 = |s: &[u8]| {
        let mut e = vec![1];
        e.extend_from_slice(&(s.len() as u16).to_be_bytes());
        e.extend_from_slice(s);
        e
    };
    pool.push(utf8(b"x/YOuterClass"));
    pool.push(vec![7, 0, 1]);
    for s in strings {
        let index = pool.len() as u16 + 2; // the Utf8 after this String
        pool.push([vec![8], index.to_be_bytes().to_vec()].concat());
        pool.push(utf8(&mutf8(s)));
    }
    let mut b = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52];
    b.extend_from_slice(&(pool.len() as u16 + 1).to_be_bytes());
    for entry in pool {
        b.extend(entry);
    }
    b.extend_from_slice(&[0; 10]); // access, this, super, counts
    b
}

#[test]
fn class_file_strings_decoded() {
    // Three parts of protoc before 3.5: 400, 400, and the rest.
    let d = java_fdp(900);
    let class = class_file(&parts(&d, 400));
    // Read raw, the descriptor would be cut at its first byte ≥ 0x80.
    assert!(scan_explained(&class).found.len() <= 1);
    assert_eq!(
        walk(&class, &InflateLimits::default()),
        [format!("found class!0..{}", d.len())]
    );
}

#[test]
fn class_file_in_a_jar() {
    let d = java_fdp(130);
    let class = class_file(&[&d]);
    let data = embedded(&[&zip_member("x/Y.class", &class, Zip::Deflated)]);
    assert_eq!(
        walk(&data, &InflateLimits::default()),
        [format!("found zip@64!x/Y.class!class!0..{}", d.len())]
    );
}

#[test]
fn not_a_class_file() {
    // A Mach-O universal header: the same magic, scanned raw.
    let a = fdp("a.proto");
    let mut macho = b"\xca\xfe\xba\xbe\x00\x00\x00\x02".to_vec();
    macho.extend(&embedded(&[&a]));
    assert_eq!(
        walk(&macho, &InflateLimits::default()),
        [format!("found {}..{}", 8 + 64, 8 + 64 + a.len())]
    );
    // A class file cut short, and a `.class` member that isn't one.
    let d = java_fdp(130);
    let class = class_file(&[&d]);
    let short = zip_member("s.class", &class[..40], Zip::Stored);
    let fake = zip_member("f.class", &embedded(&[&a]), Zip::Stored);
    let second = 64 + short.len() + 64;
    assert_eq!(
        walk(&embedded(&[&short, &fake]), &InflateLimits::default()),
        [
            "skipped zip@64!s.class class-file".to_string(),
            format!("skipped zip@{second}!f.class class-file"),
        ]
    );
}

#[test]
fn cut_descriptor_skipped() {
    // The middle part replaced by another string: the record found ends
    // inside the first part, not on a string boundary.
    let d = java_fdp(900);
    let class = class_file(&[&d[..400], b"Ljava/lang/Object;", &d[800..]]);
    let seen = walk(&class, &InflateLimits::default());
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0].starts_with("skipped class!0..") && seen[0].ends_with(" cut x/y.proto"),
        "{seen:?}"
    );
}

#[test]
fn part_shapes() {
    let limits = InflateLimits::default();
    for (size, fields) in [(400, 25), (16_000, 425)] {
        // 1 000 bytes in 400-byte parts (400, 400, 200), 17 000 in
        // 16 000-byte parts (16 000, 1 000): found.
        let d = aligned_fdp(fields);
        let whole = parts(&d, size);
        assert_eq!(
            walk(&class_file(&whole), &limits),
            [format!("found class!0..{}", d.len())]
        );
        // The last part dropped: every part full, the next maybe missing.
        let short = &whole[..whole.len() - 1];
        let len: usize = short.iter().map(|p| p.len()).sum();
        assert_eq!(
            walk(&class_file(short), &limits),
            [format!(
                "skipped class!0..{len} cut x/{}.proto",
                "a".repeat(27)
            )]
        );
    }
    // Parts protoc never writes.
    let d = aligned_fdp(25);
    assert_eq!(
        walk(&class_file(&parts(&d, 300)), &limits),
        [format!(
            "skipped class!0..1000 cut x/{}.proto",
            "a".repeat(27)
        )]
    );
}
