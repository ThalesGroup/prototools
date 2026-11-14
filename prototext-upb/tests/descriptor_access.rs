// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Generates `src/descriptor_access.rs`, the typed accessors over
//! `descriptor.proto`'s messages, from `vendor/descriptor.pb` (spec 0424
//! S6), and fails when the committed file is not what it generates.
//!
//! A check of the repository rather than of the software, and it needs
//! rustfmt: ignored, so that a downstream `cargo test` (nixpkgs' check
//! phase, a crates.io user) skips it, as it skips the pyo3 stubs' check.
//! `ci` runs it, and fails on a stale file (`descriptorAccessCheck`,
//! nix/rust.nix):
//!
//!   cargo test -p prototext-upb --test descriptor_access -- --ignored
//!
//! With `PROTOTEXT_UPB_REGENERATE=1`, it writes the file instead, as
//! `scripts/update-upb` does after vendoring a new upb.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};

use prototext_upb::{Cardinality, DynamicMessage, Enum, Field, Kind, Message, Pool, Value};

const DESCRIPTOR_PB: &[u8] = include_bytes!("../vendor/descriptor.pb");
const COMMITTED: &str = include_str!("../src/descriptor_access.rs");
const OUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/descriptor_access.rs");

#[test]
#[ignore = "a check of the repository, needing rustfmt: ci runs it with --ignored"]
fn descriptor_access_is_generated_from_the_vendored_descriptor_proto() {
    let code = generate();
    if std::env::var_os("PROTOTEXT_UPB_REGENERATE").is_some() {
        std::fs::write(OUT, &code).expect("write src/descriptor_access.rs");
        return;
    }
    assert!(
        COMMITTED == code,
        "src/descriptor_access.rs is not what vendor/descriptor.pb generates: \
         PROTOTEXT_UPB_REGENERATE=1 cargo test -p prototext-upb --test descriptor_access \
         -- --ignored"
    );
}

// ── Reading descriptor.proto ────────────────────────────────────────────────

const FILE: &str = "google/protobuf/descriptor.proto";
const PACKAGE: &str = "google.protobuf.";

/// `SourceCodeInfo.Location.path` → leading comment, for the vendored
/// file, read through the reflection the accessors are built on.
fn comments(pool: &Pool) -> HashMap<Vec<i32>, String> {
    let fdp = pool
        .get_message_by_name("google.protobuf.FileDescriptorProto")
        .unwrap();
    let bytes = pool.file_bytes(FILE).unwrap();
    let file = DynamicMessage::decode(&fdp, &bytes).unwrap();
    let mut out = HashMap::new();
    let Some(Value::Message(Some(info))) = file.as_ref().get_by_number(9) else {
        panic!("vendor/descriptor.pb has no source info: compile it with --include_source_info");
    };
    let Some(Value::List(locations)) = info.get_by_number(1) else {
        panic!()
    };
    for location in locations.iter() {
        let Value::Message(Some(location)) = location else {
            panic!()
        };
        let Some(Value::List(path)) = location.get_by_number(1) else {
            panic!()
        };
        let path: Vec<i32> = path
            .iter()
            .map(|v| match v {
                Value::I32(n) => n,
                v => panic!("{v:?}"),
            })
            .collect();
        if let Some(Value::Bytes(c)) = location.get_by_number(3) {
            if location.has_by_number(3) {
                out.insert(path, String::from_utf8(c.to_vec()).unwrap());
            }
        }
    }
    out
}

/// Every message, depth-first in declaration order.
fn all_messages(pool: &Pool) -> Vec<Message> {
    pool.all_messages()
        .into_iter()
        .filter(|m| m.parent_file().name() == FILE)
        .collect()
}

fn all_enums(pool: &Pool) -> Vec<Enum> {
    pool.all_enums()
        .into_iter()
        .filter(|e| e.parent_file().name() == FILE)
        .collect()
}

// ── Names ───────────────────────────────────────────────────────────────────

/// `DescriptorProto.ExtensionRange` → `DescriptorProtoExtensionRange`.
fn type_name(full_name: &str) -> String {
    full_name.strip_prefix(PACKAGE).unwrap().replace('.', "")
}

/// `FieldDescriptorProto.Type` → `field_descriptor_proto_type`.
fn module_name(full_name: &str) -> String {
    let mut out = String::new();
    for part in full_name.strip_prefix(PACKAGE).unwrap().split('.') {
        if !out.is_empty() {
            out.push('_');
        }
        for (i, c) in part.chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// A field name as a Rust method name.
fn method(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "static", "struct", "trait", "true", "type", "unsafe", "use",
        "where", "while", "async", "await", "yield",
    ];
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

// ── Emitting ────────────────────────────────────────────────────────────────

/// A comment as doc lines, in a `text` block: descriptor.proto's comments
/// are not Markdown, and rustdoc would read their brackets as links.
fn doc(out: &mut String, indent: &str, comment: Option<&String>) {
    let Some(comment) = comment else { return };
    let lines: Vec<&str> = comment.trim_end().lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        return;
    }
    writeln!(out, "{indent}/// ```text").unwrap();
    for line in lines {
        let line = line.strip_prefix(' ').unwrap_or(line).trim_end();
        if line.is_empty() {
            writeln!(out, "{indent}///").unwrap();
        } else {
            writeln!(out, "{indent}/// {line}").unwrap();
        }
    }
    writeln!(out, "{indent}/// ```").unwrap();
}

/// How a scalar kind is read, written and typed in Rust.
struct ScalarKind {
    rust: &'static str,
    /// The readers of one value and of a list.
    one: &'static str,
    many: &'static str,
    scalar: &'static str,
}

fn scalar_kind(kind: &Kind) -> Option<ScalarKind> {
    let (rust, one, many, scalar) = match kind {
        Kind::Double => ("f64", "f64_", "f64s", "F64"),
        Kind::Float => ("f32", "f32_", "f32s", "F32"),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => ("i64", "i64_", "i64s", "I64"),
        Kind::Uint64 | Kind::Fixed64 => ("u64", "u64_", "u64s", "U64"),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => ("i32", "i32_", "i32s", "I32"),
        Kind::Uint32 | Kind::Fixed32 => ("u32", "u32_", "u32s", "U32"),
        Kind::Bool => ("bool", "bool_", "bools", "Bool"),
        Kind::Enum(_) => ("i32", "enum_", "enums", "Enum"),
        Kind::String | Kind::Bytes => ("&'a [u8]", "bytes", "bytess", "Bytes"),
        Kind::Message(_) => return None,
    };
    Some(ScalarKind {
        rust,
        one,
        many,
        scalar,
    })
}

const HEADER: &str = r#"// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: Google LLC
//
// SPDX-License-Identifier: MIT AND BSD-3-Clause

//! Typed accessors over the messages of `descriptor.proto` (spec 0424 S6).
//!
//! GENERATED by `tests/descriptor_access.rs` from `vendor/descriptor.pb`,
//! the `descriptor.proto` of the vendored upb (`vendor/VERSION`), whose
//! comments document each type and field below. Do not edit: run
//! `PROTOTEXT_UPB_REGENERATE=1 cargo test -p prototext-upb --test
//! descriptor_access -- --ignored`, as `scripts/update-upb` does.
//!
//! Each type wraps a [`MessageRef`] (and its `…Mut` twin a [`MessageMut`])
//! of that message type, from any package: the `descriptor.proto` of the
//! input or of a variant. Each accessor goes by field number in the
//! message's own type, so a field that version of `descriptor.proto` lacks
//! reads as its zero value, empty or `None`, and cannot be written. Strings
//! are bytes, as [`Value::Bytes`] has them; an enum is its number, with the
//! enum's values as constants in a module of its name.

use crate::{MessageError, MessageMut, MessageRef, Scalar, Value};
"#;

/// The readers the accessors call: `(singular, repeated, Value variant,
/// Rust type, zero)` for the scalars.
const SCALAR_READERS: &[(&str, &str, &str, &str, &str)] = &[
    ("f64_", "f64s", "F64", "f64", "0.0"),
    ("f32_", "f32s", "F32", "f32", "0.0"),
    ("i64_", "i64s", "I64", "i64", "0"),
    ("u64_", "u64s", "U64", "u64", "0"),
    ("i32_", "i32s", "I32", "i32", "0"),
    ("u32_", "u32s", "U32", "u32", "0"),
    ("bool_", "bools", "Bool", "bool", "false"),
    ("enum_", "enums", "Enum", "i32", "0"),
];

/// The code of the readers in `used`, and of the ones they call, in a
/// fixed order: only what the accessors call is generated.
fn readers_code(used: &BTreeSet<&'static str>) -> String {
    let needed = |name: &str, repeated: &str| used.contains(name) || used.contains(repeated);
    let mut out = String::new();
    for &(one, many, variant, ty, zero) in SCALAR_READERS {
        if needed(one, many) {
            write!(
                out,
                "\nfn {one}(v: Option<Value<'_>>) -> {ty} {{\n    match v {{\n        \
                 Some(Value::{variant}(x)) => x,\n        _ => {zero},\n    }}\n}}\n"
            )
            .unwrap();
        }
        if used.contains(many) {
            write!(
                out,
                "\nfn {many}(v: Option<Value<'_>>) -> Vec<{ty}> {{\n    match v {{\n        \
                 Some(Value::List(l)) => l.iter().map(|x| {one}(Some(x))).collect(),\n        \
                 _ => Vec::new(),\n    }}\n}}\n"
            )
            .unwrap();
        }
    }
    if needed("bytes", "bytess") {
        out.push_str(
            "\nfn bytes<'a>(v: Option<Value<'a>>) -> &'a [u8] {\n    match v {\n        \
             Some(Value::Bytes(b)) => b,\n        _ => &[],\n    }\n}\n",
        );
    }
    if used.contains("bytess") {
        out.push_str(
            "\nfn bytess<'a>(v: Option<Value<'a>>) -> Vec<&'a [u8]> {\n    match v {\n        \
             Some(Value::List(l)) => l.iter().map(|x| bytes(Some(x))).collect(),\n        \
             _ => Vec::new(),\n    }\n}\n",
        );
    }
    if needed("message", "messages") {
        out.push_str(
            "\nfn message<'a>(v: Option<Value<'a>>) -> Option<MessageRef<'a>> {\n    \
             match v {\n        Some(Value::Message(m)) => m,\n        _ => None,\n    }\n}\n",
        );
    }
    if used.contains("messages") {
        out.push_str(
            "\nfn messages<'a>(v: Option<Value<'a>>) -> Vec<MessageRef<'a>> {\n    match v {\n        \
             Some(Value::List(l)) => l.iter().filter_map(|x| message(Some(x))).collect(),\n        \
             _ => Vec::new(),\n    }\n}\n",
        );
    }
    out
}

fn generate() -> String {
    let pool = Pool::new();
    let report = pool.load_set(DESCRIPTOR_PB);
    assert_eq!(report.loaded, [FILE], "{report:?}");
    let comments = comments(&pool);
    let mut used = BTreeSet::new();
    let mut body = String::new();
    for m in all_messages(&pool) {
        message_code(&mut body, &m, &comments, &mut used);
    }
    for e in all_enums(&pool) {
        enum_code(&mut body, &e, &comments);
    }
    rustfmt(&format!("{HEADER}{}{body}", readers_code(&used)))
}

/// `code` as rustfmt lays it out, the crate's edition and the defaults
/// `cargo fmt` uses here: the committed file is then what the formatting
/// check expects, and the generator does not imitate rustfmt's rules.
/// `RUSTFMT` names another rustfmt than the one on `PATH`.
fn rustfmt(code: &str) -> String {
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(&rustfmt)
        .args(["--edition", "2021"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{rustfmt:?}: {e} (set RUSTFMT)"));
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "rustfmt failed on the generated code"
    );
    String::from_utf8(output.stdout).unwrap()
}

fn message_code(
    out: &mut String,
    m: &Message,
    comments: &HashMap<Vec<i32>, String>,
    used: &mut BTreeSet<&'static str>,
) {
    let ty = type_name(m.full_name());
    let simple = m.name();
    let path = m.declaration_path();
    let fields: Vec<Field> = m.fields().collect();

    // The reader.
    writeln!(out).unwrap();
    doc(out, "", comments.get(&path));
    writeln!(out, "#[derive(Debug, Clone)]").unwrap();
    writeln!(out, "pub struct {ty}<'a>(pub MessageRef<'a>);").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "impl<'a> {ty}<'a> {{").unwrap();
    writeln!(
        out,
        "    /// `m` as a `{simple}`, when its type is one, in any package."
    )
    .unwrap();
    writeln!(out, "    pub fn new(m: MessageRef<'a>) -> Option<Self> {{").unwrap();
    writeln!(
        out,
        "        (m.descriptor().name() == \"{simple}\").then_some(Self(m))"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    for (i, f) in fields.iter().enumerate() {
        let mut fpath = path.clone();
        fpath.extend([2, i as i32]);
        reader_code(out, f, comments.get(&fpath), used);
    }
    writeln!(out, "}}").unwrap();

    // The writer.
    writeln!(out).unwrap();
    writeln!(out, "/// A [`{ty}`] to write.").unwrap();
    writeln!(out, "#[derive(Debug)]").unwrap();
    writeln!(out, "pub struct {ty}Mut<'a>(pub MessageMut<'a>);").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "impl {ty}Mut<'_> {{").unwrap();
    writeln!(out, "    pub fn as_ref(&self) -> {ty}<'_> {{").unwrap();
    writeln!(out, "        {ty}(self.0.as_ref())").unwrap();
    writeln!(out, "    }}").unwrap();
    for f in &fields {
        writer_code(out, f);
    }
    writeln!(out, "}}").unwrap();
}

fn reader_code(
    out: &mut String,
    f: &Field,
    comment: Option<&String>,
    used: &mut BTreeSet<&'static str>,
) {
    assert!(!f.is_map(), "descriptor.proto has no map field");
    let name = method(f.name());
    let n = f.number();
    let repeated = f.cardinality() == Cardinality::Repeated;
    writeln!(out).unwrap();
    doc(out, "    ", comment);
    let kind = f.kind();
    match (&kind, scalar_kind(&kind), repeated) {
        (Kind::Message(sub), _, false) => {
            let sub = type_name(sub.full_name());
            used.insert("message");
            writeln!(out, "    pub fn {name}(&self) -> Option<{sub}<'a>> {{").unwrap();
            writeln!(out, "        message(self.0.get_by_number({n})).map({sub})").unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (Kind::Message(sub), _, true) => {
            let sub = type_name(sub.full_name());
            used.insert("messages");
            writeln!(out, "    pub fn {name}(&self) -> Vec<{sub}<'a>> {{").unwrap();
            writeln!(out, "        messages(self.0.get_by_number({n}))").unwrap();
            writeln!(out, "            .into_iter()").unwrap();
            writeln!(out, "            .map({sub})").unwrap();
            writeln!(out, "            .collect()").unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (_, Some(s), false) => {
            let read = s.one;
            used.insert(read);
            writeln!(out, "    pub fn {name}(&self) -> {} {{", s.rust).unwrap();
            writeln!(out, "        {read}(self.0.get_by_number({n}))").unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (_, Some(s), true) => {
            let read = s.many;
            used.insert(read);
            writeln!(out, "    pub fn {name}(&self) -> Vec<{}> {{", s.rust).unwrap();
            writeln!(out, "        {read}(self.0.get_by_number({n}))").unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (_, None, _) => unreachable!(),
    }
    if !repeated {
        let bare = f.name();
        writeln!(out).unwrap();
        writeln!(out, "    pub fn has_{bare}(&self) -> bool {{").unwrap();
        writeln!(out, "        self.0.has_by_number({n})").unwrap();
        writeln!(out, "    }}").unwrap();
    }
}

fn writer_code(out: &mut String, f: &Field) {
    let bare = f.name();
    let n = f.number();
    let repeated = f.cardinality() == Cardinality::Repeated;
    let kind = f.kind();
    writeln!(out).unwrap();
    match (&kind, scalar_kind(&kind), repeated) {
        (Kind::Message(sub), _, false) => {
            let sub = type_name(sub.full_name());
            writeln!(
                out,
                "    pub fn {bare}_mut(&mut self) -> Result<{sub}Mut<'_>, MessageError> {{"
            )
            .unwrap();
            writeln!(
                out,
                "        self.0.message_mut_by_number({n}).map({sub}Mut)"
            )
            .unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (Kind::Message(sub), _, true) => {
            let sub = type_name(sub.full_name());
            writeln!(
                out,
                "    pub fn push_{bare}(&mut self) -> Result<{sub}Mut<'_>, MessageError> {{"
            )
            .unwrap();
            writeln!(
                out,
                "        self.0.push_message_by_number({n}).map({sub}Mut)"
            )
            .unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (_, Some(s), _) => {
            let (verb, call) = if repeated {
                ("push", "push_by_number")
            } else {
                ("set", "set_by_number")
            };
            let arg = if s.scalar == "Bytes" { "&[u8]" } else { s.rust };
            writeln!(
                out,
                "    pub fn {verb}_{bare}(&mut self, value: {arg}) -> Result<(), MessageError> {{"
            )
            .unwrap();
            writeln!(
                out,
                "        self.0.{call}({n}, Scalar::{}(value))",
                s.scalar
            )
            .unwrap();
            writeln!(out, "    }}").unwrap();
        }
        (_, None, _) => unreachable!(),
    }
    writeln!(out).unwrap();
    writeln!(out, "    pub fn clear_{bare}(&mut self) {{").unwrap();
    writeln!(out, "        self.0.clear_by_number({n})").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn enum_code(out: &mut String, e: &Enum, comments: &HashMap<Vec<i32>, String>) {
    let path = e.declaration_path();
    writeln!(out).unwrap();
    doc(out, "", comments.get(&path));
    writeln!(out, "pub mod {} {{", module_name(e.full_name())).unwrap();
    for (i, v) in e.values().enumerate() {
        let mut vpath = path.clone();
        vpath.extend([2, i as i32]);
        doc(out, "    ", comments.get(&vpath));
        writeln!(out, "    pub const {}: i32 = {};", v.name(), v.number()).unwrap();
    }
    writeln!(out, "}}").unwrap();
}
