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
        let graph_ref = graph.graph;
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            graph_ref,
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
        let graph_ref = graph.graph;
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            graph_ref,
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
        let graph_ref = graph.graph;
        let blob = std::sync::Arc::new(app.blob.clone());
        let (tx, worker_rx) = std::sync::mpsc::channel();
        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            std::sync::Arc::clone(&app.heat_caches),
            graph_ref,
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
