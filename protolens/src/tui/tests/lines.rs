// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0210: a node counts its own lines.
//!
//! No line number is stored any more, so three things that used to be
//! an array index are walks over the tree — `absolute_start` upward,
//! `line_pos` downward, `visible_row_pos` downward with folds applied —
//! and each of them can be wrong in a way that still looks like a
//! perfectly well-formed document. A missed sibling in one sum shifts
//! everything after it by a couple of lines; the screen still draws,
//! the cursor still moves, and the text under it is simply not the text
//! it claims to be.
//!
//! So each direction is pinned against something that does not share
//! its arithmetic: the downward walks against the upward one, inverted,
//! and the upward one against a table built outward from the nodes,
//! which must turn out to name an owner for every line of the document
//! and for nothing past its end.
//!
//! There used to be a third anchor here, `span.text_range` — the
//! absolute range the renderer recorded while emitting the text, and
//! the field the old implementation just read. Spec 0210 S11 retired
//! both it and the test that used it: the field is exact only until the
//! first splice, and the machinery that had kept it plausible
//! afterwards was the last whole-document walk left in a commit.
//!
//! `override_apply`'s `assert_line_counts_are_exact` covers the
//! counters themselves, against a full recount and against the
//! indentation of the text — and it runs after every splice in the
//! whole suite. What is checked here is the resolution built on top.

use super::super::*;
use super::support::*;

/// Every fixture in the suite that comes from a real decode, and whose
/// counters therefore came from the renderer's own output rather than
/// from a fixture author's arithmetic.
fn real_decodes() -> Vec<(&'static str, App)> {
    vec![
        ("type_as", type_as_fixture().0),
        ("empty_message", empty_message_fixture().0),
        ("enum_field", enum_field_fixture().0),
        ("group_type", group_type_fixture().0),
        ("repeated_scalar", repeated_scalar_fixture().0),
        ("repeated_message", repeated_message_fixture().0),
        ("packed_run_with_tail", packed_run_with_tail_fixture().0),
        ("nested_packed_run", nested_packed_run_fixture()),
        ("message_set", message_set_fixture()),
        ("nested_any", nested_any_fixture()),
        ("nested_message_set", nested_message_set_fixture()),
        ("export_fields", export_fields_fixture()),
        (
            "export_fields_group_error",
            export_fields_group_error_fixture(),
        ),
        ("pruned_tail", pruned_tail_fixture().0),
    ]
}

/// Every live node, in document order.
///
/// Walks the child chains rather than `doc_next` for the same reason
/// `repeated_message_fixture` does: `App::new`'s startup
/// `render_overrides` resettles nodes to their natural types, and
/// `splice_override` abandons the superseded subtrees in the arena
/// rather than removing them. An orphan still counts its own lines, but
/// no line of the document belongs to it, so including one would make
/// the ownership table below claim a line twice.
fn live_nodes(app: &App) -> Vec<usize> {
    let mut out = Vec::new();
    if app.tree.is_empty() {
        return out;
    }
    let mut roots = Vec::new();
    let mut r = Some(app.first_node);
    while let Some(i) = r {
        roots.push(i);
        r = app.next_sibling(i);
    }
    let mut stack = roots;
    stack.reverse();
    while let Some(i) = stack.pop() {
        out.push(i);
        let mut kids = Vec::new();
        let mut c = app.first_child(i);
        while let Some(ci) = c {
            kids.push(ci);
            c = app.next_sibling(ci);
        }
        stack.extend(kids.into_iter().rev());
    }
    out
}

/// Spec 0210 test-plan item 4: the descent inverts the position.
///
/// Also the standing witness for S1's claim that *every* line has an
/// owner. Before S1 a malformed record's line had none, and a lookup
/// there fell through to whichever node happened to straddle it — so a
/// hole in this table is not a missing feature, it is a wrong answer
/// for a line the user is looking at.
/// Who owns each line, built from the nodes outward — the opposite
/// direction from the descents that answer the same question, and so
/// the reference they are checked against. Panics if two nodes claim
/// one line, which is a corrupt tree rather than a wrong answer.
fn owners_from_the_nodes(name: &str, app: &App) -> Vec<Option<LinePos>> {
    let mut expect: Vec<Option<LinePos>> = vec![None; app.document_lines().len()];
    for idx in live_nodes(app) {
        let r = app.node_lines(idx);
        // A bracketed node owns its opening and closing lines and
        // nothing between them — the body belongs to its subtree. A
        // flat one owns every row it draws, which for a packed run
        // is one per element (spec 0216 S7/S22).
        let rows: Vec<usize> = if app.tree[idx].is_bracketed() {
            vec![0, r.len() - 1]
        } else {
            (0..r.len()).collect()
        };
        for k in rows {
            assert!(
                expect[r.start + k].is_none(),
                "{name}: line {} claimed twice",
                r.start + k
            );
            expect[r.start + k] = Some(LinePos {
                node: idx,
                line_in_node: k as u32,
            });
        }
    }
    expect
}

#[test]
fn the_descent_names_the_owner_of_every_line_and_nothing_past_the_end() {
    for (name, app) in real_decodes() {
        let expect = owners_from_the_nodes(name, &app);
        for (line, want) in expect.iter().enumerate() {
            assert!(
                want.is_some(),
                "{name}: line {line} ({:?}) is owned by nobody",
                app.document_lines()[line]
            );
            assert_eq!(app.line_pos(line), *want, "{name}: line {line}");
        }
        assert_eq!(
            app.line_pos(app.document_lines().len()),
            None,
            "{name}: past the end must be past the end"
        );
    }
}

/// Spec 0222, test-plan item 4: a drawn row carries its own owner.
///
/// `visible_window` must number its rows consecutively from `from`, and
/// each row's owner must be the one the whole-document descent gives
/// for that same line.
///
/// The window is the only place a line's owner is worked out in bulk —
/// S3 has a drawn row carry its owner from here to every consumer of
/// the frame (the fold marker, the spans, the override hint), none of
/// which resolves a line number again. So a walk that drifted from the
/// descent would be silent.
///
/// Run twice over each fixture, unfolded and then with the first
/// foldable node folded, because the walk's whole reason to exist is
/// that it steps *past* a folded body: unfolded it would agree with a
/// plain counter too.
#[test]
fn a_display_row_carries_its_own_owner() {
    for (name, mut app) in real_decodes() {
        check_window_against_the_descent(name, &app);

        let Some(victim) = live_nodes(&app)
            .into_iter()
            .find(|&i| app.tree[i].is_bracketed() && app.tree[i].lines_total > 2)
        else {
            continue;
        };
        app.toggle_fold(victim);
        check_window_against_the_descent(name, &app);
    }
}

fn check_window_against_the_descent(name: &str, app: &App) {
    let rows = app.visible_row_count();
    let window = app.visible_window(0, rows);
    assert!(!window.is_empty(), "{name}: the fixture must draw rows");
    assert_eq!(
        window.len(),
        rows,
        "{name}: the walk must produce every visible row"
    );
    assert!(
        window.windows(2).all(|w| w[0].0 < w[1].0),
        "{name}: the window must number its lines in order: {:?}",
        window.iter().map(|&(l, _)| l).collect::<Vec<_>>()
    );
    for (row, &(line, pos)) in window.iter().enumerate() {
        assert_eq!(
            app.line_pos(line),
            Some(pos),
            "{name}: line {line} must resolve the same through the walk \
             as through the descent"
        );
        assert_eq!(
            app.visible_row_pos(row),
            Some((pos, line)),
            "{name}: row {row} must be the row the descent draws there"
        );
    }
}

/// With no folds, the visible-row descent must agree with the line
/// descent everywhere — the degenerate case, worth its own check
/// because the two descents are separate copies of the same arithmetic
/// over different counters.
#[test]
fn an_unfolded_document_numbers_rows_and_lines_alike() {
    for (name, app) in real_decodes() {
        assert!(app.user_folds().is_empty(), "{name}: fixture starts folded");
        assert_eq!(
            app.visible_row_count(),
            app.document_lines().len(),
            "{name}"
        );
        for line in 0..app.document_lines().len() {
            assert_eq!(
                app.visible_row_pos(line),
                app.line_pos(line).map(|pos| (pos, line)),
                "{name}: row {line}"
            );
            assert_eq!(app.visible_row_of_line(line), Some(line), "{name}");
        }
    }
}

/// Spec 0210 test-plan item 6 (G3): a fold is O(depth).
///
/// The claim is not "folding is fast" but the narrower, checkable one
/// it rests on: the only numbers a fold may change are the folded
/// node's own and its ancestors'. Any scheme that stores a line
/// *position* per node has to rewrite every number after the fold
/// instead, so the untouched siblings below are the interesting half of
/// the assertion.
#[test]
fn a_fold_changes_only_the_folded_node_and_its_ancestors() {
    let (mut app, items) = repeated_message_fixture();
    let victim = items[0];
    let totals: Vec<u32> = app.tree.iter().map(|n| n.lines_total).collect();
    let before: Vec<u32> = app.tree.iter().map(|n| n.lines_visible).collect();

    app.toggle_fold(victim);

    let changed: Vec<usize> = (0..app.tree.len())
        .filter(|&i| app.tree[i].lines_visible != before[i])
        .collect();
    let mut want: Vec<usize> = std::iter::successors(Some(victim), |&i| app.parent(i)).collect();
    want.sort_unstable();
    assert_eq!(
        changed, want,
        "a fold must touch the node and its ancestors, and nothing else"
    );
    assert!(
        changed.len() < app.tree.len(),
        "the fixture must have nodes outside the root path, or this proves nothing"
    );
    for &later in &items[1..] {
        assert!(
            !changed.contains(&later),
            "the siblings below the fold must not move"
        );
    }
    assert_eq!(
        app.tree.iter().map(|n| n.lines_total).collect::<Vec<_>>(),
        totals,
        "a fold hides lines, it does not remove them: lines_total must not move"
    );
}

/// The visible descent under an actual fold, in both directions.
///
/// The asymmetry is deliberate and is what the callers want: a line
/// inside a fold still resolves to a node (`line_pos` knows nothing
/// about folds) but is *drawn* at the fold's own header row, because a
/// cursor or an overlay anchor sitting on a hidden line still has to be
/// put somewhere on screen.
#[test]
fn a_folded_body_is_represented_by_its_fold_header_row() {
    let (mut app, items) = repeated_message_fixture();
    let hidden = app.node_lines(items[1]);
    assert!(hidden.len() > 2, "the fold must hide something");
    app.toggle_fold(items[1]);

    let rows = app.visible_row_count();
    assert_eq!(rows, app.document_lines().len() - (hidden.len() - 1));
    for row in 0..rows {
        let (pos, line) = app.visible_row_pos(row).expect("row within the count");
        assert_eq!(app.line_pos(line), Some(pos), "row {row}");
        assert_eq!(app.visible_row_of_line(line), Some(row), "row {row}");
    }
    assert_eq!(app.visible_row_pos(rows), None);

    let fold_row = app.visible_row_of_line(hidden.start).expect("the header");
    for line in hidden.clone() {
        assert_eq!(
            app.visible_row_of_line(line),
            Some(fold_row),
            "hidden line {line} must be represented by the fold's header row"
        );
        assert!(
            app.line_pos(line).is_some(),
            "a hidden line still has an owner"
        );
    }
}

/// A chain ending in a message that can be re-read as one row:
/// `Top { Mid m { Deep d { Holder h { Inner x { id: 5 } } } } }`, where
/// `test.Blobby` declares the same field 1 as `bytes` and so draws
/// `h`'s body on a single line instead of three.
///
/// Spec 0254's tests need a node whose subtree genuinely *shrinks* — a
/// fold moves only `lines_visible`, so it cannot exercise the
/// `lines_total` difference at all, and it is the negative one a `u32`
/// would wrap on.
///
/// `Deep` is why the chain is five levels and not four. A splice's
/// refresh *starts* at the spliced node's parent, and the folded-
/// ancestor rule only applies to nodes the walk climbs *to* — so with
/// the fold one level up it is the starting sum that handles it, and
/// the rule under test never runs. `Deep` puts a plain level between
/// the two. Returns `(app, m, h)`.
fn shrinkable_chain_fixture() -> (App, usize, usize) {
    use prost_types::field_descriptor_proto::{Label, Type};

    let wraps = |name: &str, inner: &str| {
        message(
            name,
            vec![field_of("m", 1, Label::Optional, Type::Message, inner)],
        )
    };
    let fds = proto3_fds(
        "test_shrinkable_chain.proto",
        vec![
            wraps("Top", ".test.Mid"),
            wraps("Mid", ".test.Deep"),
            wraps("Deep", ".test.Holder"),
            message(
                "Holder",
                vec![field_of(
                    "x",
                    1,
                    Label::Optional,
                    Type::Message,
                    ".test.Inner",
                )],
            ),
            message("Blobby", vec![field("x", 1, Label::Optional, Type::Bytes)]),
            message("Inner", vec![field("id", 1, Label::Optional, Type::Int32)]),
        ],
    );

    // Top's body: four nested LEN field-1 wrappers around `id: 5`.
    let app = fixture_under(
        "shrinkable-chain",
        &fds,
        "test.Top",
        &[0x0Au8, 0x08, 0x0A, 0x06, 0x0A, 0x04, 0x0A, 0x02, 0x08, 0x05],
    );
    let m = node_with_type(&app, "test.Mid").expect("the chain's Mid");
    let d = node_with_type(&app, "test.Deep").expect("the chain's Deep");
    let h = node_with_type(&app, "test.Holder").expect("the chain's Holder");
    assert_eq!(app.parent(h), Some(d), "h must sit directly under d");
    assert_eq!(app.parent(d), Some(m), "d must sit directly under m");
    assert_eq!(app.parent(m), Some(app.first_node), "m must sit under root");
    (app, m, h)
}

/// Spec 0254 test-plan item 2: a subtree that shrinks carries a
/// *negative* difference up.
///
/// The growing direction is covered by every override test in the
/// suite. This is the one that would wrap if the difference were
/// computed in `u32`, and the wrap would not be a panic — it would be a
/// root claiming four billion lines.
#[test]
fn a_shrinking_subtree_carries_a_negative_difference() {
    let (mut app, m, h) = shrinkable_chain_fixture();
    let root = app.first_node;
    let (h_before, m_before, root_before) = (
        app.tree[h].lines_total,
        app.tree[m].lines_total,
        app.tree[root].lines_total,
    );
    assert_eq!(
        root_before as usize,
        app.document_lines().len(),
        "the fixture's root must account for the whole document"
    );

    app.override_target = Some(h);
    app.splice_override(h, Some("test.Blobby".to_string()), None)
        .expect("re-reading the Holder as Blobby must succeed");

    let shrank = i64::from(h_before) - i64::from(app.tree[h].lines_total);
    assert!(
        shrank > 0,
        "the fixture must shrink, or this proves nothing: {h_before} -> {}",
        app.tree[h].lines_total
    );
    assert_eq!(
        i64::from(app.tree[m].lines_total),
        i64::from(m_before) - shrank,
        "the ancestor must lose exactly what the subtree lost"
    );
    assert_eq!(
        i64::from(app.tree[root].lines_total),
        i64::from(root_before) - shrank,
        "and so must the root, two levels up"
    );
    assert_eq!(
        app.total_lines(),
        app.document_lines().len(),
        "the counts must still describe the text"
    );
}

/// Spec 0254 test-plan item 3: a folded ancestor absorbs the visible
/// difference while still passing the total one on.
///
/// The two counts part company exactly here, and only here. A folded
/// node draws one row whatever happens beneath it (spec 0193), so the
/// visible difference above it is zero — but the lines are still
/// *there*, so `lines_total` moves all the way to the root.
#[test]
fn a_folded_ancestor_absorbs_the_visible_difference() {
    let (mut app, m, h) = shrinkable_chain_fixture();
    let root = app.first_node;

    app.set_folded(m, true);
    app.refresh_line_counts(m);
    assert_eq!(app.tree[m].lines_visible, 1, "a fold shows one row");
    let (h_before, m_before, root_before) = (
        app.tree[h].lines_total,
        app.tree[m].lines_total,
        app.tree[root].lines_total,
    );
    let root_visible = app.tree[root].lines_visible;

    app.override_target = Some(h);
    app.splice_override(h, Some("test.Blobby".to_string()), None)
        .expect("re-reading the Holder as Blobby must succeed");

    let shrank = i64::from(h_before) - i64::from(app.tree[h].lines_total);
    assert!(shrank > 0, "the fixture must shrink");
    assert_eq!(
        i64::from(app.tree[m].lines_total),
        i64::from(m_before) - shrank,
        "the folded ancestor's total still moves"
    );
    assert_eq!(
        app.tree[m].lines_visible, 1,
        "while its visible count does not"
    );
    assert_eq!(
        i64::from(app.tree[root].lines_total),
        i64::from(root_before) - shrank,
        "and the total keeps travelling past it"
    );
    assert_eq!(
        app.tree[root].lines_visible, root_visible,
        "nothing above the fold may change a visible count"
    );
}

/// Spec 0254 test-plan item 4: the walk stops at an ancestor nothing
/// moved.
///
/// A fold moves no `lines_total` at all, and a folded ancestor swallows
/// the `lines_visible` difference — so folding a node underneath one
/// leaves `m` and everything above it byte-for-byte as it was, and the
/// changed set is the two levels in between and nothing else.
///
/// The early exit itself has no separate outward sign: adding a zero
/// difference is a no-op, so a walk that failed to stop would write the
/// same numbers back. What is checkable — and what the exit is there to
/// protect — is that the barrier exists at all: drop the folded rule
/// and `m`'s own visible count goes to −1 and the root's follows it
/// down.
#[test]
fn refresh_line_counts_stops_at_an_unchanged_ancestor() {
    let (mut app, m, h) = shrinkable_chain_fixture();
    let x = app.first_child(h).expect("Holder wraps an Inner");
    assert!(
        app.tree[x].is_bracketed(),
        "the fold target must be foldable"
    );

    app.set_folded(m, true);
    app.refresh_line_counts(m);
    let before: Vec<(u32, u32)> = app
        .tree
        .iter()
        .map(|n| (n.lines_total, n.lines_visible))
        .collect();

    app.set_folded(x, true);
    app.refresh_line_counts(x);

    let changed: Vec<usize> = (0..app.tree.len())
        .filter(|&i| (app.tree[i].lines_total, app.tree[i].lines_visible) != before[i])
        .collect();
    let mut want = vec![x, h, app.parent(h).expect("Holder sits under Deep")];
    want.sort_unstable();
    assert_eq!(
        changed, want,
        "a fold under a folded ancestor must stop at that ancestor"
    );
}

/// Spec 0210 test-plan item 9: the three teleports.
///
/// A teleport is the one place a full descent is still paid, and the
/// one place a fold makes absolute lines and drawn rows disagree — so
/// it is where mixing the two units up is invisible until the user is
/// looking at the wrong line. The fold here is deliberately *not* an
/// ancestor of the target: it displaces the target's row without any of
/// the unfolding the teleports themselves do.
#[test]
fn teleports_land_on_the_named_line_with_a_fold_above_them() {
    let (mut app, items) = repeated_message_fixture();
    app.main_area = Rect::new(0, 0, 40, 20);
    app.toggle_fold(items[0]);

    let target = app
        .first_child(*items.last().expect("three items"))
        .expect("each Item has a scalar child");
    let line = app.absolute_start(target);
    let row = app.visible_row_of_line(line).expect("the target is drawn");
    assert_ne!(line, row, "the fold must actually displace the target");
    let text = app.document_lines()[line].trim().to_string();

    // 1. Search.
    app.set_cursor(app.first_node);
    app.run_search(SearchScope::Main, SearchDir::Forward, &text);
    assert_eq!(
        (app.cursor, app.cursor_on_footer()),
        (target, false),
        "search"
    );
    assert_eq!(app.cursor_line(), line, "search line");
    assert_eq!(app.cursor_display_row(), row, "search row");
    assert!(
        app.is_user_folded(items[0]),
        "a search must not disturb a fold it did not land in"
    );

    // 2. Jumplist: leave the position, then come back to it.
    app.record_jump();
    app.set_cursor(app.first_node);
    assert_ne!(app.cursor_line(), line);
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert_eq!(
        (app.cursor, app.cursor_on_footer()),
        (target, false),
        "Ctrl-o"
    );
    assert_eq!(app.cursor_line(), line, "Ctrl-o line");
    assert_eq!(app.cursor_display_row(), row, "Ctrl-o row");

    // 3. A click on the row the fold displaced it to. Column 4 is
    //    inside the text, clear of the heat gutter and the fold margin.
    app.set_cursor(app.first_node);
    app.scroll.index = 0;
    app.handle_click(4, row as u16);
    assert_eq!(
        (app.cursor, app.cursor_on_footer()),
        (target, false),
        "click"
    );
    assert_eq!(app.cursor_line(), line, "click line");
    assert_eq!(app.cursor_display_row(), row, "click row");
}
