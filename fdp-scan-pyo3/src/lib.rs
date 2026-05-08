// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// PyO3 essentials
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::Bound;

// Stub-generation helpers
use pyo3_stub_gen::derive::gen_stub_pyfunction;

// ── scan ─────────────────────────────────────────────────────────────────────

/// Scan a binary buffer for FileDescriptorProto candidates.
///
/// Returns a list of (start, end) byte offsets for each candidate found.
#[gen_stub_pyfunction]
#[pyfunction]
fn scan(buffer: Bound<'_, PyBytes>) -> PyResult<Vec<(usize, usize)>> {
    let bytes = buffer.as_bytes();
    let candidates = walk_candidates(bytes);
    Ok(candidates)
}

/// Rust-to-Rust entry point — same logic as `scan()` without the PyO3 wrapper.
pub fn scan_bytes(data: &[u8]) -> Vec<(usize, usize)> {
    walk_candidates(data)
}

// ── Python module ─────────────────────────────────────────────────────────────

/// The Python module definition.
#[pymodule]
fn fdp_scan_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
///
/// Uses `from_project_root` so the module name comes from the lib target name
/// ("fdp_scan_lib") rather than pyproject.toml's [project].name ("fdp_scan").
/// The `use fdp_scan_lib::stub_info` in post_build.rs forces this lib to be
/// linked into the binary, ensuring all inventory items are present.
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let pyproject = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("pyproject.toml");
    pyo3_stub_gen::StubInfo::from_pyproject_toml(pyproject)
}

// ============================================================================

/// Maximum byte length of a `.proto` file name embedded in a FileDescriptorProto.
/// Used to reject implausibly long strings during scanning.
const MAX_PROTO_NAME_LEN: usize = 200;

fn walk_candidates(data: &[u8]) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let data_len = data.len();
    let mut offset = 0;

    while offset < data_len {
        if data[offset] == 0x0A {
            if let Some((name_len, varint_len)) = decode_varint(&data[offset + 1..]) {
                let name_start = offset + 1 + varint_len;
                let Some(name_end) = name_start.checked_add(name_len as usize) else {
                    offset += 1;
                    continue;
                };

                if name_end > data_len
                    || name_start > name_end
                    || name_end - name_start > MAX_PROTO_NAME_LEN
                {
                    offset += 1;
                    continue;
                }

                if let Ok(name_str) = std::str::from_utf8(&data[name_start..name_end]) {
                    if name_str.len() <= MAX_PROTO_NAME_LEN && name_str.ends_with(".proto") {
                        if let Some(fdp_end) = walk_protobuf_fields(data, offset) {
                            result.push((offset, fdp_end));
                            offset = fdp_end;
                            continue;
                        }
                    }
                }
            }
        }
        offset += 1;
    }

    result
}

/// Returns true if `data[pos..]` looks like the start of a new FDP —
/// i.e. 0x0a (field 1, wire type 2) followed by a varint length followed
/// by a string ending in ".proto".  Mirrors the heuristic in walk_candidates.
fn looks_like_fdp_start(data: &[u8], pos: usize) -> bool {
    let data_len = data.len();
    if pos >= data_len || data[pos] != 0x0A {
        return false;
    }
    let Some((name_len, varint_len)) = decode_varint(&data[pos + 1..]) else {
        return false;
    };
    let name_start = pos + 1 + varint_len;
    let Some(name_end) = name_start.checked_add(name_len as usize) else {
        return false;
    };
    if name_end > data_len || name_end - name_start > MAX_PROTO_NAME_LEN {
        return false;
    }
    std::str::from_utf8(&data[name_start..name_end])
        .map(|s| s.ends_with(".proto"))
        .unwrap_or(false)
}

fn walk_protobuf_fields(data: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    let data_len = data.len();
    let mut group_stack = 0;

    while pos < data_len {
        if data[pos] == 0x00 && group_stack == 0 {
            return Some(pos);
        }

        // Field 1 (name) is singular in FileDescriptorProto — a second
        // occurrence at pos > start unambiguously marks the end of this FDP
        // and the beginning of the next one.
        if pos > start && group_stack == 0 && looks_like_fdp_start(data, pos) {
            return Some(pos);
        }

        let (_field_number, wire_type, tag_len) = decode_field_tag(&data[pos..])?;
        pos += tag_len;

        match wire_type {
            0 => pos += decode_varint(&data[pos..])?.1,
            1 => pos += 8,
            2 => {
                let (len_val, varint_len) = decode_varint(&data[pos..])?;
                pos += varint_len + len_val as usize;
            }
            3 => group_stack += 1,
            4 => {
                if group_stack == 0 {
                    return None;
                }
                group_stack -= 1;
            }
            5 => pos += 4,
            _ => return None,
        }

        if pos > data_len {
            return None;
        }
    }

    Some(data_len)
}

fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result = 0u64;
    let mut shift = 0;
    for (i, &b) in data.iter().enumerate() {
        result |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift > 64 {
            return None;
        }
    }
    None
}

fn decode_field_tag(data: &[u8]) -> Option<(u32, u8, usize)> {
    let (raw, varint_len) = decode_varint(data)?;
    Some(((raw >> 3) as u32, (raw & 0x07) as u8, varint_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal FileDescriptorProto encoding with just field 1 (name).
    /// Returns the encoded bytes.
    fn make_fdp(name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        // field 1, wire type 2 (length-delimited)
        buf.push(0x0A);
        // varint length of name
        let name_bytes = name.as_bytes();
        let mut len = name_bytes.len() as u64;
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                buf.push(byte);
                break;
            } else {
                buf.push(byte | 0x80);
            }
        }
        buf.extend_from_slice(name_bytes);
        buf
    }

    #[test]
    fn test_fdp_with_null_separator() {
        // Two FDPs separated by a 0x00 byte — the classic terminator case.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.push(0x00);
        let sep = 1;
        buf.extend_from_slice(&fdp2);

        let ranges = scan_bytes(&buf);

        assert_eq!(ranges.len(), 2, "expected two FDP ranges, got {ranges:?}");
        assert_eq!(ranges[0], (0, fdp1.len()));
        assert_eq!(ranges[1], (fdp1.len() + sep, buf.len()));
    }

    #[test]
    fn test_truncated_fdp_not_returned() {
        // A FDP whose name field is cut off mid-string — should not be returned.
        let fdp = make_fdp("foo.proto");
        let truncated = &fdp[..fdp.len() - 3]; // cut off last 3 bytes of name

        let ranges = scan_bytes(truncated);

        assert!(
            ranges.is_empty(),
            "truncated FDP should not be returned, got {ranges:?}"
        );
    }

    #[test]
    fn test_consecutive_fdps_split_correctly() {
        // Two consecutive FDPs with no 0x00 separator between them.
        let fdp1 = make_fdp("foo.proto");
        let fdp2 = make_fdp("bar.proto");
        let mut buf = fdp1.clone();
        buf.extend_from_slice(&fdp2);

        let ranges = scan_bytes(&buf);

        assert_eq!(
            ranges.len(),
            2,
            "expected two separate FDP ranges, got {ranges:?}"
        );
        assert_eq!(ranges[0], (0, fdp1.len()));
        assert_eq!(ranges[1], (fdp1.len(), buf.len()));
    }
}
