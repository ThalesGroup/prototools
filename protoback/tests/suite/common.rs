// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! What the suite's `conftest.py` provided (spec 0488 S2), and what its
//! tests repeated: a fresh run directory, protoback run without the
//! ambient settings, protoc where a test compiles its input.

pub use crate::suite_fixtures::{encoded, fixture, protoc, tmp, tools};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `text` written at `path`, its directories made.
pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// How a run ended, and what it printed on stderr.
pub struct Run {
    pub code: Option<i32>,
    pub stderr: String,
}

/// protoback's command with `args`, as the suite ran it: without the
/// toolset's default descriptor set (`conftest.py`'s
/// `no_ambient_descriptor_set`, for every test), and without
/// `REPROTO_VARIANT`, which each of the suite's tests removed for itself;
/// its binary sets as text under `via_text` (spec 0427), as the suite's
/// other command ran them.
pub fn protoback_command<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args: Vec<std::ffi::OsString> =
        args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    if crate::via_text::on() {
        args = crate::via_text::arguments(args);
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_protoback"));
    c.args(args)
        .env_remove("PROTOTEXT_DESCRIPTOR_SET")
        .env_remove("PROTOTEXT_DEFAULT_DESCRIPTOR")
        .env_remove("REPROTO_VARIANT");
    c
}

/// protoback run with `args` (`protoback_command`): how it ended.
pub fn protoback<I, S>(args: I) -> Run
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let o = protoback_command(args).output().unwrap();
    Run {
        code: o.status.code(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// A `#@prototext` fixture, encoded (`encoded`), written to `path`.
pub fn encode_fixture(name: &str, sha256: &str, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encoded(name, sha256)).unwrap();
}

/// The `index`th message of a descriptor, encoded deterministically (fields
/// in number order): two messages are equal exactly when these bytes are,
/// which is what Python's `==` on them checks. `bytes` is a
/// `FileDescriptorSet` whose first file is meant when `set`, else a
/// `FileDescriptorProto`.
pub fn message_type(bytes: &[u8], set: bool, index: usize) -> Vec<u8> {
    use prototext_upb::descriptor_access::{FileDescriptorProto, FileDescriptorSet};
    use prototext_upb::{DynamicMessage, Pool};
    let pool = Pool::vendored();
    let name = if set {
        "google.protobuf.FileDescriptorSet"
    } else {
        "google.protobuf.FileDescriptorProto"
    };
    let msg = DynamicMessage::decode(&pool.get_message_by_name(name).unwrap(), bytes).unwrap();
    let file = if set {
        FileDescriptorSet(msg.as_ref())
            .file()
            .into_iter()
            .next()
            .unwrap()
    } else {
        FileDescriptorProto(msg.as_ref())
    };
    file.message_type()
        .into_iter()
        .nth(index)
        .unwrap()
        .0
        .encode_deterministic()
        .unwrap()
}

/// protoc's own include directory, holding its descriptor.proto: beside the
/// protoc on `PATH` (`conftest.py`'s `STOCK_INCLUDE`).
pub fn stock_include() -> PathBuf {
    let out = Command::new("sh")
        .args(["-c", "command -v protoc"])
        .output()
        .unwrap();
    let protoc = std::fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    protoc.parent().unwrap().parent().unwrap().join("include")
}

/// protoc's descriptor.proto as a vendor's (spec 0447): package `vendor`,
/// at vendor/proto/descriptor.proto, compiled alone into in/vdesc.pb;
/// `extra`, fields added to its FieldOptions; `top`, definitions added at
/// its end (`conftest.py`'s `vendor_descriptor`).
pub fn vendor_descriptor(dir: &Path, extra: &str, top: &str) {
    let stock =
        std::fs::read_to_string(stock_include().join("google/protobuf/descriptor.proto")).unwrap();
    let mut text = stock
        .replace("package google.protobuf;", "package vendor;")
        .replacen(
            "message FieldOptions {\n",
            &format!("message FieldOptions {{\n{extra}"),
            1,
        );
    text.push_str(top);
    write(&dir.join("src/vendor/proto/descriptor.proto"), &text);
    std::fs::create_dir_all(dir.join("in")).unwrap();
    protoc([
        format!("-I{}", dir.join("src").display()),
        format!("--descriptor_set_out={}", dir.join("in/vdesc.pb").display()),
        "vendor/proto/descriptor.proto".into(),
    ]);
}

/// `text` with every run of whitespace one space (the suite's `_words`).
pub fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `set`'s `files` build into a descriptor pool: protoc's
/// (`--descriptor_set_in`), not the upb pool protoback itself loads with,
/// where the suite used Python's `DescriptorPool`.
pub fn protoc_loads(set: &Path, files: &[&str]) {
    let o = Command::new("protoc")
        .arg(format!("--descriptor_set_in={}", set.display()))
        .args(["-o", "/dev/null"])
        .args(files)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "protoc: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A `FileDescriptorSet`'s files, decoded: each file's name, package, and
/// for each message its fields' (name, type_name), to read what a set says.
pub fn set_files(path: &Path) -> Vec<SetFile> {
    use prototext_upb::descriptor_access::FileDescriptorSet;
    use prototext_upb::{DynamicMessage, Pool};
    let bytes = std::fs::read(path).unwrap();
    let def = Pool::vendored()
        .get_message_by_name("google.protobuf.FileDescriptorSet")
        .unwrap();
    let msg = DynamicMessage::decode(&def, &bytes).unwrap();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    FileDescriptorSet(msg.as_ref())
        .file()
        .into_iter()
        .map(|f| SetFile {
            name: text(f.name()),
            package: text(f.package()),
            dependencies: f.dependency().into_iter().map(text).collect(),
            syntax: text(f.syntax()),
            edition: f.edition(),
            messages: f
                .message_type()
                .into_iter()
                .map(|m| {
                    (
                        text(m.name()),
                        m.field()
                            .into_iter()
                            .map(|x| (text(x.name()), text(x.type_name())))
                            .collect(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// One file of a set, as `set_files` reads it.
pub struct SetFile {
    pub name: String,
    pub package: String,
    pub dependencies: Vec<String>,
    /// As written: empty for proto2's implicit syntax.
    pub syntax: String,
    pub edition: i32,
    /// Each message's name and its fields' (name, type_name).
    pub messages: Vec<(String, Vec<(String, String)>)>,
}

/// `text` without its comments (`proto_normalize.py`'s `uncomment`): `//`
/// and `/* */` comments removed, string literals left whole, then each line
/// right-trimmed and the blank ones dropped. The original found comments
/// with tree-sitter's proto grammar, whose comment nodes are these lexemes
/// in a valid `.proto`; a lexer finds the same without a grammar to build.
pub fn uncomment(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                out.extend_from_slice(&b[start..i]);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n") + "\n"
}

/// `buf format` on several texts at once, in `dir` (`proto_normalize.py`'s
/// `buf_format_batch`): each written to `<label>.proto` beside a minimal
/// buf.yaml, formatted in place, read back.
pub fn buf_format_batch(dir: &Path, texts: &[(&str, String)]) -> Vec<String> {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("buf.yaml"), "version: v2\n").unwrap();
    for (label, text) in texts {
        std::fs::write(dir.join(format!("{label}.proto")), text).unwrap();
    }
    let o = Command::new("buf")
        .args(["format", "--write"])
        .arg(dir)
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "buf format: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    texts
        .iter()
        .map(|(label, _)| std::fs::read_to_string(dir.join(format!("{label}.proto"))).unwrap())
        .collect()
}

/// Uncomment, then buf-format, several texts (`normalize_proto_batch`).
pub fn normalize_proto_batch(dir: &Path, texts: &[(&str, &str)]) -> Vec<String> {
    let uncommented: Vec<(&str, String)> = texts.iter().map(|(l, t)| (*l, uncomment(t))).collect();
    buf_format_batch(dir, &uncommented)
}

/// A message of the descriptor schema, `type_name`, rendered as `prototext
/// decode --type <type_name>` prints it (annotated): `proto_normalize.py`'s
/// `_decode_pb` and `test_emit_binary.py`'s `_decode_fdp`.
pub fn render(bytes: &[u8], type_name: &str) -> String {
    let schema = prototext_core::parse_schema(prototext_upb::DESCRIPTOR_SET, type_name).unwrap();
    let root = schema.root_descriptor();
    let opts = prototext_core::RenderOpts {
        assume_binary: true,
        include_annotations: true,
        ..Default::default()
    };
    String::from_utf8(prototext_core::render_as_text(bytes, root.as_ref(), opts).unwrap()).unwrap()
}

/// A `FileDescriptorSet`'s rendering, line by line: to compare two of
/// protoc's outputs, never protoback's alone.
pub fn render_set(bytes: &[u8]) -> Vec<String> {
    render(bytes, "google.protobuf.FileDescriptorSet")
        .lines()
        .map(String::from)
        .collect()
}

/// The varint at `b[*i..]`, `*i` moved past it.
fn varint(b: &[u8], i: &mut usize) -> u64 {
    let (mut v, mut shift) = (0u64, 0);
    loop {
        let byte = b[*i];
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return v;
        }
        shift += 7;
    }
}

/// The length-delimited fields numbered `number` of a message's bytes, in
/// order: a wire walk, knowing nothing of the schema.
pub fn delimited(b: &[u8], number: u64) -> Vec<&[u8]> {
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        let key = varint(b, &mut i);
        match key & 7 {
            0 => {
                varint(b, &mut i);
            }
            1 => i += 8,
            5 => i += 4,
            2 => {
                let n = varint(b, &mut i) as usize;
                if key >> 3 == number {
                    out.push(&b[i..i + n]);
                }
                i += n;
            }
            w => panic!("wire type {w} in a descriptor"),
        }
    }
    out
}

/// The bytes of the file named `name` in a `FileDescriptorSet`
/// (`test_emit_binary.py`'s `_extract_fdp`): the set's own bytes for it,
/// where the original reserialized them.
pub fn extract_fdp(set: &[u8], name: &str) -> Vec<u8> {
    delimited(set, 1)
        .into_iter()
        .find(|f| {
            delimited(f, 1)
                .first()
                .is_some_and(|n| *n == name.as_bytes())
        })
        .unwrap_or_else(|| panic!("{name:?} not in the set"))
        .to_vec()
}

/// The lines only in `a` (`-`) or only in `b` (`+`), by a longest common
/// subsequence. The original used Python's `difflib`, whose matcher has
/// heuristics of its own; on descriptor renderings both find the same
/// changed lines.
fn line_diff(a: &[String], b: &[String]) -> Vec<(char, String)> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(('+', b[j].clone()));
            j += 1;
        } else {
            out.push(('-', a[i].clone()));
            i += 1;
        }
    }
    out
}

/// Two sets' differing lines, rendered (`pb_diff`): for a failure message.
pub fn pb_diff(a: &[u8], b: &[u8]) -> String {
    if a == b {
        return String::new();
    }
    line_diff(&render_set(a), &render_set(b))
        .into_iter()
        .map(|(s, l)| format!("{s}{l}\n"))
        .collect()
}

/// The names of the fields on the lines two sets' renderings differ by
/// (`pb_diff_fields`): each changed line's first word.
pub fn pb_diff_fields(a: &[u8], b: &[u8]) -> std::collections::BTreeSet<String> {
    if a == b {
        return Default::default();
    }
    line_diff(&render_set(a), &render_set(b))
        .into_iter()
        .filter_map(|(_, l)| {
            let w: String = l
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!w.is_empty()).then_some(w)
        })
        .collect()
}

/// Each fixture compiled by protoc to its own set in `out`, the fixtures'
/// directory on the include path (`conftest.py`'s `compile_proto`).
pub fn compile_proto(out: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|name| {
            let pb = out.join(Path::new(name).with_extension("pb").file_name().unwrap());
            protoc([
                format!("-I{}", fixture("").display()),
                format!("--descriptor_set_out={}", pb.display()),
                fixture(name).display().to_string(),
            ]);
            pb
        })
        .collect()
}

/// What prototext printed and how it exited.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// The prototext binary on PATH, run on what protoback produced (spec 0488
/// D4): a test tool, as protoc is, which a test checks for with `tools`.
pub fn prototext<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let o = Command::new("prototext")
        .args(args)
        .env_remove("PROTOTEXT_DESCRIPTOR_SET")
        .env_remove("PROTOTEXT_DEFAULT_DESCRIPTOR")
        .output()
        .unwrap();
    Output {
        code: o.status.code(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// `files`, under `root`, compiled with their imports into the set `out`;
/// the set's bytes.
pub fn compile_set(root: &Path, files: &[&str], out: &Path) -> Vec<u8> {
    let o = Command::new("protoc")
        .arg(format!("-I{}", root.display()))
        .arg("--include_imports")
        .arg(format!("--descriptor_set_out={}", out.display()))
        .args(files)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::fs::read(out).unwrap()
}

/// A descriptor message's name (field 1) and its options' bytes (field
/// `options`), empty when it has none: what protoc wrote, by a wire walk.
pub fn name_and_options(m: &[u8], options: u64) -> (String, Vec<u8>) {
    let name = delimited(m, 1)
        .first()
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    (
        name,
        delimited(m, options)
            .first()
            .map(|o| o.to_vec())
            .unwrap_or_default(),
    )
}

/// A `FileDescriptorSet`'s bytes without the files named in `drop`: the
/// set's own bytes for the others, by a wire walk, where the suite rebuilt
/// the set with Python's protobuf (which reserializes by field number: the
/// same messages).
pub fn set_without(set: &[u8], drop: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in delimited(set, 1) {
        let name = delimited(f, 1)
            .first()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        if drop.contains(&name.as_str()) {
            continue;
        }
        out.push(0x0a);
        let mut n = f.len();
        while n >= 0x80 {
            out.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        out.push(n as u8);
        out.extend_from_slice(f);
    }
    out
}

/// Fixtures compiled with their imports into one set at `out`, the
/// fixtures' directory on the include path (`conftest.py`'s
/// `compile_proto_multi`).
pub fn compile_proto_multi(out: &Path, names: &[&str]) -> PathBuf {
    let mut args = vec![
        format!("-I{}", fixture("").display()),
        "--include_imports".to_string(),
        format!("--descriptor_set_out={}", out.display()),
    ];
    args.extend(names.iter().map(|n| fixture(n).display().to_string()));
    protoc(args);
    out.to_path_buf()
}
