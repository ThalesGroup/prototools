// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A file's text (`ReFileDescriptorProto.render`), and what files and
//! messages share: the per-file context, anomaly comments, extend blocks.

use std::collections::BTreeSet;

use prototext_upb::descriptor_access::FieldDescriptorProto;

use super::names::{scope_name, Visible};
use super::pyfmt::{edition_name, json_dumps};
use super::text::{Block, Line, LineType};
use super::{text, Shared};
use crate::graph::Kind;

/// What reproto keeps on its `Context` for the file being rendered
/// (`current_file`, `syntax`, `target_syntax`), one per file (spec 0429 S4).
pub struct Ctx<'s, 'a> {
    pub shared: &'s Shared<'a>,
    /// The file node.
    pub file: usize,
    /// The file's own syntax (`fdp_syntax`: `proto2` when unset).
    pub syntax: String,
    /// The syntax it is written in.
    pub target: String,
    /// The file's edition, for an editions file.
    pub edition: Option<i32>,
    /// reproto's pool refused this file: its file options are written as
    /// A3 (W19); its elements' come from their own descriptors (spec 0429
    /// S15).
    pub refused: bool,
    /// A render step failed that reproto does not catch below the file:
    /// the file is not written.
    pub failed: std::cell::Cell<bool>,
    /// Why, when the step that failed says (spec 0457 S3); else phase 7's
    /// W6 says the pool's reason, or internal error.
    pub failure: std::cell::RefCell<Option<String>>,
    /// The SourceCodeInfo markers, when a descriptor is written with its
    /// source info (`ctx.out_sci`).
    pub sci: Option<super::sci::Sci>,
    /// What the render reports on stderr, in order (spec 0433 S3): replayed
    /// in file order once the files are rendered.
    pub events: std::cell::RefCell<Vec<RenderEvent>>,
    /// The symbols protoc sees from the file, for the type names written
    /// (spec 0434 S2).
    pub visible: Visible,
    /// The file and its imports, transitively: whose extensions its
    /// options see (spec 0429 S14).
    pub closure: crate::hash::HashSet<String>,
}

/// A render's report: an immediate warning, a counted W6, or a file
/// `--force-proto2-for-editions` patched.
#[derive(Debug, Clone)]
pub enum RenderEvent {
    Warning(&'static str, String),
    /// A squashed warning: its code and message (W22).
    /// Its fields in the report (spec 0446 S3), after its message.
    Squash(&'static str, String, serde_json::Value),
    W6 {
        /// W6, or W19 for a file the pool refused.
        code: &'static str,
        file: String,
        field: String,
        reason: String,
    },
    Prost(String),
}

impl Ctx<'_, '_> {
    /// The file's name.
    pub fn file_name(&self) -> String {
        self.shared.graph.nodes[self.file]
            .fqdn
            .strip_prefix("file:")
            .unwrap_or_default()
            .to_string()
    }

    /// An anomaly's stderr line (`anomalies.report`, `warn`), under its
    /// code (A1 to D2).
    pub fn warn(&self, code: &'static str, message: String) {
        self.events
            .borrow_mut()
            .push(RenderEvent::Warning(code, message));
    }

    /// A3 or C5: a W6, counted.
    pub fn w6(&self, file: String, field: String, reason: &str) {
        self.events.borrow_mut().push(RenderEvent::W6 {
            code: "W6",
            file,
            field,
            reason: reason.to_string(),
        });
    }

    /// The first scope protoc searches for the option names of node `i`
    /// (spec 0439 S1): `name_scope` is the element's full name, of which
    /// `LookupSymbol` drops the last component first: its parent's scope.
    pub fn option_scope(&self, i: usize) -> String {
        let parent = self.shared.graph.nodes[i].parent.unwrap_or(self.file);
        super::names::scope_name(self.shared, parent)
    }

    /// What an option's rendering reads, for this file, for an element
    /// whose option names are looked up from `option_scope`.
    pub fn scope(&self, option_scope: &str) -> super::options::Scope<'_> {
        super::options::Scope {
            variant: self.shared.variant,
            keep_descriptor: self.shared.keep_variant_descriptor,
            graph: self.shared.graph,
            closure: &self.closure,
            events: &self.events,
            file: self.file_name(),
            option_scope: option_scope.to_string(),
            names: self.shared.names,
            visible: &self.visible,
        }
    }

    /// Whether reproto's pool has node `i`: not when it is, or is under, a
    /// synthetic group (`--force-proto2-output`), which only the graph has.
    pub fn in_pool(&self, mut i: usize) -> bool {
        loop {
            let n = &self.shared.graph.nodes[i];
            if n.synthetic_group {
                return false;
            }
            match n.parent {
                Some(p) => i = p,
                None => return true,
            }
        }
    }

    pub fn proto3(&self) -> bool {
        self.target == "proto3"
    }

    pub fn editions_target(&self) -> bool {
        self.target == "editions"
    }

    /// `proto2` or `editions` (`allow_groups`, `allow_extension_ranges`,
    /// `allow_weak_import`, `allow_message_set_wire_format`).
    pub fn proto2_or_editions(&self) -> bool {
        self.target == "proto2" || self.target == "editions"
    }
}

/// `render_features_block`: an element's explicit features, in an
/// editions file written as editions only: `option features.x = V;`
/// standalone, `features.x = V,` inline; the six runtime features, in
/// that order, their values named by the edition defaults' tables.
/// [`visibility_keyword`] for symbol `node`, under `--export-used` (spec
/// 0458 S4, S5): `export ` for a local symbol another file uses, W31; a
/// nested one under `STRICT` unchanged, W32. [`export_comment`] is the
/// line said before it.
pub fn symbol_visibility(
    ctx: &Ctx<'_, '_>,
    node: usize,
    set: bool,
    visibility: i32,
) -> &'static str {
    let Some((symbol, e)) = exported(ctx, node) else {
        return visibility_keyword(ctx, set, visibility);
    };
    let file = ctx.file_name();
    let n = e.users.len();
    let count = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    let fields = serde_json::json!({ "file": file, "symbol": symbol, "users": e.users });
    let (code, message) = if e.exportable {
        (
            "W31",
            format!("'{file}': {symbol} written export: used by {count} (--export-used)"),
        )
    } else {
        (
            "W32",
            format!(
                "'{file}': {symbol} is used by {count}, but STRICT visibility can't export \
                 a nested symbol (--export-used)"
            ),
        )
    };
    ctx.events
        .borrow_mut()
        .push(RenderEvent::Squash(code, message, fields));
    if e.exportable {
        "export "
    } else {
        visibility_keyword(ctx, set, visibility)
    }
}

/// `--repair-skew`: `node` (a field, method or extension) commented out
/// (spec 0469 S3, S4): its lines orphans, the comment before them, the
/// warning. Whether it was.
pub fn repaired_out(ctx: &Ctx<'_, '_>, node: usize, depth: usize, lines: &mut Block) -> bool {
    let Some(o) = ctx.shared.repairs.and_then(|r| r.orphans.get(&node)) else {
        return false;
    };
    lines.abandon();
    lines.insert(0, anomaly("WARNING", "repair-skew", &o.comment, depth));
    ctx.events.borrow_mut().push(RenderEvent::Squash(
        o.code,
        format!("'{}': {}", ctx.file_name(), o.message),
        o.fields.clone(),
    ));
    true
}

/// The comment before a symbol `--export-used` writes `export` (spec 0458
/// S4); inserted after the symbol's SourceCodeInfo is marked, so that its
/// span starts at the symbol, as protoc's does.
pub fn export_comment(ctx: &Ctx<'_, '_>, node: usize, depth: usize) -> Option<Line> {
    let (_, e) = exported(ctx, node).filter(|(_, e)| e.exportable)?;
    let first = e.users.first()?;
    let more = match e.users.len() {
        1 => String::new(),
        n => format!(" (and {} more)", n - 1),
    };
    Some(anomaly(
        "WARNING",
        "export-used",
        &format!("exported: used by '{first}'{more}; local in the source"),
        depth,
    ))
}

/// Symbol `node`'s entry under `--export-used`, in a file written as
/// editions: its full name, and who uses it.
pub(super) fn exported<'s>(
    ctx: &Ctx<'s, '_>,
    node: usize,
) -> Option<(&'s str, &'s super::export::Export)> {
    let symbol = ctx.shared.graph.nodes[node].prefix.trim_start_matches('.');
    let e = ctx.shared.exports?.get(symbol)?;
    ctx.editions_target().then_some((symbol, e))
}

/// reproto's `visibility_prefix`: `local ` or `export ` before a message or
/// an enum whose visibility (edition 2024) is set, whatever the file's
/// default, so that it roundtrips; nothing in a file written as proto2 or
/// proto3, which has no visibility.
fn visibility_keyword(ctx: &Ctx<'_, '_>, set: bool, visibility: i32) -> &'static str {
    // descriptor.proto's SymbolVisibility.
    const LOCAL: i32 = 1;
    const EXPORT: i32 = 2;
    match (ctx.editions_target() && set, visibility) {
        (true, LOCAL) => "local ",
        (true, EXPORT) => "export ",
        _ => "",
    }
}

pub fn features_block(
    ctx: &Ctx<'_, '_>,
    fs: Option<prototext_upb::descriptor_access::FeatureSet<'_>>,
    depth: usize,
    inline: bool,
    scope: &str,
) -> Block {
    let mut out = Block::new();
    let Some(fs) = fs else {
        return out;
    };
    if !ctx.editions_target() {
        return out;
    }
    // Present but empty: written so, that it roundtrips (`features = {}`).
    if fs.0.encode().is_ok_and(|b| b.is_empty()) {
        out.push(Line::code(
            if inline {
                "features = {},"
            } else {
                "option features = {};"
            },
            depth,
        ));
        return out;
    }
    let set: [(&str, Option<i32>); 6] = [
        (
            "field_presence",
            fs.has_field_presence().then(|| fs.field_presence()),
        ),
        ("enum_type", fs.has_enum_type().then(|| fs.enum_type())),
        (
            "repeated_field_encoding",
            fs.has_repeated_field_encoding()
                .then(|| fs.repeated_field_encoding()),
        ),
        (
            "utf8_validation",
            fs.has_utf8_validation().then(|| fs.utf8_validation()),
        ),
        (
            "message_encoding",
            fs.has_message_encoding().then(|| fs.message_encoding()),
        ),
        (
            "json_format",
            fs.has_json_format().then(|| fs.json_format()),
        ),
    ];
    for (name, value) in set {
        let Some(v) = value else { continue };
        let value = feature_value_name(ctx.shared.edition_defaults, name, v);
        out.push(Line::code(
            if inline {
                format!("features.{name} = {value},")
            } else {
                format!("option features.{name} = {value};")
            },
            depth,
        ));
    }
    // The custom features, the feature set's extensions (spec 0435): written
    // as custom options are, after `features.`, the feature set read again
    // where their extensions are known.
    let custom = ctx
        .shared
        .options
        .parse_ref(super::options::Options::FeatureSet, &fs.0)
        .and_then(|msg| {
            super::options::feature_blocks(&ctx.scope(scope), &msg, inline, depth).ok()
        });
    for block in custom.into_iter().flatten() {
        out.extend(block);
    }
    out
}

/// `feature_value_name`: a feature value's name in its enum (the edition
/// defaults' `_enum_` table), not an `_UNKNOWN` one unless the value is
/// 0; its number when the table has none.
fn feature_value_name(table: &crate::phase2::EditionDefaults, feature: &str, value: i32) -> String {
    use crate::phase2::EditionDefault;
    let Some(EditionDefault::Numbers(numbers)) = table.get(&format!("_enum_{feature}")) else {
        return value.to_string();
    };
    numbers
        .iter()
        .find(|(name, &n)| n == value && !name.ends_with("_UNKNOWN") && n != 0)
        .or_else(|| numbers.iter().find(|(_, &n)| n == value))
        .map_or_else(|| value.to_string(), |(name, _)| name.clone())
}

/// `anomalies.report`'s comment line: ` SEVERITY[tag]: body`.
pub fn anomaly(severity: &str, tag: &str, body: &str, depth: usize) -> Line {
    Line::comment(format!(" {severity}[{tag}]: {body}"), depth)
}

/// `allow_extend_block`: in proto3, only the nine `*Options` (spec 0429
/// S18); an extendee the input lacks when its name is one of them.
fn allow_extend_block(ctx: &Ctx<'_, '_>, extendee: &str) -> bool {
    if ctx.proto2_or_editions() {
        return true;
    }
    let shared = ctx.shared;
    if shared
        .options_fqns
        .contains(&shared.variant.namespace(extendee))
    {
        return true;
    }
    if let Some(n) = shared.graph.find(&format!("desc:{extendee}")) {
        if shared.graph.nodes[n].present {
            return false;
        }
    }
    let last = extendee.rsplit('.').next().unwrap_or(extendee);
    OPTIONS_TYPES.contains(&last)
}

/// The nine options messages a proto3 file may extend.
pub const OPTIONS_TYPES: [&str; 9] = [
    "FileOptions",
    "MessageOptions",
    "FieldOptions",
    "OneofOptions",
    "ExtensionRangeOptions",
    "EnumOptions",
    "EnumValueOptions",
    "ServiceOptions",
    "MethodOptions",
];

/// `descriptor_options_fqns`: the `.google.protobuf.*Options`, and those
/// of every descriptor.proto among the pool's files (a file declaring
/// `FileDescriptorProto`), each also under the variant's rewritten name.
pub fn options_fqns(
    shared_db: &crate::db::Database,
    variant: &crate::variant::Variant,
) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = OPTIONS_TYPES
        .iter()
        .map(|t| format!(".google.protobuf.{t}"))
        .collect();
    for fdp in &shared_db.files {
        let f = prototext_upb::descriptor_access::FileDescriptorProto(fdp.as_ref());
        let top: BTreeSet<String> = f.message_type().iter().map(|m| text(m.name())).collect();
        if !top.contains("FileDescriptorProto") {
            continue;
        }
        let package = text(f.package());
        let prefix = if package.is_empty() {
            ".".to_string()
        } else {
            format!(".{package}.")
        };
        for t in OPTIONS_TYPES {
            if top.contains(t) {
                let name = format!("{prefix}{t}");
                names.insert(variant.namespace(&name));
                names.insert(name);
            }
        }
    }
    names
}

/// `_render_extend_blocks`: the extensions of `owner` (a file or a
/// message node), one `extend X { … }` per extendee, in the order first
/// met; an extension proto3 refuses is an anomaly instead (A5 for a file,
/// B1 for a message, the same comment). With
/// `track_orphans` (a file), an extension not summoned is an orphan, and
/// so are the brackets when all of a block's are.
pub fn extend_blocks(
    ctx: &Ctx<'_, '_>,
    owner: usize,
    extensions: &[FieldDescriptorProto<'_>],
    depth: usize,
    track_orphans: bool,
    sci_path: Option<&[i32]>,
) -> Block {
    let shared = ctx.shared;
    let mut out = Block::new();
    // Each extension's SourceCodeInfo path (spec 0141): a file's, field 7,
    // counting its summoned extensions only, as its descriptor keeps them;
    // a message's, field 6 under the message's path, counting them all.
    let is_file = shared.graph.nodes[owner].kind == Kind::File;
    let mut paths: Vec<Option<Vec<i32>>> = vec![None; extensions.len()];
    if ctx.sci.is_some() && (is_file || sci_path.is_some()) {
        let prefix = sci_path.unwrap_or(&[]);
        let mut counter = 0;
        for (i, e) in extensions.iter().enumerate() {
            if is_file {
                let summoned = shared
                    .child(Kind::Field, owner, e.name())
                    .is_some_and(|fd| shared.graph.nodes[fd].is_summoned);
                if !summoned {
                    continue;
                }
            }
            let mut p = prefix.to_vec();
            p.extend([if is_file { 7 } else { 6 }, counter]);
            paths[i] = Some(p);
            counter += 1;
        }
    }
    // Each extension's node, its extendee's short name, its path, when
    // allowed.
    let mut members: Vec<(usize, String, Option<Vec<i32>>)> = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    // The extendees whose blocks are redacted (spec 0447 S5).
    let mut redacted: Vec<String> = Vec::new();
    for (i, e) in extensions.iter().enumerate() {
        let extendee = text(e.extendee());
        if !allow_extend_block(ctx, &extendee) {
            let owner_node = &shared.graph.nodes[owner];
            if owner_node.kind == Kind::File {
                ctx.warn("A5", format!(
                    "'{}': top-level extend block for '{extendee}' is not valid in proto3; omitting",
                    ctx.file_name()
                ));
            } else {
                let msg = owner_node.fqdn.rsplit('.').next().unwrap_or_default();
                ctx.warn("B1", format!(
                    "message '{msg}': nested extend block for '{extendee}' is not valid in proto3; omitting"
                ));
            }
            out.push(anomaly(
                "OMITTED",
                "proto3",
                &format!("extend block for '{extendee}' is not valid in proto3"),
                depth,
            ));
            continue;
        }
        let Some(fd) = shared.child(Kind::Field, owner, e.name()) else {
            continue;
        };
        let node_extendee = match shared.def(fd) {
            Some(super::Def::Field(f)) => text(f.extendee()),
            _ => extendee,
        };
        // Looked up among every symbol of the scope the extension is
        // declared in (spec 0434 S3).
        let number = u32::try_from(e.number()).unwrap_or(0);
        let r = super::names::extendee_ref(ctx, &node_extendee, number, &scope_name(shared, owner));
        // Spec 0447 S5, --target portable: an extension protoc's options
        // message cannot declare (no extension range for its number) is
        // redacted, its extend block, under its own extendee, an orphan.
        if shared.variant.redacts_extension(&node_extendee, number) {
            if !redacted.contains(&r) {
                redacted.push(r.clone());
            }
            let name = text(e.name());
            ctx.events.borrow_mut().push(RenderEvent::Squash(
                "W28",
                format!(
                    "'{}': extension {name}: number {number} of {node_extendee} is not in \
                     an extension range of protoc's descriptor: written as an orphan \
                     (--target portable)",
                    ctx.file_name()
                ),
                serde_json::json!({
                    "file": ctx.file_name(), "options": node_extendee, "field": name,
                    "number": number,
                }),
            ));
        }
        if !refs.contains(&r) {
            refs.push(r.clone());
        }
        members.push((fd, r, paths[i].clone()));
    }
    for r in &refs {
        let mut block = Block::new();
        let mut is_orphan = true;
        let redact = redacted.contains(r);
        for (fd, fr, path) in &members {
            if fr != r {
                continue;
            }
            let mut blk = super::message::field(ctx, *fd, depth + 1, false);
            let repaired = repaired_out(ctx, *fd, depth + 1, &mut blk);
            let summoned = shared.graph.nodes[*fd].is_summoned && !repaired;
            if let (Some(sci), Some(path), true) = (&ctx.sci, path, !track_orphans || summoned) {
                sci.mark(&mut blk, path.clone());
            }
            // A repaired one is an orphan already, its comment kept a comment.
            if track_orphans {
                if summoned {
                    is_orphan = false;
                } else if !repaired {
                    blk.abandon();
                }
            } else if !repaired {
                is_orphan = false;
            }
            block.extend(blk);
        }
        // A block with no code line left is an orphan whole: protoc refuses
        // an empty extend block (spec 0459 S1).
        let no_code = !block
            .lines
            .iter()
            .any(|l| l.kind == LineType::Code && !l.text.is_empty());
        let kind = if redact || (track_orphans && is_orphan) || no_code {
            LineType::Orphan
        } else {
            LineType::Code
        };
        if redact {
            block.abandon();
        }
        block.insert(0, Line::new(format!("extend {r} {{"), depth, kind));
        block.push(Line::new("}", depth, kind));
        out.extend(block);
    }
    out
}

/// The file's edition, as its bytes hold it (spec 0457 S1). `Edition` is
/// a closed enum: a value the vendored descriptor.proto doesn't declare is
/// decoded as an unknown field, and `edition` reads unset; the encoding
/// keeps it, so it is read back from there, the last one winning.
pub(super) fn file_edition(f: &prototext_upb::descriptor_access::FileDescriptorProto<'_>) -> i32 {
    if f.has_edition() {
        return f.edition();
    }
    f.0.encode()
        .ok()
        .and_then(|bytes| last_varint_field(&bytes, 14))
        .map_or(f.edition(), |v| v as i32)
}

/// The value of the last top-level varint field `number` of `bytes`, a
/// message's encoding; `None` when there is none, or the bytes don't
/// parse.
fn last_varint_field(bytes: &[u8], number: u64) -> Option<u64> {
    fn varint(b: &[u8], pos: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(*pos)?;
            *pos += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
        }
        None
    }
    let (mut pos, mut found) = (0, None);
    while pos < bytes.len() {
        let tag = varint(bytes, &mut pos)?;
        match tag & 7 {
            0 => {
                let v = varint(bytes, &mut pos)?;
                if tag >> 3 == number {
                    found = Some(v);
                }
            }
            1 => pos += 8,
            2 => pos += usize::try_from(varint(bytes, &mut pos)?).ok()?,
            5 => pos += 4,
            _ => return None,
        }
    }
    (pos == bytes.len()).then_some(found).flatten()
}

/// The edition line's name for `edition` (spec 0457 S2): from the input's
/// descriptor.proto, else the vendored one, else Python protobuf's names;
/// without the `EDITION_` prefix.
fn edition_label(input: &crate::phase2::EditionNames, edition: i32) -> Option<String> {
    let strip = |n: &str| n.strip_prefix("EDITION_").unwrap_or(n).to_string();
    if let Some(n) = input.get(&edition) {
        return Some(strip(n));
    }
    if let Some(n) = prototext_upb::Pool::vendored()
        .get_enum_by_name("google.protobuf.Edition")
        .and_then(|e| {
            e.values()
                .find(|v| v.number() == edition)
                .map(|v| strip(v.name()))
        })
    {
        return Some(n);
    }
    edition_name(edition).map(str::to_string)
}

/// `ReFileDescriptorProto.render`, stage 1: the header, syntax, package,
/// imports, then services, enums, messages and extensions.
/// A file rendered (spec 0433 S3): its text and descriptor, `None` when
/// it is not written; what it reports, in order; whether a render step
/// failed (reproto's phase 7 then counts a W6).
pub struct FileRender {
    pub output: Option<(Block, Option<Vec<u8>>)>,
    pub events: Vec<RenderEvent>,
    pub failed: bool,
    /// The failed step's reason, when it gave one.
    pub reason: Option<String>,
}

pub fn render(shared: &Shared<'_>, file: usize) -> FileRender {
    let Some(f) = shared.file_def(file) else {
        // No output always reports (spec 0457 S4).
        return FileRender {
            output: None,
            events: Vec::new(),
            failed: true,
            reason: None,
        };
    };
    let syntax = {
        let s = text(f.syntax());
        if s.is_empty() {
            "proto2".to_string()
        } else {
            s
        }
    };
    let target = if shared.force_proto2_output
        || (shared.force_proto2_for_editions && syntax == "editions")
    {
        "proto2".to_string()
    } else {
        match syntax.as_str() {
            "proto2" | "proto3" | "editions" => syntax.clone(),
            _ => "proto2".to_string(),
        }
    };
    let ctx = Ctx {
        shared,
        file,
        edition: f.has_edition().then(|| f.edition()),
        syntax,
        target,
        refused: shared.loads.refused.contains_key(&text(f.name())),
        failed: std::cell::Cell::new(false),
        failure: std::cell::RefCell::new(None),
        sci: (shared.wants_descriptor() && shared.source_info).then(super::sci::Sci::default),
        events: std::cell::RefCell::new(Vec::new()),
        visible: shared.names.visible(&text(f.name())),
        closure: shared.names.closure(&text(f.name())),
    };
    // `_set_target_syntax`: a file --force-proto2-for-editions patches.
    if !shared.force_proto2_output && shared.force_proto2_for_editions && ctx.syntax == "editions" {
        ctx.events
            .borrow_mut()
            .push(RenderEvent::Prost(text(f.name())));
    }
    let output = render_body(&ctx, &f);
    FileRender {
        // A render with no output always reports (spec 0457 S4).
        failed: ctx.failed.get() || output.is_none(),
        reason: ctx.failure.take(),
        events: ctx.events.take(),
        output,
    }
}

/// The file's text, and its descriptor when one is built; `None` when a
/// render step failed.
fn render_body(
    ctx: &Ctx<'_, '_>,
    f: &prototext_upb::descriptor_access::FileDescriptorProto<'_>,
) -> Option<(Block, Option<Vec<u8>>)> {
    let shared = ctx.shared;
    let file = ctx.file;
    let name = text(f.name());
    let depth = 0;
    let mut out = Block::new();

    out.push(Line::comment(format!(" {name}"), depth));
    out.push(Line::code("", depth));
    out.div_maybe(depth);

    let comments = super::comments::file(ctx, depth);
    if !comments.is_empty() {
        out.extend(comments);
        out.div_maybe(depth);
    }

    if ctx.editions_target() {
        let edition = file_edition(f);
        let Some(e) = edition_label(ctx.shared.edition_names, edition) else {
            ctx.failed.set(true);
            *ctx.failure.borrow_mut() = Some(format!(
                "edition {edition} has no name in its descriptor.proto"
            ));
            return None;
        };
        out.push(Line::code(format!("edition = \"{e}\";"), depth));
    } else if ctx.syntax != ctx.target {
        // A1: an editions file; A2: any other syntax. Neither is printed:
        // both options are explicit opt-ins, and the comment says it
        // (reproto's rule since 0ef3aa2, for A2, applied to A1 too).
        let comment = if ctx.syntax == "editions" {
            anomaly(
                "WARNING",
                "editions",
                "editions file rendered as proto2 (--force-proto2-output)",
                depth,
            )
        } else {
            anomaly(
                "WARNING",
                "downconvert",
                &format!(
                    "original file used \"{}\" syntax; rendered as proto2",
                    ctx.syntax
                ),
                depth,
            )
        };
        out.push(comment);
        out.push(Line::code(format!("syntax = \"{}\";", ctx.target), depth));
    } else {
        out.push(Line::code(format!("syntax = \"{}\";", ctx.target), depth));
    }
    out.div_maybe(depth);

    let package = text(f.package());
    if !package.is_empty() {
        let out_package = shared.variant.output_package(
            &package,
            &text(f.name()),
            shared.keep_variant_descriptor,
        );
        out.push(Line::code(format!("package {out_package};"), depth));
        out.push(Line::code("", depth));
    }
    out.div_maybe(depth);

    let mut options = file_options(ctx, f, depth);
    options.extend(legacy_naming_style(ctx, f, depth));
    if !options.is_empty() {
        out.extend(options);
        out.div_maybe(depth);
    }

    let public: BTreeSet<i32> = f.public_dependency().into_iter().collect();
    let weak: BTreeSet<i32> = f.weak_dependency().into_iter().collect();
    for (index, d) in f.dependency().iter().enumerate() {
        let d = text(d);
        // A dependency phase 3 met has its node; reproto makes a stub for
        // any other, written as an orphan, by its FQDN.
        let Some(dep) = shared.graph.find(&format!("file:{d}")) else {
            out.push(Line::orphan(format!("import file:{d};"), depth));
            continue;
        };
        let node = &shared.graph.nodes[dep];
        let kind = if node.is_reachable || node.is_summoned {
            LineType::Code
        } else {
            LineType::Orphan
        };
        let index = index as i32;
        let cmd = if public.contains(&index) {
            "import public"
        } else if weak.contains(&index) {
            if ctx.proto2_or_editions() {
                "import weak"
            } else {
                ctx.warn("A4", format!(
                    "'{}': 'import weak' is not valid in proto3; rendering as plain import: \"{d}\"",
                    ctx.file_name()
                ));
                out.push(anomaly(
                    "WARNING",
                    "proto3",
                    &format!(
                        "'import weak \"{d}\"' is not valid in proto3; rendered as plain import"
                    ),
                    depth,
                ));
                "import"
            }
        } else {
            "import"
        };
        let dep_name = if node.is_summoned && node.present {
            let dep_file = node.fqdn.strip_prefix("file:").unwrap_or_default();
            json_dumps(
                &shared
                    .variant
                    .canonize_dependency(dep_file, shared.keep_variant_descriptor),
            )
        } else {
            node.fqdn.clone()
        };
        out.push(Line::new(format!("{cmd} {dep_name};"), depth, kind));
    }
    // Edition 2024's `import option`s, after the ordinary imports, as
    // protoc prints them; a plain import in a file written as proto2.
    let imported: Vec<&[u8]> = f.dependency();
    for d in f.option_dependency() {
        if imported.contains(&d) {
            continue;
        }
        let d = text(d);
        let cmd = if ctx.editions_target() {
            "import option"
        } else {
            "import"
        };
        let Some(dep) = shared.graph.find(&format!("file:{d}")) else {
            out.push(Line::orphan(format!("{cmd} file:{d};"), depth));
            continue;
        };
        let node = &shared.graph.nodes[dep];
        let kind = if node.is_reachable || node.is_summoned {
            LineType::Code
        } else {
            LineType::Orphan
        };
        let dep_name = if node.is_summoned && node.present {
            let dep_file = node.fqdn.strip_prefix("file:").unwrap_or_default();
            json_dumps(
                &shared
                    .variant
                    .canonize_dependency(dep_file, shared.keep_variant_descriptor),
            )
        } else {
            node.fqdn.clone()
        };
        out.push(Line::new(format!("{cmd} {dep_name};"), depth, kind));
    }
    // --repair-skew (spec 0469 S2, S5): the imports added, each said; the
    // types it could not choose a file for.
    let file_name = ctx.file_name();
    if let Some(added) = shared.repairs.and_then(|r| r.imports.get(&file_name)) {
        for (dep, symbols) in added {
            let list: Vec<&str> = symbols.iter().map(String::as_str).collect();
            out.push(anomaly(
                "WARNING",
                "repair-skew",
                &format!(
                    "import added: defines {}, used here; not imported in the source",
                    list.join(", ")
                ),
                depth,
            ));
            let shown = shared
                .variant
                .canonize_dependency(dep, shared.keep_variant_descriptor);
            out.push(Line::code(format!("import {};", json_dumps(&shown)), depth));
            ctx.events.borrow_mut().push(RenderEvent::Squash(
                "W33",
                format!(
                    "'{file_name}': import \"{dep}\" added: it defines {} (--repair-skew)",
                    list.join(", ")
                ),
                serde_json::json!({ "file": file_name, "import": dep, "symbols": list }),
            ));
        }
    }
    if let Some(ambiguous) = shared.repairs.and_then(|r| r.ambiguous.get(&file_name)) {
        for (symbol, defining) in ambiguous {
            ctx.events.borrow_mut().push(RenderEvent::Squash(
                "W36",
                format!(
                    "'{file_name}': {symbol} is defined by {}, none imported: not repaired \
                     (--repair-skew)",
                    defining.join(", ")
                ),
                serde_json::json!({ "file": file_name, "symbol": symbol, "defined_in": defining }),
            ));
        }
    }
    let topo_file = shared.topo_file(&name);
    if let Some(tf) = topo_file {
        for dep in &tf.stripped_dependencies {
            out.push(Line::orphan(format!("import \"{dep}\";"), depth));
        }
        for dep in &tf.stripped_public_dependencies {
            out.push(Line::orphan(format!("import public \"{dep}\";"), depth));
        }
        if !tf.stripped_dependencies.is_empty() || !tf.stripped_public_dependencies.is_empty() {
            out.div_maybe(depth);
        }
    }
    out.div_maybe(depth);

    // The SourceCodeInfo paths count what the descriptor keeps (spec
    // 0141): visible services, reachable enums, summoned messages.
    let mut sci_svc = 0;
    for s in f.service() {
        let Some(svc) = shared.child(Kind::Service, file, s.name()) else {
            continue;
        };
        if !shared.graph.nodes[svc].is_reachable {
            continue;
        }
        let path = ctx.sci.as_ref().map(|_| vec![6, sci_svc]);
        let mut lines = super::message::service(ctx, svc, depth, path.as_deref());
        if let (Some(sci), Some(path)) = (&ctx.sci, path) {
            sci.mark(&mut lines, path);
        }
        sci_svc += 1;
        out.extend(lines);
    }
    out.div_maybe(depth);

    let mut sci_enum = 0;
    for e in f.enum_type() {
        let Some(en) = shared.child(Kind::Enum, file, e.name()) else {
            continue;
        };
        if !shared.graph.nodes[en].is_reachable {
            continue;
        }
        let path = ctx.sci.as_ref().map(|_| vec![5, sci_enum]);
        let mut lines = super::message::enumeration(ctx, en, depth, path.as_deref());
        if let (Some(sci), Some(path)) = (&ctx.sci, path) {
            sci.mark(&mut lines, path);
        }
        if let Some(c) = export_comment(ctx, en, depth) {
            lines.insert(0, c);
        }
        sci_enum += 1;
        out.extend(lines);
    }
    out.div_maybe(depth);

    let mut sci_msg = 0;
    for m in f.message_type() {
        let Some(msg) = shared.child(Kind::Message, file, m.name()) else {
            continue;
        };
        let node = &shared.graph.nodes[msg];
        if node.is_group {
            continue;
        }
        let path = (ctx.sci.is_some() && node.is_summoned).then(|| vec![4, sci_msg]);
        let mut lines = super::message::message(ctx, msg, depth, false, path.as_deref());
        lines.insert(
            0,
            Line::code(
                format!(
                    "{}message {} {{",
                    symbol_visibility(ctx, msg, m.has_visibility(), m.visibility()),
                    text(m.name())
                ),
                depth,
            ),
        );
        super::message::collapse_empty(&mut lines);
        if let (Some(sci), Some(path)) = (&ctx.sci, path) {
            sci.mark(&mut lines, path);
            sci_msg += 1;
        }
        if let Some(c) = export_comment(ctx, msg, depth) {
            lines.insert(0, c);
        }
        lines.div_maybe(depth);
        if !node.is_summoned {
            lines.abandon();
        }
        out.extend(lines);
    }
    out.div_maybe(depth);

    let extensions = f.extension();
    out.extend(extend_blocks(ctx, file, &extensions, depth, true, None));
    out.div_maybe(depth);

    out.trim_end(0);
    if ctx.failed.get() {
        return None;
    }
    // The descriptor, with the SourceCodeInfo the text's markers resolve
    // to (spec 0429 S10).
    let descriptor = if shared.wants_descriptor() {
        let locations = ctx.sci.as_ref().map_or_else(Vec::new, |sci| {
            sci.resolve(&out, shared.redact_comments, shared.redact_orphans)
        });
        super::binary::file(ctx, &locations)
    } else {
        None
    };
    Some((out, descriptor))
}

/// `render_file_options`: the file's options, standalone; A3 when the pool
/// refused the file, or when they cannot be rendered.
/// Spec 0447, --target portable: protoc checks names from edition 2024
/// on, a check a vendor's compiler may not have made, and no option of
/// protoc's turns it off. An edition 2024 (or later) file written as
/// editions, not setting `enforce_naming_style` itself, is written with
/// `option features.enforce_naming_style = STYLE_LEGACY;` (W30). Its
/// descriptor keeps the input's options.
fn legacy_naming_style(
    ctx: &Ctx<'_, '_>,
    f: &prototext_upb::descriptor_access::FileDescriptorProto<'_>,
    depth: usize,
) -> Block {
    // descriptor.proto's EDITION_2024.
    const EDITION_2024: i32 = 1001;
    let mut out = Block::new();
    let sets_it = f
        .options()
        .and_then(|o| o.features())
        .is_some_and(|fs| fs.has_enforce_naming_style());
    if ctx.shared.variant.portable.is_none()
        || !ctx.editions_target()
        || ctx.edition.is_none_or(|e| e < EDITION_2024)
        || sets_it
    {
        return out;
    }
    out.push(Line::code(
        "option features.enforce_naming_style = STYLE_LEGACY;",
        depth,
    ));
    let file = ctx.file_name();
    ctx.events.borrow_mut().push(RenderEvent::Squash(
        "W30",
        format!(
            "'{file}': option features.enforce_naming_style = STYLE_LEGACY added: \
             protoc checks names from edition 2024 on (--target portable)"
        ),
        serde_json::json!({ "file": file }),
    ));
    out
}

fn file_options(
    ctx: &Ctx<'_, '_>,
    f: &prototext_upb::descriptor_access::FileDescriptorProto<'_>,
    depth: usize,
) -> Block {
    use super::options::{options, Options};
    let a3 = |code: &'static str, reason: &str| {
        ctx.events.borrow_mut().push(RenderEvent::W6 {
            code,
            file: ctx.file_name(),
            field: String::new(),
            reason: reason.to_string(),
        });
        let mut b = Block::new();
        b.push(anomaly(
            "OMITTED",
            "render",
            &format!("file options could not be rendered ({reason})"),
            depth,
        ));
        b
    };
    if ctx.refused {
        let reason = super::failure_reason(ctx.shared.loads, &ctx.file_name());
        // reproto's `debug_exception`, under --debug.
        if ctx.shared.debug {
            crate::console::warning(&format!("'{}': file options: {reason}", ctx.file_name()));
        }
        // W19, with the pool's reason (spec 0438 S8).
        return a3("W19", &reason);
    }
    let mut out = Block::new();
    let Some(o) = f.options() else {
        return out;
    };
    let Some(msg) = ctx.shared.options.parse_ref(Options::File, &o.0) else {
        return out;
    };
    // A file's option names are looked up from its package (spec 0439 S1).
    let package = super::names::scope_name(ctx.shared, ctx.file);
    out.extend(features_block(ctx, o.features(), depth, false, &package));
    match options(&ctx.scope(&package), Options::File, &msg, false, depth, &[]) {
        Ok(blocks) => blocks.into_iter().for_each(|b| out.extend(b)),
        Err(_) => return a3("W6", "internal error"),
    }
    out
}
