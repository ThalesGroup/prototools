// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Comments from the input's SourceCodeInfo (`render_source_code_info_comments`,
//! `source_info.SourceCodeInfoMixin`), stage 5: the file's and the
//! messages'; reproto renders no others.

use prototext_upb::descriptor_access::SourceCodeInfoLocation;

use super::file::Ctx;
use super::text::{Block, Line};
use crate::graph::Kind;

/// A comment's lines (`removesuffix('\n').split('\n')`), each a comment
/// line of the block.
fn lines(out: &mut Block, comment: &[u8], depth: usize) {
    let s = String::from_utf8_lossy(comment);
    let s = s.strip_suffix('\n').unwrap_or(&s);
    for line in s.split('\n') {
        out.push(Line::comment(line, depth));
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The file's own comments: those of the locations whose path is `[]`
/// (the file) or `[12]` (its syntax statement), in order; leading,
/// detached, then trailing, each followed by a blank line.
pub fn file(ctx: &Ctx<'_, '_>, depth: usize) -> Block {
    let mut out = Block::new();
    let Some(f) = ctx.shared.file_def(ctx.file) else {
        return out;
    };
    let Some(sci) = f.source_code_info() else {
        return out;
    };
    for loc in sci.location() {
        let path = loc.path();
        if !(path.is_empty() || path == [12]) {
            continue;
        }
        if !loc.leading_comments().is_empty() {
            lines(&mut out, loc.leading_comments(), depth);
            out.push(Line::code("", depth));
        }
        for detached in loc.leading_detached_comments() {
            lines(&mut out, detached, depth);
            out.push(Line::code("", depth));
        }
        if !loc.trailing_comments().is_empty() {
            lines(&mut out, loc.trailing_comments(), depth);
            out.push(Line::code("", depth));
        }
    }
    out
}

/// A message's source path (`_calculate_source_code_info_path`): from its
/// file down, `[4, i]` for a top-level message, `[3, j]` for each nested
/// one, each found by its definition's name in its parent's (the first of
/// that name); `None` where a name is not found.
fn message_path(ctx: &Ctx<'_, '_>, msg: usize) -> Option<Vec<i32>> {
    let shared = ctx.shared;
    let mut segments: Vec<[i32; 2]> = Vec::new();
    let mut current = msg;
    while let Some(parent) = shared.graph.nodes[current].parent {
        let name = shared.message_def(current).map(|m| text(m.name()))?;
        match shared.graph.nodes[parent].kind {
            Kind::File => {
                let f = shared.file_def(parent)?;
                let i = f
                    .message_type()
                    .iter()
                    .position(|m| text(m.name()) == name)?;
                segments.insert(0, [4, i as i32]);
                break;
            }
            Kind::Message => {
                let m = shared.message_def(parent)?;
                let j = m
                    .nested_type()
                    .iter()
                    .position(|n| text(n.name()) == name)?;
                segments.insert(0, [3, j as i32]);
            }
            _ => {}
        }
        current = parent;
    }
    let flat: Vec<i32> = segments.into_iter().flatten().collect();
    (!flat.is_empty()).then_some(flat)
}

/// A message's comments (`render_message_comments`): those of the first
/// location whose path is the message's own, leading, detached, then
/// trailing, with no blank line between.
pub fn message(ctx: &Ctx<'_, '_>, msg: usize, depth: usize) -> Block {
    let mut out = Block::new();
    let file = ctx.shared.file_of(msg);
    let Some(f) = ctx.shared.file_def(file) else {
        return out;
    };
    let Some(sci) = f.source_code_info() else {
        return out;
    };
    let Some(path) = message_path(ctx, msg) else {
        return out;
    };
    let found: Option<SourceCodeInfoLocation<'_>> =
        sci.location().into_iter().find(|loc| loc.path() == path);
    if let Some(loc) = found {
        if !loc.leading_comments().is_empty() {
            lines(&mut out, loc.leading_comments(), depth);
        }
        for detached in loc.leading_detached_comments() {
            lines(&mut out, detached, depth);
        }
        if !loc.trailing_comments().is_empty() {
            lines(&mut out, loc.trailing_comments(), depth);
        }
    }
    out
}
