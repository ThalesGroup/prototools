// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A file's text (`ReFileDescriptorProto.render`), and what files and
//! messages share: the per-file context, anomaly comments, extend blocks.

use std::collections::BTreeSet;

use prototext_upb::descriptor_access::FieldDescriptorProto;

use super::names::short_ref;
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
}

impl Ctx<'_, '_> {
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
) -> Block {
    let shared = ctx.shared;
    let mut out = Block::new();
    // Each extension's node, and its extendee's short name, when allowed.
    let mut members: Vec<(usize, String)> = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    for e in extensions {
        let extendee = text(e.extendee());
        if !allow_extend_block(ctx, &extendee) {
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
        let r = short_ref(shared, &format!("message:{node_extendee}"), owner);
        if !refs.contains(&r) {
            refs.push(r.clone());
        }
        members.push((fd, r));
    }
    for r in &refs {
        let mut block = Block::new();
        let mut is_orphan = true;
        for (fd, fr) in &members {
            if fr != r {
                continue;
            }
            let mut blk = super::message::field(ctx, *fd, depth + 1, false);
            if track_orphans {
                if shared.graph.nodes[*fd].is_summoned {
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
        block.insert(
            0,
            Line {
                text: format!("extend {r} {{"),
                level: depth,
                kind,
            },
        );
        block.push(Line {
            text: "}".into(),
            level: depth,
            kind,
        });
        out.extend(block);
    }
    out
}

/// `ReFileDescriptorProto.render`, stage 1: the header, syntax, package,
/// imports, then services, enums, messages and extensions.
pub fn render(shared: &Shared<'_>, file: usize) -> Option<Block> {
    let f = shared.file_def(file)?;
    let syntax = {
        let s = text(f.syntax());
        if s.is_empty() {
            "proto2".to_string()
        } else {
            s
        }
    };
    let target = match syntax.as_str() {
        "proto2" | "proto3" | "editions" => syntax.clone(),
        _ => "proto2".to_string(),
    };
    let ctx = Ctx {
        shared,
        file,
        edition: f.has_edition().then(|| f.edition()),
        syntax,
        target,
    };
    let name = text(f.name());
    let depth = 0;
    let mut out = Block::new();

    out.push(Line::comment(format!(" {name}"), depth));
    out.push(Line::code("", depth));
    out.div_maybe(depth);

    if ctx.editions_target() {
        let e = edition_name(f.edition())?;
        out.push(Line::code(format!("edition = \"{e}\";"), depth));
    } else if ctx.syntax != ctx.target {
        out.push(anomaly(
            "WARNING",
            "downconvert",
            &format!(
                "original file used \"{}\" syntax; rendered as proto2",
                ctx.syntax
            ),
            depth,
        ));
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
        out.push(Line {
            text: format!("{cmd} {dep_name};"),
            level: depth,
            kind,
        });
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

    for s in f.service() {
        let Some(svc) = shared.child(Kind::Service, file, s.name()) else {
            continue;
        };
        if !shared.graph.nodes[svc].is_reachable {
            continue;
        }
        out.extend(super::message::service(&ctx, svc, depth));
    }
    out.div_maybe(depth);

    for e in f.enum_type() {
        let Some(en) = shared.child(Kind::Enum, file, e.name()) else {
            continue;
        };
        if !shared.graph.nodes[en].is_reachable {
            continue;
        }
        out.extend(super::message::enumeration(&ctx, en, depth));
    }
    out.div_maybe(depth);

    for m in f.message_type() {
        let Some(msg) = shared.child(Kind::Message, file, m.name()) else {
            continue;
        };
        let node = &shared.graph.nodes[msg];
        if node.is_group {
            continue;
        }
        let mut lines = super::message::message(&ctx, msg, depth, false);
        lines.insert(
            0,
            Line::code(format!("message {} {{", text(m.name())), depth),
        );
        super::message::collapse_empty(&mut lines);
        lines.div_maybe(depth);
        if !node.is_summoned {
            lines.abandon();
        }
        out.extend(lines);
    }
    out.div_maybe(depth);

    let extensions = f.extension();
    out.extend(extend_blocks(&ctx, file, &extensions, depth, true));
    out.div_maybe(depth);

    out.trim_end(0);
    Some(out)
}
