// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Reading a built `App` back: what shapes its tree holds, and which of
//! them own which lines.
//!
//! Everything here takes an `App` and returns a description of it.
//! Nothing here builds one — that is what the other `support_*` siblings
//! do.

use super::super::*;

/// A node's identity by *content*, not by arena index.
///
/// Spec 0216 freezes the arena's numbering, so a raw `app.tree` index
/// would now survive a re-splice — but that is not what these assertions
/// are about. They are about what the reader is shown, which an index
/// does not say: a failure would print two integers naming no field, and
/// a pass would prove only that the arena did not move, which is the
/// arena's own invariant and is guarded elsewhere.
pub(super) type Shape = (usize, u64, Option<String>, std::ops::Range<usize>);

/// A rendered line's owner: `(line, the owning node's shape, is_footer)`.
pub(super) type LineOwner = (usize, Shape, bool);

/// The projection `live_shapes` and `line_owners` are both written in
/// terms of. Private to this file — a test asserts on a whole list of
/// shapes, never on one node's.
fn shape_of(app: &App, idx: usize) -> Shape {
    let s = &app.tree[idx].span;
    // Spec 0210 S1: the line range is derived from the counters, not
    // read off `span.text_range` — which is the range the node had when
    // the tree was built and is not repaired by a splice.
    // Spec 0212: widened back out here on purpose. `Shape` exists to be
    // printed in an assertion failure, and an `FqdnId(37)` says nothing.
    (
        s.level as usize,
        u64::from(s.field_number),
        app.fqdns.get(s.type_fqdn).map(str::to_owned),
        app.node_lines(idx),
    )
}

/// The first node in the arena whose resolved type is `fqdn`.
///
/// Spec 0212 S6: the name is interned once, here, and the ids compared.
/// Resolving each node's id back to a string inside the closure would need
/// `app.fqdns` borrowed alongside `app.tree` and would turn every one of
/// these iterator chains into an index loop. `id_of`'s miss is
/// `UNINTERNED`, which no span holds, so a name the document never
/// produced finds nothing rather than matching every typeless node.
pub(super) fn node_with_type(app: &App, fqdn: &str) -> Option<usize> {
    let want = app.fqdns.id_of(fqdn);
    app.tree.iter().position(|n| n.span.type_fqdn == want)
}

/// Whether any node in the arena has resolved type `fqdn` — the `any`
/// counterpart of `node_with_type`.
pub(super) fn has_node_with_type(app: &App, fqdn: &str) -> bool {
    node_with_type(app, fqdn).is_some()
}

/// `idx`'s resolved type name, for the assertions that want to *print* it
/// rather than match it.
pub(super) fn type_name_of(app: &App, idx: usize) -> Option<&str> {
    app.fqdns.get(app.tree[idx].span.type_fqdn)
}

/// Every node still reachable from the root, in document order.
pub(super) fn live_shapes(app: &App) -> Vec<Shape> {
    let mut out = Vec::new();
    let mut stack = vec![app.first_node];
    while let Some(i) = stack.pop() {
        out.push(shape_of(app, i));
        let kids: Vec<usize> = (0..app.child_count(i))
            .map(|k| app.nth_child(i, k).expect("k is below the child count"))
            .collect();
        stack.extend(kids.into_iter().rev());
    }
    out
}

/// Every line's owner, projected through `Shape`, and whether the line
/// is that owner's footer.
///
/// Derived through `line_pos` (spec 0210 S2), which is the one path
/// every reader goes through. There is no line-keyed map to read the
/// answer off instead, so "does every line still resolve to the same
/// node" has to be asked of the derivation itself.
pub(super) fn line_owners(app: &App) -> Vec<LineOwner> {
    (0..app.document_lines().len())
        .filter_map(|l| {
            app.line_pos(l)
                .map(|pos| (l, shape_of(app, pos.node), app.is_footer(pos)))
        })
        .collect()
}
