// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `hints`: what an executable tells about where its descriptors come from,
//! beyond the descriptors (seriate-tuto spec 0007), read cheaply: strings
//! found by offset or by a byte search, nothing decoded.
//!
//! Go, from the runtime's function table (`pclntab`), which survives
//! stripping:
//! - the generated `.pb.go` source paths, which name the module and its
//!   version from the module cache (`…/pubsub@v1.33.0/apiv1/pubsubpb/x.pb.go`);
//! - the current generator's `file_<path>_proto_init` functions, with the Go
//!   package that registers each `.proto`;
//! - the generated messages' `ProtoReflect` and `Descriptor` methods, with
//!   their Go package: what binds a descriptor to its package by its
//!   message names when two `.proto` share a name.

use serde_json::{json, Value};

/// `pclntab`'s magic, little-endian, by format (Go's runtime/symtab.go).
const MAGICS: [(u32, &str); 4] = [
    (0xffff_fffb, "1.2"),
    (0xffff_fffa, "1.16"),
    (0xffff_fff0, "1.18"),
    (0xffff_fff1, "1.20"),
];

/// Where a `pclntab` is, and its string tables when its format has them.
#[derive(Debug, PartialEq, Eq)]
pub struct Pclntab<'a> {
    pub format: &'static str,
    /// Function names, then file names (1.16 on); empty for 1.2, whose
    /// names are read from the whole file instead.
    pub tables: Vec<&'a [u8]>,
}

fn word(bytes: &[u8], at: usize, size: usize) -> Option<u64> {
    let b = bytes.get(at..at.checked_add(size)?)?;
    Some(match size {
        4 => u64::from(u32::from_le_bytes(b.try_into().ok()?)),
        _ => u64::from_le_bytes(b.try_into().ok()?),
    })
}

/// The first valid `pclntab` in `bytes`: its magic, then a header whose
/// fields are plausible and whose table offsets grow within the file.
pub fn find_pclntab(bytes: &[u8]) -> Option<Pclntab<'_>> {
    let mut at = 0;
    while let Some(i) = bytes[at..].windows(6).position(|w| {
        w[1..4] == [0xff, 0xff, 0xff]
            && w[4..6] == [0, 0]
            && matches!(w[0], 0xfb | 0xfa | 0xf0 | 0xf1)
    }) {
        let start = at + i;
        at = start + 1;
        let magic = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
        let format = MAGICS.iter().find(|(m, _)| *m == magic)?.1;
        let (Some(&quantum), Some(&ptr)) = (bytes.get(start + 6), bytes.get(start + 7)) else {
            continue;
        };
        if !matches!(quantum, 1 | 2 | 4) || !matches!(ptr, 4 | 8) {
            continue;
        }
        let ptr = usize::from(ptr);
        if format == "1.2" {
            return Some(Pclntab {
                format,
                tables: Vec::new(),
            });
        }
        // nfunc, nfiles, [textStart], funcnameOffset, cuOffset,
        // filetabOffset, pctabOffset, pclnOffset.
        let skip = if format == "1.16" { 2 } else { 3 };
        let field = |k: usize| word(bytes, start + 8 + k * ptr, ptr);
        let (Some(nfunc), Some(names), Some(cu), Some(files), Some(pctab)) = (
            field(0),
            field(skip),
            field(skip + 1),
            field(skip + 2),
            field(skip + 3),
        ) else {
            continue;
        };
        let rest = (bytes.len() - start) as u64;
        if nfunc == 0 || !(names < cu && cu <= files && files < pctab && pctab <= rest) {
            continue;
        }
        let table = |a: u64, b: u64| &bytes[start + a as usize..start + b as usize];
        return Some(Pclntab {
            format,
            tables: vec![table(names, cu), table(files, pctab)],
        });
    }
    None
}

/// A Go symbol's package and the rest: the package ends at the first `.`
/// after its last `/`. `%2e` in a package's last element is a `.`.
fn split_symbol(s: &str) -> Option<(String, &str)> {
    let slash = s.rfind('/').map_or(0, |i| i + 1);
    let dot = slash + s[slash..].find('.')?;
    Some((s[..dot].replace("%2e", "."), &s[dot + 1..]))
}

/// The Go hints of one executable, or `None` if it has no `pclntab`.
pub fn go_hints(bytes: &[u8]) -> Option<Value> {
    let tab = find_pclntab(bytes)?;
    let regions: Vec<&[u8]> = if tab.tables.is_empty() {
        vec![bytes]
    } else {
        tab.tables.clone()
    };
    let mut pb_go = Vec::new();
    let mut inits = Vec::new();
    let mut types = std::collections::BTreeMap::<(String, String), Vec<&str>>::new();
    for region in regions {
        for s in region.split(|&b| b == 0) {
            let Ok(s) = std::str::from_utf8(s) else {
                continue;
            };
            if s.ends_with(".pb.go") {
                pb_go.push(s.to_string());
            } else if let Some(stem) = s.strip_suffix("_proto_init") {
                if let Some((package, f)) = split_symbol(stem) {
                    if let Some(name) = f.strip_prefix("file_") {
                        inits.push(json!([package, name]));
                    }
                }
            } else {
                for method in ["ProtoReflect", "Descriptor"] {
                    let Some(stem) = s.strip_suffix(method).and_then(|s| s.strip_suffix(")."))
                    else {
                        continue;
                    };
                    if let Some((package, t)) = split_symbol(stem) {
                        if let Some(t) = t.strip_prefix("(*") {
                            types
                                .entry((package, t.to_string()))
                                .or_default()
                                .push(method);
                        }
                    }
                }
            }
        }
    }
    pb_go.sort();
    pb_go.dedup();
    inits.sort_by_key(|v| v.to_string());
    inits.dedup();
    let types: Vec<Value> = types
        .into_iter()
        .map(|((p, t), m)| json!([p, t, m]))
        .collect();
    Some(json!({
        "pclntab": tab.format,
        "region": if tab.tables.is_empty() { "whole file" } else { "tables" },
        "pb_go": pb_go,
        "proto_init": inits,
        "types": types,
    }))
}

/// A `.proto` path from the name protobuf's C++ generator gives its
/// descriptor table, `descriptor_table_<escaped path without .proto>_2eproto`:
/// every character but a letter or a digit escaped `_XX`, its hex code
/// (`google_2fapi_2fhttp` is `google/api/http`).
fn unescape_cpp(escaped: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            // Exactly two hex digits, for a character a path can hold.
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() != 2 {
                return None;
            }
            let byte = u8::from_str_radix(&hex, 16).ok()?;
            if !(0x20..0x7f).contains(&byte) {
                return None;
            }
            out.push(char::from(byte));
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            return None;
        }
    }
    Some(out)
}

/// The Bazel repository a generated file's path names:
/// `…/external/<repo>/…`, the main repository's own files having none.
fn bazel_repository(path: &str) -> Option<&str> {
    let rest = &path[path.find("external/")? + "external/".len()..];
    rest.split('/').next().filter(|r| !r.is_empty())
}

/// The C++ hints of one executable or library: what protobuf's C++
/// generator leaves in its symbols (kept by most release builds we have
/// seen) and strings, or `None` if it has none of them.
/// - the `.proto` files whose descriptor tables it holds, the registrant
///   being this binary;
/// - the generated `.pb.cc` files, from their static initializers' names
///   (`_GLOBAL__sub_I_any.pb.cc`) and paths in strings, with the Bazel
///   repository a path names.
pub fn cpp_hints(bytes: &[u8]) -> Option<Value> {
    let mut tables = std::collections::BTreeSet::new();
    let mut pb_cc = std::collections::BTreeMap::new();
    for s in bytes.split(|&b| b == 0) {
        let Ok(s) = std::str::from_utf8(s) else {
            continue;
        };
        if let Some(rest) = s.strip_prefix("descriptor_table_") {
            if let Some(stem) = rest.find("_2eproto").map(|i| &rest[..i]) {
                if let Some(path) = unescape_cpp(stem) {
                    tables.insert(format!("{path}.proto"));
                }
            }
        } else if s.ends_with(".pb.cc") {
            let name = s
                .strip_prefix("_GLOBAL__sub_I.00101_")
                .or_else(|| s.strip_prefix("_GLOBAL__sub_I_"))
                .unwrap_or(s);
            pb_cc.insert(name.to_string(), bazel_repository(name).map(str::to_string));
        }
    }
    if tables.is_empty() && pb_cc.is_empty() {
        return None;
    }
    let pb_cc: Vec<Value> = pb_cc.into_iter().map(|(p, r)| json!([p, r])).collect();
    Some(json!({ "descriptor_tables": tables, "pb_cc": pb_cc }))
}

/// All the hints of one executable's bytes.
pub fn hints(bytes: &[u8]) -> Value {
    json!({ "go": go_hints(bytes), "cpp": cpp_hints(bytes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1.20 `pclntab` at offset 3, 64-bit, its two tables after the header.
    fn go120(names: &[u8], files: &[u8]) -> Vec<u8> {
        let header = 8 + 8 * 8;
        let mut v = vec![0xaa; 3];
        v.extend_from_slice(&[0xf1, 0xff, 0xff, 0xff, 0, 0, 1, 8]);
        let n = names.len() as u64;
        let f = files.len() as u64;
        let h = header as u64;
        for w in [1, 1, 0, h, h + n, h + n, h + n + f, h + n + f] {
            v.extend_from_slice(&u64::to_le_bytes(w));
        }
        v.extend_from_slice(names);
        v.extend_from_slice(files);
        v
    }

    #[test]
    fn a_go120_pclntab_gives_its_two_tables() {
        let bytes = go120(b"main.main\0", b"main.go\0");
        let tab = find_pclntab(&bytes).unwrap();
        assert_eq!(tab.format, "1.20");
        assert_eq!(tab.tables, vec![&b"main.main\0"[..], &b"main.go\0"[..]]);
    }

    #[test]
    fn a_magic_with_an_implausible_header_is_not_a_pclntab() {
        let mut bytes = go120(b"main.main\0", b"main.go\0");
        bytes[3 + 7] = 5; // pointer size
        assert_eq!(find_pclntab(&bytes), None);
    }

    #[test]
    fn the_go_hints_are_read_from_the_tables() {
        let names =
            b"cloud.google.com/go/pubsub/apiv1/pubsubpb.file_google_pubsub_v1_pubsub_proto_init\0\
            cloud.google.com/go/pubsub/apiv1/pubsubpb.(*Topic).ProtoReflect\0\
            cloud.google.com/go/pubsub/apiv1/pubsubpb.(*Topic).Descriptor\0\
            gopkg.in/yaml%2ev3.(*Node).Descriptor\0main.main\0";
        let files = b"/root/go/pkg/mod/cloud.google.com/go/pubsub@v1.33.0/apiv1/pubsubpb/pubsub.pb.go\0main.go\0";
        let hints = go_hints(&go120(names, files)).unwrap();
        assert_eq!(hints["pclntab"], "1.20");
        assert_eq!(
            hints["pb_go"],
            json!([
                "/root/go/pkg/mod/cloud.google.com/go/pubsub@v1.33.0/apiv1/pubsubpb/pubsub.pb.go"
            ])
        );
        assert_eq!(
            hints["proto_init"],
            json!([[
                "cloud.google.com/go/pubsub/apiv1/pubsubpb",
                "google_pubsub_v1_pubsub"
            ]])
        );
        assert_eq!(
            hints["types"],
            json!([
                [
                    "cloud.google.com/go/pubsub/apiv1/pubsubpb",
                    "Topic",
                    ["ProtoReflect", "Descriptor"]
                ],
                ["gopkg.in/yaml.v3", "Node", ["Descriptor"]],
            ])
        );
    }

    #[test]
    fn the_cpp_hints_are_read_from_symbols_and_strings() {
        let bytes = b"\0descriptor_table_google_2fapi_2fhttp_2eproto\0\
            descriptor_table_google_2fapi_2fhttp_2eproto_once\0\
            descriptor_table_my_5fpkg_2fa_2db_2eproto_getterv\0\
            _GLOBAL__sub_I.00101_any.pb.cc\0\
            bazel-out/k8-opt/bin/external/com_google_protobuf/src/google/protobuf/any.pb.cc\0";
        let hints = cpp_hints(bytes).unwrap();
        assert_eq!(
            hints["descriptor_tables"],
            json!(["google/api/http.proto", "my_pkg/a-b.proto"])
        );
        assert_eq!(
            hints["pb_cc"],
            json!([
                ["any.pb.cc", null],
                ["bazel-out/k8-opt/bin/external/com_google_protobuf/src/google/protobuf/any.pb.cc", "com_google_protobuf"],
            ])
        );
    }

    #[test]
    fn a_malformed_escape_is_no_table() {
        assert_eq!(unescape_cpp("google_2"), None);
        assert_eq!(unescape_cpp("a.b"), None);
        assert_eq!(cpp_hints(b"\0descriptor_table_x_zz_2eproto\0"), None);
    }

    #[test]
    fn no_pclntab_no_go_hints() {
        assert_eq!(go_hints(b"\x7fELF not go at all"), None);
    }
}
