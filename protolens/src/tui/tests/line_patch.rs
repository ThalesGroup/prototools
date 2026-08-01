// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0167: `materialize_line_patches` — one pass over a whole batch of
//! patches instead of one `Vec::splice` per patch.

use super::super::line_patch::{LinePatch, LinePatchTarget};
use super::super::*;
use super::support::*;

/// Seed `app` with `n` synthetic committed lines, so that the patch
/// tests below have a document to merge against without depending on
/// whatever the fixture happened to render.
fn seed_committed_lines(app: &mut App, n: usize) {
    app.lines = (0..n).map(|i| format!("old{i}")).collect();
}

fn original_patch(range: Range<usize>, lines: &[&str]) -> LinePatch {
    LinePatch {
        target: LinePatchTarget::Original(range.clone()),
        global_start: range.start,
        children_base_shift: 0,
        lines: lines.iter().map(|l| l.to_string()).collect(),
    }
}

/// Flaw C2 (worklist W2): `materialize_line_patches`'s forward merge is
/// only correct for ascending, non-overlapping ranges. That used to rest
/// entirely on *callers* queueing in order, guarded by a
/// `debug_assert!` — which a project built exclusively with `--release`
/// never compiles, so the guard was absent from every binary anyone ran.
/// The requirement was violable at a distance, and was in fact violated
/// (the 2026-07-25 `doc_next` cycle bug presented as a bare slice panic
/// in this loop rather than naming the splice that queued the bad
/// patch). Ordering is now the merge's own business: it sorts. This test
/// queues back-to-front and demands the correct merge anyway, and it
/// runs under `--release` like every other test here, which is the whole
/// point.
#[test]
fn top_level_line_patches_merge_correctly_when_queued_out_of_order() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 6);

    app.pending_line_patches = vec![
        original_patch(4..6, &["B"]),
        original_patch(1..2, &["A0", "A1"]),
    ];
    app.materialize_line_patches();

    assert_eq!(app.lines, ["old0", "A0", "A1", "old2", "old3", "B"]);
    assert!(app.pending_line_patches.is_empty());
}

/// Flaw C2, the nested half: a patch's `Nested` children are resolved by
/// the same forward merge, against the parent patch's own lines, and so
/// need the same ordering guarantee.
#[test]
fn nested_line_patches_merge_correctly_when_queued_out_of_order() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 4);

    let nested = |parent: usize, range: Range<usize>, line: &str| LinePatch {
        target: LinePatchTarget::Nested(parent, range),
        global_start: 0,
        children_base_shift: 0,
        lines: vec![line.to_string()],
    };
    app.pending_line_patches = vec![
        original_patch(1..3, &["p0", "p1", "p2", "p3"]),
        nested(0, 2..3, "Y"),
        nested(0, 0..1, "X"),
    ];
    app.materialize_line_patches();

    assert_eq!(app.lines, ["old0", "X", "p1", "Y", "p3", "old3"]);
}

/// Flaw C2: overlap is a genuine contradiction rather than a permutation
/// problem, so it stays an assertion — but a real one, naming the
/// offending pair instead of surfacing as `slice index starts at 3 but
/// ends at 2` inside the merge loop.
#[test]
#[should_panic(expected = "overlapping top-level line patches")]
fn overlapping_line_patches_panic_with_a_directed_message() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    seed_committed_lines(&mut app, 6);

    app.pending_line_patches = vec![original_patch(0..3, &["A"]), original_patch(2..4, &["B"])];
    app.materialize_line_patches();
}

/// The 2026-07-25 crash report as it was actually reported — `Down`,
/// then `t`, then `Enter` — driven through `handle_key`, on a fixture
/// that is in this repository.
///
/// The three tests above cover the merge that panicked, and
/// `a_splice_keeps_packed_record_starts_in_the_documents_byte_frame`
/// covers the offset that fed it the bad patch. Neither covers the
/// *sequence*: everything between the keypress and the merge — the
/// preview splice `t` fires, the reset it leaves behind, and the two
/// separate batches `Enter` then runs, one from the root and one from
/// the cursor — was reachable only through the `#[ignore]`d repro tests,
/// which need a 1 MB descriptor set at `/tmp/pdb.desc` that nobody has.
/// A regression test nobody can run is a note, not a test.
///
/// What is asserted afterwards is that the document still holds
/// together: every line has an owner, and the owners account for every
/// line exactly once. That is the invariant the crash violated — the
/// panic was a symptom of line ranges that no longer described the text
/// — and it is checkable on a four-line fixture.
#[test]
fn the_reported_down_t_enter_sequence_leaves_the_document_coherent() {
    let mut app = nested_packed_run_fixture();
    app.splash = false;
    app.term_width = 120;

    // Down to the `blob` field, which is the one with something inside
    // it to re-splice.
    let blob_node = app.resolve_path("/2").expect("blob is /2");
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.cursor, blob_node, "two Downs must reach `blob`");

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert!(
        app.override_target.is_some(),
        "`t` must have opened the pane, or the rest of this proves nothing",
    );
    // Aimed rather than left on whatever the pane happens to highlight:
    // confirming the type the node already has is a no-op, and a no-op
    // never reaches the merge.
    app.override_highlight = app
        .override_candidates
        .iter()
        .position(|(fqdn, _)| fqdn == "test.Payload")
        .expect("Payload must be offered");
    let before = app.lines.len();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.overrides.entries().iter().any(|e| e.active),
        "`Enter` must have confirmed a candidate — an inert sequence \
         cannot reach the splice this is about",
    );
    assert_ne!(
        app.lines.len(),
        before,
        "the confirm must have re-spliced the document, or nothing was \
         merged and the coherence check below is vacuous",
    );

    let owners = line_owners(&app);
    assert_eq!(
        owners.len(),
        app.lines.len(),
        "every line must resolve to an owner after the confirm",
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
}

/// Spec 0186 S3, the crux. A splice grows every ancestor's
/// `lines_total`, so an ancestor's *footer* line moves while its
/// *header* line does not — the asymmetry the old line maps had to be
/// repaired by line index rather than by subtree, and the one the
/// counters now have to reproduce for free.
///
/// Note what this asserts beyond "the new footer resolves": that
/// *nothing* still claims the old one. A stale duplicate passes the
/// weaker check and then navigates the user to the wrong node.
#[test]
fn a_deeply_nested_splice_moves_its_ancestors_footers_without_leaving_stale_entries() {
    let mut app = nested_any_fixture();
    // The auto-expanded `Any` itself, three message levels down —
    // deliberately not its `acme.Payload` value, every alternative
    // rendering of which happens to occupy the same three lines in this
    // fixture, so no ancestor footer would move at all.
    let payload =
        node_with_type(&app, "acme.Payload").expect("the Any's value must resolve to acme.Payload");
    let target = app
        .parent(payload)
        .expect("the payload must sit inside the Any");

    // Only ancestors that actually have a footer line of their own — a
    // single-line node's `end - 1` is its header, and the full rebuild
    // records no footer entry for it either.
    let mut ancestors: Vec<(usize, usize)> = Vec::new();
    let mut p = app.parent(target);
    while let Some(pi) = p {
        let r = &app.node_lines(pi);
        if r.end - 1 > r.start {
            ancestors.push((pi, r.end - 1));
        }
        p = app.parent(pi);
    }
    assert!(
        ancestors.len() >= 3,
        "the fixture must be deep enough to exercise the ancestor walk, \
         got {} ancestors with footers",
        ancestors.len()
    );

    // Collapse the whole `Any` subtree into a single scalar line, so
    // every one of those ancestor footers moves up. `string`, not a
    // numeric type: since spec 0219 a LEN record retyped to a packable
    // primitive renders as a packed run, one line per element, which
    // would grow the document instead of shrinking it.
    let lines_before = app.lines.len();
    app.splice_override(target, Some("string".to_string()), false, None)
        .expect("re-typing the Any as a scalar must succeed");
    assert!(
        app.lines.len() < lines_before,
        "the fixture must actually shrink the document: {:#?}",
        app.lines
    );

    for &(ancestor, old_footer) in &ancestors {
        let new_footer = app.node_lines(ancestor).end - 1;
        assert_ne!(
            new_footer, old_footer,
            "the fixture is not proving anything unless ancestor \
             {ancestor}'s footer actually moved"
        );
        assert_eq!(
            app.node_at_footer_line(new_footer),
            Some(ancestor),
            "ancestor {ancestor}'s footer must be mapped at its new line \
             {new_footer}"
        );
        assert_ne!(
            app.node_at_footer_line(old_footer),
            Some(ancestor),
            "a stale entry survived at ancestor {ancestor}'s *old* footer \
             line {old_footer}"
        );
    }
}

/// Spec 0186 S3's asymmetry: the shift walk is guarded by `delta != 0`,
/// the map repair must not be. Re-splicing a node as the type it already
/// has shifts nothing at all, but the repair still dropped every entry
/// at or after the patch and owes them back.
#[test]
fn a_zero_delta_splice_still_repairs_the_line_maps() {
    let mut app = nested_any_fixture();
    let target =
        node_with_type(&app, "acme.Payload").expect("the Any's value must resolve to acme.Payload");
    let lines_before = app.lines.clone();

    app.splice_override(target, Some("acme.Payload".to_string()), false, None)
        .expect("re-splicing a node as its current type must succeed");

    assert_eq!(
        app.lines, lines_before,
        "this is only the zero-delta case if the content is unchanged"
    );
    let mut cur = Some(app.first_node);
    while let Some(c) = cur {
        let r = app.node_lines(c);
        assert_eq!(
            app.node_at_header_line(r.start),
            Some(c),
            "node {c}'s header line {} lost its map entry to a repair \
             that ran only because lines moved",
            r.start
        );
        if r.end - 1 > r.start {
            assert_eq!(
                app.node_at_footer_line(r.end - 1),
                Some(c),
                "node {c}'s footer line {} lost its map entry",
                r.end - 1
            );
        }
        cur = app.doc_next(c);
    }
}

/// Spec 0186 S2 / spec 0188 G1: a batch that queues no patches leaves
/// `pending_patch_min_line` at `None`, and the finalizer then returns
/// without touching anything.
///
/// Spec 0186 read that `None` as "no safe lower bound" and rebuilt the
/// whole document from it. It is the opposite: no patch means no text
/// was replaced and no span was shifted, so there is nothing to
/// rebuild. What this test pins is that the skip really is a no-op —
/// the lines, the visible rows and every line's owner must come out
/// identical.
#[test]
fn a_batch_that_patches_nothing_leaves_the_document_intact() {
    let mut app = nested_any_fixture();
    let lines_before = app.lines.clone();
    let rows_before = app.visible_row_count();
    let owners_before = line_owners(&app);

    // The fixture's own construction already ran this pass, so a second
    // one has nothing left to resettle.
    let cursor = app.cursor;
    app.render_overrides(cursor);

    assert!(
        app.pending_patch_min_line.is_none(),
        "a second identical pass must queue no patches"
    );
    assert_eq!(app.lines, lines_before);
    assert_eq!(app.visible_row_count(), rows_before);
    assert_eq!(line_owners(&app), owners_before);
}

/// The visible rows are walked from the tree's counters rather than
/// held in an array (spec 0210 S2), so ascending order is a property of
/// the walk and cannot be asserted of a stored vector. What is worth
/// pinning is that the walk stays within the spliced document and
/// enumerates every row exactly once, in order.
#[test]
fn the_visible_row_walk_stays_ordered_across_a_splice() {
    fn walked(app: &App) -> Vec<usize> {
        app.visible_window(0, app.visible_row_count())
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    let mut app = nested_any_fixture();
    let before = walked(&app);
    assert!(
        before.windows(2).all(|w| w[0] < w[1]),
        "the visible rows must come out ascending before the splice"
    );

    let target =
        node_with_type(&app, "acme.Payload").expect("the Any's value must resolve to acme.Payload");
    app.splice_override(target, None, false, None)
        .expect("collapsing the payload to raw bytes must succeed");

    let after = walked(&app);
    assert!(
        after.windows(2).all(|w| w[0] < w[1]),
        "...and after it: {after:?}"
    );
    assert!(
        after.iter().all(|&l| l < app.lines.len()),
        "no row may point past the spliced document"
    );
    assert_eq!(
        after.len(),
        app.visible_row_count(),
        "the walk must yield exactly as many rows as the counters claim"
    );
}

/// Spec 0210 test-plan items 12 and 14: a subtree that follows a splice
/// and that the override walk *prunes* must still report the right lines.
///
/// This is the shape S11's deleted code existed for. The walk used to
/// carry the splice's line-count delta down the tree and, on reaching a
/// pruned child, walk that child's whole subtree by `doc_next` to shift
/// every stored range inside it — which on a real document meant
/// shifting most of the arena for an override near the top. Now nothing
/// is shifted, because nothing is stored: `node_lines` sums the
/// counters, and a count does not care what happened above it.
///
/// So the assertions below are unchanged from when they caught the
/// original bug (fold handles and heat cues landing on the closing brace
/// instead of the opening one, everywhere below an override), and that
/// is the point — S11's claim is that it removes work with no observable
/// effect. Running under `verify_repair` also puts item 1's recount
/// oracle on the exact path the deleted arm used to take.
///
/// Retyping `head` from `Wrap` to `Blob` reads the same four bytes as
/// one string line instead of a nested `leaf { v: 5 }`, shortening the
/// document by two lines. `tail` renders identically either way, so the
/// walk prunes it — and its `leaf`/`v` interior is what used to be left
/// two lines behind.
#[test]
fn a_pruned_subtree_after_a_splice_still_reports_its_own_lines() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, _head, tail, tail_leaf, tail_v) = pruned_tail_fixture();
    let root = app.first_node;

    assert_eq!(
        app.lines,
        vec![
            "1 {  #@ Outer = 1",
            "  head {  #@ Wrap = 1",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "  tail {  #@ Wrap = 2",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "}",
        ],
        "fixture must start out as two identically shaped Wraps"
    );

    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/1".to_string(),
        },
        Some("test.Blob".to_string()),
    );
    app.render_overrides(root);

    assert_eq!(
        app.lines,
        vec![
            "1 {  #@ Outer = 1",
            "  head {  #@ Blob = 1",
            "    leaf: \"\\010\\005\"  #@ string = 1",
            "  }",
            "  tail {  #@ Wrap = 2",
            "    leaf {  #@ Leaf = 1",
            "      v: 5  #@ int32 = 1",
            "    }",
            "  }",
            "}",
        ],
        "retyping head as Blob must shorten the document by two lines"
    );

    // The premise: had the walk descended into `tail`, the recursion
    // would have shifted its interior and this test would prove nothing.
    // Marks persist across batches (spec 0188 S4), so this reads the
    // decision the batch actually made.
    assert!(
        !app.descend[tail],
        "the walk must have pruned `tail` for this test to mean anything"
    );

    for (node, name, want) in [
        (tail, "tail", 4..9),
        (tail_leaf, "tail.leaf", 5..8),
        (tail_v, "tail.leaf.v", 6..7),
    ] {
        assert_eq!(
            app.node_lines(node),
            want,
            "{name} reports pre-splice line numbers, so the pruned \
             subtree's interior did not follow the splice"
        );
    }

    // And the symptom itself: the fold handle and the heat cue come from
    // the header/footer resolution alone, so this is the assertion a
    // user would recognize.
    for (line, node, name) in [
        (4, tail, "tail's `{` line"),
        (5, tail_leaf, "tail.leaf's `{` line"),
        (6, tail_v, "tail.leaf.v's own scalar line"),
    ] {
        assert_eq!(app.node_at_header_line(line), Some(node), "{name}");
    }
    for (line, node, name) in [
        (8, tail, "tail's `}` line"),
        (7, tail_leaf, "tail.leaf's `}` line"),
    ] {
        assert_eq!(app.node_at_footer_line(line), Some(node), "{name}");
    }
    assert_eq!(
        app.node_at_footer_line(6),
        None,
        "a scalar line must never carry a fold handle"
    );
}

/// A seeded random walk of override activations and toggles over every
/// document shape the suite has a fixture for.
///
/// The hand-written tests around it each pin one situation someone
/// already thought of. This one exists for the situations nobody did —
/// and the bug it was written after is exactly that kind: it needed a
/// splice *followed by* a subtree the walk prunes, a combination no
/// fixture had, because a fixture is built to demonstrate one mechanism
/// and this bug lives in the interaction of two.
///
/// It asserts nothing itself. Everything it can catch is asserted by
/// `finalize_override_batch` under `verify_repair` — the span/text
/// consistency oracle and the spec 0186 G3 map equivalence — so this is
/// purely an input generator, and any new invariant hung off that
/// finalizer is exercised by these sequences for free.
///
/// Deliberately seeded rather than randomly seeded: a failure has to
/// replay exactly, and a test that fails one run in twenty is a test
/// people learn to re-run.
#[test]
fn randomized_override_sequences_keep_every_span_consistent() {
    use crate::override_pane::OverrideOrigin;

    for seed in 1u64..=6 {
        for (shape, mut app) in [
            ("pruned_tail", pruned_tail_fixture().0),
            ("repeated_message", repeated_message_fixture().0),
            ("packed_run_with_tail", packed_run_with_tail_fixture().0),
            ("nested_packed_run", nested_packed_run_fixture()),
            ("nested_any", nested_any_fixture()),
            ("message_set", message_set_fixture()),
            ("nested_message_set", nested_message_set_fixture()),
        ] {
            // The candidate types are the ones this document actually
            // mentions, plus raw — so each shape is retyped into things
            // that are plausible for it rather than into a fixed list
            // that most fixtures would reject outright.
            let mut types: Vec<Option<String>> = vec![None];
            let mut cur = Some(app.first_node);
            while let Some(c) = cur {
                if let Some(t) = type_name_of(&app, c) {
                    let t = t.to_owned();
                    if !types.iter().any(|k| k.as_deref() == Some(t.as_str())) {
                        types.push(Some(t));
                    }
                }
                cur = app.doc_next(c);
            }

            let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut next = move || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) as usize
            };

            for step in 0..40 {
                if next() % 10 < 7 {
                    let mut pick = None;
                    for _ in 0..50 {
                        let l = next() % app.lines.len();
                        if let Some(i) = app.node_at_header_line(l) {
                            if app.can_override(i) {
                                pick = Some(i);
                                break;
                            }
                        }
                    }
                    let Some(idx) = pick else { continue };
                    let ty = types[next() % types.len()].clone();
                    let path = app.positional_path(idx);
                    // Printed, not asserted: cargo shows a failing
                    // test's stdout, so the sequence that reached the
                    // failure is in the report without costing anything
                    // on the passing runs.
                    println!("{shape} seed {seed} step {step}: {path} as {ty:?}");
                    app.overrides
                        .activate(OverrideOrigin::Path { path }, ty.clone());
                    app.render_overrides(app.first_node);
                } else if !app.overrides.entries().is_empty() {
                    let e = next() % app.overrides.entries().len();
                    println!("{shape} seed {seed} step {step}: toggle entry {e}");
                    app.overrides.toggle_active(e);
                    app.render_overrides(app.first_node);
                }
            }
        }
    }
}

/// A packed run's `packed_record_start` is a byte offset into
/// `self.blob`, read back with `parse_wiretag`/`parse_varint` by
/// `packed_record_extent`, `extract::message_payload_range` and the heat
/// cue. A *splice* is where it used to go wrong: `prototext-core` hands
/// the offset back in the retyped node's own byte frame, and
/// `splice_override` had to translate it into the document's.
///
/// It did not, and the consequence was not a cosmetic one. On
/// `/tmp/pdb.desc`, retyping a `SourceCodeInfo.Location` and then one of
/// the `span:` elements the retype produced made `packed_record_extent`
/// parse a tag and a length out of unrelated bytes near the start of the
/// file: a 2-byte varint was replaced by a re-render of the whole 1 MB
/// blob, the tree came back with a node as its own descendant, and
/// `collect_descendants` recursed until the stack ran out. A 256 MB
/// stack did not help, which is what identified it as unbounded rather
/// than deep.
///
/// Spec 0216 S12 removes the translation rather than fixing it: the
/// offset is taken from the arena, which is expressed against
/// `self.blob` by construction and never saw the retyped node's frame.
/// The property is unchanged and still worth pinning — this is the test
/// that would have caught the original bug — so it is asserted here of
/// the end state rather than of the translation step.
#[test]
fn a_splice_keeps_packed_record_starts_in_the_documents_byte_frame() {
    use crate::override_pane::OverrideOrigin;

    let mut app = nested_packed_run_fixture();
    // The two frames must genuinely differ, or this test proves nothing:
    // `blob`'s own tag is at byte 2, so everything the retype decodes is
    // reported 2 bytes lower than it really sits.
    assert_eq!(
        &app.blob[..],
        &[0x0A, 0x09, 0x08, 0x01, 0x12, 0x05, 0x0A, 0x03, 0x05, 0x06, 0x07]
    );
    let blob_node = app.resolve_path("/2").expect("blob is /2");
    assert_eq!(app.tree[blob_node].span.raw_range, 4..11);

    app.overrides.activate(
        OverrideOrigin::Path {
            path: "/2".to_string(),
        },
        Some("test.Payload".to_string()),
    );
    app.render_overrides(app.first_node);

    // Spec 0216 S22: the run is one node covering all three elements,
    // where it used to be three sibling nodes sharing one record.
    assert_eq!(app.child_count(blob_node), 1, "Payload.vals is one record");
    let run = app.nth_child(blob_node, 0).expect("the run");
    assert_eq!(
        app.tree[run].lines_total, 3,
        "the record draws one row per packed int"
    );
    assert_eq!(
        app.tree[run].span.packed_record_start, 6,
        "the packed record's tag is at byte 6 of the document; a value \
         of 2 is the retyped node's own frame leaking out"
    );
    // The offset is only ever used by way of a re-parse, so assert the
    // thing that re-parse produces: tag 0x0A at 6, length 3 at 7,
    // payload 8..11.
    let (raw, _text) = app.packed_record_extent(run);
    assert_eq!(raw, 6..11);
}

/// Spec 0221 S5/N5: a refusal raised from inside a running session
/// keeps reporting through the status line. Only the *startup* pass
/// changed channel, because only it had nowhere to be seen; a refusal
/// the user just caused by typing a command is exactly what
/// `self.message` is for (spec 0147 G5), and the TUI owns the terminal
/// by then, so stderr is not available to it anyway.
#[test]
fn a_session_refusal_still_uses_the_status_line() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, inner, _) = type_as_fixture();
    let path = app.positional_path(inner);
    app.overrides.activate(
        OverrideOrigin::Path { path },
        Some("test.NoSuchType".to_string()),
    );
    app.message.clear();
    app.render_overrides(app.first_node);

    assert_eq!(
        app.refusals.len(),
        1,
        "the node must be recorded as refused"
    );
    assert!(
        app.message.starts_with("cannot apply override: "),
        "a single refusal keeps the wording spec 0202 introduced: {}",
        app.message
    );
    assert!(
        app.message.contains("test.NoSuchType"),
        "the status line names the type that was asked for: {}",
        app.message
    );
}

/// Spec 0221 G3/S1: the second refusal of a pass no longer destroys the
/// first. The status line can only carry one, so it says how many there
/// were and shows the first; `refusals` carries both for the caller
/// that can print them.
#[test]
fn a_pass_with_two_refusals_reports_both() {
    use crate::override_pane::OverrideOrigin;

    let (mut app, inner, _) = type_as_fixture();
    let inner_path = app.positional_path(inner);
    let root_path = app.positional_path(app.first_node);
    app.overrides.activate(
        OverrideOrigin::Path { path: inner_path },
        Some("test.NoSuchType".to_string()),
    );
    app.overrides.activate(
        OverrideOrigin::Path { path: root_path },
        Some("test.AlsoMissing".to_string()),
    );
    app.render_overrides(app.first_node);

    assert_eq!(app.refusals.len(), 2, "both refusals survive the pass");
    assert!(
        app.message.starts_with("2 overrides refused"),
        "the status line reports the count it cannot show in full: {}",
        app.message
    );
}
