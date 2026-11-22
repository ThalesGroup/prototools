// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::fmt::Write as _;

use prototext_upb::descriptor_access::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
};
use prototext_upb::{DynamicMessage, MessageRef, Pool, Value};

/// One extension a file declares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Extension {
    pub extendee: String,
    pub number: i32,
    pub full_name: String,
}

#[derive(Debug, Clone)]
pub struct FdpInfo {
    /// The descriptor's name (`foo/bar.proto`).
    pub name: String,
    pub dependencies: Vec<String>,
    /// Full names a pool registers: messages (nested included), enums and
    /// their values (in the enum's enclosing scope), services, extensions.
    pub symbols: Vec<String>,
    pub extensions: Vec<Extension>,
    /// Each top-level message, enum, service and extension, by full name, in
    /// a canonical form that ignores `json_name`.
    pub definitions: BTreeMap<String, String>,
    /// The whole file in that canonical form.
    pub canonical: String,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Read a serialized `FileDescriptorProto`. `None` if it doesn't decode.
pub fn read(fdp: &[u8]) -> Option<FdpInfo> {
    let def = Pool::vendored().get_message_by_name("google.protobuf.FileDescriptorProto")?;
    let msg = DynamicMessage::decode(&def, fdp).ok()?;
    let f = FileDescriptorProto::new(msg.as_ref())?;

    let package = text(f.package());
    let scope = if package.is_empty() {
        String::new()
    } else {
        format!(".{package}")
    };

    let mut info = FdpInfo {
        name: text(f.name()),
        dependencies: f.dependency().into_iter().map(text).collect(),
        symbols: Vec::new(),
        extensions: Vec::new(),
        definitions: BTreeMap::new(),
        canonical: canonical(msg.as_ref()),
    };

    for m in f.message_type() {
        message(&mut info, &m, &scope);
        info.definitions
            .insert(format!("{scope}.{}", text(m.name())), canonical(m.0));
    }
    for e in f.enum_type() {
        enumeration(&mut info, &e, &scope);
        info.definitions
            .insert(format!("{scope}.{}", text(e.name())), canonical(e.0));
    }
    for s in f.service() {
        let full = format!("{scope}.{}", text(s.name()));
        info.symbols.push(full.clone());
        info.definitions.insert(full, canonical(s.0));
    }
    for x in f.extension() {
        extension(&mut info, &x, &scope);
        info.definitions
            .insert(format!("{scope}.{}", text(x.name())), canonical(x.0));
    }
    Some(info)
}

fn message(info: &mut FdpInfo, m: &DescriptorProto, scope: &str) {
    let full = format!("{scope}.{}", text(m.name()));
    info.symbols.push(full.clone());
    for n in m.nested_type() {
        message(info, &n, &full);
    }
    for e in m.enum_type() {
        enumeration(info, &e, &full);
    }
    for x in m.extension() {
        extension(info, &x, &full);
    }
}

fn enumeration(info: &mut FdpInfo, e: &EnumDescriptorProto, scope: &str) {
    // Enum values are siblings of their enum, in the enclosing scope.
    info.symbols.push(format!("{scope}.{}", text(e.name())));
    for v in e.value() {
        info.symbols.push(format!("{scope}.{}", text(v.name())));
    }
}

fn extension(info: &mut FdpInfo, x: &FieldDescriptorProto, scope: &str) {
    let full = format!("{scope}.{}", text(x.name()));
    info.symbols.push(full.clone());
    info.extensions.push(Extension {
        extendee: text(x.extendee()),
        number: x.number(),
        full_name: full,
    });
}

/// A canonical text of a message: every field set, by number, recursively,
/// `json_name` left out. Two definitions are identical when these are equal.
pub fn canonical(m: MessageRef) -> String {
    let mut out = String::new();
    write_message(&mut out, &m);
    out
}

fn write_message(out: &mut String, m: &MessageRef) {
    out.push('{');
    for (field, value) in m.set_fields() {
        if field.name() == "json_name" {
            continue;
        }
        let _ = write!(out, "{}:", field.number());
        write_value(out, &value);
        out.push(';');
    }
    out.push('}');
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Bool(v) => {
            let _ = write!(out, "{v}");
        }
        Value::I32(v) | Value::Enum(v) => {
            let _ = write!(out, "{v}");
        }
        Value::I64(v) => {
            let _ = write!(out, "{v}");
        }
        Value::U32(v) => {
            let _ = write!(out, "{v}");
        }
        Value::U64(v) => {
            let _ = write!(out, "{v}");
        }
        Value::F32(v) => {
            let _ = write!(out, "{:?}", v.to_bits());
        }
        Value::F64(v) => {
            let _ = write!(out, "{:?}", v.to_bits());
        }
        Value::Bytes(b) => {
            let _ = write!(out, "{b:?}");
        }
        Value::Message(Some(m)) => write_message(out, m),
        Value::Message(None) => out.push_str("{}"),
        Value::List(list) => {
            out.push('[');
            for v in list.iter() {
                write_value(out, &v);
                out.push(',');
            }
            out.push(']');
        }
        Value::Map(_) => out.push_str("map"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prototext_upb::text_format::parse_descriptor;

    fn fdp(text: &str) -> FdpInfo {
        read(&parse_descriptor(text, false).unwrap()).unwrap()
    }

    #[test]
    fn symbols_and_extensions() {
        let i = fdp(r#"
            name: "a/b.proto" package: "p" dependency: "c.proto"
            message_type { name: "M"
              nested_type { name: "N" }
              enum_type { name: "E" value { name: "X" number: 0 } }
              extension { name: "x" number: 100 extendee: ".q.O" }
            }
            enum_type { name: "F" value { name: "Y" number: 0 } }
            service { name: "S" }
            extension { name: "y" number: 7 extendee: ".q.O" }
        "#);
        assert_eq!(i.name, "a/b.proto");
        assert_eq!(i.dependencies, ["c.proto"]);
        assert_eq!(
            i.symbols,
            [".p.M", ".p.M.N", ".p.M.E", ".p.M.X", ".p.M.x", ".p.F", ".p.Y", ".p.S", ".p.y"]
        );
        assert_eq!(
            i.extensions
                .iter()
                .map(|x| (x.number, x.full_name.as_str()))
                .collect::<Vec<_>>(),
            [(100, ".p.M.x"), (7, ".p.y")]
        );
        assert_eq!(
            i.definitions.keys().collect::<Vec<_>>(),
            [".p.F", ".p.M", ".p.S", ".p.y"]
        );
    }

    #[test]
    fn json_name_is_ignored() {
        let a = fdp(
            r#"name: "a.proto" message_type { name: "M" field { name: "f_g" number: 1 json_name: "fG" } }"#,
        );
        let b =
            fdp(r#"name: "a.proto" message_type { name: "M" field { name: "f_g" number: 1 } }"#);
        let c =
            fdp(r#"name: "a.proto" message_type { name: "M" field { name: "f_g" number: 2 } }"#);
        assert_eq!(a.canonical, b.canonical);
        assert_eq!(a.definitions, b.definitions);
        assert_ne!(a.definitions, c.definitions);
    }
}
