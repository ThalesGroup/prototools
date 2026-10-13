// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use nix::sys::signal::{killpg, signal, SigHandler, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{getpgrp, tcsetpgrp, Pid};
use prototext_core::schema::descriptor::SourceCodeInfo;
use prototext_core::schema::Pool;
use ratatui::backend::Backend;
use ratatui::Terminal;

use super::terminal::{enable_raw_mode_and_reenter, restore_terminal};
use super::App;

/// A fully-resolved `v` target: an absolute `.proto` path plus a
/// 1-indexed line/column, ready to hand to Neovim's `cursor()`.
pub(crate) struct EditorRequest {
    pub(crate) path: PathBuf,
    pub(crate) line: u32,
    pub(crate) col: u32,
}

/// Resolve `fqdn` (a message or enum) to the `.proto` file it's declared
/// in — relative to the descriptor set's compile root, e.g.
/// `path/to/my_package.proto`, which is `FileDescriptor::name()`'s
/// documented format — and its 1-indexed declaration line/column, by
/// matching its `declaration_path()` against a linear scan of
/// `source_code_info.location`. upb keeps no `source_code_info`; it is
/// read from the file's own bytes, which the pool keeps (spec 0417 S7).
///
/// `None` only when `fqdn` resolves to no message or enum in `pool` at
/// all (G3's "unknown type" case; G2 already screens out primitives,
/// sentinels and the internal MessageSet-item FQDN). Falls back to line
/// 1, column 1 — never `None` — when the type resolves but the file's
/// `SourceCodeInfo` has no matching `Location`, e.g. a descriptor set
/// compiled without `--include_source_info`, or when the matching
/// `Location`'s `span` is too short to read a line and column from.
/// `Location.span` is 0-indexed `[start_line, start_col, end_line]` or
/// `[start_line, start_col, end_line, end_col]` (standard protoc
/// convention).
pub(crate) fn locate_declaration(pool: &Pool, fqdn: &str) -> Option<(PathBuf, u32, u32)> {
    let (file, path) = if let Some(m) = pool.get_message_by_name(fqdn) {
        (m.parent_file(), m.declaration_path())
    } else if let Some(e) = pool.get_enum_by_name(fqdn) {
        (e.parent_file(), e.declaration_path())
    } else {
        return None;
    };
    // A file whose `source_code_info` does not decode is treated as one
    // without it: the fallback below, not an error.
    let info = pool
        .file_bytes(file.name())
        .and_then(|bytes| SourceCodeInfo::of_file(&bytes).ok().flatten());
    let (line, col) = info
        .as_ref()
        .and_then(|sci| sci.location.iter().find(|loc| loc.path == path))
        // The three- or four-element shape documented above is protoc's
        // convention, not a guarantee of the wire format: `span` is a
        // plain repeated field, and a descriptor set from any other
        // producer may carry a shorter one. Falling back to the head of
        // the file, exactly as a missing `Location` already does, beats
        // panicking with the terminal in raw mode.
        .and_then(|loc| match loc.span[..] {
            [line, col, ..] => Some((line as u32 + 1, col as u32 + 1)),
            _ => None,
        })
        .unwrap_or((1, 1));
    Some((PathBuf::from(file.name()), line, col))
}

/// `v`'s Neovim handoff state (G5) — stored on `App` (`editor_state`,
/// `mod.rs`) rather than a process-global `static`: `main.rs`
/// constructs exactly one `App` for the process's entire lifetime, and
/// this is a single-threaded event loop, so there is no concurrency to
/// guard against and no risk of losing track of the handoff across
/// `App` instances (see the Goals section G5 "State" note).
pub(crate) enum EditorState {
    NotRunning,
    Suspended { pgid: Pid, socket_path: PathBuf },
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("protolens-nvim-{}.sock", std::process::id()))
}

// A `thread_local`, not a `static`: `cargo test` runs tests in parallel
// threads of one process, and the spawn happens on the same thread as
// the test that set this, so a thread-local override cannot be observed
// by any other test and needs no lock.
#[cfg(test)]
thread_local! {
    pub(super) static EDITOR_PROGRAM: std::cell::Cell<&'static str> =
        const { std::cell::Cell::new("nvim") };
}

/// The editor binary `open_editor` spawns — `"nvim"`, except that a
/// test may point it elsewhere via `EDITOR_PROGRAM`.
///
/// The override exists because the branch worth a regression test is
/// the one where the spawn *fails*: that is where the 2026-07-18 crash
/// lived. Reaching it means naming a binary that is not on `PATH`, and
/// the test used to arrange that by probing for a real `nvim` and
/// returning early if it found one — so on any machine with Neovim
/// installed, which is most of them, the test skipped itself and
/// reported a pass. Overriding the name instead exercises the branch
/// everywhere.
fn editor_program() -> &'static str {
    #[cfg(test)]
    {
        EDITOR_PROGRAM.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        "nvim"
    }
}

/// Pure `WaitStatus -> EditorState` transition (G7): a `Stopped` result
/// re-arms `Suspended` with the same `pgid`/`socket_path` (nothing else
/// distinguishes one stop from the next); any other result — `Exited`,
/// `Signaled`, or otherwise — collapses to `NotRunning`. `pgid` is
/// passed in rather than read from `status` because `waitpid` is
/// called directly on the leader `pgid` (see `open_editor` below, not
/// the negated group-wide form), so the caller already knows it, and a
/// `Stopped(pid, _)` result's `pid` is always that same `pgid` by
/// construction — no reconciliation needed.
///
/// Kept pure so it is unit-testable against real, publicly
/// constructible `WaitStatus`/`Pid`/`Signal` values, with no mocking.
fn next_state(pgid: Pid, socket_path: PathBuf, status: WaitStatus) -> EditorState {
    match status {
        WaitStatus::Stopped(..) => EditorState::Suspended { pgid, socket_path },
        _ => EditorState::NotRunning,
    }
}

/// `v`'s Neovim handoff (G5), called by `run_loop` right after the
/// `handle_key` call that armed `app.pending_editor_open`. Mirrors
/// `suspend`'s own shape — leave the terminal exactly as a normal exit
/// would, hand off control, restore raw/alt-screen/mouse-capture on the
/// way back (spec 0113 D31) — generalized to two cases: no Neovim
/// running yet (fork/exec a fresh instance via `--listen`) or one
/// already `Suspended` from an earlier `v` press (send it `--remote-
/// tab`/`--remote-send` requests over its existing `--listen` socket
/// instead of spawning a second instance, then `SIGCONT` it) — G5's
/// "single instance" decision.
///
/// The `--remote-tab`/`--remote-send` flags and the `<C-\><C-N>`
/// mode-escape prefix are verified against Neovim 0.11.5: a combined
/// single-call `--remote-tab +cmd` does *not* apply `+cmd` to the newly
/// opened tab, so two separate calls are required, as below.
pub(crate) fn open_editor<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    req: EditorRequest,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    restore_terminal();

    // Reclaiming the terminal below (`tcsetpgrp(io::stdin(), getpgrp())`,
    // after `waitpid` returns) happens while protolens is itself a
    // background process group relative to the terminal — Neovim's pgid
    // is still the foreground one at that point. The default SIGTTOU
    // disposition would stop protolens right there, same as any other
    // background process attempting a terminal-control operation,
    // leaving the user stuck in the invoking shell until a manual `fg`.
    // Ignoring SIGTTOU for the process's lifetime — the same policy an
    // interactive shell applies to itself — lets the reclaim go through.
    // SAFETY: installing a signal handler for our own process is always
    // sound; `SigHandler::SigIgn` takes no captured state.
    unsafe {
        let _ = signal(Signal::SIGTTOU, SigHandler::SigIgn);
    }

    let (pgid, socket_path, resuming) = match &app.editor_state {
        EditorState::NotRunning => {
            let socket_path = socket_path();
            let goto = format!("+call cursor({},{})", req.line, req.col);
            // spec 0145 G6: load the bundled Lua config (`.proto`
            // filetype/syntax + `buf lsp serve` navigation) when packaged
            // via Nix; falls back to the user's own Neovim config in a
            // non-Nix dev build (no error).
            //
            // A non-Nix dev build (`cargo run`/`cargo build`) has no
            // wrapper to set PROTOLENS_NVIM_CONFIG, so fall back here to
            // the repo-relative config baked in at compile time via
            // CARGO_MANIFEST_DIR — only when unset (never overrides the
            // Nix wrapper's own value) and only when that path actually
            // exists (preserves the graceful degraded fallback if the
            // binary is copied away from its source checkout).
            //
            // A local, not `std::env::set_var`: the heat worker is alive
            // at this point (the Neovim handoff shuts down the *input
            // reader*, nothing else), and mutating the process
            // environment while another thread may be reading it is
            // undefined behavior. Nothing downstream reads the variable
            // anyway — it only ever fed the `-u` flag below.
            let config = std::env::var_os("PROTOLENS_NVIM_CONFIG").or_else(|| {
                let default_config = concat!(env!("CARGO_MANIFEST_DIR"), "/nvim/init.lua");
                std::path::Path::new(default_config)
                    .exists()
                    .then(|| default_config.into())
            });
            let mut cmd = Command::new(editor_program());
            if let Some(config) = config {
                cmd.arg("-u").arg(config);
            }
            if let Some(proto_root) = &app.proto_root {
                cmd.env("PROTOTEXT_PROTO_ROOT", proto_root);
            }
            let child = cmd
                .arg("--listen")
                .arg(&socket_path)
                .arg(goto)
                .arg(&req.path)
                .process_group(0) // new pgid, equal to the child's own pid
                .spawn();
            let child = match child {
                Ok(child) => child,
                Err(e) => {
                    // Not installed is the common case, and the one the
                    // user can act on (spec 0410 S2): name the remedy.
                    app.message = if e.kind() == io::ErrorKind::NotFound {
                        "nvim not found on PATH: install Neovim to use the editor key".into()
                    } else {
                        format!("cannot launch nvim: {e}")
                    };
                    enable_raw_mode_and_reenter(terminal)?;
                    return Ok(());
                }
            };
            let pgid = Pid::from_raw(child.id() as i32);
            (pgid, socket_path, false)
        }
        EditorState::Suspended { pgid, socket_path } => {
            let goto = format!("cursor({},{})", req.line, req.col);
            let _ = Command::new("nvim")
                .arg("--server")
                .arg(socket_path)
                .arg("--remote-tab")
                .arg(&req.path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = Command::new("nvim")
                .arg("--server")
                .arg(socket_path)
                .arg("--remote-send")
                .arg(format!("<C-\\><C-N>:call {goto}<CR>"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            (*pgid, socket_path.clone(), true)
        }
    };

    tcsetpgrp(io::stdin(), pgid).ok();
    if resuming {
        if let Err(e) = killpg(pgid, Signal::SIGCONT) {
            // The suspended instance is gone, or out of reach. Take the
            // terminal back — it was handed to that process group one
            // line ago — and forget the instance, so the next `v`
            // spawns a fresh one instead of signalling the same corpse
            // again. Propagating with `?` would unwind out of the event
            // loop with the terminal cooked, off the alternate screen,
            // and someone else's process group in the foreground: from
            // the user's side, a shell that no longer responds.
            tcsetpgrp(io::stdin(), getpgrp()).ok();
            let _ = std::fs::remove_file(&socket_path);
            app.editor_state = EditorState::NotRunning;
            app.message = format!("cannot resume nvim: {e}");
            enable_raw_mode_and_reenter(terminal)?;
            return Ok(());
        }
    }

    let status = waitpid(pgid, Some(WaitPidFlag::WUNTRACED));
    // Unconditional, and before the result is examined: a failed
    // `waitpid` hands the terminal back no more than a successful one
    // does, and the same unwind-with-a-stolen-terminal applies.
    tcsetpgrp(io::stdin(), getpgrp()).ok();
    let status = match status {
        Ok(status) => status,
        Err(e) => {
            app.message = format!("cannot wait for nvim: {e}");
            enable_raw_mode_and_reenter(terminal)?;
            return Ok(());
        }
    };
    let next = next_state(pgid, socket_path.clone(), status);
    if matches!(next, EditorState::NotRunning) {
        let _ = std::fs::remove_file(&socket_path);
        if !matches!(status, WaitStatus::Exited(..) | WaitStatus::Signaled(..)) {
            // No dedicated logging mechanism exists in protolens (no
            // `log`/`tracing` dependency; `eprintln!` is used only
            // pre-TUI in main.rs) — an unexpected `waitpid` result
            // routes through the same auto-dismissing `app.message`
            // notice every other failure in this spec already uses.
            app.message = format!("nvim exited unexpectedly: {status:?}");
        }
    }
    app.editor_state = next;

    enable_raw_mode_and_reenter(terminal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use prototext_core::schema::descriptor::source_code_info::Location;
    use prototext_core::schema::descriptor::{
        DescriptorProto, Encode as _, EnumDescriptorProto, EnumValueDescriptorProto,
        FileDescriptorProto,
    };

    /// A file declaring `pkg.Outer`, `pkg.Outer.Inner` and
    /// `pkg.Outer.Inner.Kind`, with a `source_code_info` that places each
    /// at its own line and column, as protoc writes it: field 9 of the
    /// file, which the descriptor structures do not carry.
    fn pool_with_source_info() -> Pool {
        let kind = EnumDescriptorProto {
            name: Some("Kind".into()),
            value: vec![EnumValueDescriptorProto {
                name: Some("KIND_ZERO".into()),
                number: Some(0),
            }],
        };
        let file = FileDescriptorProto {
            name: Some("pkg/outer.proto".into()),
            package: Some("pkg".into()),
            syntax: Some("proto2".into()),
            message_type: vec![DescriptorProto {
                name: Some("Outer".into()),
                nested_type: vec![DescriptorProto {
                    name: Some("Inner".into()),
                    enum_type: vec![kind],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let location = |path: Vec<i32>, span: Vec<i32>| Location { path, span };
        let info = SourceCodeInfo {
            location: vec![
                location(vec![4, 0], vec![4, 0, 12, 1]),
                location(vec![4, 0, 3, 0], vec![6, 2, 10, 3]),
                location(vec![4, 0, 3, 0, 4, 0], vec![7, 4, 9, 5]),
            ],
        };
        let mut bytes = file.encode_to_vec();
        let info = info.encode_to_vec();
        bytes.push(9 << 3 | 2);
        bytes.push(u8::try_from(info.len()).unwrap());
        bytes.extend_from_slice(&info);
        let pool = prototext_core::new_pool();
        pool.add_file(&bytes).unwrap();
        pool
    }

    /// Spec 0417 S7: the line and column come from the file's own
    /// `source_code_info`, one-based, for a top-level message, a nested
    /// one and a nested enum.
    #[test]
    fn locate_declaration_reads_the_files_source_code_info() {
        let pool = pool_with_source_info();
        let at = |fqdn: &str| locate_declaration(&pool, fqdn).unwrap();
        assert_eq!(at("pkg.Outer"), (PathBuf::from("pkg/outer.proto"), 5, 1));
        assert_eq!(
            at("pkg.Outer.Inner"),
            (PathBuf::from("pkg/outer.proto"), 7, 3)
        );
        assert_eq!(
            at("pkg.Outer.Inner.Kind"),
            (PathBuf::from("pkg/outer.proto"), 8, 5)
        );
        assert_eq!(locate_declaration(&pool, "pkg.Missing"), None);
    }

    #[test]
    fn socket_path_is_process_specific_and_in_temp_dir() {
        let path = socket_path();
        assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, format!("protolens-nvim-{}.sock", std::process::id()));
    }

    #[test]
    fn next_state_stopped_rearms_suspended_with_same_pgid_and_socket() {
        let pgid = Pid::from_raw(4242);
        let socket_path = PathBuf::from("/tmp/protolens-nvim-4242.sock");
        let status = WaitStatus::Stopped(pgid, Signal::SIGTSTP);
        match next_state(pgid, socket_path.clone(), status) {
            EditorState::Suspended {
                pgid: got_pgid,
                socket_path: got_socket,
            } => {
                assert_eq!(got_pgid, pgid);
                assert_eq!(got_socket, socket_path);
            }
            EditorState::NotRunning => panic!("expected Suspended"),
        }
    }

    #[test]
    fn next_state_exited_collapses_to_not_running() {
        let pgid = Pid::from_raw(4242);
        let status = WaitStatus::Exited(pgid, 0);
        assert!(matches!(
            next_state(pgid, PathBuf::from("/tmp/x.sock"), status),
            EditorState::NotRunning
        ));
    }

    #[test]
    fn next_state_signaled_collapses_to_not_running() {
        let pgid = Pid::from_raw(4242);
        let status = WaitStatus::Signaled(pgid, Signal::SIGKILL, false);
        assert!(matches!(
            next_state(pgid, PathBuf::from("/tmp/x.sock"), status),
            EditorState::NotRunning
        ));
    }
}
