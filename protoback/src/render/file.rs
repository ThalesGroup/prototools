// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A file's text (`ReFileDescriptorProto.render`), and what files and
//! messages share: the per-file context, anomaly comments, extend blocks.

use std::collections::BTreeSet;

use prototext_upb::descriptor_access::FieldDescriptorProto;

use super::names::{scope_name, type_ref, Mode, Visible};
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
    /// Its index among the summoned files, in render order.
    pub k: usize,
    /// The file's own syntax (`fdp_syntax`: `proto2` when unset).
    pub syntax: String,
    /// The syntax it is written in.
    pub target: String,
    /// The file's edition, for an editions file.
    pub edition: Option<i32>,
    /// reproto's pool refused this file: its elements' options are not
    /// found, and an enum value's lookup fails the file.
    pub refused: bool,
    /// A render step failed that reproto does not catch below the file:
    /// the file is not written.
    pub failed: std::cell::Cell<bool>,
    /// The SourceCodeInfo markers, when a descriptor is written with its
    /// source info (`ctx.out_sci`).
    pub sci: Option<super::sci::Sci>,
    /// What the render reports on stderr, in order (spec 0433 S3): replayed
    /// in file order once the files are rendered.
    pub events: std::cell::RefCell<Vec<RenderEvent>>,
    /// The symbols protoc sees from the file, for the type names written
    /// (spec 0434 S2).
    pub visible: Visible,
}

/// A render's report: an immediate warning, a counted W6, or a file
/// `--force-proto2-for-editions` patched.
#[derive(Debug, Clone)]
pub enum RenderEvent {
    Warning(&'static str, String),
    /// A squashed warning: its code and message (W22).
    Squash(&'static str, String),
    W6 {
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
            graph: self.shared.graph,
            rank: &self.shared.loads.rank,
            k: self.k,
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
        let r = type_ref(ctx, &node_extendee, &scope_name(shared, owner), Mode::All);
        if !refs.contains(&r) {
            refs.push(r.clone());
        }
        members.push((fd, r, paths[i].clone()));
    }
    for r in &refs {
        let mut block = Block::new();
        let mut is_orphan = true;
        for (fd, fr, path) in &members {
            if fr != r {
                continue;
            }
            let mut blk = super::message::field(ctx, *fd, depth + 1, false);
            let summoned = shared.graph.nodes[*fd].is_summoned;
            if let (Some(sci), Some(path), true) = (&ctx.sci, path, !track_orphans || summoned) {
                sci.mark(&mut blk, path.clone());
            }
            if track_orphans {
                if summoned {
                    is_orphan = false;
                } else {
                    blk.abandon();
                }
            } else {
                is_orphan = false;
            }
            block.extend(blk);
        }
        let kind = if track_orphans && is_orphan {
            LineType::Orphan
        } else {
            LineType::Code
        };
        block.insert(0, Line::new(format!("extend {r} {{"), depth, kind));
        block.push(Line::new("}", depth, kind));
        out.extend(block);
    }
    out
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
}

pub fn render(shared: &Shared<'_>, file: usize, k: usize) -> FileRender {
    let Some(f) = shared.file_def(file) else {
        return FileRender {
            output: None,
            events: Vec::new(),
            failed: false,
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
        k,
        edition: f.has_edition().then(|| f.edition()),
        syntax,
        target,
        refused: shared.loads.refused.contains_key(&text(f.name())),
        failed: std::cell::Cell::new(false),
        sci: (shared.wants_descriptor() && shared.source_info).then(super::sci::Sci::default),
        events: std::cell::RefCell::new(Vec::new()),
        visible: shared.names.visible(&text(f.name())),
    };
    // `_set_target_syntax`: a file --force-proto2-for-editions patches.
    if !shared.force_proto2_output && shared.force_proto2_for_editions && ctx.syntax == "editions" {
        ctx.events
            .borrow_mut()
            .push(RenderEvent::Prost(text(f.name())));
    }
    let output = render_body(&ctx, &f);
    FileRender {
        failed: ctx.failed.get(),
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
        let e = edition_name(f.edition())?;
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
        let out_package = if shared.keep_variant_descriptor {
            package
        } else {
            shared.variant.namespace_package(&package)
        };
        out.push(Line::code(format!("package {out_package};"), depth));
        out.push(Line::code("", depth));
    }
    out.div_maybe(depth);

    let options = file_options(ctx, f, depth);
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
            Line::code(format!("message {} {{", text(m.name())), depth),
        );
        super::message::collapse_empty(&mut lines);
        if let (Some(sci), Some(path)) = (&ctx.sci, path) {
            sci.mark(&mut lines, path);
            sci_msg += 1;
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
fn file_options(
    ctx: &Ctx<'_, '_>,
    f: &prototext_upb::descriptor_access::FileDescriptorProto<'_>,
    depth: usize,
) -> Block {
    use super::options::{options, Options};
    const POOL_REFUSED: &str = "the descriptor pool refused this file or one of its imports";
    let a3 = |reason: &str| {
        ctx.w6(ctx.file_name(), String::new(), reason);
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
        // reproto's `debug_exception`: the pool's own reason behind the
        // fixed one, under --debug.
        if ctx.shared.debug {
            if let Some(reason) = ctx.shared.loads.refused.get(&ctx.file_name()) {
                crate::console::warning(&format!("'{}': file options: {reason}", ctx.file_name()));
            }
        }
        return a3(POOL_REFUSED);
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
        Err(_) => return a3("internal error"),
    }
    out
}
