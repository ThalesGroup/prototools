// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Options (`option_rendering.py`, `option_renderers.py`,
//! `field_descriptor.py`, `simple_types.ReMessage`), stage 2: built-in
//! options, those of the variant's descriptor.proto, parsed as reproto's
//! options pool parses them, rendered as its `dump_option` renders them.

use std::collections::{BTreeSet, HashMap};

use prototext_upb::{Cardinality, DynamicMessage, Field, Kind, Pool};
use prototext_upb::{MessageRef, Value};

use super::pyfmt::{float_repr, json_dumps, render_bytes, str_repr};
use super::text::{Block, Line, LineType};
use crate::db::Database;
use crate::variant::Variant;

/// The options messages, by reproto's names for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Options {
    File,
    Message,
    Field,
    Oneof,
    ExtensionRange,
    Enum,
    EnumValue,
    Service,
    Method,
}

impl Options {
    fn type_name(self) -> &'static str {
        match self {
            Options::File => "FileOptions",
            Options::Message => "MessageOptions",
            Options::Field => "FieldOptions",
            Options::Oneof => "OneofOptions",
            Options::ExtensionRange => "ExtensionRangeOptions",
            Options::Enum => "EnumOptions",
            Options::EnumValue => "EnumValueOptions",
            Options::Service => "ServiceOptions",
            Options::Method => "MethodOptions",
        }
    }
}

/// reproto's options pool (`create_option_message_classes`): the variant's
/// descriptor.proto, whose options messages parse every element's options.
pub struct OptionsPool {
    pool: Pool,
    /// The package of the variant's descriptor.proto (`ctx.options_package`).
    pub package: String,
}

impl OptionsPool {
    pub fn new(db: &Database, variant: &Variant) -> Option<OptionsPool> {
        let fdp = db.file_by_name(&variant.descriptor_proto)?;
        let bytes = fdp.encode().ok()?;
        let pool = Pool::new();
        pool.add_file(&bytes).ok()?;
        let f = prototext_upb::descriptor_access::FileDescriptorProto(fdp.as_ref());
        let package = String::from_utf8_lossy(f.package()).into_owned();
        Some(OptionsPool { pool, package })
    }

    /// An element's serialized options, parsed (`parse_options`); `None`
    /// where reproto has no class for them (`ExtensionRangeOptions` in a
    /// descriptor.proto older than it).
    /// An element's options, from its descriptor's options message.
    pub fn parse_ref(&self, kind: Options, options: &MessageRef<'_>) -> Option<DynamicMessage> {
        self.parse(kind, &options.encode().ok()?)
    }

    pub fn parse(&self, kind: Options, bytes: &[u8]) -> Option<DynamicMessage> {
        let name = if self.package.is_empty() {
            kind.type_name().to_string()
        } else {
            format!("{}.{}", self.package, kind.type_name())
        };
        let def = self.pool.get_message_by_name(&name)?;
        DynamicMessage::decode(&def, bytes).ok()
    }
}

/// Which files reproto's `ctx.pool` refuses to build (spec 0429 S4,
/// S16): files are loaded as rendering asks for them, each after its
/// imports, in the order of the summoned files; a file whose build
/// fails, or one of whose imports does, is refused. Computed once, before
/// rendering, in that order: rendering itself only reads it.
pub fn refused_files(db: &Database, order: &[String]) -> BTreeSet<String> {
    struct Loader<'a> {
        db: &'a Database,
        pool: Pool,
        done: HashMap<String, bool>,
    }
    impl Loader<'_> {
        fn load(&mut self, name: &str) -> bool {
            if let Some(&ok) = self.done.get(name) {
                return ok;
            }
            // In progress: a cycle fails, as an import not yet loaded.
            self.done.insert(name.to_string(), false);
            let Some(fdp) = self.db.file_by_name(name) else {
                return false;
            };
            let f = prototext_upb::descriptor_access::FileDescriptorProto(fdp.as_ref());
            let deps: Vec<String> = f
                .dependency()
                .iter()
                .map(|d| String::from_utf8_lossy(d).into_owned())
                .collect();
            for d in &deps {
                self.load(d);
            }
            let ok = fdp
                .encode()
                .ok()
                .is_some_and(|bytes| self.pool.add_file(&bytes).is_ok());
            self.done.insert(name.to_string(), ok);
            ok
        }
    }
    let mut loader = Loader {
        db,
        pool: Pool::new(),
        done: HashMap::new(),
    };
    let mut refused = BTreeSet::new();
    for name in order {
        if !loader.load(name) {
            refused.insert(name.clone());
        }
    }
    refused
}

/// What rendering an option can fail on, as reproto's `except` clauses
/// catch it: an enum value with no name (`values_by_number.get(v).name`).
pub struct Unrenderable;

/// `canonize_opt_name` for a built-in option: its short name, `None` for
/// `features` and `feature_support` (rendered elsewhere, or not at all);
/// an orphan when the variant lists it. The pattern covers eight options
/// messages, not `OneofOptions`: a oneof's options keep their full name.
fn builtin_name(variant: &Variant, kind: Options, field: &Field) -> Option<(String, bool)> {
    if kind == Options::Oneof {
        let full = field.full_name();
        return Some((format!(".{full}"), false));
    }
    let short = field.name().to_string();
    if short == "features" || short == "feature_support" {
        return None;
    }
    let orphan = variant
        .orphans
        .get(kind.type_name())
        .is_some_and(|names| names.contains(&short));
    Some((short, orphan))
}

/// A scalar option value's text (`Scalar.__str__`, through `get_scalar`).
fn scalar(field: &Field, value: &Value<'_>) -> Result<String, Unrenderable> {
    Ok(match (value, field.kind()) {
        (Value::Bool(b), _) => if *b { "true" } else { "false" }.to_string(),
        (Value::I32(v), _) => v.to_string(),
        (Value::I64(v), _) => v.to_string(),
        (Value::U32(v), _) => v.to_string(),
        (Value::U64(v), _) => v.to_string(),
        (Value::F32(v), _) => float_repr(f64::from(*v)),
        (Value::F64(v), _) => float_repr(*v),
        (Value::Enum(n), Kind::Enum(e)) => e
            .get_value(*n)
            .map(|v| v.name().to_string())
            .ok_or(Unrenderable)?,
        (Value::Bytes(b), Kind::String) => json_dumps(&String::from_utf8_lossy(b)),
        (Value::Bytes(b), _) => render_bytes(b),
        _ => return Err(Unrenderable),
    })
}

/// `ReMessage.render`: an aggregate's fields, one per line, `name: value`,
/// scalars as Python's `repr` writes them.
fn aggregate(msg: &MessageRef<'_>, depth: usize, out: &mut Block) -> Result<(), Unrenderable> {
    for (field, value) in msg.set_fields() {
        let name = if field.is_extension() {
            format!("[{}]", field.full_name())
        } else if field.is_group() {
            match field.kind() {
                Kind::Message(m) => m.name().to_string(),
                _ => field.name().to_string(),
            }
        } else {
            field.name().to_string()
        };
        match value {
            Value::List(list) => {
                for item in list.iter() {
                    aggregate_value(&field, &item, &name, depth, out)?;
                }
            }
            v => aggregate_value(&field, &v, &name, depth, out)?,
        }
    }
    Ok(())
}

fn aggregate_value(
    field: &Field,
    value: &Value<'_>,
    name: &str,
    depth: usize,
    out: &mut Block,
) -> Result<(), Unrenderable> {
    let text = match (value, field.kind()) {
        (Value::Message(m), _) => {
            out.push(Line::code(format!("{name}: {{"), depth));
            if let Some(m) = m {
                aggregate(m, depth + 1, out)?;
            }
            out.push(Line::code("}", depth));
            return Ok(());
        }
        (Value::Enum(n), Kind::Enum(e)) => e
            .get_value(*n)
            .map(|v| v.name().to_string())
            .ok_or(Unrenderable)?,
        (Value::Bytes(b), Kind::Bytes) => {
            let escaped: String = b.iter().map(|x| format!("\\x{x:02x}")).collect();
            format!("\"{escaped}\"")
        }
        (Value::Bytes(b), _) => str_repr(&String::from_utf8_lossy(b)),
        (Value::Bool(b), _) => if *b { "True" } else { "False" }.to_string(),
        (Value::I32(v), _) => v.to_string(),
        (Value::I64(v), _) => v.to_string(),
        (Value::U32(v), _) => v.to_string(),
        (Value::U64(v), _) => v.to_string(),
        (Value::F32(v), _) => float_repr(f64::from(*v)),
        (Value::F64(v), _) => float_repr(*v),
        _ => return Err(Unrenderable),
    };
    out.push(Line::code(format!("{name}: {text}"), depth));
    Ok(())
}

/// `ReFieldDescriptor.dump_option`: one value of option `name`.
fn dump_option(
    field: &Field,
    value: &Value<'_>,
    name: &str,
    depth: usize,
) -> Result<Block, Unrenderable> {
    let mut block = Block::new();
    match value {
        Value::Message(m) => {
            let empty = m.as_ref().is_none_or(|m| m.set_fields().is_empty());
            if empty {
                block.push(Line::code(format!("{name} = {{}}"), depth));
            } else {
                block.push(Line::code(format!("{name} = {{"), depth));
                if let Some(m) = m {
                    aggregate(m, depth + 1, &mut block)?;
                }
                block.push(Line::code("}", depth));
            }
        }
        v => block.push(Line::code(format!("{name} = {}", scalar(field, v)?), depth)),
    }
    Ok(block)
}

/// `render_options_from_message`'s first loop (and `ReOptions.render`):
/// the built-in options set on `msg`, in field number order, each value a
/// block: `option x = v;` standalone, `x = v,` in a composite; an orphan
/// when the variant lists it. `Err` when one cannot be rendered, which
/// drops them all, as reproto's callers do.
pub fn builtin(
    variant: &Variant,
    kind: Options,
    msg: &DynamicMessage,
    composite: bool,
    depth: usize,
    exclude: &[&str],
) -> Result<Vec<Block>, Unrenderable> {
    let mut blocks = Vec::new();
    for (field, value) in msg.as_ref().set_fields() {
        if field.is_extension() || exclude.contains(&field.name()) {
            continue;
        }
        let Some((name, orphan)) = builtin_name(variant, kind, &field) else {
            continue;
        };
        let values: Vec<Value<'_>> = match (field.cardinality(), value) {
            (Cardinality::Repeated, Value::List(list)) => list.iter().collect(),
            (_, v) => vec![v],
        };
        for v in values {
            let mut block = dump_option(&field, &v, &name, depth)?;
            if composite {
                block.postpend(",");
            } else {
                if let Some(first) = block.lines.first_mut() {
                    first.text.insert_str(0, "option ");
                }
                block.postpend(";");
            }
            let kind = if orphan {
                LineType::Orphan
            } else {
                LineType::Code
            };
            for line in &mut block.lines {
                line.kind = kind;
            }
            blocks.push(block);
        }
    }
    Ok(blocks)
}
