// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::super::heat_worker::{HeatRequest, HeatWorkerHandle};
use super::super::render::{window_styles_for, ACTIVITY_GLYPH};
use super::super::*;
use super::support::*;
use prototext_core::serialize::encode_text::annotation_start;
use ratatui::buffer::Buffer;

/// Regression test: a legitimately-empty decode (e.g. reopening an
/// extracted `google.protobuf.Empty`, or any all-default submessage —
/// decoding zero bytes yields zero `TreeNode`s, not an error) must not
/// panic on the first `render()` call or on keypresses, now that
/// `main.rs` no longer refuses to open such a blob.
#[test]
fn empty_tree_renders_and_handles_keys_without_panicking() {
    let mut app = empty_app();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // Dismiss the startup splash first (any key), then exercise a
    // handful of keys that are unguarded `self.tree[...]` indexing
    // sites for a non-empty tree.
    app.splash = false;
    // `z` goes last: spec 0194 S6 made it a chord prefix, so anything
    // after it would be eaten as the chord's second half.
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Char('0'),
        KeyCode::Char('$'),
        KeyCode::Char('%'),
        KeyCode::Char(' '),
        KeyCode::Char('x'),
        KeyCode::End,
        KeyCode::Char('z'),
        KeyCode::Char('c'),
    ] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    // Quitting from an empty tree still works: `handle_key` returns
    // early on one, so `:` has to be dispatched before that gate — it
    // is now the only way out (spec 0236 S20).
    for code in [KeyCode::Char(':'), KeyCode::Char('q'), KeyCode::Enter] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    assert!(app.should_quit);
}

/// A document nested as deeply as the wire walk will accept.
///
/// `MAX_WIRE_DEPTH` is 1000 and real documents run to about a dozen
/// (measured on googleapis: 13), so every level between the two is
/// reachable and untried. What is down there is recursion — the render
/// walk, the override walk that `App::new` runs over the whole document,
/// and `render_node_as`, which recurses per nested override — and a
/// stack that runs out does not report anything, it takes the process
/// with it.
///
/// The document is built at the cap rather than at some comfortable
/// depth: a limit is only a limit if something has stood at it.
fn deeply_nested_app(depth: usize) -> App {
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{FileDescriptorProto, FileDescriptorSet};

    // `Nest { Nest inner = 1; int32 leaf = 2; }` — self-recursive, so
    // one message type describes a document of any depth.
    let fds = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            name: Some("test_deep.proto".to_string()),
            package: Some("test".to_string()),
            syntax: Some("proto2".to_string()),
            message_type: vec![message(
                "Nest",
                vec![
                    field_of("inner", 1, Label::Optional, Type::Message, ".test.Nest"),
                    field("leaf", 2, Label::Optional, Type::Int32),
                ],
            )],
            ..Default::default()
        }],
    };

    // Built from the innermost outward, since each level's length prefix
    // covers everything already encoded.
    let mut blob: Vec<u8> = vec![0x10, 0x01]; // leaf: 1
    for _ in 0..depth {
        let mut level = vec![0x0A]; // tag: field 1, WT_LEN
        let mut len = blob.len();
        while len >= 0x80 {
            level.push((len as u8) | 0x80);
            len >>= 7;
        }
        level.push(len as u8);
        level.append(&mut blob);
        blob = level;
    }

    fixture_under("deep", &fds, "test.Nest", &blob)
}

#[test]
fn a_document_nested_to_the_wire_depth_limit_opens_and_draws() {
    // The deepest document the walk accepts, found by trying: the
    // wrapper record the blob is opened inside and the innermost `leaf`
    // field each take a level off the top, and the walk keeps the last.
    // One more and `build_arena` refuses the input, which is a different
    // test than this one.
    //
    // `render_overrides_inner` is recursive and fires from `App::new`,
    // so constructing the fixture alone can overflow the test thread's
    // default 8 MiB stack in a debug build (~11.5 KiB per frame ×
    // 997 frames ≈ 11.5 MiB). Run the body on a thread sized for the
    // worst case, matching `SCORING_THREAD_STACK_SIZE`'s reasoning in
    // `sweep.rs` which faces the same bound.
    std::thread::Builder::new()
        .stack_size(crate::sweep::SCORING_THREAD_STACK_SIZE)
        .spawn(|| {
            let depth = prototext_core::helpers::MAX_WIRE_DEPTH - 3;
            let mut app = deeply_nested_app(depth);

            // Each level renders an opening line and a closing one, so
            // the document is deep in rows as well as in structure — a
            // fixture that decoded to a handful of lines would prove
            // nothing about either.
            assert!(
                app.document_lines().len() > depth,
                "{} lines for {depth} levels",
                app.document_lines().len()
            );

            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();

            // The line-owner descent walks one level per iteration, so
            // the deepest line is the longest walk in the program. Asked
            // for at the very bottom, where the nesting is.
            let last = app.document_lines().len() - 1;
            assert!(app.line_pos(last).is_some(), "no owner for the last line");
            assert!(app.line_pos(app.document_lines().len() / 2).is_some());

            // To the bottom and back, folding on the way — the recursive
            // walks are over the *visible* structure, so a fold is what
            // makes them re-run at every depth rather than once.
            for code in [
                KeyCode::End,
                KeyCode::Char(' '),
                KeyCode::Home,
                KeyCode::Char(' '),
                KeyCode::Char(' '),
                KeyCode::End,
            ] {
                app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
                terminal.draw(|frame| app.render(frame)).unwrap();
            }
        })
        .expect("thread spawn failed")
        .join()
        .expect("deep-nesting test panicked");
}

/// A terminal with no room in it, in every layout the program can be
/// asked to draw.
///
/// `render` lays out by subtraction — a `Length(1)` global row taken off
/// the bottom, another off each pane for its own statusline, a
/// `Length(1)` separator column between two halves — and every one of
/// those subtractions has nothing to take from at zero. The arithmetic
/// downstream is on `usize`, where coming up short is not a small number
/// but a panic, and it is reached again on every key that measures the
/// page it is scrolling.
///
/// A zero-size terminal is not hypothetical: it is what a window manager
/// reports mid-resize, and what a pty is between its creation and its
/// first `TIOCSWINSZ`. `1x1` and `2x1` are here for the same reason — a
/// single subtraction is survivable, two in a row are not, and the sizes
/// that expose the difference are exactly the ones nobody draws at.
#[test]
fn a_terminal_with_no_room_in_it_still_draws() {
    let (mut app, inner_idx, _) = type_as_fixture();
    app.splash = false;

    for (width, height) in [(0, 0), (1, 1), (2, 1), (1, 2), (3, 3), (0, 24), (80, 0)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        // The three layouts: the main pane alone, the 50/50 split with
        // the override selection pane, and the same split with the
        // management pane.
        for open in 0..3 {
            app.override_target = if open == 1 { Some(inner_idx) } else { None };
            app.manage_open = open == 2;
            terminal.draw(|frame| app.render(frame)).unwrap();

            // Keys that measure the pane they are moving through — a
            // page is `main_area.height` rows, which is what is zero
            // here — with a draw after each, since the clamping that
            // reconciles a movement with the pane happens in `render`.
            for code in [
                KeyCode::PageDown,
                KeyCode::PageUp,
                KeyCode::End,
                KeyCode::Home,
                KeyCode::Down,
                KeyCode::Right,
            ] {
                app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
                terminal.draw(|frame| app.render(frame)).unwrap();
            }
        }
    }
}

/// Spec 0192 G1: a frame costs the same wherever the cursor is. The
/// regression this guards is `positional_path` becoming O(ordinal)
/// again — which it was, via `sibling_position`'s `prev_sibling` walk,
/// once per drawn row. On a 20 000-sibling document that made the last
/// screenful roughly four orders of magnitude more expensive to draw
/// than the first (measured on `googleapis.desc`: 80 us of override
/// resolution at the top, 17 080 us at the end).
///
/// A timing assertion, because the property is about cost and nothing
/// structural distinguishes the two frames. The bound is deliberately
/// loose — a regression here is not a 3x one, it is a 100x one — so
/// scheduler noise cannot produce a false failure.
#[test]
fn a_frame_costs_the_same_at_the_end_of_a_wide_document_as_at_the_start() {
    const N: usize = 20_000;
    let mut app = wide_sibling_scalars_app(N);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

    // Draw once before timing anything: the first frame of a fresh
    // `App` pays one-off warm-up (the startup render's line map, the
    // syntax-highlighting window's first fill) that belongs to neither
    // position.
    terminal.draw(|frame| app.render(frame)).unwrap();

    let time_here = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
        // Best of several: the minimum is the run least disturbed by
        // the scheduler, and it is the ratio of the two minima that
        // carries the signal.
        (0..5)
            .map(|_| {
                let t = Instant::now();
                terminal.draw(|frame| app.render(frame)).unwrap();
                t.elapsed()
            })
            .min()
            .expect("at least one sample")
    };

    // `render` clamps `scroll.index` to the cursor itself
    // (render.rs:507), so moving the cursor is all it takes.
    app.cursor = 0;
    let at_start = time_here(&mut app, &mut terminal);

    app.cursor = N - 1;
    let at_end = time_here(&mut app, &mut terminal);

    assert!(
        at_end.as_nanos() <= at_start.as_nanos().saturating_mul(10),
        "drawing the end of a {N}-sibling document took {at_end:?} against \
         {at_start:?} at the start — a per-row cost that scales with the \
         cursor's ordinal position is back"
    );
}

/// One node drawing `n` rows, its text held as spec 0222 S1 holds a
/// packed run's: all `n` lines joined by `\n` in a single allocation.
///
/// Synthetic — a real run of this length would need a 100 000-element
/// blob to decode — but it is the exact shape S4 is about, since what
/// makes the k-th row cost k is that the run is one string.
fn long_packed_run_app(n: usize) -> App {
    let mut app = wide_sibling_scalars_app(1);
    let text: Vec<String> = (0..n).map(|i| format!("  {i}")).collect();
    app.node_text_mut()[0] = Some(text.join("\n").into_boxed_str());
    app.tree_mut()[0].lines_total = n as u32;
    app.tree_mut()[0].lines_visible = n as u32;
    app
}

/// Spec 0222, test-plan item 5: a frame drawn deep inside a long
/// packed run scans the run once, not once per row.
///
/// The run's rows share one string, so the k-th of them starts k
/// newlines in. Resolving each drawn row's offset on its own would
/// make a 48-row frame at row 90 000 scan 4.3 M bytes instead of the
/// 1.7 M the entry costs once — S4's byte cursor is what collapses
/// the 48 into 1.
///
/// So the comparison is a full frame against a single row *at the
/// same place*: both pay the one entry scan, and only the broken
/// version pays it again per row. Comparing against a frame at the
/// top would not work — the entry scan is legitimately absent there.
///
/// A timing assertion for the same reason as the test above, and with
/// the same deliberately loose bound: the regression is a 48x one.
#[test]
fn a_frame_inside_a_long_packed_run_scans_once() {
    const N: usize = 100_000;
    const AT: usize = 90_000;
    const HEIGHT: usize = 48;
    let app = long_packed_run_app(N);

    // The offsets must be the real line starts, or the frame is fast
    // and wrong.
    let window = app.build_window(AT, HEIGHT);
    assert_eq!(window.len(), HEIGHT);
    for (k, row) in window.iter().enumerate() {
        let DisplayRow::Committed(c) = *row else {
            panic!("row {k} is not committed");
        };
        assert_eq!(c.line, AT + k);
        assert_eq!(app.line_text(c.pos), format!("  {}", AT + k));
        assert_eq!(
            app.line_text_at(c.pos, c.offset),
            app.line_text(c.pos),
            "row {k}: the walked offset must name the same line the \
             from-scratch resolution does"
        );
    }

    // Best of several, as above: the minimum is the run least
    // disturbed by the scheduler, and it is the ratio that carries the
    // signal.
    let time = |count: usize| {
        (0..5)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(app.build_window(AT, count));
                t.elapsed()
            })
            .min()
            .expect("at least one sample")
    };
    let one_row = time(1);
    let whole_frame = time(HEIGHT);

    assert!(
        whole_frame.as_nanos() <= one_row.as_nanos().saturating_mul(10),
        "a {HEIGHT}-row frame at row {AT} of a {N}-line run took \
         {whole_frame:?} against {one_row:?} for a single row there — the \
         entry scan is being paid per row again"
    );
}

/// Spec 0192 S2: the active-override check runs once per addressable
/// record, not once per drawn row — a packed run draws one row per
/// element but is one record (spec 0184 S2, one node since spec 0216),
/// so its rows share one positional path and one answer.
#[test]
fn the_override_check_runs_once_per_record_not_once_per_row() {
    let (app, run, ..) = packed_run_with_tail_fixture();
    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();

    let (flags, resolutions) = app.override_emphasis(&window);
    assert_eq!(flags.len(), window.len());

    // Or the count assertion below would pass vacuously.
    assert!(
        app.node_lines(run).len() > 1,
        "fixture must contain a multi-element run"
    );

    // The whole answer, recomputed with no collapsing at all.
    let naive: Vec<Modifier> = window
        .iter()
        .map(|&row| match row {
            DisplayRow::Committed(c) => app
                .node_at_own_line(c.line)
                .and_then(|idx| app.resolve_active_override_entry(idx))
                .map_or(Modifier::empty(), |e| {
                    if e.auto {
                        Modifier::BOLD
                    } else {
                        Modifier::BOLD | Modifier::UNDERLINED
                    }
                }),
            DisplayRow::Overlay(_) => Modifier::empty(),
        })
        .collect();
    assert_eq!(flags, naive, "collapsing must not change the answer");

    let rows_with_a_node = window
        .iter()
        .filter(|&&row| match row {
            DisplayRow::Committed(c) => app.node_at_own_line(c.line).is_some(),
            DisplayRow::Overlay(_) => false,
        })
        .count();
    assert!(
        resolutions < rows_with_a_node,
        "{resolutions} resolutions for {rows_with_a_node} rows carrying a \
         node — the per-row work did not collapse at all"
    );
}

/// The runs of characters `drawn` gives a different style from `plain`
/// — the emphasis, read off the finished spans character by character
/// rather than off the span list's shape, which the fold margin now
/// splits or does not split depending on what it has to say.
fn restyled_runs(plain: &[Span<'static>], drawn: &[Span<'static>]) -> Vec<String> {
    let flatten = |spans: &[Span<'static>]| -> Vec<(char, Style)> {
        spans
            .iter()
            .flat_map(|s| {
                let style = s.style;
                s.content.chars().map(move |c| (c, style))
            })
            .collect()
    };
    let (plain, drawn) = (flatten(plain), flatten(drawn));
    assert_eq!(
        plain.len(),
        drawn.len(),
        "the emphasis must not change what the row says"
    );
    let mut runs: Vec<String> = Vec::new();
    let mut open = false;
    for ((_, was), (c, now)) in plain.iter().zip(drawn.iter()) {
        if was == now {
            open = false;
            continue;
        }
        if !open {
            runs.push(String::new());
            open = true;
        }
        runs.last_mut().expect("just pushed").push(*c);
    }
    runs
}

/// Spec 0192 S2, as revised 2026-08-05: an active override weights the
/// three places that say what an override *is* — the row's key, its
/// fold marker, and the type name in its `#@ Type = N` annotation — and
/// nothing else. A deliberate override is underlined as well as bold;
/// an auto-derived one (spec 0120) is not.
///
/// Both halves matter. The `a` case is why the key is on the list at
/// all: with the annotations hidden there is no type name left, and the
/// key is what keeps the row from reading as un-overridden. The bare
/// footer is the deliberate loss — it has none of the three, and the
/// header it closes is what speaks for the node.
///
/// The comparison is against the same row drawn with no emphasis, so it
/// states "only these" directly rather than by enumerating the
/// modifiers every other span happens to carry.
///
/// The type name is the one target whose cue is not always *visible*:
/// the ANSI-16 fallback palette already spends bold on `Type`, so an
/// auto-derived override adds nothing the eye can see there. That is a
/// property of the palette, not of this pass, and the expectation is
/// read off `theme::style_for` rather than assumed — the terminal the
/// suite runs under is not fixed, and a Nix sandbox has no `COLORTERM`.
/// It also makes the point the three targets exist for: the key and the
/// marker are unstyled in both palettes, so they carry the cue when the
/// type name cannot.
#[test]
fn an_override_weights_the_key_the_marker_and_the_type_name() {
    let mut app = nested_any_fixture();
    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();

    // The fixture's root carries the deliberate override that named its
    // type; the `Any` payload carries the auto-derived one. Both are
    // bracketed, so each also has a footer row carrying the same
    // override and none of the three places.
    let row_of = |needle: &str| {
        window
            .iter()
            .position(|&r| app.row_content(r).contains(needle))
            .unwrap_or_else(|| panic!("no row containing {needle:?}"))
    };
    let manual = row_of("#@ Level1");
    let auto = row_of("#@ Payload");
    let glyph = render::FOLD_GLYPH_OPEN.to_string();

    let check = |app: &mut App, want_manual: &[String], want_auto: &[String]| {
        app.refresh_window_styles(&window);
        let (emphasis, _) = app.override_emphasis(&window);
        assert_eq!(emphasis[manual], Modifier::BOLD | Modifier::UNDERLINED);
        assert_eq!(emphasis[auto], Modifier::BOLD);

        for (i, &row) in window.iter().enumerate() {
            let plain = app.row_spans(row, i, Modifier::empty());
            let drawn = app.row_spans(row, i, emphasis[i]);
            // The glyph takes the weight but never the underline: a
            // rule drawn straight through a triangle reads as neither.
            for span in &drawn {
                assert!(
                    !span.content.contains(render::FOLD_GLYPH_OPEN)
                        || !span.style.add_modifier.contains(Modifier::UNDERLINED),
                    "row {i}: the fold glyph is underlined"
                );
            }
            let want: &[String] = match i {
                _ if i == manual => want_manual,
                _ if i == auto => want_auto,
                _ => &[],
            };
            assert_eq!(
                restyled_runs(&plain, &drawn),
                want,
                "row {i}: {:?}",
                app.row_content(row)
            );
        }
    };

    let run = |parts: &[&str]| -> Vec<String> { parts.iter().map(|s| s.to_string()).collect() };

    // Does weighting `Type` change how it is drawn at all? Under RGB it
    // always does; under ANSI-16 a bare `BOLD` does not.
    let plain_type = theme::style_for(SyntaxRole::Type, app.theme);
    let shows = |emphasis: Modifier| plain_type.add_modifier(emphasis) != plain_type;
    let with_type = |mut base: Vec<String>, emphasis: Modifier, name: &str| {
        if shows(emphasis) {
            base.push(name.to_string());
        }
        base
    };
    check(
        &mut app,
        &with_type(
            run(&[&glyph, "1"]),
            Modifier::BOLD | Modifier::UNDERLINED,
            "Level1",
        ),
        &with_type(run(&[&glyph, "value"]), Modifier::BOLD, "Payload"),
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(!app.annotations, "`a` must have hidden the annotations");
    check(&mut app, &run(&[&glyph, "1"]), &run(&[&glyph, "value"]));
}

/// Spec 0307 S6: the wrapper root's key is the `1` of the field `Blob`
/// wrapped the document in — a number protolens wrote, not one the file
/// carries — and the document row marks it the way the wire row marks
/// the bytes that spell it. No other row's key is marked, because no
/// other row's key was invented.
///
/// The assertion is about the key alone, not about the whole row: the
/// ANSI-16 fallback palette already spends italic on `Comment`, so every
/// row's `#@` annotation wears it there and "only this row is italic"
/// would be false in a terminal without `COLORTERM` — which the Nix
/// sandbox is.
#[test]
fn the_wrapper_roots_key_says_protolens_wrote_it() {
    let (mut app, ..) = packed_run_with_tail_fixture();
    assert!(app.wrapper_offset > 0, "the fixture must be wrapped");

    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();
    app.refresh_window_styles(&window);

    // The key is the row's first glyph past the fold margin and the
    // indent, read off the finished spans so that what is measured is
    // what is drawn.
    let key = |app: &App, i: usize| -> (char, bool) {
        let (c, style) = app
            .row_spans(window[i], i, Modifier::empty())
            .iter()
            .flat_map(|s| {
                let style = s.style;
                s.content.chars().map(move |c| (c, style))
            })
            .find(|&(c, _)| c.is_alphanumeric())
            .unwrap_or_else(|| panic!("row {i} has no key: {:?}", app.row_content(window[i])));
        (c, style.add_modifier.contains(theme::SYNTHETIC))
    };

    assert_eq!(app.parent(app.line_pos(0).expect("line 0").node), None);
    assert_eq!(key(&app, 0), ('1', true), "the root's key is protolens'");

    let child = (1..window.len())
        .find(|&i| {
            matches!(window[i], DisplayRow::Committed(c)
                if app.node_at_own_line(c.line).is_some_and(|n| app.parent(n).is_some()))
        })
        .expect("the fixture has a child row");
    assert!(!key(&app, child).1, "row {child}: a real key is not marked");
}

/// A passive status message auto-dismisses once `MESSAGE_TIMEOUT` has
/// elapsed since it was set — detected by `track_message_timeout`
/// (called from `render`) noticing an expired `message_deadline`.
#[test]
fn message_auto_dismisses_after_timeout() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.message = "pattern not found: xyz".to_string();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.message_deadline.is_some());
    assert_eq!(app.message, "pattern not found: xyz");

    // Not yet expired: still showing on a later render.
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.message, "pattern not found: xyz");

    // Force expiry (real time never actually elapses in a unit test).
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.message.is_empty());
    assert!(app.message_deadline.is_none());
}

/// Item 13 of 2026-07-17 feedback: the startup splash auto-dismisses
/// once `SPLASH_TIMEOUT` has elapsed, in addition to its existing
/// keypress/mouse dismissal — detected by `track_splash_timeout`
/// (called from `render`) noticing an expired `splash_deadline`.
#[test]
fn splash_auto_dismisses_after_timeout() {
    let mut app = empty_app();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    assert!(app.splash);
    terminal.draw(|frame| app.render(frame)).unwrap();
    // Not yet expired: still showing on a later render.
    assert!(app.splash);

    // Force expiry (real time never actually elapses in a unit test).
    app.splash_deadline = Instant::now() - Duration::from_millis(1);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!app.splash);
}

/// Spec 0197 test 9 (§S3, second channel). A TUI launch is exactly the
/// case where nobody was watching stderr, so the splash pane repeats
/// the eager-fallback warning.
#[test]
fn the_eager_fallback_warning_reaches_the_splash_pane() {
    let mut app = eager_fallback_app();
    assert!(app.splash);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("warning:") && text.contains("index.rkyv"),
        "the splash must name the fallback: {text:?}"
    );
}

/// Spec 0197 test 10 (§S3, third channel). The splash pane is gone
/// after `SPLASH_TIMEOUT`, which is the case the status line covers: a
/// user who is still reading the document at t+10 s can still find out
/// why startup was slow.
///
/// The bound on this is spec 0147 G5, asserted below: the *first
/// keypress* clears the status line unconditionally, and that rule is
/// not carved out for this warning. So the third channel buys
/// persistence in wall-clock time, not persistence across interaction.
#[test]
fn the_eager_fallback_warning_survives_the_splash_timing_out() {
    let mut app = eager_fallback_app();
    assert!(
        app.message.contains("index.rkyv"),
        "App::new must seed the status line: {:?}",
        app.message
    );

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    app.splash_deadline = Instant::now() - Duration::from_millis(1);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(!app.splash, "the splash must have timed out");

    assert!(
        app.message.contains("index.rkyv"),
        "the warning must outlive the pane that also showed it: {:?}",
        app.message
    );

    // ...but only until the user does something. Spec 0147 G5.
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(app.message.is_empty());
}

/// A message never auto-dismisses while the bottom bar is actively
/// serving as a text-entry prompt (`command_buffer`): it is awaiting a
/// keypress, unlike a plain notice. Since spec 0236 S20 dropped the
/// `q` quit confirmation, that is the only such state left.
#[test]
fn message_is_not_dismissed_while_a_prompt_is_active() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.message = "some notice".to_string();
    app.command_buffer = Some(String::new());
    terminal.draw(|frame| app.render(frame)).unwrap();
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        app.message, "some notice",
        "prompt active: must not dismiss"
    );

    app.command_buffer = None;
    app.message_deadline = Some(Instant::now() - Duration::from_millis(1));
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.message.is_empty(), "prompt closed: must dismiss");
}

/// Spec 0133 G3/G4: the main-pane `a` key toggles display of each
/// line's trailing `#@ ...` annotation, purely at render time — the
/// underlying `self.document_lines()`/`self.line_styles` are untouched, so
/// toggling `a` twice restores byte-for-byte identical rendering.
/// Distinct from the override pane's own `i` (candidate sort,
/// exercised above) and the manage pane's own `a` (entry active
/// toggle) — this fixture has neither pane open, so only the
/// main-pane binding is reachable.
#[test]
fn a_toggles_the_main_pane_annotation_display() {
    let line = "  id: 5  #@ int32 = 1".to_string();
    let node = TreeNode {
        span: NodeSpan {
            field_number: 1,
            raw_range: 0..2,
            text_range: 0..1,
            level: 0,
            type_fqdn: NO_FQDN,
            is_message: false,
            packed_record_start: NO_PACKED_RECORD,
            wire_type: WT_VARINT as u8,
        },
        lines_total: 1,
        lines_visible: 1,
        rendered_as: NOT_RENDERED,
    };
    let decoded = Decoded {
        total_lines: 1,
        // Spec 0257 S1: a hand-built document was never bounded.
        stops: Vec::new(),
        // Spec 0323 S2: a hand-built tree writes its own counts, so
        // nothing in it is folded.
        user_folded: FoldSet::default(),
        row_budget: None,
        node_text: vec![Some(Box::from(line.as_str()))],
        tree: vec![node],
        root_type: "test.Msg".to_string(),
        arena: crate::decode::arena_of(&[0x08, 0x05]),
        blob: Arc::new(Blob::unwrapped(vec![0x08, 0x05])),
        wrapper_offset: 0,
        root_candidates: Vec::new(),
        fqdns: FqdnTable::new(),
    };
    let mut app = app_named(decoded, DescriptorContext::empty_for_test(), "test.pb");
    app.splash = false;

    // Spec 0193 S1: every row carries the two-column fold field, blank
    // here since this node has nothing to fold.
    assert!(app.annotations);
    assert_eq!(
        app.row_content(app.committed_row(0).unwrap()),
        format!("  {line}")
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(!app.annotations);
    assert_eq!(app.row_content(app.committed_row(0).unwrap()), "    id: 5");
    // Spec 0187 S3: `row_spans` reads `window_styles`, which is a
    // per-frame product of the rows being drawn, so the window has to be
    // established before the row's spans can be asked for. Index 0 is
    // this one-row window's only position.
    let window = [app.committed_row(0).unwrap()];
    app.refresh_window_styles(&window);
    let spanned: String = app
        .row_spans(window[0], 0, Modifier::empty())
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(spanned, "    id: 5");

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(app.annotations);
    assert_eq!(
        app.row_content(app.committed_row(0).unwrap()),
        format!("  {line}")
    );
}

/// Spec 0223 test 3, guarding G3. A frame drawn while terminal events
/// are still queued drops *syntax highlighting only*: it must be the
/// same screen, character for character, with the cursor row still
/// picked out — mid-scroll the cursor is how the user knows where they
/// are, and a frame that loses it is worse than a slow one.
#[test]
fn a_pending_frame_has_no_syntax_styles_but_keeps_its_chrome() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    app.input_pending = true;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        app.window_styles.is_empty(),
        "a pending frame must not run tree-sitter"
    );
    let pending = terminal.backend().buffer().clone();

    app.input_pending = false;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        app.window_styles.iter().any(|hints| !hints.is_empty()),
        "a settled frame over the same window must be highlighted"
    );
    let settled = terminal.backend().buffer().clone();

    // Same text, same glyphs, same positions — only the colors differ.
    let symbols = |buf: &Buffer| -> Vec<String> {
        buf.content.iter().map(|c| c.symbol().to_string()).collect()
    };
    assert_eq!(symbols(&pending), symbols(&settled));

    // And the cursor row survives. Its tint is a background, which
    // `window_styles` never sets, so it must be identical in both.
    let cursor_bg = theme::cursor_row_style(app.theme).bg;
    assert!(cursor_bg.is_some(), "the fixture's theme must tint the row");
    let tinted = |buf: &Buffer| -> usize {
        buf.content
            .iter()
            .filter(|c| c.bg == cursor_bg.unwrap())
            .count()
    };
    assert!(tinted(&pending) > 0, "the cursor row must still be marked");
    assert_eq!(tinted(&pending), tinted(&settled));
}

/// Spec 0223 test 4, the specific defect §S3 warns about. `row_spans`
/// indexes `window_styles` *by position in the window*, so hints left
/// over from a previous viewport are not merely stale — they color a
/// different set of rows. Clearing is what makes "no styles" mean
/// "monochrome" rather than "wrong".
#[test]
fn stale_styles_are_cleared_not_left() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        !app.window_styles.is_empty(),
        "the settled frame must have populated the hints being cleared"
    );

    app.input_pending = true;
    app.set_scroll_top(app.scroll_top() + 1);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.window_styles.is_empty());
}

/// Spec 0245 S3, the other half of the test above: a pending frame
/// whose window did not move keeps the hints it already has. They are
/// not stale — `window_styles_for` is a pure function of the window
/// text and the indent size, and neither changed — so there is nothing
/// to recompute and nothing to fear from reusing them. This is what
/// stops a wheel held against the top of the document, which pans
/// nothing, from flickering the pane between gray and colored.
#[test]
fn an_unchanged_window_keeps_its_styles_while_input_is_pending() {
    let (mut app, _inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let settled = app.window_styles.clone();
    assert!(settled.iter().any(|hints| !hints.is_empty()));

    app.input_pending = true;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.window_styles, settled);
}

/// Spec 0113 D33: the bold override hint applies to a node's own
/// header and (when it has children) footer line, but must not
/// cascade to descendant lines.
#[test]
fn the_active_override_hint_marks_header_and_footer_but_not_children() {
    // `render`'s hoisted override pass (spec 0192 S2) asks exactly this
    // of every drawn row.
    fn bolded(app: &App, line_idx: usize) -> bool {
        app.node_at_own_line(line_idx)
            .is_some_and(|idx| app.resolve_active_override(idx).is_some())
    }

    let (mut app, inner_idx, id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.run_command(&format!(
        "override {} --as test.Inner",
        app.positional_path(app.cursor)
    ));
    assert_eq!(type_name_of(&app, inner_idx), Some("test.Inner"));

    let header_line = app.absolute_start(inner_idx);
    let footer_line = app.node_lines(inner_idx).end - 1;
    assert!(bolded(&app, header_line));
    assert!(bolded(&app, footer_line));

    let id_line = app.absolute_start(id_idx);
    assert!(!bolded(&app, id_line));
}

/// 2026-07-18 feedback: when the cursor rests on a node's own closing
/// `}` line (spec 0142), the status line's `L<n>` must report the
/// footer line's own number, not the header's.
#[test]
fn status_line_reports_the_footer_line_number_for_a_footer_resting_cursor() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.splash = false;

    let footer_line = app.node_lines(inner_idx).end - 1;
    app.cursor = inner_idx;
    // Spec 0216 S7: resting on the footer is a coordinate, and the
    // footer is the node's *last* row, not its second.
    app.cursor_line_in_node = app.tree[inner_idx].lines_total - 1;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
    assert!(
        text.contains(&format!("L{}/", footer_line + 1)),
        "status line must report the footer's own 1-based line number: {text:?}"
    );
}

/// Spec 0147 G6: `MESSAGE_TIMEOUT` is exactly 3 seconds (down from 4),
/// per the original proposal's stated value.
#[test]
fn message_timeout_is_three_seconds() {
    assert_eq!(MESSAGE_TIMEOUT, Duration::from_secs(3));
}

/// Spec 0147 G2: the main pane's local statusline shows a
/// right-flushed `[start..end)  L<curr>/<total>` ruler when it is the
/// only pane open (full width), but drops it entirely — not
/// truncates it — once a side pane is open and the main pane is only
/// half-width.
#[test]
fn main_statusline_omits_the_ruler_when_a_side_pane_is_open() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains(".."),
        "full width: the byte-range ruler must be shown: {row_text:?}"
    );

    app.toggle_override();
    assert!(app.override_target.is_some());
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..app.main_area.width)
        .map(|x| {
            buffer[(app.main_area.x + x, statusline_row)]
                .symbol()
                .to_string()
        })
        .collect();
    assert!(
        !row_text.contains(".."),
        "half width: the byte-range ruler must be omitted: {row_text:?}"
    );
}

/// 2026-07-19 feedback item 7: the main pane's local statusline shows
/// the full path as given on the command line (`App::new`'s
/// `blob_path` argument), not just the short filename.
#[test]
fn main_statusline_shows_the_full_command_line_path_not_just_the_filename() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.blob_path = PathBuf::from("some/nested/dir/test.pb");

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.contains("some/nested/dir/test.pb"),
        "the statusline must show the full command-line path: {row_text:?}"
    );
}

/// Spec 0193 S3 revises spec 0147 G2's truncation rule: when the
/// terminal is too narrow for the local statusline's full left-hand
/// content, the *head* is dropped and a leading `<` marker announces
/// it, so the reader keeps the informative tail (the node's own name
/// and span, rather than the enclosing file path). The right-flushed
/// ruler remains shown in full either way.
#[test]
fn main_statusline_truncates_the_left_side_with_a_marker_when_narrow() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;

    let backend = TestBackend::new(30, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let statusline_row = app.main_area.y + app.main_area.height;
    let buffer = terminal.backend().buffer();
    let row_text: String = (0..buffer.area.width)
        .map(|x| buffer[(x, statusline_row)].symbol().to_string())
        .collect();
    assert!(
        row_text.starts_with('<'),
        "narrow terminal: the dropped head must be marked with a leading `<`: {row_text:?}"
    );
    assert!(
        row_text.contains("[message][2..4)"),
        "the left side's tail must survive the truncation: {row_text:?}"
    );
    assert!(
        !row_text.contains("test.pb"),
        "it is the head that is dropped, not the tail: {row_text:?}"
    );
    assert!(
        row_text.trim_end().ends_with("All"),
        "the right-flushed ruler must still be shown in full: {row_text:?}"
    );
}

/// Spec 0147 G3: the `Length(1)` vertical separator column between
/// the main pane and an open side pane is filled with `'│'` for the
/// full height of `main_outer`/`right_outer` (content rows plus each
/// pane's own local statusline row).
#[test]
fn vertical_separator_renders_between_main_and_side_pane() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.toggle_override();
    assert!(app.override_target.is_some());

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();

    let separator_x = app.side_area.x - 1;
    for y in app.main_area.y..=(app.main_area.y + app.main_area.height) {
        assert_eq!(
            buffer[(separator_x, y)].symbol(),
            "│",
            "separator column must render '│' at row {y}"
        );
    }
}

/// 2026-07-19 feedback item 5: local statuslines render in vim-style
/// inverted video (`Modifier::REVERSED`), and the focused pane's own
/// statusline uses a brighter accent (`Color::White`) than an
/// unfocused pane's (`Color::Gray`).
#[test]
fn local_statuslines_are_reversed_and_the_focused_pane_is_brighter() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.toggle_override();
    assert!(app.override_target.is_some());
    assert!(app.override_focus);

    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();

    let main_statusline_row = app.main_area.y + app.main_area.height;
    let main_cell = &buffer[(app.main_area.x, main_statusline_row)];
    assert!(
        main_cell.modifier.contains(Modifier::REVERSED),
        "unfocused main pane's statusline must be reversed"
    );
    assert_eq!(
        main_cell.fg,
        Color::Gray,
        "unfocused pane's statusline accent must be the dimmer gray"
    );

    let side_statusline_row = app.side_area.y + app.side_area.height;
    let side_cell = &buffer[(app.side_area.x, side_statusline_row)];
    assert!(
        side_cell.modifier.contains(Modifier::REVERSED),
        "focused override pane's statusline must be reversed"
    );
    assert_eq!(
        side_cell.fg,
        Color::White,
        "focused pane's statusline accent must be the brighter white"
    );
}

/// Spec 0147 G4: the global command/message row is always reserved at
/// a fixed `Length(1)` height, regardless of whether it is blank,
/// showing a passive message, or showing active command entry — the
/// main content area must never resize because of it.
#[test]
fn global_command_message_row_stays_fixed_height() {
    let mut app = empty_app();
    app.splash = false;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(app.cmd_area.is_none());
    let main_height = app.main_area.height;

    app.message = "some notice".to_string();
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.cmd_area.unwrap().height, 1);
    assert_eq!(
        app.main_area.height, main_height,
        "main content area must not resize when a message appears"
    );

    app.message.clear();
    app.command_buffer = Some("cmd".to_string());
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(app.cmd_area.unwrap().height, 1);
    assert_eq!(
        app.main_area.height, main_height,
        "main content area must not resize during active command entry"
    );
}

// ── Spec 0190: the activity dot ──────────────────────────────────────────

/// Spec 0190 test plan item 5 / spec 0191 test plan item 5. The dot
/// occupies column 0 of the bottom row and renders `App::activity_shown`
/// — the value `run_loop` decided — rather than probing the worker's
/// atomics at draw time (0191 S2). Installing a worker and pushing to it
/// must therefore *not* change the cell on its own; only the field does.
///
/// The color is asserted, not just the glyph: `heat_style`'s levels are
/// all >= 4 precisely so this cell is never blank-because-invisible on
/// an ANSI-16 terminal, and the tiers must stay distinguishable there.
#[test]
fn the_activity_dot_renders_the_decided_level_in_column_zero() {
    let mut app = message_node_app();
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    let dot = (0u16, 9u16); // column 0 of the bottom (global) row

    // Nothing decided yet — the cell is blank and the command row still
    // starts one column in.
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "no decided activity means nothing to report"
    );

    // A live worker with a queued request is *not* enough: the dot is no
    // longer a probe. This is the assertion that pins 0191 S2 — it fails
    // against the pre-0191 implementation, which read `heat_activity()`
    // here and would light the cell from the render's own cue lookups.
    app.heat_worker = Some(HeatWorkerHandle::stub_for_test());
    app.heat_worker.as_ref().unwrap().push(
        HeatRequest {
            range: usize::MAX - 1..usize::MAX,
            current_key: None,
            start: 0,
            end: 1,
            tier: tiered::Tier::User,
        },
        tiered::Tier::User,
    );
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        terminal.backend().buffer()[dot].symbol(),
        " ",
        "a queued request the loop has not yet accounted for must not light the dot"
    );

    app.activity_shown = Some(tiered::Tier::Visible);
    terminal.draw(|frame| app.render(frame)).unwrap();
    let visible_style = terminal.backend().buffer()[dot].style();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);

    app.activity_shown = Some(tiered::Tier::User);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(terminal.backend().buffer()[dot].symbol(), ACTIVITY_GLYPH);
    assert_ne!(
        terminal.backend().buffer()[dot].style(),
        visible_style,
        "User must outrank Visible and look different, ANSI-16 included"
    );
}

/// Test plan item 6. Reserving the dot's field must move the command
/// row, and everything derived from it, that far right — the cursor
/// included. This is the assertion that would catch an implementation
/// that inset the dot at each use site instead of re-binding `cmd_row`.
#[test]
fn reserving_the_dot_column_shifts_the_command_row_and_its_cursor() {
    let mut app = message_node_app();
    app.splash = false;
    app.command_buffer = Some("cmd".to_string());
    app.command_cursor = 3;
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();

    terminal.draw(|frame| app.render(frame)).unwrap();
    let cmd_area = app.cmd_area.expect("an active command buffer draws a row");
    let field = render::ACTIVITY_FIELD_WIDTH;
    assert_eq!(cmd_area.x, field, "the command row starts after the dot");
    assert_eq!(cmd_area.width, 80 - field, "and is that much narrower");
    assert_eq!(
        terminal.backend().buffer()[(field, 9u16)].symbol(),
        ":",
        "the command prefix must land after the dot's field, not in it"
    );
    // ":cmd" with the cursor past the last char.
    assert_eq!(terminal.get_cursor_position().unwrap().x, field + 4);
}

// ── Spec 0187: highlighting is a property of the viewport ────────────────

/// Test plan item 1. The whole point of S3's synthetic enclosing
/// context: a window parsed on its own must be colored exactly as those
/// same lines are colored inside the complete document. Checked for
/// *every* scroll offset and every window height, because the failure
/// mode is offset-dependent — a window that happens to start at depth
/// zero passes even with no framing at all.
#[test]
fn window_highlighting_matches_whole_document_highlighting() {
    let app = nested_message_set_fixture();
    let lines = &app.document_lines();
    let document = colorize::hints_by_line(lines, &colorize::colorize(&lines.join("\n")));

    for height in 1..=lines.len() {
        for start in 0..=(lines.len() - height) {
            let window = lines[start..start + height].to_vec();
            let got = window_styles_for(&window, app.indent_size);
            assert_eq!(
                got,
                document[start..start + height],
                "window {start}..{} (height {height}) must be colored as the \
                 document colors those same lines; window text was {window:#?}",
                start + height
            );
        }
    }
}

/// Test plan item 2. The synthetic openers and closers S3 wraps the
/// window in are parser scaffolding, not content: they must not appear
/// as extra buckets, and no hint may be attributed to a line it does not
/// fit inside. Exercised from a window that starts at the deepest line
/// of the fixture, where the scaffolding is largest.
#[test]
fn the_synthetic_context_is_dropped_not_drawn() {
    let app = nested_message_set_fixture();
    let deepest = app
        .document_lines()
        .iter()
        .enumerate()
        .max_by_key(|(_, l)| l.len() - l.trim_start().len())
        .map(|(i, _)| i)
        .expect("the fixture has lines");

    for height in 1..=(app.document_lines().len() - deepest) {
        let window = app.document_lines()[deepest..deepest + height].to_vec();
        let styles = window_styles_for(&window, app.indent_size);
        assert_eq!(
            styles.len(),
            window.len(),
            "one bucket per drawn row, with the {height}-row window at line {deepest}"
        );
        for (line, hints) in window.iter().zip(&styles) {
            for (range, role) in hints {
                assert!(
                    range.end <= line.len(),
                    "hint {range:?} ({role:?}) runs past the end of {line:?}"
                );
            }
        }
    }
}

/// Spec 0318 G3: every row of a preview is prototext, so an overlay
/// colorizes exactly as the same lines do on their own.
///
/// This replaces the test that pinned spec 0174 S4's `...` marker being
/// blanked before the parser saw it. The marker is gone (S8) and the
/// property is now the stronger one: there is nothing to blank. What
/// would break it is any future non-grammar row, whose error recovery
/// swallows *following* siblings — see
/// `colorize::bare_decimal_field_name_does_not_corrupt_sibling_captures`
/// — so the rows *beneath* it would silently lose their colors.
#[test]
fn every_overlay_row_is_prototext() {
    let mut app = nested_message_set_fixture();
    let lines = app.document_lines().clone();
    let expected = window_styles_for(&lines, app.indent_size);

    let rows: Vec<DisplayRow> = (0..lines.len()).map(DisplayRow::Overlay).collect();
    app.preview_overlay = Some(PreviewOverlay {
        first_row: 0,
        covered_rows: 0,
        lines: lines.clone(),
        spans: Vec::new(),
        bytes: Vec::new(),
        tier: PreviewTier::Clean,
        tier_column: 0,
        ellipsis_row: None,
    });
    app.refresh_window_styles(&rows);

    assert_eq!(app.window_styles.len(), lines.len(), "one bucket per row");
    assert_eq!(
        app.window_styles, expected,
        "an overlay row must colorize as the same line does alone"
    );
}

/// Every drawn cell whose symbol is `glyph`, as `(x, y, foreground)`.
fn margin_cells(
    app: &App,
    terminal: &Terminal<TestBackend>,
    glyph: char,
) -> Vec<(u16, u16, Color)> {
    let glyph = glyph.to_string();
    let buffer = terminal.backend().buffer();
    let mut out = Vec::new();
    for y in app.main_area.y..app.main_area.y + app.main_area.height {
        for x in app.main_area.x..app.main_area.x + app.main_area.width {
            if buffer[(x, y)].symbol() == glyph {
                out.push((x, y, buffer[(x, y)].fg));
            }
        }
    }
    out
}

/// The drawn bar cells that carry spec 0334 S3's `DIM`, or those that do
/// not — which is how a test names the caret node's own bar apart from
/// its ancestors', the two now being the same glyph in the same field.
fn tier_bar_cells_dim(
    app: &App,
    terminal: &Terminal<TestBackend>,
    dim: bool,
) -> Vec<(u16, u16, Color)> {
    let glyph = crate::tui::render::TIER_BAR_GLYPH.to_string();
    let buffer = terminal.backend().buffer();
    let mut out = Vec::new();
    for y in app.main_area.y..app.main_area.y + app.main_area.height {
        for x in app.main_area.x..app.main_area.x + app.main_area.width {
            let cell = &buffer[(x, y)];
            if cell.symbol() == glyph && cell.modifier.contains(Modifier::DIM) == dim {
                out.push((x, y, cell.fg));
            }
        }
    }
    out
}

/// Spec 0318 S7: the previewed node keeps its own fold toggle on the
/// preview's first row, and the bar starts directly below it and runs to
/// the closing brace.
///
/// The triangle is the one control on the row the reader is deciding
/// about, and the overlay hides the committed row it would have been
/// drawn on — so covering it with the bar would take it away entirely.
///
/// The bar's color answers one question, so it has two states: default
/// foreground when the preview is the whole node, violet when it is not.
/// `Clean` and `Ragged` are two decisions in `preview_truncate` and one
/// answer here.
#[test]
fn overlay_rows_draw_the_tier_bar_below_the_previewed_nodes_triangle() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let row_count = app
        .preview_overlay
        .as_ref()
        .expect("`t` must put a preview up")
        .lines
        .len();
    assert!(row_count >= 3, "the fixture must have an interior to cover");

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();

    for tier in [PreviewTier::Whole, PreviewTier::Clean, PreviewTier::Ragged] {
        app.preview_overlay.as_mut().unwrap().tier = tier;
        terminal.draw(|frame| app.render(frame)).unwrap();

        // Spec 0334 N2: the overlay's bar is the only *undimmed* one on
        // its rows. The committed rows around it now carry the caret
        // node's ancestors' dimmed bars, which this test is not about.
        let cells = tier_bar_cells_dim(&app, &terminal, false);
        assert_eq!(
            cells.len(),
            row_count - 1,
            "the bar covers every overlay row but the first, {tier:?}: {cells:?}"
        );

        // The row above the bar is the preview's first, and the toggle
        // has to be on it, in the bar's own column. Other triangles are
        // on screen — the committed rows above the overlay have theirs —
        // so this is asked of the one position that matters rather than
        // of the pane.
        let (bx, by, _) = cells[0];
        let triangles = margin_cells(&app, &terminal, crate::tui::render::FOLD_GLYPH_OPEN);
        assert!(
            triangles.iter().any(|&(x, y, _)| (x, y) == (bx, by - 1)),
            "the previewed node keeps its toggle, {tier:?}: {triangles:?}"
        );

        let want =
            crate::theme::preview_bar_color(tier.is_whole(), app.theme).unwrap_or(Color::Reset);
        for (i, &(x, y, fg)) in cells.iter().enumerate() {
            assert_eq!(x, bx, "the bar must stay below the triangle, {tier:?}");
            assert_eq!(y, by + i as u16, "the bar must be contiguous");
            assert_eq!(fg, want, "the bar's color, {tier:?}");
        }
    }

    // The two states must not draw alike, or the bar says nothing.
    let complete = crate::theme::preview_bar_color(true, app.theme);
    let cut = crate::theme::preview_bar_color(false, app.theme);
    assert_eq!(complete, None, "a whole preview withholds no color either");
    assert!(cut.is_some(), "a cut preview must be visibly marked");
}

/// Spec 0322 S6 / test-plan item 7: a previewed *leaf* has no toggle to
/// preserve, but it may have an anomaly mark — and the row the reader is
/// deciding about is the last place to cover the one thing on it that
/// says the node is wrong. So the bar starts at row 1 there too, exactly
/// as it does under a triangle.
#[test]
fn a_previewed_leaf_keeps_its_anomaly_mark() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    assert!(app.first_child(id_idx).is_none(), "`id` must be a leaf");

    // Give the leaf an anomaly of its own. The overlay renders the node
    // afresh as the highlighted candidate, but the mark is read off the
    // *committed* status, which is what this sets.
    let line = app.document_lines()[app.absolute_start(id_idx)].clone();
    app.node_text_mut()[id_idx] = Some(Box::from(format!("{line}; val_ohb: 3").as_str()));
    app.rebuild_status();

    app.cursor = id_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let row_count = app
        .preview_overlay
        .as_ref()
        .expect("`t` must put a preview up")
        .lines
        .len();

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let want =
        crate::theme::status_color(crate::node_status::Status::NonCanonical, app.theme).unwrap();
    let marks = margin_cells(&app, &terminal, crate::tui::render::ANOMALY_GLYPH);
    assert_eq!(
        marks.len(),
        1,
        "one mark, on the preview's first row: {marks:?}"
    );
    let (mx, my, fg) = marks[0];
    assert_eq!(fg, want, "and in the status hue");

    // Undimmed: spec 0334 puts the caret's ancestors' bars on the
    // committed rows around the overlay, and this is about the overlay.
    let bars = tier_bar_cells_dim(&app, &terminal, false);
    assert_eq!(
        bars.len(),
        row_count - 1,
        "the bar must start below the mark, not over it: {bars:?}"
    );
    for (i, &(x, y, _)) in bars.iter().enumerate() {
        assert_eq!(
            (x, y),
            (mx, my + 1 + i as u16),
            "contiguous, below the mark"
        );
    }
}

/// Spec 0318 S7's claim, and the one that would silently regress: the
/// fold column is free on an overlay row *at every indent setting*,
/// because `display_row_source` gives an overlay row no owner and so
/// `fold_marker_of` gives it no glyph. The one triangle on screen is the
/// previewed node's, drawn deliberately by `overlay_margin_spans` rather
/// than found by the ordinary path. `--indent 1` is the case where a
/// committed row's marker sits in the reserved field rather than in its
/// own indentation, i.e. where a collision would first show.
#[test]
fn overlay_fold_column_is_free_at_indent_one() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.indent_size = 1;
    app.cursor = inner_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let row_count = app
        .preview_overlay
        .as_ref()
        .expect("`t` must put a preview up")
        .lines
        .len();

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let cells = tier_bar_cells_dim(&app, &terminal, false);
    assert_eq!(
        cells.len(),
        row_count - 1,
        "one bar per overlay row below the first: {cells:?}"
    );

    // No fold triangle on any row the bar is on — only on the row above
    // it, which is the previewed node's own.
    let buffer = terminal.backend().buffer();
    for &(_, y, _) in &cells {
        for x in app.main_area.x..app.main_area.x + app.main_area.width {
            let symbol = buffer[(x, y)].symbol();
            assert!(
                symbol != crate::tui::render::FOLD_GLYPH_OPEN.to_string()
                    && symbol != crate::tui::render::FOLD_GLYPH_CLOSED.to_string(),
                "an overlay row has no owner, so it draws no fold marker: \
                 row {y} column {x}"
            );
        }
    }
}

/// Spec 0328 test plan item 1, and G1/S1/S2/S3 at once: the bar hangs
/// directly under the current node's own triangle, stops one row above
/// its closing brace, and wears the triangle's color.
///
/// All four in one test because they are one mark, and any three of
/// them holding while the fourth does not is a bar that says the wrong
/// thing rather than a bar with a small defect.
///
/// The bar no longer reaches the brace itself: the brace is the node's
/// extent already drawn, and a bar beside it says the same thing twice.
/// So the count is `total - 2` — the header keeps its triangle at the
/// top, the brace stands alone at the bottom.
///
/// Spec 0334 S3: the caret node's bar is the *undimmed* one. Its
/// ancestors now draw bars of their own in the same field, and
/// `every_ancestor_wears_a_dimmer_bar` is where those are asserted.
#[test]
fn the_current_node_wears_a_bar() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    let total = app.tree[inner_idx].lines_total as usize;
    assert!(total >= 3, "the fixture's node must have an interior");

    let terminal = drawn_frame(&mut app, 120, 24);
    let bars = tier_bar_cells_dim(&app, &terminal, false);
    assert_eq!(
        bars.len(),
        total - 2,
        "one bar per interior row: none on the header, which keeps its \
         triangle, and none on the closing brace: {bars:?}"
    );

    // S3: the very call the triangle's own color comes from, so the two
    // agree by construction. Asserted of both, or a renderer that
    // colored neither would pass.
    let want = app
        .margin_glyph_color(Some(inner_idx))
        .unwrap_or(Color::Reset);
    let (bx, by, _) = bars[0];
    let triangles = margin_cells(&app, &terminal, crate::tui::render::FOLD_GLYPH_OPEN);
    let header = triangles
        .iter()
        .find(|&&(x, y, _)| (x, y) == (bx, by - 1))
        .expect("the header keeps its triangle, directly above the bar");
    assert_eq!(header.2, want, "the triangle's color");

    for (i, &(x, y, fg)) in bars.iter().enumerate() {
        assert_eq!(x, bx, "the bar stays in the triangle's column");
        assert_eq!(y, by + i as u16, "the bar is continuous");
        assert_eq!(fg, want, "and is the triangle's color");
    }
}

/// Spec 0328 test plan item 2 / S2's two consequences that need no code:
/// a folded node draws its body as the one-row `{ ... }` collapse, so
/// there is no range to run a bar down — right, since a collapsed
/// node's extent *is* the row you are on — and a leaf has
/// `lines_total == 1` and likewise gets none.
///
/// What the caret node contributing nothing no longer means is that
/// nothing is undimmed. The undimmed bar is the *nearest* one, not the
/// caret node's, so it falls through to the nearest ancestor that has
/// one — which is the point of the rule: the reader keeps a "you are
/// here" mark on exactly the rows where the caret node cannot supply
/// one itself.
///
/// The fold target has to be a node with a **sibling**. Folding an only
/// child collapses its parent to a header and a brace, which has no
/// interior either, and the fall-through then has nothing to land on —
/// true, but it tests the empty case rather than this one.
#[test]
fn a_folded_node_hands_its_undimmed_bar_to_the_nearest_ancestor() {
    let mut app = nested_message_set_fixture();
    let target = (0..app.tree.len())
        .find(|&i| {
            app.child_count(i) > 0
                && app
                    .parent(i)
                    .is_some_and(|p| app.child_count(p) > 1 && app.tree[p].lines_total >= 5)
        })
        .expect("the fixture must have a foldable node with a sibling");
    let parent = app.parent(target).expect("found via its parent");
    app.cursor = target;

    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert!(app.is_folded(target), "`z` must fold the cursor node");
    assert!(
        app.tree[parent].lines_total >= 3,
        "the parent must keep an interior across the fold, or there is \
         no nearest ancestor for the undimmed bar to fall through to"
    );

    let terminal = drawn_frame(&mut app, 120, 24);
    let undimmed = tier_bar_cells_dim(&app, &terminal, false);
    assert_eq!(
        undimmed_columns(&undimmed),
        1,
        "a collapsed node draws no bar of its own, so its parent's is \
         the one undimmed bar: {undimmed:?}"
    );
}

/// How many distinct columns a set of drawn bar cells occupies — one
/// bar runs down several rows, so counting cells would count rows.
fn undimmed_columns(cells: &[(u16, u16, Color)]) -> usize {
    let mut columns: Vec<u16> = cells.iter().map(|&(x, ..)| x).collect();
    columns.sort_unstable();
    columns.dedup();
    columns.len()
}

/// Spec 0334 G1/G2/S1/S2/S3, test plan item 3: with the caret two levels
/// down, each ancestor draws a bar of its own — in its own column,
/// strictly left of the caret node's, over its own rows, dimmed.
#[test]
fn every_ancestor_wears_a_dimmer_bar() {
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    // `>= 3`, not `>= 2`: a header and a closing brace with no interior
    // between them draw no bar, so such an ancestor contributes no
    // column and must not be counted as one.
    let ancestors: Vec<usize> = std::iter::successors(app.parent(inner_idx), |&i| app.parent(i))
        .filter(|&i| app.tree[i].lines_total >= 3)
        .collect();
    assert!(
        !ancestors.is_empty(),
        "the fixture must put the caret inside something"
    );

    let terminal = drawn_frame(&mut app, 120, 24);
    let own = tier_bar_cells_dim(&app, &terminal, false);
    let dimmed = tier_bar_cells_dim(&app, &terminal, true);
    let own_column = own.first().expect("the caret node draws a bar").0;

    let mut columns: Vec<u16> = dimmed.iter().map(|&(x, ..)| x).collect();
    columns.sort_unstable();
    columns.dedup();
    assert_eq!(
        columns.len(),
        ancestors.len(),
        "one column per ancestor: {dimmed:?}"
    );

    for (&idx, &x) in ancestors.iter().zip(columns.iter().rev()) {
        assert!(
            x < own_column,
            "an ancestor's column is left of the caret's"
        );
        // Two adjustments, both in `bar_style`: an ancestor with no
        // status color of its own falls back to an explicit `DarkGray`
        // rather than the terminal's default foreground, and whatever
        // color it ends up with is blended toward the page, since most
        // terminals ignore `Modifier::DIM` on a 24-bit foreground.
        let want = crate::theme::dimmed(
            app.margin_glyph_color(Some(idx)).unwrap_or(Color::DarkGray),
            app.theme,
        );
        let rows: Vec<u16> = dimmed
            .iter()
            .filter(|&&(bx, ..)| bx == x)
            .map(|&(_, y, fg)| {
                assert_eq!(fg, want, "an ancestor's bar wears its own triangle's color");
                y
            })
            .collect();
        assert_eq!(
            rows.len(),
            app.tree[idx].lines_total as usize - 2,
            "one bar per interior row of the ancestor: not on its header, \
             which keeps its triangle, and not on its closing brace"
        );
        for (i, &y) in rows.iter().enumerate() {
            assert_eq!(y, rows[0] + i as u16, "the ancestor's bar is continuous");
        }
    }
}

/// Spec 0334 S1's tail, test plan item 4: a caret on a leaf contributes
/// no bar of its own, and the walk carries on past it, so every ancestor
/// still draws one.
///
/// The nearest of those ancestors is now the undimmed bar — the leaf
/// having none to be undimmed — and the ones beyond it stay dimmed, so
/// the path is still read innermost-outward.
#[test]
fn a_bar_on_a_leaf_comes_from_its_ancestors() {
    let (mut app, _inner_idx, id_idx) = type_as_fixture();
    app.cursor = id_idx;
    assert!(app.first_child(id_idx).is_none(), "`id` must be a leaf");

    let terminal = drawn_frame(&mut app, 120, 24);
    let undimmed = tier_bar_cells_dim(&app, &terminal, false);
    assert_eq!(
        undimmed_columns(&undimmed),
        1,
        "the leaf's nearest bracketed ancestor supplies the one undimmed \
         bar: {undimmed:?}"
    );
    assert!(
        !tier_bar_cells_dim(&app, &terminal, true).is_empty(),
        "and the rest of the path up to the root is still drawn, dimmed"
    );
}

/// Spec 0328 S4, test plan item 3: **the row's own mark wins the cell.**
///
/// Under the default `--indent 2` the two never meet — a child's marker
/// is at least two columns deeper — so the case has to be built at
/// `--indent 1`, where `marker_column` floors at 0 for the first two
/// levels and an ancestor's bar lands in the very cell a child's
/// triangle wants. The triangle is a control, and the row's own; the bar
/// is an ancestor's readout.
///
/// The one fixture where two fold columns collide at all, so spec 0334
/// S4's tie-break between two *bars* is measured on it too.
fn narrowly_indented_fixture() -> App {
    let mut app = nested_message_set_fixture();
    app.indent_size = 1;
    // Re-indent as `--indent 1` would have. Only leading whitespace
    // changes, so every node's line range stays where it was.
    for text in app.node_text_mut().iter_mut().flatten() {
        let reindented: Vec<String> = text
            .split('\n')
            .map(|l| {
                let depth = (l.len() - l.trim_start().len()) / 2;
                format!("{}{}", " ".repeat(depth), l.trim_start())
            })
            .collect();
        *text = reindented.join("\n").into_boxed_str();
    }
    app
}

#[test]
fn a_child_marker_outranks_the_bar() {
    let mut app = narrowly_indented_fixture();
    app.cursor = 0;
    let total = app.tree[0].lines_total as usize;
    let terminal = drawn_frame(&mut app, 120, 24);

    // The root's own triangle fixes the shared column; every marker at
    // depth 0 or 1 lands in it.
    let open = crate::tui::render::FOLD_GLYPH_OPEN;
    let column = margin_cells(&app, &terminal, open)
        .first()
        .expect("the root has a triangle")
        .0;

    let buffer = terminal.backend().buffer();
    let first = app.main_area.y;
    let mut triangles = 0;
    // The interior rows only: the header at `first` keeps its triangle,
    // and the closing brace at `first + total - 1` now draws neither a
    // marker nor a bar, so including it would assert on a blank cell.
    for y in first + 1..first + total as u16 - 1 {
        let symbol = buffer[(column, y)].symbol().to_string();
        if symbol == open.to_string() {
            triangles += 1;
            continue;
        }
        assert_eq!(
            symbol,
            crate::tui::render::TIER_BAR_GLYPH.to_string(),
            "row {y} of the cursor node draws its own marker or the bar"
        );
    }
    assert!(
        triangles > 0,
        "the fixture must actually collide: at `--indent 1` a child's \
         marker shares the root's column"
    );
}

/// Spec 0334 S4, test plan item 5: where two bars want the same cell,
/// **the nearer node's wins**.
///
/// Only reachable at `--indent 1`, where `marker_column` floors: with
/// the caret on a depth-1 node, its bar and the root's both want column
/// 0. Every row of the caret node must show the caret node's — the
/// undimmed one — and the root's dimmed bar must appear only on the rows
/// the caret node does not cover, which is what says it was suppressed
/// rather than never drawn.
#[test]
fn the_nearer_bar_wins_a_shared_column() {
    let mut app = narrowly_indented_fixture();
    let child = app.first_child(0).expect("the root has a child");
    assert!(
        app.tree[child].lines_total >= 3,
        "the caret node must have an interior, or its bar covers no row \
         and the tie-break is vacuous"
    );
    app.cursor = child;

    let terminal = drawn_frame(&mut app, 120, 24);
    let column = margin_cells(&app, &terminal, crate::tui::render::FOLD_GLYPH_OPEN)
        .first()
        .expect("the root has a triangle")
        .0;
    let at = |dim: bool| -> Vec<u16> {
        tier_bar_cells_dim(&app, &terminal, dim)
            .iter()
            .filter(|&&(x, ..)| x == column)
            .map(|&(_, y, _)| y)
            .collect()
    };

    let own = at(false);
    assert_eq!(
        own.len(),
        app.tree[child].lines_total as usize - 2,
        "the caret node's bar shares the root's column and holds all of it"
    );
    let root_bar = at(true);
    assert!(
        !root_bar.is_empty(),
        "the root's bar must reach rows of its own, or the tie-break is untested"
    );
    for y in &root_bar {
        assert!(
            !own.contains(y),
            "row {y}: the root's bar is drawn only where the caret node's is not"
        );
    }
}

/// Spec 0328 G2/S5, test plan item 4: a wire row takes its left margin
/// from the same function its document row does, so both bars run
/// unbroken through the hex.
///
/// The defect this replaces was visible: with bytes shown the bar was
/// drawn on every other terminal row and read as a dotted line meaning
/// nothing.
#[test]
fn a_bar_survives_a_wire_row() {
    // The committed bar, with the whole subtree's bytes shown.
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    let header = app.absolute_start(inner_idx);
    let total = app.tree[inner_idx].lines_total as usize;
    let span = app.wire_span_of_lines(header, header + total - 1);
    app.set_wire_span(span, 0);
    let terminal = drawn_frame(&mut app, 120, 24);
    assert_contiguous_bars(&app, &terminal, total - 1);

    // And the preview's, which is the bar spec 0318 S7 drew and this
    // spec repairs. A single covered committed row puts every overlay
    // row in the run (spec 0185 S2).
    let (mut app, inner_idx, _id_idx) = type_as_fixture();
    app.cursor = inner_idx;
    let header = app.absolute_start(inner_idx);
    let span = app.wire_span_of_lines(header, header);
    app.set_wire_span(span, 0);
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let rows = app
        .preview_overlay
        .as_ref()
        .expect("`t` must put a preview up")
        .lines
        .len();
    let terminal = drawn_frame(&mut app, 120, 24);
    assert_contiguous_bars(&app, &terminal, rows - 1);
}

/// The bars drawn in `terminal` form one unbroken run down one column,
/// and there are more of them than `document_rows` — which is what says
/// the wire rows were drawn through rather than skipped.
///
/// The undimmed ones: spec 0334's ancestor bars run down columns of
/// their own, and this is about the caret node's (or the preview's).
fn assert_contiguous_bars(app: &App, terminal: &Terminal<TestBackend>, document_rows: usize) {
    let bars = tier_bar_cells_dim(app, terminal, false);
    assert!(
        bars.len() > document_rows,
        "the wire rows must carry the bar too: {} bar(s) for {document_rows} \
         document row(s)",
        bars.len()
    );
    for pair in bars.windows(2) {
        assert_eq!(pair[0].0, pair[1].0, "one column: {bars:?}");
        assert_eq!(pair[1].1, pair[0].1 + 1, "unbroken: {bars:?}");
    }
}

/// Test plan item 3. Hiding annotations is now driven by the format's
/// own `  #@ ` rule (`prototext_core::serialize::encode_text::
/// annotation_start`) rather than by the highlighter's Comment capture.
/// The case that separates a correct implementation from a naive one is
/// a line whose *string value* itself contains a literal `#@`: only a
/// rightmost-first scan with a leftward fallback gets it right.
#[test]
fn hiding_annotations_respects_a_hash_at_inside_a_string_value() {
    let line = "  name: \"a  #@ b\"  #@ string = 2".to_string();
    let mut app = sibling_leaves_app(&["x"]);
    app.node_text_mut()[0] = Some(Box::from(line.as_str()));
    app.annotations = false;
    // Spec 0193 S1's blank fold field accounts for the two extra columns.
    assert_eq!(
        app.row_content(app.committed_row(0).unwrap()),
        "    name: \"a  #@ b\""
    );
    app.annotations = true;
    assert_eq!(
        app.row_content(app.committed_row(0).unwrap()),
        format!("  {line}")
    );
}

/// Test plan item 3b. The one place this spec knowingly changes what the
/// user sees. An empty packed record renders as a comment-only line
/// (`render_text/packed.rs`'s `pack_size == 0` arm: indentation, then
/// the annotation with no value token before it). The old rule cut at
/// the highlighter's Comment capture and then trimmed, leaving `""`
/// only by accident of the trim; the format's rule reaches the same
/// answer deliberately, because the two separator spaces it excludes
/// *are* the line's whole indentation at level 1.
///
/// A test rather than a comment because someone reading a blank row
/// might reasonably "fix" it back into a run of spaces.
#[test]
fn an_empty_packed_record_row_hides_to_nothing() {
    let mut app = sibling_leaves_app(&["x"]);
    app.node_text_mut()[0] = Some(Box::from("  #@ = 7 pack_size: 0"));
    app.annotations = false;
    assert_eq!(app.row_content(app.committed_row(0).unwrap()), "");
}

/// Test plan item 3c. The rule moved across a crate boundary, from the
/// highlighter's Comment capture to the encoder's own byte scan. Pin
/// that the move is behavior-preserving: over real renderer output, the
/// old rule (first Comment hint's start, then `trim_end`) and the new
/// one must pick the same truncation point on every line.
#[test]
fn the_format_rule_and_the_parser_rule_agree_on_rendered_lines() {
    for app in [nested_message_set_fixture(), nested_any_fixture()] {
        let per_line = colorize::hints_by_line(
            &app.document_lines(),
            &colorize::colorize(&app.document_lines().join("\n")),
        );
        for (line, hints) in app.document_lines().iter().zip(&per_line) {
            let old = hints
                .iter()
                .find(|(_, role)| *role == SyntaxRole::Comment)
                .map(|(range, _)| line[..range.start].trim_end())
                .unwrap_or(line.as_str());
            let new = match annotation_start(line) {
                Some(pos) => &line[..pos],
                None => line.as_str(),
            };
            assert_eq!(new, old, "the two rules disagree on {line:?}");
        }
    }
}

/// Test plan item 4. `row_content` no longer consults any hint vector,
/// which is what lets it keep working on lines that are nowhere near
/// the viewport. Selecting a range entirely below the visible window and
/// copying it must still strip annotations — under the old rule this
/// read `line_styles` at an index the viewport had never populated.
#[test]
fn clipboard_copy_strips_annotations_outside_the_viewport() {
    let mut app = nested_message_set_fixture();
    app.annotations = false;
    // A window covering only the first two rows; everything selected
    // below is outside it.
    let window = [app.committed_row(0).unwrap(), app.committed_row(1).unwrap()];
    app.refresh_window_styles(&window);

    let last = app.document_lines().len() - 1;
    let anchor = app.line_pos(last - 3).expect("the anchor row must exist");
    app.select_anchor = Some(CursorPos {
        node: anchor.node,
        line_in_node: anchor.line_in_node,
        column: 0,
    });
    app.select_engaged = true;
    let end = app.line_pos(last).expect("the last row must exist");
    app.cursor = end.node;
    app.cursor_line_in_node = end.line_in_node;
    app.caret_to_line_end();
    let (count, copied) = app.selected_text().expect("the selection must yield text");

    assert_eq!(count, 4);
    for line in copied.lines() {
        assert!(
            !line.contains("#@"),
            "annotations must be stripped outside the viewport too, got {line:?}"
        );
    }
}

/// Draw one frame of `app`, splash dismissed. Spec 0194 draws the caret
/// in `render`, over the finished span list, so its assertions have to
/// read the frame rather than `row_spans`.
fn drawn_frame(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    terminal
}

/// Every main-pane cell of a drawn frame whose style satisfies `pick`,
/// as `(committed line index, symbol)`.
fn marked_cells(
    app: &App,
    terminal: &Terminal<TestBackend>,
    pick: impl Fn(Style) -> bool,
) -> Vec<(usize, String)> {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    let mut found = Vec::new();
    for y in area.y..area.y + area.height {
        let Some((_, line)) = app.visible_row_pos(app.scroll.index + (y - area.y) as usize) else {
            continue;
        };
        for x in area.x..area.x + area.width {
            let cell = &buffer[(x, y)];
            if pick(cell.style()) {
                found.push((line, cell.symbol().to_string()));
            }
        }
    }
    found
}

/// The cells drawn in `theme::caret_style` — the one inverted character
/// (spec 0194 S2).
fn caret_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    marked_cells(app, terminal, |s| {
        s.add_modifier.contains(Modifier::REVERSED)
    })
}

/// The cells drawn in `theme::brace_match_style` — the brace matching
/// the one the caret is standing on (spec 0233 S3).
fn brace_match_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    let want = crate::theme::brace_match_style(app.theme).bg;
    marked_cells(app, terminal, move |s| s.bg == want)
}

/// The cells drawn in `theme::search_current_style` — the one match the
/// sweep is standing on (spec 0235 S14).
fn search_current_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    let want = crate::theme::search_current_style(app.theme).bg;
    marked_cells(app, terminal, move |s| s.bg == want)
}

/// The cells drawn in `theme::search_match_style` — every *other*
/// occurrence of the pattern in the window (spec 0235 S14).
fn search_match_cells(app: &App, terminal: &Terminal<TestBackend>) -> Vec<(usize, String)> {
    let want = crate::theme::search_match_style(app.theme).bg;
    marked_cells(app, terminal, move |s| s.bg == want)
}

/// Search for `pattern` the way the keyboard does — `/`, the pattern,
/// `Enter` — which is the only path that leaves `last_search` set, and
/// so the only one spec 0235 S15's highlight outlives.
fn search_by_key(app: &mut App, pattern: &str) {
    app.splash = false;
    app.term_width = 120;
    for c in std::iter::once('/').chain(pattern.chars()) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

/// Put the caret on one member of the cursor node's brace pair.
fn put_caret_on_brace(app: &mut App, closing: bool) -> (usize, usize) {
    let (open, close) = app
        .cursor_brace_pair()
        .expect("the cursor node must be bracketed");
    let (line, column) = if closing { close } else { open };
    // Spec 0216 S7: the caret's row is a coordinate within the node, so
    // the closing brace sits on `lines_total - 1` — which is row 1 only
    // for a node with nothing between its braces.
    app.cursor_line_in_node = (line - app.absolute_start(app.cursor)) as u32;
    app.cursor_column = column;
    (line, column)
}

/// The display column `needle` is drawn in, in characters — not bytes,
/// since the fold marker is three bytes wide and one column wide.
fn column_of(content: &str, needle: &str) -> usize {
    let byte = content
        .find(needle)
        .unwrap_or_else(|| panic!("{content:?} must contain {needle:?}"));
    content[..byte].chars().count()
}

/// Spec 0247 S10/S11: the fold toggle is what the status is *for*, so
/// the color has to survive all the way to a drawn cell.
///
/// Both directions, because only the pair is informative: the fixture
/// with one undeclared field must tint every marker above it, and the
/// same document without that field must tint none — otherwise a test
/// that only looked for the color would pass on a renderer that tinted
/// everything.
#[test]
fn a_defect_tints_the_fold_marker_of_every_node_above_it() {
    let want = crate::theme::status_color(crate::node_status::Status::Unknown, ThemeKind::Dark)
        .expect("an unknown field must have a color of its own");

    let (mut app, ..) = unknown_field_fixture();
    app.theme = ThemeKind::Dark;
    let terminal = drawn_frame(&mut app, 120, 12);
    // The whole fold field is styled, indentation and all; only the
    // glyph has ink in it, so the blank cells say nothing either way.
    //
    // Spec 0328 S3: the current node's bar takes its color from the very
    // same `margin_glyph_color` call, so on this fixture — where the
    // caret rests on the root and the root is the tinted node — it is a
    // second bearer of `want` running down the column. That is the bar
    // agreeing with its triangle, which is what S3 asks for; this test
    // is about which *triangles* the color reaches.
    let tinted: Vec<_> = marked_cells(&app, &terminal, |s| s.fg == Some(want))
        .into_iter()
        .filter(|(_, sym)| sym != " " && sym != &render::TIER_BAR_GLYPH.to_string())
        .collect();
    // The root and `inner {` — every foldable node on the path.
    let glyph = render::FOLD_GLYPH_OPEN.to_string();
    assert_eq!(
        tinted,
        vec![(0, glyph.clone()), (1, glyph)],
        "the status color must reach the marker of every node above the defect"
    );

    let (mut clean, ..) = type_as_fixture();
    clean.theme = ThemeKind::Dark;
    clean.splash = false;
    let terminal = drawn_frame(&mut clean, 120, 12);
    assert_eq!(
        marked_cells(&clean, &terminal, |s| s.fg == Some(want)),
        Vec::new(),
        "a document with nothing wrong must tint nothing"
    );
}

/// Spec 0322 S1/S3 and its test-plan items 1 and 2: a leaf whose own
/// status is an anomaly wears the diamond, in the status hue — and its
/// clean sibling wears nothing, which is the half that would pass on a
/// renderer that marked every leaf.
#[test]
fn a_leaf_anomaly_wears_a_diamond() {
    let want =
        crate::theme::status_color(crate::node_status::Status::NonCanonical, ThemeKind::Dark)
            .expect("a non-canonical node must have a color of its own");

    let mut app = sibling_leaves_app(&["x: 1  #@ varint; val_ohb: 3", "y: 2  #@ varint"]);
    app.theme = ThemeKind::Dark;
    let terminal = drawn_frame(&mut app, 120, 8);
    // Where a leaf's toggle would have gone: the heat gutter, then
    // `marker_column` — 0 here, since a root-level row has no
    // indentation for the mark to sit in.
    let at = (
        app.main_area.x
            + render::HEAT_FIELD_WIDTH as u16
            + render::marker_column(&app.document_lines()[0]),
        app.main_area.y + app.visible_row_of_line(0).expect("row 0 is drawn") as u16,
    );
    assert_eq!(
        margin_cells(&app, &terminal, render::ANOMALY_GLYPH),
        vec![(at.0, at.1, want)],
        "the non-canonical leaf, and only it, must be marked"
    );

    // Test-plan item 4, the case the spec exists for: `a` truncates the
    // row at `annotation_start`, so the annotation that used to be the
    // leaf's only status signal is gone outright — and the mark is not.
    app.annotations = false;
    let terminal = drawn_frame(&mut app, 120, 8);
    assert!(
        !app.row_content(app.committed_row(0).unwrap())
            .contains("val_ohb"),
        "`a` must have removed the annotation, not merely dimmed it"
    );
    assert_eq!(
        margin_cells(&app, &terminal, render::ANOMALY_GLYPH),
        vec![(at.0, at.1, want)],
        "the mark must outlive the annotation it stands in for"
    );
}

/// Spec 0322 N1 / test-plan item 3: `Unknown` is absence of information
/// rather than a defect, and spec 0247 S12 makes it universal in the
/// documents that produce it — so an unknown leaf gets no mark. The
/// fixture is the one `a_defect_tints_the_fold_marker_of_every_node_
/// above_it` uses, which pins that its ancestors' toggles *are* tinted,
/// so this is not passing by nothing being wrong with it.
#[test]
fn an_unknown_leaf_wears_no_diamond() {
    let (mut app, ..) = unknown_field_fixture();
    app.theme = ThemeKind::Dark;
    let terminal = drawn_frame(&mut app, 120, 12);
    assert_eq!(
        margin_cells(&app, &terminal, render::ANOMALY_GLYPH),
        Vec::new(),
        "an undeclared field is not an anomaly"
    );
}

/// Spec 0193 test-plan items 1 and 2 (G1, G2). `nested_any_fixture`
/// renders `type_url: "..."` and `value {` as siblings at the same
/// depth — one with nothing to fold, one foldable — which is exactly
/// the pair the complaint was about: today the second one's name sits
/// one column right of the first's.
#[test]
fn the_fold_marker_does_not_displace_the_row_it_marks() {
    let app = nested_any_fixture();
    let line_of = |needle: &str| {
        app.document_lines()
            .iter()
            .position(|l| l.trim_start().starts_with(needle))
            .unwrap_or_else(|| panic!("fixture must render a {needle:?} line"))
    };
    let plain = app.row_content(app.committed_row(line_of("type_url:")).unwrap());
    let foldable = app.row_content(app.committed_row(line_of("value {")).unwrap());
    assert_eq!(
        column_of(&plain, "type_url"),
        column_of(&foldable, "value"),
        "a foldable row's text must start in the same column as its \
         non-foldable sibling's: {plain:?} against {foldable:?}"
    );

    // Item 2, at both ends of the S1 rule: `value {` is indented deeply
    // enough for the marker to take the last two columns of its own
    // indentation, while the root row has no indentation at all and
    // falls back to the reserved field.
    let root = app.row_content(app.committed_row(0).unwrap());
    for (content, token) in [(&foldable, "value"), (&root, "1 {")] {
        assert_eq!(
            column_of(content, &render::FOLD_GLYPH_OPEN.to_string()) + render::FOLD_FIELD_WIDTH,
            column_of(content, token),
            "the marker must sit immediately left of the token: {content:?}"
        );
    }
}

/// Spec 0193 test-plan item 3 (G6). `marker_column` is what the mouse's
/// fold hit test steers by, so it must report the column the glyph is
/// actually drawn in — for every `--indent`, not just the default 2.
/// The `1` case is the one that discriminates: the marker cannot fit in
/// a one-column indent, so it falls back into the reserved field and
/// its column stops tracking the indent.
#[test]
fn marker_column_agrees_with_what_is_drawn_at_every_indent_width() {
    for width in [0usize, 1, 2, 4] {
        let mut app = nested_any_fixture();
        // Re-indent the document as `--indent width` would have. Only
        // the leading whitespace changes, so every node's line range —
        // and therefore every fold marker — stays where it was.
        // Spec 0222 S1: the text is the nodes', so the re-indent is
        // per node. A closing `}` needs no touching — it is derived
        // from its header, so it follows the header's new indent.
        for text in app.node_text_mut().iter_mut().flatten() {
            let reindented: Vec<String> = text
                .split('\n')
                .map(|l| {
                    let depth = (l.len() - l.trim_start().len()) / 2;
                    format!("{}{}", " ".repeat(depth * width), l.trim_start())
                })
                .collect();
            *text = reindented.join("\n").into_boxed_str();
        }

        let window: Vec<DisplayRow> = (0..app.composed_row_count())
            .filter_map(|d| app.display_row(d))
            .collect();
        app.refresh_window_styles(&window);
        for (i, &row) in window.iter().enumerate() {
            let DisplayRow::Committed(committed) = row else {
                continue;
            };
            let drawn: String = app
                .row_spans(row, i, Modifier::empty())
                .iter()
                .map(|s| s.content.to_string())
                .collect();
            let Some(marker) = drawn.chars().position(|c| c == render::FOLD_GLYPH_OPEN) else {
                continue;
            };
            let line = committed.line;
            assert_eq!(
                marker,
                render::marker_column(&app.line_text(committed.pos)) as usize,
                "--indent {width}, line {line}: the hit test and the drawn \
                 glyph must agree on the column: {drawn:?}"
            );
        }
    }
}

/// Spec 0193 test-plan item 4. Spec 0185 G3 requires a preview and the
/// commit that follows it to be byte-identical, which rests on
/// `row_content` and `row_spans` staying in step; S1 and S2 edit both.
/// The fixture is folded partway through so the collapse summary, the
/// margin and the brace highlight all appear.
#[test]
fn row_content_and_row_spans_agree_byte_for_byte() {
    let (mut app, items) = repeated_message_fixture();
    app.toggle_fold(items[1]);
    app.cursor = items[0];

    let window: Vec<DisplayRow> = (0..app.composed_row_count())
        .filter_map(|d| app.display_row(d))
        .collect();
    app.refresh_window_styles(&window);
    assert!(window.len() > 4, "the fixture must have rows to compare");
    for (i, &row) in window.iter().enumerate() {
        let drawn: String = app
            .row_spans(row, i, Modifier::empty())
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        // Spec 0328 S4: the current node's bar is chrome the renderer
        // substitutes into a fold-column cell that was blank — a cell
        // for a cell, so every column downstream still means what it
        // did, which is the property `max_visible_line_len` and the two
        // hover hit tests read `row_content` for. Put back and the two
        // agree byte for byte, as they must.
        //
        // Spec 0334 S1 puts more than one of them on a row, and the
        // glyph is three bytes standing in for one blank — so the
        // second bar's offset in `drawn` runs two bytes ahead of its
        // offset in `content` per bar already passed.
        let bar = render::TIER_BAR_GLYPH.to_string();
        let content = app.row_content(row);
        for (seen, (at, _)) in drawn.match_indices(&bar).enumerate() {
            assert_eq!(
                content.as_bytes().get(at - 2 * seen),
                Some(&b' '),
                "row {i}: the bar may only stand where the margin was blank"
            );
        }
        assert_eq!(drawn.replace(&bar, " "), content, "row {i} disagrees");
    }
}

/// Spec 0233 test-plan item 1 — spec 0194's item 11 with the two styles
/// the other way round. The three states the pair can be in, and in all
/// three the caret's own cell is `caret_style`: that invariance is what
/// the spec is for.
#[test]
fn a_brace_pairs_with_its_match_only_when_the_caret_is_on_it() {
    // Off a brace: one inverted cell, and no match tint anywhere. One
    // column right of Home, since Home on a bracketed header is a
    // pairing state of its own (spec 0234).
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let header = app.absolute_start(items[1]);
    app.cursor_column += 1;
    app.caret_anchor = CaretAnchor::Free;
    let second = app.document_lines()[header]
        .trim_start()
        .chars()
        .nth(1)
        .expect("the header row is longer than one character")
        .to_string();
    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, second)],
        "the caret alone, one column into the row's text"
    );
    assert!(
        brace_match_cells(&app, &terminal).is_empty(),
        "nothing is matched with a caret that is not on a brace"
    );

    // On a brace whose match is scrolled out of the window: nothing to
    // point at, so nothing is tinted.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[2]);
    let (footer, _) = put_caret_on_brace(&mut app, true);
    let mut terminal = drawn_frame(&mut app, 40, 8);
    // No cursor movement in between, so `render`'s auto-pan-into-view
    // guard leaves this alone.
    app.scroll.index = footer;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(footer, "}".to_string())],
        "an unmatchable brace is drawn like any other caret"
    );
    assert!(brace_match_cells(&app, &terminal).is_empty());

    // On a brace whose match is in the window: the match is tinted and
    // the caret is drawn exactly as it was in the other two states.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let (open_line, _) = put_caret_on_brace(&mut app, false);
    let close_line = app.node_lines(items[1]).end - 1;
    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(open_line, "{".to_string())],
        "the caret keeps its own rendering on a brace"
    );
    assert_eq!(
        brace_match_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "and the brace it names wears the tint"
    );
}

/// Spec 0234 test-plan item 1. The caret's cell being a brace is not
/// where the caret spends its time — `set_cursor`, `^`/`Ctrl-a` and
/// `j`/`k` all leave it on the row's first non-blank — so a *voluntary*
/// Home speaks for the `{` its row opens. A Home reached by a vertical
/// move's clamp does not, on spec 0199 S1's rule.
#[test]
fn a_home_caret_lights_the_brace_its_row_opens() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let header = app.absolute_start(items[1]);
    let close_line = app.node_lines(items[1]).end - 1;
    let first_non_blank = app.document_lines()[header]
        .trim_start()
        .chars()
        .next()
        .expect("the header row is not blank")
        .to_string();
    assert_eq!(
        app.caret_anchor,
        CaretAnchor::Home,
        "a node-level jump declares the anchor"
    );

    let mut terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        brace_match_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "the row opens a message, so its close is named"
    );
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, first_non_blank.clone())],
        "and the caret is drawn where and how it always is"
    );

    // The same column, arrived at the way a vertical move would leave
    // it: pinned there by a clamp with a longer column still wanted.
    app.caret_anchor = CaretAnchor::Free;
    app.desired_column = app.cursor_column + 5;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        brace_match_cells(&app, &terminal).is_empty(),
        "an involuntary Home was passing over the row, not reading it"
    );
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, first_non_blank)]
    );
}

/// Spec 0233 test-plan item 2, spec 0194's item 12 renamed. Whether the
/// match is on screen is a property of the frame, not of the last
/// keypress, so it has to be re-resolved on every draw: panning the
/// match off the left edge must drop the tint and panning back must
/// restore it — with nothing that moves the cursor pressed in between,
/// and with the caret untouched throughout.
#[test]
fn the_match_highlight_is_resolved_every_frame() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    let (open_line, _) = put_caret_on_brace(&mut app, false);
    let close_line = app.node_lines(items[1]).end - 1;
    let caret = vec![(open_line, "{".to_string())];
    let mut terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        brace_match_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "matched to begin with"
    );
    assert_eq!(caret_cells(&app, &terminal), caret);

    // Pan just far enough to take the closing brace off the left edge.
    // The caret's own `{` is further right on a more deeply indented
    // row, so it survives the same pan and stays drawn.
    let (_, (_, close_column)) = app.cursor_brace_pair().expect("the node is bracketed");
    app.pan_offset = render::FOLD_FIELD_WIDTH + close_column + 1;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert!(
        brace_match_cells(&app, &terminal).is_empty(),
        "a match panned out of sight is not tinted"
    );
    assert_eq!(
        caret_cells(&app, &terminal),
        caret,
        "and the caret does not notice"
    );

    app.pan_offset = 0;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        brace_match_cells(&app, &terminal),
        vec![(close_line, "}".to_string())],
        "panning back restores the pair"
    );
    assert_eq!(caret_cells(&app, &terminal), caret);
}

/// Spec 0233 test-plan item 3, spec 0194's item 13. A folded node
/// carries its whole pair on one row, and the closing half of it is
/// synthetic — text that exists only as an insertion, at a byte offset
/// `row_text` and `row_spans` disagree about. Drawing the caret by
/// character index over the finished span list is what keeps the two
/// halves apart here; and this is the row where the match tint is laid
/// over `cursor_row_style`, so the two backgrounds have to differ.
#[test]
fn a_folded_node_pairs_its_synthetic_closing_brace_on_the_same_row() {
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[0]);
    app.toggle_fold(items[0]);
    let header = app.absolute_start(items[0]);
    put_caret_on_brace(&mut app, false);

    let terminal = drawn_frame(&mut app, 40, 12);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, "{".to_string())],
        "the caret is on the opening brace"
    );
    assert_eq!(
        brace_match_cells(&app, &terminal),
        vec![(header, "}".to_string())],
        "and the collapse summary's synthetic closing brace is the match"
    );
}

/// A drawable app whose rows read exactly `texts`.
///
/// `sibling_leaves_app` cannot be drawn: its `raw_range`s point into an
/// empty blob, and the heat cue's payload walk trips over that on the
/// first frame. This borrows `wide_sibling_scalars_app`'s real
/// two-byte-per-node blob — one node per row either way, so substituting
/// the text leaves every node's line count valid.
fn text_rows_app(texts: &[&str]) -> App {
    let mut app = wide_sibling_scalars_app(texts.len());
    for (slot, text) in texts.iter().enumerate() {
        app.node_text_mut()[slot] = Some(Box::from(*text));
    }
    app
}

/// The character (not byte) column `needle` was drawn in. The heat and
/// fold glyphs are multi-byte, so a byte offset would be off by their
/// width.
fn drawn_column_of(row: &str, needle: &str) -> usize {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{row:?} must contain {needle:?}"));
    row[..byte].chars().count()
}

/// One drawn main-pane row, as a string — used where an assertion is
/// about *where* something landed on screen rather than how it is
/// styled.
fn drawn_row(app: &App, terminal: &Terminal<TestBackend>, row: u16) -> String {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    (area.x..area.x + area.width)
        .map(|x| buffer[(x, area.y + row)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The foregrounds of one drawn main-pane row.
fn drawn_row_fgs(app: &App, terminal: &Terminal<TestBackend>, row: u16) -> Vec<Color> {
    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    (area.x..area.x + area.width)
        .map(|x| buffer[(x, area.y + row)].fg)
        .collect()
}

/// Spec 0194 test-plan item 1 (G1). vim's caret, not vim's
/// `cursorline`: one cell inverted, and the rest of the row still in
/// the syntax colors it would have had with the cursor elsewhere.
#[test]
fn the_caret_covers_one_cell_and_leaves_the_rows_colors_alone() {
    let mut app = text_rows_app(&["a: 1", "b: 2"]);
    app.cursor = 0;
    app.reset_caret_column();
    let terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(
        caret_cells(&app, &terminal).len(),
        1,
        "one cell, not the whole row"
    );
    let with_caret = drawn_row_fgs(&app, &terminal, 0);

    app.cursor = 1;
    app.reset_caret_column();
    let terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(
        with_caret,
        drawn_row_fgs(&app, &terminal, 0),
        "the caret's row keeps every foreground it has without one"
    );
}

/// Spec 0194 test-plan item 2 (S2, G7), as amended by spec 0242 S11 and
/// again on 2026-08-05. The caret's row gets vim's `cursorline` tint,
/// the selection tints the selected *characters* more strongly on top
/// of it, and the caret is the one reversed cell — it no longer cancels
/// against the selection, because the selection no longer reverses.
#[test]
fn a_selected_row_and_the_caret_row_stay_distinguishable() {
    let mut app = text_rows_app(&["a: 1", "b: 2", "c: 3"]);
    // Anchored at the end of the second row with the caret at the start
    // of the first: both rows are selected in full, and the caret sits
    // on one of them.
    app.cursor = 1;
    app.reset_caret_column();
    app.caret_to_line_end();
    app.select_anchor = Some(app.cursor_pos());
    app.select_engaged = true;
    app.cursor = 0;
    app.reset_caret_column();
    let terminal = drawn_frame(&mut app, 40, 6);

    let area = app.main_area;
    let buffer = terminal.backend().buffer();
    // The heat-cue column and the fold field come first; the row's own
    // four characters start after them, and are the only cells a
    // selection may touch.
    let text_x = (render::HEAT_FIELD_WIDTH + render::FOLD_FIELD_WIDTH) as u16;
    let reversed = |row: u16, x: u16| {
        buffer[(area.x + x, area.y + row)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    };
    let selected = |row: u16, x: u16| {
        buffer[(area.x + x, area.y + row)].style().bg == crate::theme::selection_style(app.theme).bg
    };

    assert!(
        (text_x..text_x + 4).all(|x| selected(1, x)),
        "a fully selected row that is not the caret's is tinted across its text"
    );
    assert!(
        (0..text_x).all(|x| !selected(1, x)),
        "and the gutter it is drawn beside is not in the selection"
    );
    assert!(
        (text_x..text_x + 4).all(|x| !reversed(1, x)),
        "the selection is a tint, never an inversion — inversion is the caret's"
    );
    let inverted: Vec<u16> = (text_x..text_x + 4).filter(|&x| reversed(0, x)).collect();
    assert_eq!(
        inverted,
        vec![text_x],
        "on a row that is both, the caret cell alone is inverted, and it is still selected"
    );
    assert!(
        selected(0, text_x),
        "the caret's own cell keeps the selection tint under the inversion"
    );
    assert_eq!(
        buffer[(area.x, area.y)].style().bg,
        crate::theme::cursor_row_style(app.theme).bg,
        "and the caret's row carries the cursorline tint underneath"
    );
    assert!(
        !selected(2, text_x),
        "the unselected row below is left entirely alone"
    );
}

/// Spec 0194 test-plan item 5 (S1, S3), drawing half. The heat suffix
/// is reachable even though it is drawn past the row's text; the heat
/// glyph in the first column is not, because the caret's screen index
/// is one-based by construction.
#[test]
fn the_caret_reaches_the_heat_suffix_but_never_the_heat_glyph() {
    let mut app = message_node_app();
    let idx = 0;
    let range = extract::message_payload_range(&app.blob, &app.tree[idx].span.raw_range);
    super::heat_cue::seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    app.cursor = idx;
    app.heat_cues = heat_cue::HeatCueMode::Findings;
    let header = app.absolute_start(idx);

    // `$` — the last reachable column is the suffix's closing bracket.
    let terminal = drawn_frame(&mut app, 60, 8);
    assert!(
        drawn_row(&app, &terminal, 0).ends_with("[10/50]"),
        "the fixture must actually draw a heat suffix: {:?}",
        drawn_row(&app, &terminal, 0)
    );
    app.caret_to_line_end();
    let terminal = drawn_frame(&mut app, 60, 8);
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(header, "]".to_string())]
    );

    // `0` — and the leftmost is the row's first character, never the
    // glyph column the cue itself occupies.
    app.caret_to_line_start();
    let terminal = drawn_frame(&mut app, 60, 8);
    let area = app.main_area;
    assert!(
        !terminal.backend().buffer()[(area.x, area.y)]
            .style()
            .add_modifier
            .contains(Modifier::REVERSED),
        "column 0 belongs to the heat glyph and is unreachable"
    );
    assert_eq!(caret_cells(&app, &terminal).len(), 1);
}

/// Spec 0194 test-plan item 8 (G6). The caret addresses a character,
/// not a screen column, so nothing that only moves the row sideways may
/// move it: a horizontal pan, or the fold marker appearing beside it.
/// The heat suffix is appended after the pan, so it does not slide
/// either — and the caret standing in it does not slide within it.
#[test]
fn the_caret_holds_its_character_across_a_pan_and_a_fold() {
    let mut app = text_rows_app(&["    abcdefgh"]);
    app.cursor = 0;
    app.cursor_column = 7;
    app.desired_column = 7;
    let mut terminal = drawn_frame(&mut app, 40, 6);
    assert_eq!(caret_cells(&app, &terminal), vec![(0, "d".to_string())]);

    app.pan_offset = 3;
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        vec![(0, "d".to_string())],
        "the pan slides the row, not the caret"
    );

    // The marker appearing beside a row must not shift its text either
    // — spec 0193 reserves the field whether or not it is used.
    let (mut app, items) = repeated_message_fixture();
    app.set_cursor(items[1]);
    app.caret_right();
    let mut terminal = drawn_frame(&mut app, 60, 12);
    let before = caret_cells(&app, &terminal);
    app.toggle_fold(items[1]);
    terminal.draw(|frame| app.render(frame)).unwrap();
    assert_eq!(
        caret_cells(&app, &terminal),
        before,
        "folding the row under the caret does not move it"
    );
}

/// Spec 0194 test-plan item 8, suffix half. The heat suffix is appended
/// after the pan is applied, so it stays put while the text slides
/// underneath it.
#[test]
fn the_heat_suffix_does_not_slide_under_a_pan() {
    let mut app = message_node_app();
    let range = extract::message_payload_range(&app.blob, &app.tree[0].span.raw_range);
    super::heat_cue::seed_range_heat_entry(
        &mut app,
        range.start,
        Some(50),
        1,
        "google.protobuf.DescriptorProto",
        Some(10),
    );
    app.cursor = 0;
    app.heat_cues = heat_cue::HeatCueMode::Findings;

    let mut terminal = drawn_frame(&mut app, 60, 8);
    let unpanned = drawn_row(&app, &terminal, 0);
    let suffix_at = drawn_column_of(&unpanned, "[10/50]");

    app.caret_to_line_end();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let caret_at_end = caret_cells(&app, &terminal);

    app.pan_offset = 3;
    terminal.draw(|frame| app.render(frame)).unwrap();
    let panned = drawn_row(&app, &terminal, 0);
    assert_eq!(
        drawn_column_of(&panned, "[10/50]"),
        suffix_at - 3,
        "the suffix keeps its place relative to the text it follows"
    );
    assert_ne!(panned, unpanned, "the text itself did pan");
    assert_eq!(
        caret_cells(&app, &terminal),
        caret_at_end,
        "and the caret does not move within the suffix"
    );
}

/// Spec 0194 test-plan item 16 (S1). A row with no text of its own —
/// or none past the fold field — still has one reachable column, drawn
/// on a synthetic trailing space. Without it the caret would simply
/// vanish on such a row.
#[test]
fn the_caret_is_drawn_on_a_synthetic_space_on_a_blank_row() {
    for text in ["", "   "] {
        let mut app = text_rows_app(&[text, "a: 1"]);
        app.cursor = 0;
        app.reset_caret_column();
        let terminal = drawn_frame(&mut app, 40, 6);
        assert_eq!(
            caret_cells(&app, &terminal),
            vec![(0, " ".to_string())],
            "a blank row keeps exactly one reachable column, on {text:?}"
        );
    }
}

/// Spec 0194 test-plan item 18 (A5). The caret's column counts
/// characters, so one press crosses a multi-byte character whole — and
/// the character-index walk that draws it never splits one, which a
/// byte-offset version would.
#[test]
fn the_caret_crosses_a_multi_byte_character_in_one_press() {
    let text = "s: \"héllo\"";
    let mut app = text_rows_app(&[text]);
    app.cursor = 0;
    app.reset_caret_column();

    for want in text.chars() {
        let terminal = drawn_frame(&mut app, 40, 6);
        assert_eq!(
            caret_cells(&app, &terminal),
            vec![(0, want.to_string())],
            "one press per character, including the multi-byte one"
        );
        app.caret_right();
    }
}

/// Spec 0193 test-plan item 10 (S4). vim's own rule, boundaries
/// included — the last full screen must read `Bot` rather than `99%`,
/// which is the case a naive percentage gets wrong.
#[test]
fn viewport_label_matches_vims_rule_at_the_boundaries() {
    for (top, height, total, want) in [
        (0isize, 10usize, 10usize, "All"),
        (0, 10, 3, "All"),
        (0, 10, 100, "Top"),
        (90, 10, 100, "Bot"),
        (89, 10, 100, "98%"),
        (45, 10, 100, "50%"),
        (1, 10, 100, "1%"),
    ] {
        assert_eq!(
            viewport_label(top, height, total),
            want,
            "viewport_label({top}, {height}, {total})"
        );
    }
}

/// Spec 0244 test-plan item 10 (S10). An over-panned viewport is the
/// case the old `total <= height` first test got wrong: a document
/// shorter than the pane still reads `All` only while both of its ends
/// are on screen, and says which one has left once one has.
#[test]
fn viewport_label_over_a_short_over_panned_document() {
    for (top, height, total, want) in [
        // Three rows in a ten-row pane, panned neither way.
        (0isize, 10usize, 3usize, "All"),
        // Panned up until only the first row is left, on the last row of
        // the pane — the bottom edge is far past the content's end, but
        // the content's own top is not on screen.
        (-9, 10, 3, "Top"),
        // Blank rows above, and still every row on screen.
        (-7, 10, 3, "All"),
        // Panned down until only the last row is left, on the pane's
        // first row.
        (2, 10, 3, "Bot"),
    ] {
        assert_eq!(
            viewport_label(top, height, total),
            want,
            "viewport_label({top}, {height}, {total})"
        );
    }
}

/// Spec 0193 test-plan item 11 (G5) — the reported complaint, end to
/// end. Panning moves the whole screen without moving the cursor, so
/// `L<n>/<m>` is right to sit still; what was missing was anything at
/// all that reported the viewport.
#[test]
fn panning_changes_the_viewport_label_and_not_the_cursor_ruler() {
    let mut app = wide_sibling_scalars_app(200);
    app.splash = false;
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    let statusline = |app: &App, terminal: &Terminal<TestBackend>| -> String {
        let buffer = terminal.backend().buffer();
        let y = app.main_area.y + app.main_area.height;
        (app.main_area.x..app.main_area.x + app.main_area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    };

    terminal.draw(|frame| app.render(frame)).unwrap();
    let before = statusline(&app, &terminal);
    assert!(before.ends_with("L1/200  Top"), "at rest: {before:?}");

    let cursor = app.cursor;
    // Five `PAN_STEP`s down: far enough to be neither `Top` nor `Bot`,
    // so the indicator has to answer with a real percentage.
    for _ in 0..5 {
        app.pan_vertical_down();
    }
    terminal.draw(|frame| app.render(frame)).unwrap();
    let after = statusline(&app, &terminal);

    assert_eq!(app.cursor, cursor, "panning must not move the cursor");
    assert!(
        after.ends_with("L1/200  22%"),
        "the cursor ruler stands still and the viewport indicator moves: {after:?}"
    );
}

/// An `App` opened over a descriptor set with no `index.rkyv` sidecar,
/// so `DescriptorContext::load` took the eager path and recorded spec
/// 0197 §S3's warning.
///
/// Returned untouched — splash still up, status line still carrying the
/// warning — because those two are exactly what the §S3 tests inspect.
fn eager_fallback_app() -> App {
    use prost_types::field_descriptor_proto::{Label, Type};

    use crate::decode::{decode, RootType};

    let fds = proto3_fds(
        "test_eager_fallback.proto",
        vec![message(
            "Inner",
            vec![field("id", 1, Label::Optional, Type::Int32)],
        )],
    );

    // Not `fixture_under`: that lowers the splash and widens the
    // terminal, and this fixture's whole point is the state `App::new`
    // leaves behind.
    let mut ctx = ctx_from_fds("eager-fallback", &fds);
    let blob = [0x08u8, 0x05];
    let decoded = decode(wrapped(&blob), &mut ctx, RootType::Named("test.Inner"), 2).unwrap();
    app_named(decoded, ctx, "test.pb")
}

/// Spec 0235 test-plan item 13 (G5, S14). Every occurrence in the window
/// is tinted, over its whole extent, and the one `Enter` landed on is
/// tinted differently — which is what makes `n` readable when several
/// matches share a row.
#[test]
fn every_visible_match_is_tinted_and_the_current_one_differently() {
    let mut app = sibling_leaves_app(&["ab ab ab", "zz"]);
    // Spec 0246 S2/S3: forward from column 0 of the first row, which is
    // itself a match — so the walk steps off it onto the *second*
    // occurrence of the same row rather than leaving the row entirely.
    search_by_key(&mut app, "ab");
    assert_eq!((app.cursor, app.cursor_column), (0, 3));

    let terminal = drawn_frame(&mut app, 40, 8);
    assert_eq!(
        search_current_cells(&app, &terminal),
        vec![(0, "a".to_string()), (0, "b".to_string())],
        "the current match, over its whole extent"
    );
    assert_eq!(
        search_match_cells(&app, &terminal),
        vec![
            (0, "a".to_string()),
            (0, "b".to_string()),
            (0, "a".to_string()),
            (0, "b".to_string())
        ],
        "and the other two occurrences of the row"
    );
}

/// Spec 0274 S13. A pattern that may cross a row tints the current hit
/// over its whole extent — across the row boundary, the tail of one row
/// and the head of the next — *and* every other occurrence on screen,
/// exactly as a single-row pattern does.
///
/// Both halves matter. The extent is the result the request asked to
/// see; the other occurrence is the parity with single-row search that
/// the reader has no reason to expect to lose just because the pattern
/// grew a `\n`. It is affordable because the scan is bounded by the
/// window rather than by the document: one pass over the drawn rows,
/// cut where they are not document-adjacent.
#[test]
fn a_cross_row_match_is_tinted_over_both_rows_and_over_every_occurrence() {
    let mut app = sibling_leaves_app(&["ab", "cd", "ab", "cd"]);
    search_by_key(&mut app, r"b\nc");

    let terminal = drawn_frame(&mut app, 40, 8);
    assert_eq!(
        search_current_cells(&app, &terminal),
        vec![(0, "b".to_string()), (1, "c".to_string())],
        "the tail of the first row and the head of the second"
    );
    assert_eq!(
        search_match_cells(&app, &terminal),
        vec![(2, "b".to_string()), (3, "c".to_string())],
        "and the identical pair on rows 2 and 3, in the other style"
    );
}

/// Spec 0235 test-plan item 14 (G5, S15). The highlight is not the
/// prompt's — it outlives the commit, and `n` moves which occurrence
/// wears the strong tint without turning the rest off.
#[test]
fn the_highlight_survives_the_commit_and_n() {
    let mut app = sibling_leaves_app(&["ab one", "ab two"]);
    search_by_key(&mut app, "ab");
    assert_eq!(app.cursor, 1);

    let terminal = drawn_frame(&mut app, 40, 8);
    assert_eq!(
        search_current_cells(&app, &terminal),
        vec![(1, "a".to_string()), (1, "b".to_string())]
    );
    assert_eq!(
        search_match_cells(&app, &terminal),
        vec![(0, "a".to_string()), (0, "b".to_string())]
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    let terminal = drawn_frame(&mut app, 40, 8);
    assert_eq!(
        search_current_cells(&app, &terminal),
        vec![(0, "a".to_string()), (0, "b".to_string())],
        "the strong tint followed `n`"
    );
    assert_eq!(
        search_match_cells(&app, &terminal),
        vec![(1, "a".to_string()), (1, "b".to_string())],
        "and the row it left is still context"
    );
}

/// Spec 0235 test-plan item 15 (S15). vim's `:nohlsearch`, without the
/// command — a highlight the user cannot dismiss leaves a third of a
/// document tinted after a search for `id`.
#[test]
fn esc_outside_the_prompt_clears_the_highlight() {
    let mut app = sibling_leaves_app(&["ab one", "ab two"]);
    search_by_key(&mut app, "ab");
    let terminal = drawn_frame(&mut app, 40, 8);
    assert!(!search_current_cells(&app, &terminal).is_empty());

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let terminal = drawn_frame(&mut app, 40, 8);
    assert!(search_current_cells(&app, &terminal).is_empty());
    assert!(search_match_cells(&app, &terminal).is_empty());
}

/// Spec 0235 test-plan item 16 (S14's ordering). The caret is applied
/// last, so the cell it shares with a match keeps its inversion — the
/// search tint is a background and composes with it rather than
/// replacing it.
#[test]
fn the_search_highlight_yields_its_cell_to_the_caret() {
    let mut app = sibling_leaves_app(&["zz", "ab cd"]);
    search_by_key(&mut app, "ab");
    assert_eq!((app.cursor, app.cursor_column), (1, 0));

    let terminal = drawn_frame(&mut app, 40, 8);
    assert_eq!(caret_cells(&app, &terminal), vec![(1, "a".to_string())]);
    assert!(search_current_cells(&app, &terminal).contains(&(1, "a".to_string())));
}

/// Spec 0235 test-plan item 21 (S22). A path is not on screen, so a
/// path match marks the one cell `Enter` would put the caret on — and
/// only for the current match, or a pattern matching most paths would
/// tint most of the document for nothing.
#[test]
fn a_path_match_tints_only_the_current_row() {
    let (mut app, ..) = packed_run_with_tail_fixture();
    assert!(
        app.document_lines().iter().all(|l| !l.contains('/')),
        "no line's text may match, or the path rule is not what is tested"
    );

    app.set_cursor(app.first_node);
    // `//` is `/` typed into a prompt `/` opened — a pattern every
    // positional path contains.
    search_by_key(&mut app, "/");

    let terminal = drawn_frame(&mut app, 80, 16);
    let current = search_current_cells(&app, &terminal);
    assert_eq!(current.len(), 1, "one cell, not a range: {current:?}");
    assert_eq!(current[0].0, 1, "the first row after the one it started on");
    assert!(search_match_cells(&app, &terminal).is_empty());
}
