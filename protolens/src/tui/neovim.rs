// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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
/// 1-indexed line/column, ready to hand to the editor.
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

// ── Which editor (spec 0420 S1, S2) ─────────────────────────────────────────

/// Where the editor command came from, named in messages (spec 0420 S6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Visual,
    Editor,
    Default,
}

impl Source {
    fn describe(self) -> &'static str {
        match self {
            Source::Visual => "from VISUAL",
            Source::Editor => "from EDITOR",
            Source::Default => "the default",
        }
    }
}

/// What protolens knows how to drive beyond "run it at a line" (S2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorKind {
    /// Reused across `v` presses through its `--listen` socket (S3).
    Neovim,
    /// Positioned at the exact column with `+call cursor(L,C)` (S4).
    Vim,
    /// Anything else, `vi` included: positioned with `+LINE` (S4).
    Other,
}

/// The editor `v` runs: a command, possibly with arguments, as the user
/// wrote it in `VISUAL` or `EDITOR`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Editor {
    value: OsString,
    source: Source,
    kind: EditorKind,
}

/// git's list of shell metacharacters (`run-command.c`): a command holding
/// none of them, and no space, is executed directly; any other goes
/// through `sh` (spec 0420 S1).
const SHELL_METACHARS: &[u8] = b"|&;<>()$`\\\"' \t\n*?[#~=%";

/// `sh`'s exit status for a command it could not find.
const SHELL_NOT_FOUND: i32 = 127;

impl Editor {
    /// The first word of the command: the program, for messages and for
    /// Neovim's `--server` calls.
    fn program(&self) -> &OsStr {
        let bytes = self.value.as_bytes();
        let word = bytes
            .split(|b| b.is_ascii_whitespace())
            .find(|w| !w.is_empty())
            .unwrap_or(bytes);
        OsStr::from_bytes(word)
    }

    /// The command with `args` after it, run as git runs an editor.
    fn command(&self, args: &[OsString]) -> Command {
        if self
            .value
            .as_bytes()
            .iter()
            .any(|b| SHELL_METACHARS.contains(b))
        {
            let mut script = self.value.clone();
            script.push(" \"$@\"");
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(script).arg(&self.value).args(args);
            cmd
        } else {
            let mut cmd = Command::new(&self.value);
            cmd.args(args);
            cmd
        }
    }

    /// The message for a command that is not there (S6).
    fn not_found(&self) -> String {
        format!(
            "{} not found ({}): set VISUAL or EDITOR to your editor",
            self.program().to_string_lossy(),
            self.source.describe()
        )
    }
}

/// The kind of `value`, from the basename of its first word, without
/// running anything (S2).
fn kind_of(value: &OsStr) -> EditorKind {
    let bytes = value.as_bytes();
    let word = bytes
        .split(|b| b.is_ascii_whitespace())
        .find(|w| !w.is_empty())
        .unwrap_or(bytes);
    match Path::new(OsStr::from_bytes(word))
        .file_name()
        .map(OsStr::as_bytes)
    {
        Some(b"nvim") => EditorKind::Neovim,
        Some(b"vim") => EditorKind::Vim,
        _ => EditorKind::Other,
    }
}

/// `VISUAL` if set and not empty, else `EDITOR`, else `nvim` when
/// `nvim_on_path` says it is installed, else `vi` (S1). The variables are
/// parameters so that tests need not touch the process environment.
pub(crate) fn choose_editor(
    visual: Option<OsString>,
    editor: Option<OsString>,
    nvim_on_path: impl FnOnce() -> bool,
) -> Editor {
    let (value, source) = match (visual, editor) {
        (Some(v), _) if !v.is_empty() => (v, Source::Visual),
        (_, Some(e)) if !e.is_empty() => (e, Source::Editor),
        _ if nvim_on_path() => ("nvim".into(), Source::Default),
        _ => ("vi".into(), Source::Default),
    };
    Editor {
        kind: kind_of(&value),
        value,
        source,
    }
}

/// Whether `name` is an executable file in a directory of `PATH`.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

// A `thread_local`, not a `static`: `cargo test` runs tests in parallel
// threads of one process, and the spawn happens on the same thread as
// the test that set this, so a thread-local override cannot be observed
// by any other test and needs no lock.
#[cfg(test)]
thread_local! {
    /// The editor command a test runs, as if it were `EDITOR`.
    pub(super) static EDITOR_COMMAND: std::cell::Cell<&'static str> =
        const { std::cell::Cell::new("protolens-test-no-editor-set") };
}

/// The editor `v` runs: from the environment, or, in a test, from
/// `EDITOR_COMMAND`.
///
/// The override exists because the branch worth a regression test is
/// the one where the spawn *fails*: that is where the 2026-07-18 crash
/// lived. Reaching it means naming a binary that is not on `PATH`, and
/// the test used to arrange that by probing for a real `nvim` and
/// returning early if it found one — so on any machine with Neovim
/// installed, which is most of them, the test skipped itself and
/// reported a pass. Overriding the command instead exercises the branch
/// everywhere, and never reads the developer's own `EDITOR`.
fn editor() -> Editor {
    #[cfg(test)]
    {
        let value = EDITOR_COMMAND.with(std::cell::Cell::get);
        choose_editor(None, Some(value.into()), || false)
    }
    #[cfg(not(test))]
    {
        choose_editor(
            std::env::var_os("VISUAL"),
            std::env::var_os("EDITOR"),
            || on_path("nvim"),
        )
    }
}

// ── The handoff ─────────────────────────────────────────────────────────────

/// `v`'s Neovim handoff state (spec 0144 G5) — stored on `App`
/// (`editor_state`, `mod.rs`) rather than a process-global `static`:
/// `main.rs` constructs exactly one `App` for the process's entire
/// lifetime, and this is a single-threaded event loop. Only Neovim is ever
/// `Suspended`: any other editor runs to its end (spec 0420 S4).
pub(crate) enum EditorState {
    NotRunning,
    Suspended { pgid: Pid, socket_path: PathBuf },
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("protolens-nvim-{}.sock", std::process::id()))
}

/// Pure `WaitStatus -> EditorState` transition (G7): a `Stopped` result
/// re-arms `Suspended` with the same `pgid`/`socket_path` (nothing else
/// distinguishes one stop from the next); any other result — `Exited`,
/// `Signaled`, or otherwise — collapses to `NotRunning`. `pgid` is
/// passed in rather than read from `status` because `waitpid` is
/// called directly on the leader `pgid` (see `open_in_neovim` below, not
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

/// `v`'s editor handoff, called by `run_loop` right after the `handle_key`
/// call that armed `app.pending_editor_open`. Leaves the terminal exactly
/// as a normal exit would, hands it to the editor, and restores raw mode,
/// the alternate screen and mouse capture on the way back (spec 0113 D31).
/// Neovim is driven as spec 0144 G5 describes (`open_in_neovim`); any other
/// editor is run to its end (`run_to_exit`, spec 0420 S4). A failure is a
/// message, never an error: the TUI keeps running.
pub(crate) fn open_editor<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    req: EditorRequest,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    let editor = editor();
    restore_terminal();
    match editor.kind {
        EditorKind::Neovim => open_in_neovim(app, &editor, req),
        EditorKind::Vim | EditorKind::Other => run_to_exit(app, &editor, req),
    }
    enable_raw_mode_and_reenter(terminal)?;
    Ok(())
}

/// Vim or any other editor (spec 0420 S4): run in protolens's own process
/// group, so Ctrl-Z suspends both back to the shell, as `git commit` does,
/// and wait for it to exit.
fn run_to_exit(app: &mut App, editor: &Editor, req: EditorRequest) {
    let goto = match editor.kind {
        EditorKind::Vim => format!("+call cursor({},{})", req.line, req.col),
        _ => format!("+{}", req.line),
    };
    let mut cmd = editor.command(&[goto.into(), req.path.into_os_string()]);
    if let Some(proto_root) = &app.proto_root {
        cmd.env("PROTOTEXT_PROTO_ROOT", proto_root);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            app.message = launch_failure(editor, &e);
            return;
        }
    };
    // Ignored only once the editor has started, as git does: an ignored
    // disposition survives exec, and the editor must keep its own Ctrl-C.
    // From here until the restore, a Ctrl-C meant for the editor does not
    // end protolens, which shares its process group.
    // SAFETY: installing a signal disposition for our own process is
    // always sound; neither takes captured state.
    let (int, quit) = unsafe {
        (
            signal(Signal::SIGINT, SigHandler::SigIgn),
            signal(Signal::SIGQUIT, SigHandler::SigIgn),
        )
    };
    let status = child.wait();
    // SAFETY: as above, putting back what was there.
    unsafe {
        if let Ok(previous) = int {
            let _ = signal(Signal::SIGINT, previous);
        }
        if let Ok(previous) = quit {
            let _ = signal(Signal::SIGQUIT, previous);
        }
    }
    match status {
        Ok(status) if status.code() == Some(SHELL_NOT_FOUND) => {
            app.message = editor.not_found();
        }
        Ok(status) if !status.success() => {
            app.message = format!(
                "{} exited with {status}",
                editor.program().to_string_lossy()
            );
        }
        Ok(_) => {}
        Err(e) => {
            app.message = format!(
                "cannot wait for {}: {e}",
                editor.program().to_string_lossy()
            );
        }
    }
}

/// The message for an editor that could not be started (S6): a missing
/// one names the remedy; any other failure keeps the OS error.
fn launch_failure(editor: &Editor, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        editor.not_found()
    } else {
        format!("cannot launch {}: {e}", editor.program().to_string_lossy())
    }
}

/// Neovim (spec 0144 G5, spec 0420 S3): no instance yet, start one with
/// `--listen`; one already `Suspended` from an earlier `v` press, send it
/// `--remote-tab`/`--remote-send` requests over that socket instead of
/// starting a second instance, then `SIGCONT` it — G5's "single instance".
///
/// The `--remote-tab`/`--remote-send` flags and the `<C-\><C-N>`
/// mode-escape prefix are verified against Neovim 0.11.5: a combined
/// single-call `--remote-tab +cmd` does *not* apply `+cmd` to the newly
/// opened tab, so two separate calls are required, as below.
fn open_in_neovim(app: &mut App, editor: &Editor, req: EditorRequest) {
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
    let program = editor.program().to_string_lossy().into_owned();

    let (pgid, socket_path, resuming) = match &app.editor_state {
        EditorState::NotRunning => {
            let socket_path = socket_path();
            let goto = format!("+call cursor({},{})", req.line, req.col);
            // The user's own configuration, with nothing of protolens's
            // (spec 0420 S3): no `-u`.
            let mut cmd = editor.command(&[
                "--listen".into(),
                socket_path.clone().into_os_string(),
                goto.into(),
                req.path.into_os_string(),
            ]);
            if let Some(proto_root) = &app.proto_root {
                cmd.env("PROTOTEXT_PROTO_ROOT", proto_root);
            }
            let child = cmd
                .process_group(0) // new pgid, equal to the child's own pid
                .spawn();
            let child = match child {
                Ok(child) => child,
                Err(e) => {
                    app.message = launch_failure(editor, &e);
                    return;
                }
            };
            let pgid = Pid::from_raw(child.id() as i32);
            (pgid, socket_path, false)
        }
        EditorState::Suspended { pgid, socket_path } => {
            let goto = format!("cursor({},{})", req.line, req.col);
            let _ = Command::new(editor.program())
                .arg("--server")
                .arg(socket_path)
                .arg("--remote-tab")
                .arg(&req.path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = Command::new(editor.program())
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
            app.message = format!("cannot resume {program}: {e}");
            return;
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
            app.message = format!("cannot wait for {program}: {e}");
            return;
        }
    };
    let next = next_state(pgid, socket_path.clone(), status);
    if matches!(next, EditorState::NotRunning) {
        let _ = std::fs::remove_file(&socket_path);
        match status {
            // Through `sh` (a command with arguments), a missing Neovim
            // is `sh`'s status, not a spawn error.
            WaitStatus::Exited(_, SHELL_NOT_FOUND) => app.message = editor.not_found(),
            WaitStatus::Exited(..) | WaitStatus::Signaled(..) => {}
            // No dedicated logging mechanism exists in protolens (no
            // `log`/`tracing` dependency; `eprintln!` is used only
            // pre-TUI in main.rs) — an unexpected `waitpid` result
            // routes through the same auto-dismissing `app.message`
            // notice every other failure here already uses.
            _ => app.message = format!("{program} exited unexpectedly: {status:?}"),
        }
    }
    app.editor_state = next;
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

    // ── Which editor (spec 0420 S1, S2) ─────────────────────────────────

    fn os(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn visual_wins_then_editor_then_the_default() {
        let pick = |visual, editor, nvim| {
            let e = choose_editor(visual, editor, || nvim);
            (e.value.into_string().unwrap(), e.source)
        };
        assert_eq!(
            pick(os("code --wait"), os("vim"), true),
            ("code --wait".into(), Source::Visual)
        );
        assert_eq!(pick(None, os("vim"), true), ("vim".into(), Source::Editor));
        assert_eq!(
            pick(os(""), os("nano"), true),
            ("nano".into(), Source::Editor),
            "an empty VISUAL is unset"
        );
        assert_eq!(pick(None, os(""), true), ("nvim".into(), Source::Default));
        assert_eq!(pick(None, None, false), ("vi".into(), Source::Default));
    }

    #[test]
    fn the_kind_comes_from_the_first_words_basename() {
        let kind = |v: &str| kind_of(OsStr::new(v));
        assert_eq!(kind("nvim"), EditorKind::Neovim);
        assert_eq!(kind("/usr/bin/nvim -p"), EditorKind::Neovim);
        assert_eq!(kind("  vim"), EditorKind::Vim);
        assert_eq!(kind("vi"), EditorKind::Other, "vi may be nvi or BusyBox vi");
        assert_eq!(kind("nano"), EditorKind::Other);
        assert_eq!(kind("nvim-qt"), EditorKind::Other);
    }

    fn argv(cmd: &Command) -> Vec<String> {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_plain_command_is_executed_and_one_with_arguments_goes_through_sh() {
        let args = [OsString::from("+3"), OsString::from("a b.proto")];
        let plain = choose_editor(None, os("nano"), || false);
        assert_eq!(argv(&plain.command(&args)), ["nano", "+3", "a b.proto"]);
        assert_eq!(plain.program(), "nano");

        let with_args = choose_editor(None, os("code --wait"), || false);
        assert_eq!(
            argv(&with_args.command(&args)),
            [
                "sh",
                "-c",
                "code --wait \"$@\"",
                "code --wait",
                "+3",
                "a b.proto"
            ]
        );
        assert_eq!(with_args.program(), "code");
    }

    /// The default editor is `nvim` only when `on_path` finds it (S1).
    #[test]
    fn on_path_finds_an_installed_program_and_not_a_missing_one() {
        assert!(on_path("sh"), "sh is on every PATH the tests run under");
        assert!(!on_path("protolens-test-no-such-program"));
    }

    #[test]
    fn a_missing_editor_is_named_with_its_source() {
        let e = choose_editor(os("subl -w"), None, || false);
        assert_eq!(
            e.not_found(),
            "subl not found (from VISUAL): set VISUAL or EDITOR to your editor"
        );
        let e = choose_editor(None, None, || false);
        assert_eq!(
            e.not_found(),
            "vi not found (the default): set VISUAL or EDITOR to your editor"
        );
    }
}
