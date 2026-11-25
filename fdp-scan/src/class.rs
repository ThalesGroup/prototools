// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Java class files' string constants (spec 0481 S1, S2).
//!
//! Java's generated code keeps a descriptor as `descriptorData`, string
//! constants concatenated at run time. A class file stores each as modified
//! UTF-8, so its raw bytes hold a descriptor only up to its first byte
//! ≥ 0x80. Read here: the constants, decoded back to the bytes
//! protobuf-java hands the descriptor parser.

/// What a buffer is, read as a class file.
pub(crate) enum Read {
    /// Not a class file: no magic, or a version no class file has.
    NotClass,
    /// The magic and version, but a constant pool that doesn't parse.
    Broken,
    /// Its string constants, in pool order, each decoded to bytes; those
    /// that can't be descriptor data left out.
    Class(Vec<Vec<u8>>),
}

const MAGIC: [u8; 4] = [0xca, 0xfe, 0xba, 0xbe];

/// JDK 1.1's major version, the first. A Mach-O universal binary has the
/// same magic, followed by its count of architectures, a small number: the
/// version tells them apart.
const FIRST_MAJOR: u16 = 45;

/// The size protoc cuts `descriptorData` into (spec 0481 S3): 400 bytes up
/// to protoc 3.4 (`i % 400`, a byte offset, though its comment says "every
/// 400 lines"), 16 000 since 3.5.0 (`kBytesPerLine` 40 × `kLinesPerPart`
/// 400), read in `java/shared_code_generator.cc` at each tag from 2.5.0 to
/// `main`.
const PART_SIZES: [usize; 2] = [400, 16_000];

const CONSTANT_UTF8: u8 = 1;
const CONSTANT_STRING: u8 = 8;

pub(crate) fn read(data: &[u8]) -> Read {
    if data.get(..4) != Some(&MAGIC) || be16(data, 6).is_none_or(|v| v < FIRST_MAJOR) {
        return Read::NotClass;
    }
    match strings(data) {
        Some(parts) => Read::Class(parts),
        None => Read::Broken,
    }
}

/// The constant pool's strings, in pool order (spec 0481 S2): javac pools
/// `descriptorData`'s elements where `<clinit>` first loads them, one
/// after the other.
fn strings(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let count = usize::from(be16(data, 8)?);
    let mut utf8: Vec<Option<&[u8]>> = vec![None; count];
    let mut refs = Vec::new();
    let (mut pos, mut index) = (10, 1);
    while index < count {
        let tag = *data.get(pos)?;
        pos += 1;
        pos += match tag {
            CONSTANT_UTF8 => {
                let len = usize::from(be16(data, pos)?);
                utf8[index] = Some(data.get(pos + 2..pos + 2 + len)?);
                2 + len
            }
            CONSTANT_STRING => {
                refs.push(usize::from(be16(data, pos)?));
                2
            }
            // Class, MethodType, Module, Package: one index.
            7 | 16 | 19 | 20 => 2,
            // MethodHandle: a kind and an index.
            15 => 3,
            // Integer, Float, the references, NameAndType, Dynamic,
            // InvokeDynamic.
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => 4,
            // Long and Double take two slots.
            5 | 6 => {
                index += 1;
                8
            }
            _ => return None,
        };
        index += 1;
    }
    if pos > data.len() {
        return None;
    }
    let mut parts = Vec::with_capacity(refs.len());
    for r in refs {
        if let Some(bytes) = latin1(utf8.get(r).copied().flatten()?) {
            parts.push(bytes);
        }
    }
    Some(parts)
}

/// Whether string parts of these lengths are a descriptor as protoc cuts it
/// (spec 0481 S3): every part but the last exactly one of [`PART_SIZES`],
/// the last shorter. All parts full, for either size, the next may be
/// missing: no. That includes one 400-byte part, which protoc 3.5 and later
/// write whole, but which could be an older protoc's first.
pub(crate) fn protoc_shape(lengths: &[usize]) -> bool {
    let Some((last, inner)) = lengths.split_last() else {
        return false;
    };
    let full = |size: usize| lengths.iter().all(|&l| l == size);
    if PART_SIZES.into_iter().any(full) {
        return false;
    }
    PART_SIZES
        .iter()
        .any(|&size| inner.iter().all(|&l| l == size) && *last < size)
}

/// Modified UTF-8 decoded, each char one byte, as protobuf-java turns
/// `descriptorData` into bytes (`ISO_8859_1`); `None` for a string holding
/// a char above U+00FF, which can't be descriptor data, or not well formed.
fn latin1(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        if b < 0x80 {
            out.push(b);
            i += 1;
        } else if b & 0xe0 == 0xc0 {
            let next = *raw.get(i + 1)?;
            if next & 0xc0 != 0x80 {
                return None;
            }
            let c = (u16::from(b & 0x1f) << 6) | u16::from(next & 0x3f);
            out.push(u8::try_from(c).ok()?);
            i += 2;
        } else {
            // Three bytes and more: a char above U+07FF.
            return None;
        }
    }
    Some(out)
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_utf8() {
        assert_eq!(latin1(b"ab").unwrap(), b"ab");
        // U+0000 as C0 80, U+00E9 as C3 A9.
        assert_eq!(latin1(&[0xc0, 0x80, 0xc3, 0xa9]).unwrap(), [0x00, 0xe9]);
        // U+0100 and above can't be descriptor data.
        assert!(latin1(&[0xc4, 0x80]).is_none());
        assert!(latin1(&[0xe2, 0x82, 0xac]).is_none());
        assert!(latin1(&[0xc3]).is_none());
    }

    #[test]
    fn protoc_shapes() {
        assert!(protoc_shape(&[153]));
        assert!(protoc_shape(&[400, 400, 120]));
        assert!(protoc_shape(&[16_000, 1_000]));
        assert!(protoc_shape(&[800]), "one part of protoc 3.5 or later");
        // All parts full: the next may be missing.
        assert!(!protoc_shape(&[400]));
        assert!(!protoc_shape(&[400, 400]));
        assert!(!protoc_shape(&[16_000]));
        // Not protoc's cut.
        assert!(!protoc_shape(&[300, 300, 100]));
        assert!(!protoc_shape(&[400, 16_000, 5]));
        assert!(!protoc_shape(&[]));
    }

    #[test]
    fn not_a_class_file() {
        assert!(matches!(
            read(b"\xca\xfe\xba\xbe\x00\x00\x00\x02"),
            Read::NotClass
        ));
        assert!(matches!(read(b"\x7fELF"), Read::NotClass));
        assert!(matches!(
            read(b"\xca\xfe\xba\xbe\x00\x00\x00\x34\x00\x05\x01"),
            Read::Broken
        ));
    }
}
