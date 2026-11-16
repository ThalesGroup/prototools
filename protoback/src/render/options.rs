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

/// What reproto's `ctx.pool` holds, as rendering fills it (spec 0429 S4,
/// S16): files are loaded as rendering asks for them, each after its
/// imports, in the order of the summoned files. Computed once, before
/// rendering, in that order: rendering itself only reads it.
pub struct PoolLoads {
    /// The files whose build fails, or one of whose imports does.
    pub refused: BTreeSet<String>,
    /// Each file loaded: the index, among the summoned files, of the first
    /// one whose render loaded it. While the k-th renders, the pool holds
    /// the files of rank k or less.
    pub rank: HashMap<String, usize>,
    /// The files loaded, in the order they were.
    pub order: Vec<String>,
}

pub fn pool_loads(db: &Database, summoned: &[String]) -> PoolLoads {
    struct Loader<'a> {
        db: &'a Database,
        pool: Pool,
        done: HashMap<String, bool>,
        loads: PoolLoads,
        k: usize,
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
            if ok {
                self.loads.rank.insert(name.to_string(), self.k);
                self.loads.order.push(name.to_string());
            }
            ok
        }
    }
    let mut loader = Loader {
        db,
        pool: Pool::new(),
        done: HashMap::new(),
        loads: PoolLoads {
            refused: BTreeSet::new(),
            rank: HashMap::new(),
            order: Vec::new(),
        },
        k: 0,
    };
    for (k, name) in summoned.iter().enumerate() {
        loader.k = k;
        if !loader.load(name) {
            loader.loads.refused.insert(name.clone());
        }
    }
    loader.loads
}

/// A file as reproto's options database serves it
/// (`_MapsAsRepeatedDatabase`): every message's `map_entry` cleared, and
/// its options with it when nothing else is set, so that a map in an
/// option parses as the repeated entries it is on the wire, in order.
fn without_map_entry(fdp: &DynamicMessage) -> Option<Vec<u8>> {
    use prototext_upb::descriptor_access::{DescriptorProtoMut, FileDescriptorProtoMut};
    fn clear(mut m: DescriptorProtoMut<'_>) {
        if m.as_ref().options().is_some_and(|o| o.has_map_entry()) {
            if let Ok(mut o) = m.options_mut() {
                o.clear_map_entry();
            }
            if m.as_ref()
                .options()
                .is_some_and(|o| o.0.set_fields().is_empty())
            {
                m.clear_options();
            }
        }
        for j in 0..m.as_ref().nested_type().len() {
            if let Ok(n) = m.nested_type_mut(j) {
                clear(n);
            }
        }
    }
    let mut copy = DynamicMessage::decode(fdp.descriptor(), &fdp.encode().ok()?).ok()?;
    {
        let mut f = FileDescriptorProtoMut(copy.as_mut());
        for i in 0..f.as_ref().message_type().len() {
            if let Ok(m) = f.message_type_mut(i) {
                clear(m);
            }
        }
    }
    copy.encode().ok()
}

/// reproto's options pool (`create_option_message_classes`,
/// `option_extensions`): the variant's descriptor.proto, whose options
/// messages parse every element's options, then the files `ctx.pool`
/// loads, in that order, whose extensions parse as the custom options
/// they are once their file is loaded (`PoolLoads::rank`).
pub struct OptionsPool {
    pool: Pool,
    /// The package of the variant's descriptor.proto (`ctx.options_package`).
    pub package: String,
}

impl OptionsPool {
    pub fn new(db: &Database, variant: &Variant, loads: &PoolLoads) -> Option<OptionsPool> {
        let fdp = db.file_by_name(&variant.descriptor_proto)?;
        let pool = Pool::new();
        pool.add_file(&without_map_entry(fdp)?).ok()?;
        for name in &loads.order {
            if *name == variant.descriptor_proto {
                continue;
            }
            let Some(bytes) = db.file_by_name(name).and_then(without_map_entry) else {
                continue;
            };
            // reproto's options pool builds what its ctx.pool built, the
            // same file with only map_entry cleared, which a map field
            // survives as the repeated field it is: a failure here would be
            // a file ctx.pool built, which then parses no more options.
            if pool.add_file(&bytes).is_err() {
                continue;
            }
        }
        let f = prototext_upb::descriptor_access::FileDescriptorProto(fdp.as_ref());
        let package = String::from_utf8_lossy(f.package()).into_owned();
        Some(OptionsPool { pool, package })
    }

    /// An element's options, from its descriptor's options message
    /// (`parse_options`); `None` where reproto has no class for them
    /// (`ExtensionRangeOptions` in a descriptor.proto older than it).
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

/// What rendering an option can fail on, as reproto's `except` clauses
/// catch it: an enum value with no name (`values_by_number.get(v).name`).
pub struct Unrenderable;

/// What an option's rendering reads beyond the option: the variant (its
/// orphans), the graph (whether a custom option's extension is summoned),
/// and which extensions reproto's options pool knows while the file
/// renders (spec 0429 S4).
pub struct Scope<'a> {
    pub variant: &'a Variant,
    pub graph: &'a crate::graph::Graph,
    pub rank: &'a HashMap<String, usize>,
    /// The rendering file's index among the summoned files.
    pub k: usize,
}

impl Scope<'_> {
    /// An extension reproto's options pool knows while the k-th file
    /// renders: one declared in the variant's descriptor.proto, or in a
    /// file the renders of files 0 to k loaded. Any other is, to reproto,
    /// an unknown field, which it does not render.
    fn visible(&self, ext: &Field) -> bool {
        let file = ext.parent_file();
        let name = file.name();
        name == self.variant.descriptor_proto || self.rank.get(name).is_some_and(|&r| r <= self.k)
    }

    /// `canonize_opt_name` for a custom option: `(.full.name)`, an orphan
    /// unless its extension's node is summoned.
    fn custom_name(&self, ext: &Field) -> (String, bool) {
        let full = format!(".{}", ext.full_name());
        let orphan = !self
            .graph
            .find(&format!("fdsc:{full}"))
            .is_some_and(|n| self.graph.nodes[n].is_summoned);
        (format!("({full})"), orphan)
    }
}

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
fn aggregate(
    scope: &Scope<'_>,
    msg: &MessageRef<'_>,
    depth: usize,
    out: &mut Block,
) -> Result<(), Unrenderable> {
    for (field, value) in msg.set_fields() {
        if field.is_extension() && !scope.visible(&field) {
            continue;
        }
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
                    aggregate_value(scope, &field, &item, &name, depth, out)?;
                }
            }
            v => aggregate_value(scope, &field, &v, &name, depth, out)?,
        }
    }
    Ok(())
}

fn aggregate_value(
    scope: &Scope<'_>,
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
                aggregate(scope, m, depth + 1, out)?;
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
    scope: &Scope<'_>,
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
                    aggregate(scope, m, depth + 1, &mut block)?;
                }
                block.push(Line::code("}", depth));
            }
        }
        v => block.push(Line::code(format!("{name} = {}", scalar(field, v)?), depth)),
    }
    Ok(block)
}

/// `render_options_from_message` (and `ReOptions`, `ReExtensions`): the
/// built-in options set on `msg`, in field number order, then its custom
/// options reproto's options pool knows, by number (S4). Each value a
/// block: `option x = v;` standalone, `x = v,` in a composite; an orphan
/// when the variant lists it (built-in) or its extension is not summoned
/// (custom). `Err` when one cannot be rendered, which drops them all, as
/// reproto's callers do.
pub fn options(
    scope: &Scope<'_>,
    kind: Options,
    msg: &DynamicMessage,
    composite: bool,
    depth: usize,
    exclude: &[&str],
) -> Result<Vec<Block>, Unrenderable> {
    let fields = msg.as_ref().set_fields();
    let mut entries: Vec<(String, bool, &Field, &Value<'_>)> = Vec::new();
    for (field, value) in &fields {
        if field.is_extension() || exclude.contains(&field.name()) {
            continue;
        }
        if let Some((name, orphan)) = builtin_name(scope.variant, kind, field) {
            entries.push((name, orphan, field, value));
        }
    }
    let mut custom: Vec<&(Field, Value<'_>)> = fields
        .iter()
        .filter(|(f, _)| f.is_extension() && scope.visible(f))
        .collect();
    custom.sort_by_key(|(f, _)| f.number());
    for (field, value) in custom {
        let (name, orphan) = scope.custom_name(field);
        entries.push((name, orphan, field, value));
    }

    let mut blocks = Vec::new();
    for (name, orphan, field, value) in entries {
        let values: Vec<Value<'_>> = match (field.cardinality(), value) {
            (Cardinality::Repeated, Value::List(list)) => list.iter().collect(),
            (_, v) => vec![v.clone()],
        };
        for v in values {
            let mut block = dump_option(scope, field, &v, &name, depth)?;
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
