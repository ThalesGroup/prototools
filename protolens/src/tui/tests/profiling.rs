// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// Throwaway diagnostic for the 2026-07-24 override-pane `Down` slowdown
// report — not meant to stay in the tree. Run with:
//   cargo test --release -p protolens --lib tui::tests::profiling -- --ignored --nocapture

use std::path::Path;
use std::time::Instant;

use super::super::*;
use crate::decode::decode;

/// Throwaway diagnostic: does `/tmp/pdb.desc`'s decoded tree contain
/// any parent with a very large number of direct children? Hypothesis
/// under test: `sibling_position`/`positional_path` are O(k) in a
/// node's own sibling index (walking `prev_sibling` back to the
/// start), so a parent with N children makes computing `positional_
/// path` for all of them O(N^2) -- and `render_overrides_inner`
/// computes it (repeatedly) for every node in the whole document.
#[test]
#[ignore]
fn diagnose_pdb_max_children_per_parent() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }
    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    eprintln!("total tree nodes: {}", decoded.tree.len());

    let mut children_count = vec![0usize; decoded.tree.len()];
    for (idx, node) in decoded.tree.iter().enumerate() {
        if let Some(p) = node.parent {
            children_count[p] += 1;
        }
        let _ = idx;
    }
    let mut with_counts: Vec<(usize, usize)> = children_count.into_iter().enumerate().collect();
    with_counts.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    eprintln!("top 10 parents by direct-child count:");
    for (idx, c) in with_counts.iter().take(10) {
        eprintln!(
            "  node {idx}: {c} children, field_number={}, is_message={}",
            decoded.tree[*idx].span.field_number, decoded.tree[*idx].span.is_message
        );
    }
}

#[test]
#[ignore]
fn profile_override_pane_down_on_db3() {
    let desc_path = Path::new("/tmp/db3.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/db3.desc not present");
        return;
    }

    let t0 = Instant::now();
    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    eprintln!("DescriptorContext::load: {:?}", t0.elapsed());

    let blob = std::fs::read(desc_path).expect("read blob");
    let t1 = Instant::now();
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    eprintln!("decode: {:?}", t1.elapsed());

    let t2 = Instant::now();
    let mut app = App::new(
        decoded,
        "db3.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!("App::new: {:?}", t2.elapsed());
    eprintln!("total lines: {}", app.lines.len());
    eprintln!("total tree nodes: {}", app.tree.len());

    // Mirror `mod.rs`'s `run` setup: `App::new` alone never spawns the
    // background heat-scoring worker (it starts as `None`) — without this,
    // `heat_lookup` requests would enqueue and sit forever, unlike a real
    // interactive session.
    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx,
        ));
    }

    // Use App::new's own default cursor position, unmodified — exact repro
    // of the user's report: launch, then immediately press `t`/`Down`
    // without navigating first.
    eprintln!("default cursor = {}", app.cursor);
    eprintln!(
        "cursor node: is_message={}, field_number={}, wire_type={}",
        app.tree[app.cursor].span.is_message,
        app.tree[app.cursor].span.field_number,
        app.tree[app.cursor].span.wire_type
    );

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t3 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!("'t' (open override pane, no draw): {:?}", t3.elapsed());
    let t3b = Instant::now();
    terminal.draw(|frame| app.render(frame)).unwrap();
    eprintln!("draw after 't': {:?}", t3b.elapsed());
    assert!(app.override_target.is_some(), "override pane must open");
    eprintln!(
        "override_candidates.len() = {}, complete = {}, pending = {}",
        app.override_candidates.len(),
        app.override_candidates_complete,
        app.override_candidates_pending
    );

    // Mimic the real event loop's background-worker polling (mod.rs's
    // `app.poll_pending_override_work()`) until the candidate list settles
    // or a generous timeout elapses, so the Down measurements below see a
    // realistic (non-empty) candidate list, same as an interactive session.
    let t_poll = Instant::now();
    let mut polls = 0;
    while app.override_candidates_pending && t_poll.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_pending_override_work();
        polls += 1;
    }
    eprintln!(
        "settle-poll: {:?} ({polls} polls), candidates.len() = {}, complete = {}",
        t_poll.elapsed(),
        app.override_candidates.len(),
        app.override_candidates_complete
    );

    terminal.draw(|frame| app.render(frame)).unwrap();

    for i in 0..6 {
        let t = Instant::now();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let key_elapsed = t.elapsed();
        let td = Instant::now();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let draw_elapsed = td.elapsed();
        eprintln!(
            "Down #{i}: key={key_elapsed:?} draw={draw_elapsed:?} tree.len()={}",
            app.tree.len()
        );
    }
}

/// Throwaway diagnostic for the 2026-07-26 report: "browsing types for
/// the first child is faster than for the root, but still noticeably
/// slower". Times `preview_override_highlight` alone (no scoring, no
/// draw) on the root and on its first child, on the 1.1 MB
/// `/tmp/db3.desc`. Under spec 0185 the preview renders only the
/// target's own byte-budgeted interior and touches nothing
/// document-sized, so the two must now cost the same; any gap left is
/// the render itself, not the document.
#[test]
#[ignore]
fn profile_preview_root_versus_first_child_on_db3() {
    let desc_path = Path::new("/tmp/db3.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/db3.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "db3.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!(
        "tree.len()={} lines.len()={}",
        app.tree.len(),
        app.lines.len()
    );

    let root = app.first_node;
    let first_child = app.tree[root].first_child.expect("root must have a child");

    // Two fixed candidates, alternated, so each measurement is a real
    // re-render rather than a `render_cache` hit.
    let candidates: Vec<(String, Option<i64>)> = vec![
        ("google.protobuf.FileDescriptorSet".to_string(), None),
        ("google.protobuf.FileDescriptorProto".to_string(), None),
    ];

    for (label, idx) in [("root", root), ("first child", first_child)] {
        app.override_target = Some(idx);
        app.override_candidates = candidates.clone();
        // Warm the cache the same way for both, then measure.
        app.override_highlight = 0;
        app.preview_override_highlight();

        let t = Instant::now();
        const N: usize = 20;
        for i in 0..N {
            app.override_highlight = i % 2;
            app.preview_override_highlight();
        }
        eprintln!(
            "{label} (idx {idx}): {N} previews in {:?} ({:?}/preview), overlay={} lines, \
             tree.len()={} lines.len()={}",
            t.elapsed(),
            t.elapsed() / N as u32,
            app.preview_overlay.as_ref().map_or(0, |o| o.lines.len()),
            app.tree.len(),
            app.lines.len(),
        );
    }
    app.close_override();
}

/// Throwaway diagnostic for the 2026-07-24 report: `t`, `Down`, `Enter`
/// (confirming an override) against `/tmp/pdb.desc` — reported as
/// "painfully slow, does it even complete??".
#[test]
#[ignore]
fn profile_override_pane_enter_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let t0 = Instant::now();
    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    eprintln!("DescriptorContext::load: {:?}", t0.elapsed());

    let blob = std::fs::read(desc_path).expect("read blob");
    let t1 = Instant::now();
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    eprintln!("decode: {:?}", t1.elapsed());

    let t2 = Instant::now();
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!("App::new: {:?}", t2.elapsed());
    eprintln!("total lines: {}", app.lines.len());
    eprintln!("total tree nodes: {}", app.tree.len());

    let mut rx = None;
    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx,
        ));
        rx = Some(worker_rx);
    }
    eprintln!("heat_worker spawned = {}", app.heat_worker.is_some());

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t3 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!("'t': {:?}", t3.elapsed());
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t_poll = Instant::now();
    let mut polls = 0;
    while app.override_candidates_pending && t_poll.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_pending_override_work();
        if let Some(rx) = &rx {
            while rx.try_recv().is_ok() {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
            }
        }
        polls += 1;
    }
    eprintln!(
        "settle-poll: {:?} ({polls} polls), candidates.len() = {}",
        t_poll.elapsed(),
        app.override_candidates.len()
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t4 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    eprintln!("'Down': {:?}", t4.elapsed());
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!("about to press Enter -- tree.len()={}", app.tree.len());
    let t5 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!("'Enter': {:?}", t5.elapsed());
    eprintln!(
        "after Enter: tree.len()={} manage_open={} entries.len()={}",
        app.tree.len(),
        app.manage_open,
        app.overrides.entries().len()
    );
    let td = Instant::now();
    terminal.draw(|frame| app.render(frame)).unwrap();
    eprintln!("draw after Enter: {:?}", td.elapsed());

    // 2026-07-25 follow-up report: activating a *different* override
    // for root (reopening the selection pane from the manage pane and
    // confirming a different candidate) is still slow after the
    // `positional_path` fix. Reproduce: Enter (reopen selection pane on
    // the highlighted -- root -- entry), Down (pick a different
    // candidate), Enter (confirm).
    let t6 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!("reopen 'Enter': {:?}", t6.elapsed());
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t7 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    eprintln!("second 'Down': {:?}", t7.elapsed());
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!("about to press 2nd Enter -- tree.len()={}", app.tree.len());
    let t8 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!("2nd confirm 'Enter': {:?}", t8.elapsed());
    eprintln!(
        "after 2nd Enter: tree.len()={} manage_open={} entries.len()={}",
        app.tree.len(),
        app.manage_open,
        app.overrides.entries().len()
    );
}

/// 2026-07-25 follow-up: does `t` immediately followed by `Enter` (no
/// `Down` -- confirming the *first* listed candidate) stay slow on
/// `/tmp/pdb.desc`, or was the earlier `Enter` timing dominated by
/// which candidate happened to get selected?
#[test]
#[ignore]
fn profile_override_pane_first_candidate_enter_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!("total tree nodes: {}", app.tree.len());

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    let t_poll = Instant::now();
    while app.override_candidates_pending && t_poll.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_pending_override_work();
    }
    eprintln!("candidates.len() = {}", app.override_candidates.len());
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!("about to press Enter -- tree.len()={}", app.tree.len());
    let t5 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!("'Enter' (first candidate): {:?}", t5.elapsed());
    eprintln!(
        "after Enter: tree.len()={} manage_open={} entries.len()={}",
        app.tree.len(),
        app.manage_open,
        app.overrides.entries().len()
    );
}

/// Throwaway diagnostic for the 2026-07-24 report: main-pane `Down`
/// (no override pane ever opened) is also slow on a large document.
/// Isolates key-handling cost from draw cost, same as the override-pane
/// harness above.
#[test]
#[ignore]
fn profile_main_pane_down_on_db3() {
    let desc_path = Path::new("/tmp/db3.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/db3.desc not present");
        return;
    }

    let t0 = Instant::now();
    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    eprintln!("DescriptorContext::load: {:?}", t0.elapsed());

    let blob = std::fs::read(desc_path).expect("read blob");
    let t1 = Instant::now();
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    eprintln!("decode: {:?}", t1.elapsed());

    let t2 = Instant::now();
    let mut app = App::new(
        decoded,
        "db3.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!("App::new: {:?}", t2.elapsed());
    eprintln!("total lines: {}", app.lines.len());
    eprintln!("total tree nodes: {}", app.tree.len());

    // Mirror `mod.rs`'s real `run()` setup (2026-07-24 follow-up
    // feedback: the first test never spawned this, so it never
    // exercised the cache-miss path the user actually reported) — the
    // worker is spawned *before* any navigation, exactly like a real
    // session. Unlike the first attempt, the receiver is kept (not
    // discarded) so `HeatWorkerProgress` events can be pumped into
    // `recheck_pending_heat_states`/`poll_pending_override_work` below,
    // exactly like `mod.rs`'s `run_loop` does — the first attempt's
    // `let (tx, _rx) = ...` silently skipped that call path entirely,
    // which is why it never reproduced the reported slowdown.
    let mut rx = None;
    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx,
        ));
        rx = Some(worker_rx);
    }
    eprintln!("heat_worker spawned = {}", app.heat_worker.is_some());

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // No warm-up pass here (unlike `run()`), and enough iterations to
    // scroll well past the initial viewport into genuinely
    // never-visited nodes, matching the user's "first time you Down to
    // a node whose status isn't in the cache" report.
    for i in 0..60 {
        let t = Instant::now();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let key_elapsed = t.elapsed();

        // Single (likely single-core) sandbox: the worker thread never
        // gets scheduled unless the main thread actually yields, unlike
        // a real interactive session where `rx.recv_timeout` blocks
        // between keystrokes. Sleep briefly so the worker can actually
        // process the requests this render just queued.
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Drain every `HeatWorkerProgress` the worker has posted so
        // far, same as `run_loop`'s `rx.recv()` match arm — each one
        // triggers `recheck_pending_heat_states`.
        let mut progress_events = 0;
        let t_progress = Instant::now();
        if let Some(rx) = &rx {
            while rx.try_recv().is_ok() {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
                progress_events += 1;
            }
        }
        let progress_elapsed = t_progress.elapsed();

        let td = Instant::now();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let draw_elapsed = td.elapsed();
        eprintln!(
            "Down #{i}: key={key_elapsed:?} progress={progress_elapsed:?} ({progress_events} events) draw={draw_elapsed:?}"
        );
    }
}

/// Repro for the 2026-07-25 crash report: `Down` (into the first
/// `FileDescriptorProto`), then `t`, then `Enter` (confirming the
/// first candidate) -- panicked with "slice index starts at 39 but
/// ends at 35" in `materialize_line_patches`.
#[test]
#[ignore]
fn repro_crash_down_t_enter_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!("total tree nodes: {}", app.tree.len());

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let mut rx = None;
    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx.clone(),
        ));
        rx = Some(worker_rx);

        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    eprintln!(
        "cursor after Down: {} type_fqdn={:?}",
        app.cursor, app.tree[app.cursor].span.type_fqdn
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after 't': override_target={:?} sort={:?} pending={} message={}",
        app.override_target, app.override_sort, app.override_candidates_pending, app.message
    );
    let t_poll = Instant::now();
    while app.override_candidates_pending && t_poll.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_pending_override_work();
        if let Some(rx) = &rx {
            while rx.try_recv().is_ok() {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
            }
        }
    }
    eprintln!("candidates.len() = {}", app.override_candidates.len());
    if app.override_candidates.is_empty() {
        eprintln!("Inferred mode gave 0 candidates -- switching to Lexicographic");
        app.override_sort = SortMode::Lexicographic;
        app.recompute_override_candidates();
        eprintln!(
            "Lexicographic candidates.len() = {}",
            app.override_candidates.len()
        );
        // Pick the first real message FQDN (skip the `None` sentinel and
        // primitive keywords), same convention as override_select.rs's
        // own tests -- a guaranteed real retype, not a no-op.
        let chosen = "google.protobuf.FileDescriptorProto".to_string();
        let row = app
            .override_candidates
            .iter()
            .position(|(f, _)| *f == chosen)
            .expect("chosen FQDN must be a candidate");
        app.override_highlight = row;
        eprintln!("chosen candidate (same-as-current, no-op retype): {chosen} at row {row}");
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!("about to press Enter -- tree.len()={}", app.tree.len());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!(
        "after Enter: tree.len()={} manage_open={} message={}",
        app.tree.len(),
        app.manage_open,
        app.message
    );
}

/// Repro attempt #2 for the 2026-07-25 crash report, per the user's
/// more precise follow-up: `Down` (into the first `FileDescriptorProto`
/// -- root having *already* been rendered twice by this point: once as
/// `None`/raw at `App::new`, then re-rendered as `FileDescriptorSet`
/// once `apply_resolved_root_type` lands), then immediately `t` then
/// `Enter` -- confirming the pane's *default* initial highlight with no
/// waiting for candidates to settle and no candidate navigation, unlike
/// `repro_crash_down_t_enter_on_pdb` above (which polled up to 120s for
/// candidates and picked a specific no-op candidate on empty-Inferred
/// fallback). Mirrors real fast keypress timing far more closely.
#[test]
#[ignore]
fn repro_crash_down_t_enter_immediate_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;
    eprintln!(
        "root rendered_as after App::new: {:?}",
        app.tree[app.first_node].rendered_as
    );

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, _worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx,
        ));

        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }
    eprintln!(
        "root rendered_as after apply_resolved_root_type: {:?}",
        app.tree[app.first_node].rendered_as
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    eprintln!("cursor after Down: {}", app.cursor);
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after 't': override_target={:?} sort={:?} highlight={} candidates.len()={} pending={}",
        app.override_target,
        app.override_sort,
        app.override_highlight,
        app.override_candidates.len(),
        app.override_candidates_pending
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!(
        "about to press Enter (no wait) -- tree.len()={}",
        app.tree.len()
    );
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!(
        "after Enter: tree.len()={} manage_open={} message={}",
        app.tree.len(),
        app.manage_open,
        app.message
    );
}

/// Surgical repro attempt #3, prompted by the user's report that the
/// crash happens even with a long wait between `t` and `Enter` --
/// ruling out a timing/race explanation entirely. This bypasses the
/// interactive pane/candidate machinery altogether and reproduces
/// exactly the *structural* effect `Enter` has on the cursor node:
/// `preview_override_highlight` (fired at pane-open time, `t`) force-
/// resets `rendered_as = None` on the cursor node after its own
/// preview splice (see its doc comment) precisely so a later confirm
/// always re-splices for real -- so here we do that reset directly,
/// then call `render_overrides` from the document root exactly like
/// the real Enter-confirm path (`key_dispatch.rs`) does, with no pane
/// state involved at all.
#[test]
#[ignore]
fn repro_crash_forced_resplice_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }
    eprintln!(
        "root rendered_as after apply_resolved_root_type: {:?}",
        app.tree[app.first_node].rendered_as
    );

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cursor = app.cursor;
    eprintln!(
        "cursor after Down: {} rendered_as={:?}",
        cursor, app.tree[cursor].rendered_as
    );

    // Mimic exactly what `preview_override_highlight` does after its own
    // (budget-capped) preview splice at pane-open time: force `rendered_as`
    // back to `None` so the node is treated as needing a real re-splice.
    app.tree[cursor].rendered_as = None;

    eprintln!(
        "about to force-resplice from root -- tree.len()={}",
        app.tree.len()
    );
    app.render_overrides(app.first_node);
    eprintln!(
        "after forced resplice: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );
}

/// Repro attempt #4: replays the *exact* two-batch sequence the real
/// `Enter`-confirm key handler performs (`key_dispatch.rs`'s
/// `handle_override_key`'s `KeyCode::Enter` arm), skipping only the
/// pane/candidate-list bookkeeping that attempt #3 already showed is
/// irrelevant to the crash. The critical difference from attempt #3:
/// (1) an explicit override entry is *activated* (`overrides.activate`)
/// before the first `render_overrides(self.first_node)` call, and (2)
/// a *second*, independent batch -- `render_overrides(idx)` scoped to
/// the cursor node itself, not the root -- immediately follows, exactly
/// as `close_override` does right after every real Enter-confirm.
#[test]
#[ignore]
fn repro_crash_activate_then_close_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cursor = app.cursor;
    let own_fqdn = app.tree[cursor].span.type_fqdn.clone();
    eprintln!(
        "cursor after Down: {} type_fqdn={:?} rendered_as={:?}",
        cursor, own_fqdn, app.tree[cursor].rendered_as
    );

    // Mimic `toggle_override`'s preview splice + forced rendered_as reset
    // (attempt #3 already showed this alone doesn't crash, but keep it
    // for fidelity -- it's what real `t` always does before Enter).
    app.tree[cursor].rendered_as = None;

    // Mimic `handle_override_key`'s `Enter` arm exactly.
    let origin = app
        .override_origin_for_kind(cursor)
        .expect("origin for cursor");
    app.overrides.activate(origin.clone(), own_fqdn.clone());
    eprintln!(
        "about to batch#1 render_overrides(first_node) -- tree.len()={}",
        app.tree.len()
    );
    app.render_overrides(app.first_node);
    eprintln!(
        "after batch#1: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );

    // Mimic `close_override`'s own follow-up call, scoped to `cursor`
    // (not the root) -- this is the second, independent batch.
    eprintln!("about to batch#2 render_overrides(cursor={cursor})");
    app.render_overrides(cursor);
    eprintln!(
        "after batch#2: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );
}

/// Repro attempt #4.5: attempt #4 (activate+render_overrides, no real
/// `t` keypress) did *not* crash; attempt #5 (real `t` then a long
/// candidate-settling wait then real `Enter`) *did* crash, picking the
/// exact same target type as #4. The only structural difference: #5's
/// real `t` keypress goes through `toggle_override` -> `preview_
/// override_highlight`, which does an immediate, budget-capped (spec
/// 0163, default 200 spans) preview splice of the cursor's ~600k-node
/// subtree -- something #4 never did. This test isolates exactly that:
/// a real `t` keypress (no waiting at all) immediately followed by the
/// same manual activate+render_overrides(first_node)+render_overrides
/// (cursor) sequence #4 used, skipping the pane/candidate machinery
/// entirely otherwise.
#[test]
#[ignore]
fn repro_crash_real_t_then_manual_confirm_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cursor = app.cursor;
    let own_fqdn = app.tree[cursor].span.type_fqdn.clone();
    eprintln!("cursor after Down: {cursor} type_fqdn={own_fqdn:?}");

    // Real `t` keypress -- no waiting, no candidate polling.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after real 't': override_target={:?} rendered_as={:?} tree.len()={}",
        app.override_target,
        app.tree[cursor].rendered_as,
        app.tree.len()
    );

    // Mimic `handle_override_key`'s `Enter` arm's core effect manually
    // (bypassing the candidate list -- already shown irrelevant).
    let origin = app
        .override_origin_for_kind(cursor)
        .expect("origin for cursor");
    app.overrides.activate(origin.clone(), own_fqdn.clone());
    eprintln!(
        "about to batch#1 render_overrides(first_node) -- tree.len()={}",
        app.tree.len()
    );
    app.render_overrides(app.first_node);
    eprintln!(
        "after batch#1: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );

    eprintln!("about to batch#2 render_overrides(cursor={cursor})");
    app.render_overrides(cursor);
    eprintln!(
        "after batch#2: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );
}

/// Repro attempt #4.75: #4.5 (one real `t` keypress, i.e. exactly one
/// preview splice of the cursor node, then manual confirm) did *not*
/// crash. The real interactive session's wait loop calls `poll_
/// pending_override_work` repeatedly, which -- once `override_seek_
/// target` finally resolves -- calls `preview_override_highlight()` a
/// *second* time on the very same node (`override_select.rs`'s own
/// `poll_pending_override_work`, the `seek_override_highlight` success
/// branch). This test isolates that: two successive preview splices of
/// the same huge subtree (mimicking what the pane-open seek + the
/// eventual poll-driven re-seek both do), then the same manual confirm
/// sequence as #4/#4.5.
#[test]
#[ignore]
fn repro_crash_two_previews_then_manual_confirm_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cursor = app.cursor;
    let own_fqdn = app.tree[cursor].span.type_fqdn.clone();
    eprintln!("cursor after Down: {cursor} type_fqdn={own_fqdn:?}");

    // Real `t` keypress -- exactly one preview splice.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after real 't' (preview #1): override_target={:?} rendered_as={:?} tree.len()={}",
        app.override_target,
        app.tree[cursor].rendered_as,
        app.tree.len()
    );

    // A second preview splice of the *same* node -- mimicking `poll_
    // pending_override_work`'s own seek-retry-success call.
    app.preview_override_highlight();
    eprintln!(
        "after preview #2: rendered_as={:?} tree.len()={}",
        app.tree[cursor].rendered_as,
        app.tree.len()
    );

    // Manual confirm (bypassing the candidate list -- already shown
    // irrelevant to which type gets picked here).
    let origin = app
        .override_origin_for_kind(cursor)
        .expect("origin for cursor");
    app.overrides.activate(origin.clone(), own_fqdn.clone());
    eprintln!(
        "about to batch#1 render_overrides(first_node) -- tree.len()={}",
        app.tree.len()
    );
    app.render_overrides(app.first_node);
    eprintln!(
        "after batch#1: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );

    eprintln!("about to batch#2 render_overrides(cursor={cursor})");
    app.render_overrides(cursor);
    eprintln!(
        "after batch#2: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );
}

/// Repro attempt #4.9: instrumenting #5 revealed the real wait-loop's
/// splice events are *tiny* (a few nodes, not the whole subtree) --
/// because at `t` time `override_candidates` is still empty (cold
/// cache), so `preview_override_highlight`'s `tentative` is `None`:
/// the very *first* preview is of the node as **raw** (untyped), not
/// as its own current type! Only once real candidates arrive does a
/// *second* preview (via `poll_pending_override_work`'s seek-retry)
/// switch it to the real type -- a raw-preview-then-typed-preview
/// transition #4.75 never actually exercised (it called `preview_
/// override_highlight()` twice back-to-back with an empty candidate
/// list both times, i.e. raw-then-raw). This test isolates exactly
/// that transition: real `t` (raw preview), then manually seed
/// `override_candidates` with the real type and preview again (typed),
/// then the same manual confirm sequence.
#[test]
#[ignore]
fn repro_crash_raw_then_typed_preview_then_manual_confirm_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let cursor = app.cursor;
    let own_fqdn = app.tree[cursor].span.type_fqdn.clone();
    eprintln!("cursor after Down: {cursor} type_fqdn={own_fqdn:?}");
    let baseline_len = app.tree.len();

    // Real `t` keypress -- with a cold (empty) candidate cache, this
    // previews the node as *raw*.
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after real 't' (raw preview): candidates.len()={} tree.len()={} (delta {})",
        app.override_candidates.len(),
        app.tree.len(),
        app.tree.len() as isize - baseline_len as isize
    );

    // Manually seed the candidate list with the node's real type at
    // row 0 (mimicking what the heat worker eventually provides) and
    // re-preview -- the raw -> typed transition.
    app.override_candidates = vec![(own_fqdn.clone().unwrap(), Some(1))];
    app.override_highlight = 0;
    app.preview_override_highlight();
    eprintln!(
        "after typed preview: tree.len()={} (delta {}) message={}",
        app.tree.len(),
        app.tree.len() as isize - baseline_len as isize,
        app.message
    );

    // Manual confirm (bypassing the candidate list dispatch itself --
    // already shown irrelevant to which type gets picked here).
    let origin = app
        .override_origin_for_kind(cursor)
        .expect("origin for cursor");
    app.overrides.activate(origin.clone(), own_fqdn.clone());
    eprintln!(
        "about to batch#1 render_overrides(first_node) -- tree.len()={}",
        app.tree.len()
    );
    app.render_overrides(app.first_node);
    eprintln!(
        "after batch#1: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );

    eprintln!("about to batch#2 render_overrides(cursor={cursor})");
    app.render_overrides(cursor);
    eprintln!(
        "after batch#2: tree.len()={} message={}",
        app.tree.len(),
        app.message
    );
}

/// Repro attempt #5: unlike every earlier attempt, does *not* force the
/// override pane's highlight onto the cursor node's own current type --
/// waits (like `repro_crash_down_t_enter_on_pdb`) for the background
/// heat worker to settle, then presses Enter on *whatever* candidate
/// naturally ends up highlighted (real `open_override_on_type`'s own
/// seek/fallback logic, untouched), even if that's a genuinely
/// different type than the node's current one -- through the real
/// `app.handle_key` dispatch path throughout, exactly as a human would
/// experience it. Prints the actually-confirmed candidate FQDN either
/// way, so a crash or a clean pass are both informative.
#[test]
#[ignore]
fn repro_crash_natural_highlight_on_pdb() {
    let desc_path = Path::new("/tmp/pdb.desc");
    if !desc_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(desc_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    let mut app = App::new(
        decoded,
        "pdb.desc",
        std::path::PathBuf::from(desc_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let mut rx = None;
    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob,
            tx.clone(),
        ));
        rx = Some(worker_rx);

        let original_blob = app.blob[app.wrapper_offset..].to_vec();
        if let Some(fqdn) = decode::resolve_root_winner_fqdn(&original_blob, graph_ref.graph()) {
            eprintln!("resolved root type: {fqdn}");
            app.apply_resolved_root_type(fqdn);
        }
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    eprintln!(
        "cursor after Down: {} type_fqdn={:?}",
        app.cursor, app.tree[app.cursor].span.type_fqdn
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!(
        "right after 't': sort={:?} highlight={} pending={} seek_target={:?}",
        app.override_sort,
        app.override_highlight,
        app.override_candidates_pending,
        app.override_seek_target
    );
    let t_poll = Instant::now();
    let mut last_len = app.tree.len();
    let mut splice_events = 0usize;
    while (app.override_candidates_pending
        || app.override_complete_pending
        || app.override_seek_target.is_some())
        && t_poll.elapsed().as_secs() < 120
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.poll_pending_override_work();
        if app.tree.len() != last_len {
            splice_events += 1;
            eprintln!(
                "  [poll] tree.len() changed: {} -> {} (event #{splice_events}, highlight={}, candidates.len()={})",
                last_len,
                app.tree.len(),
                app.override_highlight,
                app.override_candidates.len()
            );
            last_len = app.tree.len();
        }
        if let Some(rx) = &rx {
            while rx.try_recv().is_ok() {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
                if app.tree.len() != last_len {
                    splice_events += 1;
                    eprintln!(
                        "  [drain] tree.len() changed: {} -> {} (event #{splice_events}, highlight={}, candidates.len()={})",
                        last_len,
                        app.tree.len(),
                        app.override_highlight,
                        app.override_candidates.len()
                    );
                    last_len = app.tree.len();
                }
            }
        }
    }
    eprintln!("total splice events during wait loop: {splice_events}");
    eprintln!(
        "settled: sort={:?} highlight={} candidates.len()={} seek_target={:?}",
        app.override_sort,
        app.override_highlight,
        app.override_candidates.len(),
        app.override_seek_target
    );
    if let Some((fqdn, _)) = app.override_candidates.get(app.override_highlight) {
        eprintln!("about to confirm candidate: {fqdn}");
    } else {
        eprintln!("no candidate at highlight row {} -- Enter will no-op with a message, not crash; skipping", app.override_highlight);
        return;
    }
    terminal.draw(|frame| app.render(frame)).unwrap();

    eprintln!("about to press Enter -- tree.len()={}", app.tree.len());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    eprintln!(
        "after Enter: tree.len()={} manage_open={} message={}",
        app.tree.len(),
        app.manage_open,
        app.message
    );
}

/// Throwaway diagnostic for the 2026-07-25 report: even on the tiny
/// `/tmp/sim` fixture (459 bytes), `Down` then `t` reportedly stalls
/// forever at "Scoring candidates…", pegging one CPU core. Bounds the
/// settle-poll loop tightly (5s) and counts real `inferred_candidates`/
/// `score_all` calls via the worker's own test counter to distinguish
/// "genuinely slow" from "never terminates, repeatedly redoing the same
/// work".
#[test]
#[ignore]
fn diagnose_sim_t_stall() {
    let desc_path = Path::new("/tmp/pdb.desc");
    let blob_path = Path::new("/tmp/sim");
    if !desc_path.exists() || !blob_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc or /tmp/sim not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(blob_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");
    eprintln!("total tree nodes: {}", decoded.tree.len());

    let mut app = App::new(
        decoded,
        "sim",
        std::path::PathBuf::from(blob_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob_arc = std::sync::Arc::new(app.blob.clone());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob_arc,
            tx,
        ));
    }
    eprintln!("scoring graph present: {}", app.ctx.graph.is_some());

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    // 2026-07-25 report: `t` *at the default cursor position*, no
    // navigation first, reportedly loops forever.
    eprintln!(
        "default cursor: {} (is_message={}, field_number={})",
        app.cursor, app.tree[app.cursor].span.is_message, app.tree[app.cursor].span.field_number
    );

    let t0 = Instant::now();
    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    eprintln!("'t' (open override pane): {:?}", t0.elapsed());
    terminal.draw(|frame| app.render(frame)).unwrap();
    eprintln!(
        "right after open: pending={} complete_pending={} candidates.len()={} message={}",
        app.override_candidates_pending,
        app.override_complete_pending,
        app.override_candidates.len(),
        app.message
    );

    let t_poll = Instant::now();
    let mut polls = 0;
    while (app.override_candidates_pending || app.override_complete_pending)
        && t_poll.elapsed().as_secs() < 5
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_pending_override_work();
        polls += 1;
    }
    eprintln!(
        "settle-poll: {:?} ({polls} polls), pending={} complete_pending={} \
         candidates.len()={} complete={} score_all_calls={}",
        t_poll.elapsed(),
        app.override_candidates_pending,
        app.override_complete_pending,
        app.override_candidates.len(),
        app.override_candidates_complete,
        app.heat_worker.as_ref().map_or(0, |w| w.score_all_calls()),
    );
}

/// Throwaway diagnostic for the 2026-07-25 report: on `/tmp/sim`,
/// navigate to node `/3`, press `t`, then `Down` *inside the override
/// pane* (moving the highlight, triggering a live preview) — reportedly
/// stalls at 100% CPU. Bounds every poll loop tightly so the test itself
/// can never hang forever even if the underlying bug does.
#[test]
#[ignore]
fn diagnose_sim_node3_t_down_stall() {
    let desc_path = Path::new("/tmp/pdb.desc");
    let blob_path = Path::new("/tmp/sim");
    if !desc_path.exists() || !blob_path.exists() {
        eprintln!("skipping: /tmp/pdb.desc or /tmp/sim not present");
        return;
    }

    let mut ctx = DescriptorContext::load(desc_path).expect("load descriptor set");
    let blob = std::fs::read(blob_path).expect("read blob");
    let decoded = decode(&blob, &mut ctx, None, 2, true).expect("decode");

    let mut app = App::new(
        decoded,
        "sim",
        std::path::PathBuf::from(blob_path),
        2,
        ctx,
        ThemeKind::Dark,
        None,
    );
    app.splash = false;
    app.term_width = 120;

    if let Some(graph) = &app.ctx.graph {
        let graph_ref = std::sync::Arc::clone(graph);
        let blob_arc = std::sync::Arc::new(app.blob.clone());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            std::sync::Arc::clone(&graph_ref),
            blob_arc,
            tx,
        ));
    }

    let backend = ratatui::backend::TestBackend::new(120, 50);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();

    let idx3 = app.resolve_path("/3").expect("node /3 must exist");
    app.set_cursor(idx3);
    eprintln!(
        "node /3: idx={idx3} is_message={} field_number={}",
        app.tree[idx3].span.is_message, app.tree[idx3].span.field_number
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t_open = Instant::now();
    let mut open_polls = 0;
    while (app.override_candidates_pending || app.override_complete_pending)
        && t_open.elapsed().as_secs() < 5
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_pending_override_work();
        open_polls += 1;
    }
    eprintln!(
        "settle after open: {:?} ({open_polls} polls), pending={} complete_pending={} \
         candidates.len()={} complete={} score_all_calls={}",
        t_open.elapsed(),
        app.override_candidates_pending,
        app.override_complete_pending,
        app.override_candidates.len(),
        app.override_candidates_complete,
        app.heat_worker.as_ref().map_or(0, |w| w.score_all_calls()),
    );

    eprintln!(
        "candidate[0]={:?} candidate[1]={:?} candidate[2]={:?}",
        app.override_candidates.first(),
        app.override_candidates.get(1),
        app.override_candidates.get(2),
    );
    eprintln!("highlight before Down: {}", app.override_highlight);

    let t_down = Instant::now();
    app.move_override_highlight(1);
    eprintln!(
        "move_override_highlight(1) (direct call, bypassing handle_key): {:?}, tree.len()={}, highlight={}",
        t_down.elapsed(),
        app.tree.len(),
        app.override_highlight,
    );
    terminal.draw(|frame| app.render(frame)).unwrap();

    let t_poll = Instant::now();
    let mut polls = 0;
    while (app.override_candidates_pending || app.override_complete_pending)
        && t_poll.elapsed().as_secs() < 5
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
        app.poll_pending_override_work();
        polls += 1;
    }
    eprintln!(
        "settle after Down: {:?} ({polls} polls), pending={} complete_pending={} \
         candidates.len()={} complete={} score_all_calls={}",
        t_poll.elapsed(),
        app.override_candidates_pending,
        app.override_complete_pending,
        app.override_candidates.len(),
        app.override_candidates_complete,
        app.heat_worker.as_ref().map_or(0, |w| w.score_all_calls()),
    );
}
