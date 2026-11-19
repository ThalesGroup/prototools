// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Options (`option_rendering.py`, `option_renderers.py`,
//! `field_descriptor.py`, `simple_types.ReMessage`), stage 2: built-in
//! options, those of the variant's descriptor.proto, parsed as reproto's
//! options pool parses them, rendered as its `dump_option` renders them.

use crate::hash::{HashMap, HashSet};
use std::collections::BTreeMap;

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
    /// Not an options message, but read as one for its extensions, the
    /// custom features (spec 0435).
    FeatureSet,
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
            Options::FeatureSet => "FeatureSet",
        }
    }
}

/// What reproto's `ctx.pool` holds, as rendering fills it (spec 0429 S4,
/// S16): files are loaded as rendering asks for them, each after its
/// imports, in the order of the summoned files. Computed once, before
/// rendering, in that order: rendering itself only reads it.
pub struct PoolLoads {
    /// The files whose build fails, or one of whose imports does, each
    /// with its reason (`refused_itself`, `refused_import`): what W19, W20,
    /// W21, and W6 in such a file, say.
    pub refused: BTreeMap<String, String>,
    /// The files loaded: whose extensions a scoring graph lists (spec 0430
    /// S1).
    pub loaded: HashSet<String>,
    /// The files loaded, in the order they were.
    pub order: Vec<String>,
    /// The pool itself, every file loaded: what the scoring graphs read
    /// (spec 0430 S1).
    pub pool: Pool,
}

pub fn pool_loads(db: &Database, summoned: &[String]) -> PoolLoads {
    struct Loader<'a> {
        db: &'a Database,
        pool: Pool,
        done: HashMap<String, bool>,
        /// Why each file failed, as the pool said.
        reasons: HashMap<String, String>,
        loads: PoolLoads,
    }
    impl Loader<'_> {
        fn load(&mut self, name: &str) -> bool {
            if let Some(&ok) = self.done.get(name) {
                return ok;
            }
            // In progress: a cycle fails, as an import not yet loaded.
            self.done.insert(name.to_string(), false);
            let Some(fdp) = self.db.file_by_name(name) else {
                self.reasons
                    .insert(name.to_string(), format!("file not found: {name}"));
                return false;
            };
            let f = prototext_upb::descriptor_access::FileDescriptorProto(fdp.as_ref());
            let deps: Vec<String> = f
                .dependency()
                .iter()
                .map(|d| String::from_utf8_lossy(d).into_owned())
                .collect();
            // Each import loaded first; the first refused names the reason.
            let mut refused_import = None;
            for d in &deps {
                if !self.load(d) && refused_import.is_none() {
                    refused_import = Some(d.clone());
                }
            }
            let added = fdp
                .encode()
                .map_err(|e| e.to_string())
                .and_then(|bytes| self.pool.add_file(&bytes).map_err(|e| e.0));
            if let Err(upb) = &added {
                let reason = match &refused_import {
                    Some(d) => super::refused_import(d),
                    None => super::refused_itself(upb),
                };
                self.reasons.insert(name.to_string(), reason);
            }
            let ok = added.is_ok();
            self.done.insert(name.to_string(), ok);
            if ok {
                self.loads.loaded.insert(name.to_string());
                self.loads.order.push(name.to_string());
            }
            ok
        }
    }
    let mut loader = Loader {
        db,
        pool: Pool::new(),
        done: HashMap::default(),
        reasons: HashMap::default(),
        loads: PoolLoads {
            refused: BTreeMap::new(),
            loaded: HashSet::default(),
            order: Vec::new(),
            pool: Pool::new(),
        },
    };
    for name in summoned {
        if !loader.load(name) {
            let reason = loader.reasons.get(name).cloned().unwrap_or_default();
            loader.loads.refused.insert(name.clone(), reason);
        }
    }
    // Complete: leaked, so that the render threads share it without a
    // count or a lock (spec 0442 S2); adding to it now panics.
    loader.loads.pool = loader.pool.leak();
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
/// they are once their file is loaded, if the rendering file's closure
/// declares them (spec 0429 S14).
pub struct OptionsPool {
    pool: Pool,
    /// The package of the variant's descriptor.proto (`ctx.options_package`).
    pub package: String,
}

impl OptionsPool {
    /// The pool, or why the variant's descriptor.proto could not be built
    /// into it: no option can then be read (`DescriptorProtoUnbuildableError`).
    pub fn new(db: &Database, variant: &Variant, loads: &PoolLoads) -> Result<OptionsPool, String> {
        let fdp = db
            .file_by_name(&variant.descriptor_proto)
            .ok_or_else(|| format!("{} is not loaded", variant.descriptor_proto))?;
        let bytes = without_map_entry(fdp).ok_or("it cannot be encoded")?;
        let pool = Pool::new();
        pool.add_file(&bytes).map_err(|e| e.0)?;
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
        // Complete: leaked, as the pool loads are (spec 0442 S2).
        Ok(OptionsPool {
            pool: pool.leak(),
            package,
        })
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
    /// `--keep-descriptor-path`: names written as the input's (spec 0440).
    pub keep_descriptor: bool,
    pub graph: &'a crate::graph::Graph,
    /// The rendering file's import closure (spec 0429 S14).
    pub closure: &'a crate::hash::HashSet<String>,
    /// Where its warnings go, and its name (W22, spec 0438 S6).
    pub events: &'a std::cell::RefCell<Vec<super::file::RenderEvent>>,
    pub file: String,
    /// The first scope protoc searches for this element's option names,
    /// and what it sees (spec 0439 S1).
    pub option_scope: String,
    pub names: &'a super::names::SymbolTable,
    pub visible: &'a super::names::Visible,
}

impl Scope<'_> {
    /// An extension the rendering file's options see: declared in the
    /// variant's descriptor.proto or in the file's import closure (spec
    /// 0429 S14). Any other is an unknown field, an orphan.
    fn visible(&self, ext: &Field) -> bool {
        let file = ext.parent_file();
        let name = file.name();
        name == self.variant.descriptor_proto || self.closure.contains(name)
    }

    /// `canonize_opt_name` for a custom option, an orphan unless its
    /// extension's node is summoned; named as googleapis names it, from the
    /// element's option scope (spec 0439 S1).
    fn custom_name(&self, ext: &Field) -> (String, bool) {
        let full = format!(".{}", ext.full_name());
        let orphan = !self
            .graph
            .find(&format!("fdsc:{full}"))
            .is_some_and(|n| self.graph.nodes[n].is_summoned);
        let name = self.names.option_name(
            &self.variant.output_symbol(&full, self.keep_descriptor),
            self.visible,
            &self
                .variant
                .output_package(&self.option_scope, &self.file, self.keep_descriptor),
        );
        (format!("({name})"), orphan)
    }
}

/// `canonize_opt_name` for a built-in option: its short name, `None` for
/// `features` and `feature_support` (rendered elsewhere, or not at all);
/// an orphan when the variant lists it. The nine options messages,
/// `OneofOptions` included.
fn builtin_name(variant: &Variant, kind: Options, field: &Field) -> Option<(String, bool)> {
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
    entries.extend(custom_entries(scope, &fields));
    let mut blocks = render_entries(scope, entries, composite, depth)?;
    blocks.extend(unknown_blocks(scope, msg, &fields, composite, depth));
    Ok(blocks)
}

/// A field as the wire holds it, a group's fields included.
enum Wire<'b> {
    Number(u64),
    Bytes(&'b [u8]),
    Group(Vec<(u32, Wire<'b>)>),
}

/// The fields of `b` from `*pos`, up to the end-group marker of `group`
/// when inside one; `None` when not well formed.
fn wire_fields<'b>(
    b: &'b [u8],
    pos: &mut usize,
    group: Option<u32>,
) -> Option<Vec<(u32, Wire<'b>)>> {
    use prototext_upb::wire::read_varint;
    let mut out = Vec::new();
    let fixed = |pos: &mut usize, n: usize| -> Option<u64> {
        let end = pos.checked_add(n).filter(|&e| e <= b.len())?;
        let v = b[*pos..end]
            .iter()
            .rev()
            .fold(0u64, |v, &x| (v << 8) | u64::from(x));
        *pos = end;
        Some(v)
    };
    while *pos < b.len() {
        let tag = read_varint(b, pos)?;
        let number = u32::try_from(tag >> 3).ok()?;
        let value = match tag & 7 {
            0 => Wire::Number(read_varint(b, pos)?),
            1 => Wire::Number(fixed(pos, 8)?),
            5 => Wire::Number(fixed(pos, 4)?),
            2 => {
                let len = usize::try_from(read_varint(b, pos)?).ok()?;
                let end = pos.checked_add(len).filter(|&e| e <= b.len())?;
                let v = &b[*pos..end];
                *pos = end;
                Wire::Bytes(v)
            }
            3 => Wire::Group(wire_fields(b, pos, Some(number))?),
            4 if group == Some(number) => return Some(out),
            _ => return None,
        };
        out.push((number, value));
    }
    group.is_none().then_some(out)
}

/// `_unknown_value`: an unknown field's value, by its wire type alone: a
/// varint or a fixed-width number in decimal, bytes quoted with octal
/// escapes, a group as its fields, `{ <N>: V ... }`, recursively.
fn unknown_value(w: &Wire<'_>) -> String {
    match w {
        Wire::Number(v) => v.to_string(),
        Wire::Bytes(payload) => {
            let mut out = String::from("\"");
            for &b in *payload {
                match b {
                    b'"' | b'\\' => {
                        out.push('\\');
                        out.push(char::from(b));
                    }
                    0x20..=0x7e => out.push(char::from(b)),
                    _ => out.push_str(&format!("\\{b:03o}")),
                }
            }
            out.push('"');
            out
        }
        Wire::Group(fields) if fields.is_empty() => "{ }".to_string(),
        Wire::Group(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(n, v)| format!("<{n}>: {}", unknown_value(v)))
                .collect();
            format!("{{ {} }}", inner.join(" "))
        }
    }
}

/// `unknown_option_blocks`: each field of `msg` its type does not
/// declare, and no extension reproto's pool knows declares, as an orphan,
/// `option <N> = V;` (`<N> = V,` in a composite), with W22, which is said
/// even when `--redact-orphans` removes the line. An extension the pool
/// does not know yet (`Scope::visible`) is one, as it is to reproto.
fn unknown_blocks(
    scope: &Scope<'_>,
    msg: &DynamicMessage,
    fields: &[(Field, Value<'_>)],
    composite: bool,
    depth: usize,
) -> Vec<Block> {
    let known: Vec<u32> = fields
        .iter()
        .filter(|(f, _)| !f.is_extension() || scope.visible(f))
        .map(|(f, _)| f.number())
        .collect();
    let Some(bytes) = msg.encode().ok() else {
        return Vec::new();
    };
    // Not well formed, upb would not have parsed it: nothing is unknown.
    let Some(records) = wire_fields(&bytes, &mut 0, None) else {
        return Vec::new();
    };
    let type_name = msg.descriptor().full_name().to_string();
    let mut blocks = Vec::new();
    for (number, value) in records.iter().filter(|(n, _)| !known.contains(n)) {
        let text = format!("<{number}> = {}", unknown_value(value));
        let text = if composite {
            format!("{text},")
        } else {
            format!("option {text};")
        };
        let mut block = Block::new();
        block.push(Line::orphan(text, depth));
        blocks.push(block);
        scope
            .events
            .borrow_mut()
            .push(super::file::RenderEvent::Squash(
                "W22",
                format!(
                    "'{}': option field {number} of {type_name} is declared nowhere: \
                 written as an orphan",
                    scope.file
                ),
                serde_json::json!({
                    "file": scope.file, "options": type_name, "number": number,
                }),
            ));
    }
    blocks
}

/// The extensions among `fields` reproto's options pool knows, by number,
/// each with its name and whether it is an orphan.
fn custom_entries<'f, 'v>(
    scope: &Scope<'_>,
    fields: &'f [(Field, Value<'v>)],
) -> Vec<(String, bool, &'f Field, &'f Value<'v>)> {
    let mut custom: Vec<&(Field, Value<'_>)> = fields
        .iter()
        .filter(|(f, _)| f.is_extension() && scope.visible(f))
        .collect();
    custom.sort_by_key(|(f, _)| f.number());
    custom
        .into_iter()
        .map(|(field, value)| {
            let (name, orphan) = scope.custom_name(field);
            (name, orphan, field, value)
        })
        .collect()
}

/// Each option a block: `option NAME = V;` standalone, `NAME = V,` in a
/// composite, a repeated one's values one each; an orphan's lines marked.
fn render_entries(
    scope: &Scope<'_>,
    entries: Vec<(String, bool, &Field, &Value<'_>)>,
    composite: bool,
    depth: usize,
) -> Result<Vec<Block>, Unrenderable> {
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

/// `feature_blocks` (spec 0439 S2): the custom features set on `msg` (a
/// FeatureSet read with the options pool), by number, each field of each
/// a path: `option features.(E).f = V;`, `features.(E).f = V,` in a
/// composite; an empty one `features.(E) = {}`. A value protoc's rules
/// exclude (`feature_resolver.cc`: singular enums or bools, no extension)
/// is written as an aggregate, as any message-valued option.
pub fn feature_blocks(
    scope: &Scope<'_>,
    msg: &DynamicMessage,
    composite: bool,
    depth: usize,
) -> Result<Vec<Block>, Unrenderable> {
    let fields = msg.as_ref().set_fields();
    let mut blocks = Vec::new();
    let statement = |text: String, orphan: bool| {
        let text = if composite {
            format!("{text},")
        } else {
            format!("option {text};")
        };
        let kind = if orphan {
            LineType::Orphan
        } else {
            LineType::Code
        };
        let mut block = Block::new();
        block.push(Line::new(text, depth, kind));
        block
    };
    for (name, orphan, field, value) in custom_entries(scope, &fields) {
        let name = format!("features.{name}");
        let flat = match (field.cardinality(), value) {
            (Cardinality::Repeated, _) => None,
            (_, Value::Message(m)) => flat_feature(m.as_ref()),
            _ => None,
        };
        let Some(leaves) = flat else {
            let mut block = dump_option(scope, field, value, &name, depth)?;
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
            continue;
        };
        if leaves.is_empty() {
            blocks.push(statement(format!("{name} = {{}}"), orphan));
        }
        for (leaf, v) in &leaves {
            blocks.push(statement(
                format!("{name}.{} = {}", leaf.name(), scalar(leaf, v)?),
                orphan,
            ));
        }
    }
    Ok(blocks)
}

/// `_is_flat_feature`: a feature's value's fields, when it holds only what
/// protoc admits in one (singular enums or bools, no extension, nothing
/// unknown); `None` otherwise.
fn flat_feature<'m>(m: Option<&MessageRef<'m>>) -> Option<Vec<(Field, Value<'m>)>> {
    let Some(m) = m else {
        return Some(Vec::new());
    };
    let fields = m.set_fields();
    let flat = fields.iter().all(|(f, _)| {
        !f.is_extension()
            && f.cardinality() != Cardinality::Repeated
            && matches!(f.kind(), Kind::Enum(_) | Kind::Bool)
    });
    let known: Vec<u32> = fields.iter().map(|(f, _)| f.number()).collect();
    let bytes = m.encode().ok()?;
    let unknown =
        wire_fields(&bytes, &mut 0, None).is_none_or(|r| r.iter().any(|(n, _)| !known.contains(n)));
    (flat && !unknown).then_some(fields)
}
