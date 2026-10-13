// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Just enough wire reading to split a `FileDescriptorSet` and order its
//! files (spec 0415 S3), in safe Rust, before any byte reaches upb.
//!
//! Only two facts are read from each file: its name (field 1) and its
//! imports (field 3). Everything else is upb's to parse.

/// Read a varint at `*pos`, advancing it. `None` if the buffer ends inside
/// the varint or it runs past ten bytes.
fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The fields of one message: `(field number, payload)` for each
/// length-delimited field, the others skipped. Groups are skipped by
/// counting their nesting, iteratively, so no input can exhaust the stack.
fn len_fields(buf: &[u8]) -> Result<Vec<(u64, &[u8])>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut group_depth = 0usize;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos).ok_or("malformed tag")?;
        let (number, wire_type) = (tag >> 3, tag & 7);
        if number == 0 {
            return Err("field number 0".into());
        }
        match wire_type {
            0 => {
                read_varint(buf, &mut pos).ok_or("malformed varint")?;
            }
            1 | 5 => {
                let width = if wire_type == 1 { 8 } else { 4 };
                if buf.len() - pos < width {
                    return Err("truncated fixed-width field".into());
                }
                pos += width;
            }
            2 => {
                let len = read_varint(buf, &mut pos).ok_or("malformed length")?;
                if len > (buf.len() - pos) as u64 {
                    return Err("length runs past the end".into());
                }
                let end = pos + len as usize;
                if group_depth == 0 {
                    out.push((number, &buf[pos..end]));
                }
                pos = end;
            }
            3 => group_depth += 1,
            4 => {
                group_depth = group_depth
                    .checked_sub(1)
                    .ok_or("end of a group that was never opened")?;
            }
            _ => return Err(format!("invalid wire type {wire_type}")),
        }
    }
    if group_depth != 0 {
        return Err("unterminated group".into());
    }
    Ok(out)
}

/// The serialized files of a `FileDescriptorSet` (its field 1), in order.
pub(crate) fn split_set(buf: &[u8]) -> Result<Vec<&[u8]>, String> {
    Ok(len_fields(buf)?
        .into_iter()
        .filter(|(n, _)| *n == 1)
        .map(|(_, payload)| payload)
        .collect())
}

/// A file's name and imports, read from its serialized
/// `FileDescriptorProto`. The name must be present and UTF-8: upb checks
/// every identifier it builds, but not the file name.
pub(crate) fn name_and_imports(fdp: &[u8]) -> Result<(String, Vec<String>), String> {
    let mut name = None;
    let mut imports = Vec::new();
    for (number, payload) in len_fields(fdp)? {
        let text = || {
            std::str::from_utf8(payload)
                .map(str::to_owned)
                .map_err(|_| "a file name that is not UTF-8".to_string())
        };
        match number {
            1 => name = Some(text()?),
            3 => imports.push(text()?),
            _ => {}
        }
    }
    let name = name.ok_or("no file name")?;
    if name.is_empty() {
        return Err("an empty file name".into());
    }
    Ok((name, imports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len_field(number: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![number << 3 | 2, payload.len() as u8];
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn a_set_splits_into_its_files() {
        let mut set = len_field(1, b"\x0a\x01a");
        set.extend(len_field(1, b"\x0a\x01b"));
        assert_eq!(
            split_set(&set).unwrap(),
            vec![&b"\x0a\x01a"[..], b"\x0a\x01b"]
        );
    }

    #[test]
    fn a_file_gives_its_name_and_imports() {
        let mut fdp = len_field(1, b"a.proto");
        fdp.extend(len_field(3, b"b.proto"));
        fdp.extend(len_field(2, b"pkg"));
        fdp.extend(len_field(3, b"c.proto"));
        let (name, imports) = name_and_imports(&fdp).unwrap();
        assert_eq!(name, "a.proto");
        assert_eq!(imports, ["b.proto", "c.proto"]);
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        for bad in [
            &b"\x0a\x05abc"[..],                             // length past the end
            b"\x0b",                                         // unterminated group
            b"\x0c",                                         // group end never opened
            b"\x0e",                                         // wire type 6
            b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff", // varint too long
            b"\x00\x00",                                     // field 0
            b"\x09\x01\x02",                                 // truncated fixed64
        ] {
            assert!(split_set(bad).is_err(), "{bad:?}");
        }
        assert!(name_and_imports(&len_field(1, b"\xff\xfe")).is_err());
        assert!(name_and_imports(b"").is_err());
    }

    #[test]
    fn deep_groups_do_not_recurse() {
        let mut deep = vec![0x0b; 200_000];
        deep.extend(vec![0x0c; 200_000]);
        assert!(split_set(&deep).unwrap().is_empty());
    }
}
