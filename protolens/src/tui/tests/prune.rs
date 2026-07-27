// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0183: the override walk descends only into subtrees that can
//! change. Every test here exists because that change fails *quietly* —
//! a wrongly skipped subtree keeps whatever text it was last rendered
//! with, with no panic, no assertion and no bad index. So the checks are
//! byte equality, not shape.

use super::super::*;
use super::support::*;

/// The acceptance criterion (spec 0183 G4/G5), in the cheapest form
/// that is still exact.
///
/// The spec describes rendering a document twice, once pruned and once
/// with the gate forced to its old `is_message` shape, and comparing.
/// Running the unpruned walk *after* the pruned one has already settled
/// the document tests the same thing and needs only one `App`: if the
/// pruned walk reached everything the unpruned one would have, then the
/// unpruned walk finds every node already matching its `rendered_as`
/// and changes nothing. Anything the pruned walk missed, it fixes — and
/// the comparison fails.
///
/// This also covers the startup pass, which a two-`App` comparison
/// could not: `App::new` runs `render_overrides` itself, before a test
/// can set any flag.
/// The one difference between the two walks that is not a pruning bug
/// (spec 0183 G5's exception).
///
/// Arriving at a node is not free: `resettle_node` splices whenever
/// `current != rendered_as`, and at startup `rendered_as` is `None`
/// everywhere, so the unpruned walk re-splices *every* message node in
/// the document. A re-splice renders the node against
/// `decode::register_wrapper`'s synthetic one-field descriptor, whose
/// field is hard-coded `Label::Optional` (`decode.rs`), so the `#@`
/// annotation comes back without the `repeated` qualifier the original
/// decode wrote there. The pruned walk never arrives, so it keeps
/// decode's own — strictly better — annotation.
///
/// Tolerating it here is deliberately as narrow as it can be made: the
/// pruned line must be the unpruned line with `repeated ` inserted into
/// its annotation and nothing else changed. Any other divergence, in
/// either direction, is a reachability bug and fails.
fn only_lost_the_repeated_qualifier(pruned: &str, unpruned: &str) -> bool {
    pruned == unpruned || pruned.replace("#@ repeated ", "#@ ") == unpruned
}

/// A node's identity by *content*, not by arena index.
///
/// The arena is append-only: a re-splice pushes a fresh copy of the
/// subtree and abandons the old one in place, so the reference walk
/// legitimately renumbers every node it resettles. Comparing
/// `app.tree` or the raw `line_to_node` values across the two walks
/// would therefore report a difference that says nothing about
/// reachability. Projecting through this instead compares what the two
/// walks actually have to agree on.
type Shape = (usize, u64, Option<String>, std::ops::Range<usize>);

fn shape_of(app: &App, idx: usize) -> Shape {
    let s = &app.tree[idx].span;
    (
        s.level,
        s.field_number,
        s.type_fqdn.clone(),
        s.text_range.clone(),
    )
}

/// Every node still reachable from the root, in document order.
fn live_shapes(app: &App) -> Vec<Shape> {
    let mut out = Vec::new();
    let mut stack = vec![app.first_node];
    while let Some(i) = stack.pop() {
        out.push(shape_of(app, i));
        let mut kids = Vec::new();
        let mut c = app.tree[i].first_child;
        while let Some(ci) = c {
            kids.push(ci);
            c = app.tree[ci].next_sibling;
        }
        stack.extend(kids.into_iter().rev());
    }
    out
}

fn shaped_map(app: &App, map: &[Option<u32>]) -> Vec<(usize, Shape)> {
    map.iter()
        .enumerate()
        .filter_map(|(l, n)| n.map(|n| (l, shape_of(app, n as usize))))
        .collect()
}

fn assert_unpruned_walk_changes_nothing(app: &mut App, what: &str) {
    let lines = app.lines.clone();
    let shapes = live_shapes(app);
    let line_to_node = shaped_map(app, &app.line_to_node);
    let footer_line_to_node = shaped_map(app, &app.footer_line_to_node);
    let entries = format!("{:#?}", app.overrides.entries());

    let root = app.first_node;
    app.unpruned_walk = true;
    app.render_overrides(root);
    app.unpruned_walk = false;

    let diff: Vec<String> = lines
        .iter()
        .zip(app.lines.iter())
        .enumerate()
        .filter(|(_, (a, b))| !only_lost_the_repeated_qualifier(a, b))
        .map(|(n, (a, b))| format!("line {n}:\n  pruned:   {a:?}\n  unpruned: {b:?}"))
        .collect();
    assert!(
        diff.is_empty() && app.lines.len() == lines.len(),
        "{what}: the pruned walk left stale text — the unpruned walk \
         reached a node it did not ({} pruned lines vs {} unpruned):\n{}",
        lines.len(),
        app.lines.len(),
        diff.join("\n")
    );
    assert_eq!(
        live_shapes(app),
        shapes,
        "{what}: the live tree differs after the unpruned walk"
    );
    assert_eq!(
        shaped_map(app, &app.line_to_node),
        line_to_node,
        "{what}: line_to_node differs after the unpruned walk"
    );
    assert_eq!(
        shaped_map(app, &app.footer_line_to_node),
        footer_line_to_node,
        "{what}: footer_line_to_node differs after the unpruned walk"
    );
    assert_eq!(
        format!("{:#?}", app.overrides.entries()),
        entries,
        "{what}: the unpruned walk seeded an override the pruned walk \
         missed"
    );
}

/// The startup pass, across every fixture shape that has something for
/// it to find. `nested_any_fixture`/`nested_message_set_fixture` are the
/// load-bearing ones: their auto-expansion targets sit under plain
/// message ancestors, which is exactly what the old `is_message`
/// disjunct descended through for free.
#[test]
fn the_startup_pass_reaches_everything_the_unpruned_walk_would() {
    assert_unpruned_walk_changes_nothing(&mut nested_any_fixture(), "nested Any");
    assert_unpruned_walk_changes_nothing(&mut nested_message_set_fixture(), "nested MessageSet");
    assert_unpruned_walk_changes_nothing(&mut message_set_fixture(), "flat MessageSet");
    assert_unpruned_walk_changes_nothing(&mut repeated_message_fixture().0, "repeated message");
    assert_unpruned_walk_changes_nothing(&mut repeated_scalar_fixture().0, "packed run");
    assert_unpruned_walk_changes_nothing(&mut type_as_fixture().0, "nested message");
    assert_unpruned_walk_changes_nothing(&mut empty_message_fixture().0, "empty message");
    assert_unpruned_walk_changes_nothing(&mut enum_field_fixture().0, "enum field");
    assert_unpruned_walk_changes_nothing(&mut group_type_fixture().0, "schema group");
}

/// The same criterion after a real override has been applied — the
/// state in which `descend`'s three sources all have something in them
/// at once.
#[test]
fn an_applied_override_leaves_nothing_for_the_unpruned_walk_to_fix() {
    let (mut app, _outer, inner) = type_as_fixture();
    let inner_path = app.positional_path(inner);

    app.overrides.activate(
        override_pane::OverrideOrigin::Path {
            path: inner_path.clone(),
        },
        None,
    );
    let root = app.first_node;
    app.render_overrides(root);
    assert_unpruned_walk_changes_nothing(&mut app, "after a raw override");

    app.overrides.activate(
        override_pane::OverrideOrigin::Path { path: inner_path },
        Some("test.Inner".to_string()),
    );
    app.render_overrides(root);
    assert_unpruned_walk_changes_nothing(&mut app, "after retyping back");
}

/// An `FqdnField` origin is not path-shaped, so it cannot be pruned by
/// path prefix (spec 0183 S5). It is marked instead by matching each
/// node's own field number against its parent's resolved type, which is
/// exact — so a document where the same `fqdn:field` matches several
/// scattered nodes must still settle every one of them.
#[test]
fn an_fqdn_field_override_marks_every_node_of_that_type() {
    let (mut app, items) = repeated_message_fixture();
    assert_eq!(items.len(), 3);

    app.overrides.activate(
        override_pane::OverrideOrigin::FqdnField {
            fqdn: "test.Item".to_string(),
            field: 1,
        },
        None,
    );
    let root = app.first_node;
    app.render_overrides(root);

    for (n, item) in items.iter().enumerate() {
        assert!(
            app.tree[*item]
                .first_child
                .map(|c| app.tree[c].rendered_as.is_some())
                .unwrap_or(false),
            "item {n}'s field 1 must have been settled under the \
             fqdn:field override: {:#?}",
            app.lines
        );
    }
    assert_unpruned_walk_changes_nothing(&mut app, "after an fqdn:field override");
}

/// Spec 0183 L3, the trap the spec calls out by name.
///
/// `rendered_as.is_some()` is an O(1) field read, so the obvious plan is
/// to leave it in the gate untouched. That is wrong: it only ever speaks
/// about the node it is read on, and it is only read if the walk arrives
/// there — reachability the deleted `is_message` disjunct was silently
/// supplying. A node spliced under an override, sitting beneath plain
/// ancestors that no override touches, must still be revisited after
/// that override is deactivated, so it can fall back to its natural
/// type. Marking the node alone would not do it; the ancestors have to
/// be marked too.
#[test]
fn a_deactivated_override_still_falls_back_under_unmarked_ancestors() {
    let mut app = nested_any_fixture();
    // The payload's own `label` leaf: five plain levels above it, none
    // of which carries an override of its own — the depth is the point
    // of the test. Deliberately *not* the `acme.Payload` node itself,
    // which is the auto-expansion's own target: overriding that origin
    // supersedes the auto entry, so deactivating would leave the field
    // at its `bytes` natural type and prove nothing about reachability.
    let payload = app
        .tree
        .iter()
        .position(|n| n.span.type_fqdn.as_deref() == Some("acme.Payload"))
        .expect("the Any's value must have auto-expanded");
    let deep = app.tree[payload]
        .first_child
        .expect("the payload must have its `label` field");
    let deep_path = app.positional_path(deep);
    let origin = override_pane::OverrideOrigin::Path {
        path: deep_path.clone(),
    };
    let root = app.first_node;

    app.overrides.activate(origin.clone(), None);
    app.render_overrides(root);
    let raw = app.lines.clone();
    // The raw render still shows the string bytes — what it loses is the
    // schema, so the field name is the discriminator, not the payload.
    assert!(
        !raw.iter().any(|l| l.contains("label")),
        "the raw override must have replaced the expanded payload with a \
         structural guess: {raw:?}"
    );

    let entry = app
        .overrides
        .entries()
        .iter()
        .position(|e| e.origin == origin && e.r#type.is_none())
        .expect("the raw entry must exist");
    app.overrides.toggle_active(entry);
    app.render_overrides(root);

    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hello")),
        "deactivating the override must settle the node back to its \
         natural type, five plain ancestors down: {:?}",
        app.lines
    );
    assert_unpruned_walk_changes_nothing(&mut app, "after deactivation");
}

/// Spec 0183 L1: a node's auto-expand eligibility can only *change*
/// inside content that was re-decoded in this same batch, which S3
/// descends in full. MessageSet tier 2 is the live example — the
/// `message` field only becomes a candidate once tier 1 has retyped its
/// parent to the synthetic `Item` shape, which happens during the walk,
/// long after `compute_descend_marks` ran and could have marked it.
#[test]
fn a_candidate_that_only_appears_mid_batch_is_still_expanded() {
    let mut app = nested_message_set_fixture();
    let root = app.first_node;

    // Retype the root raw and back, so the whole document is re-decoded
    // and both MessageSet tiers have to be rediscovered from scratch
    // within a single batch.
    let root_origin = override_pane::OverrideOrigin::Path {
        path: "/".to_string(),
    };
    app.overrides.activate(root_origin.clone(), None);
    app.render_overrides(root);
    app.overrides
        .activate(root_origin, Some("ms_test.Top".to_string()));
    app.render_overrides(root);

    assert!(
        app.tree
            .iter()
            .any(|n| n.span.type_fqdn.as_deref() == Some("ms_test.ExtPayload")),
        "tier 2 must be rediscovered inside freshly spliced content: {:#?}",
        app.lines
    );
    assert!(
        app.lines
            .iter()
            .any(|l| l.contains("label") && l.contains("hi")),
        "the rediscovered extension payload must be rendered: {:?}",
        app.lines
    );
    assert_unpruned_walk_changes_nothing(&mut app, "after a root round-trip");
}

/// Spec 0183 S2's marking is what the gate reads, so it is worth
/// asserting directly rather than only through its effects: the walk
/// must not descend into a plain scalar leaf that nothing marks, which
/// is the spec 0119 bug the original gate was introduced to prevent
/// (`natural_type` demoting an ordinary string field to a raw record
/// dump).
#[test]
fn an_untouched_scalar_leaf_is_not_descended_into() {
    let app = nested_any_fixture();
    let type_url = app
        .lines
        .iter()
        .position(|l| l.contains("type.googleapis.com"))
        .expect("the Any's type_url must be rendered");
    assert!(
        app.lines[type_url].contains("type.googleapis.com/acme.Payload"),
        "the type_url must still render as a string, not as a raw \
         record dump: {:?}",
        app.lines[type_url]
    );
}
