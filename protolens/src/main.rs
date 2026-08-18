// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

mod affinity;
mod annotation;
mod blob;
mod colorize;
mod complete;
mod decode;
mod export_descriptor;
mod extract;
mod fold_set;
mod man;
mod node_status;
mod override_pane;
mod provenance;
mod render_cache;
mod script;
mod sweep;
mod theme;
mod tui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{CommandFactory, Parser};
use clap_complete::engine::ArgValueCompleter;
use clap_complete::CompleteEnv;

use crate::blob::Blob;

/// Interactive TUI to decode, navigate, and extract raw bytes from a binary
/// protobuf.
///
/// Enable shell completion:
///   bash:  source <(PROTOLENS_COMPLETE=bash protolens)
///   zsh:   source <(PROTOLENS_COMPLETE=zsh protolens)
///   fish:  PROTOLENS_COMPLETE=fish protolens | source
#[derive(Parser)]
#[command(name = "protolens", version, about)]
struct Cli {
    /// Batch action to perform and exit. If omitted, protolens launches
    /// the interactive TUI (spec 0123).
    #[command(subcommand)]
    command: Option<Command>,

    /// FileDescriptorSet for root-type determination and type resolution.
    /// May be a raw binary FileDescriptorSet or a `#@ prototext`-format
    /// FileDescriptorSet. Shares its env var with `prototext`/`reproto`
    /// (spec 0090), so a single `PROTOTEXT_DESCRIPTOR_SET` covers the
    /// whole toolset.
    #[arg(
        long = "descriptor-set",
        env = complete::DESCRIPTOR_SET_ENV,
        add = ArgValueCompleter::new(complete::complete_any_path),
    )]
    descriptor_set: Option<PathBuf>,

    /// Root directory `.proto` source files are resolved against for `v`'s
    /// jump-to-definition (spec 0144). Shares its env var naming with
    /// `PROTOTEXT_DESCRIPTOR_SET` (spec 0090); set externally by the
    /// internal `prototools` package embedding this repo. When neither
    /// this nor the env var is set, falls back to `<stub>/proto/` next
    /// to `--descriptor-set` (`<stub>` = descriptor-set path with its
    /// extension stripped — the same stub `reproto --schema-db-out`
    /// uses), if that directory exists (spec 0155 G2). Still fully
    /// optional — `v` reports a message rather than failing at startup
    /// when no root is found either way.
    #[arg(
        long = "proto-root",
        short = 'I',
        env = "PROTOTEXT_PROTO_ROOT",
        add = ArgValueCompleter::new(complete::complete_dir_path),
    )]
    proto_root: Option<PathBuf>,

    /// Root message type. If omitted, inferred automatically from
    /// --descriptor-set (requires a hopcroft.rkyv scoring graph next to
    /// it); if inference is unavailable or inconclusive, the blob is
    /// rendered with no known type.
    #[arg(
        short = 't',
        long = "type",
        conflicts_with = "raw",
        add = ArgValueCompleter::new(complete::complete_type_names),
    )]
    r#type: Option<String>,

    /// Skip root-type inference and open the blob with no type at all,
    /// as raw wire bytes (spec 0168). Inference is the one startup phase
    /// whose cost scales with the size of the schema *database* rather
    /// than the blob, so on a large descriptor set this is the fast way
    /// in; it is also the way to look at a blob whose inferred type is
    /// wrong. A type can still be chosen interactively afterwards, via
    /// the override pane.
    #[arg(long = "raw")]
    raw: bool,

    /// Number of spaces per nesting level in the rendered text.
    #[arg(long = "indent", default_value_t = 2)]
    indent: usize,

    /// CPU budget for root-type inference (spec 0217). Inference is the
    /// one startup phase whose cost scales with the size of the schema
    /// database rather than the blob, and it divides cleanly: the
    /// candidate types are partitioned and swept in parallel.
    ///
    /// This is a ceiling for the whole session, not a target. It is
    /// clamped to the number of CPUs actually available to the process
    /// (which respects a cgroup quota and an affinity mask), and while
    /// the TUI is running one thread of it is left to the main loop.
    /// Defaults to every available CPU; `1` restores the single-threaded
    /// sweep.
    #[arg(
        long = "jobs",
        short = 'j',
        default_value_t = sweep::available_cpus() as u64,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    jobs: u64,

    /// Syntax-highlighting color palette (spec 0116 §9). `system` probes
    /// the terminal's actual background once at startup and resolves to
    /// `dark` or `light`.
    #[arg(long = "theme", value_enum, default_value = "system")]
    theme: theme::ThemeKind,

    /// Maximum number of interior bytes of a live-preview override
    /// candidate handed to the renderer (spec 0174) — guards against a
    /// structurally mismatched candidate type making the
    /// recursive-descent decoder mis-parse arbitrary bytes into a
    /// pathologically large synthetic tree while the user is just
    /// browsing candidates. A truncated preview ends with a `...` line.
    /// A *confirmed* override is always rendered completely, regardless
    /// of this value.
    #[arg(
        long = "override-preview-byte-budget",
        default_value_t = tui::App::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT,
    )]
    override_preview_byte_budget: usize,

    /// Take the blob for wire bytes without looking (spec 0216 S29).
    ///
    /// A file whose first 13 bytes are `#@ prototext:` is normally
    /// decoded from the textual form instead. This says not to, for the
    /// rare binary blob that happens to begin with those bytes.
    #[arg(long = "assume-binary")]
    assume_binary: bool,

    /// Read the blob into memory instead of mapping it (spec 0216 S29).
    ///
    /// A large binary blob is mapped, which keeps its pages evictable
    /// rather than making them the process's own. The cost is that the
    /// file must not change underneath the session: truncating it, or
    /// losing the network filesystem it lives on, turns a read into a
    /// `SIGBUS` rather than an error at startup. Use this for a blob
    /// that cannot be trusted to hold still.
    ///
    /// Mapping is declined by itself for a small blob and for anything
    /// that is not a regular file, so this is only needed for the case
    /// the size test cannot see.
    #[arg(long = "eager-read")]
    eager_read: bool,

    /// Guided walk through the blob to load (spec 0271). A missing or
    /// malformed file is a hard error.
    ///
    /// With no `--script`, the blob path with its extension replaced by
    /// `.script` is loaded if it exists.
    #[arg(
        long = "script",
        add = ArgValueCompleter::new(complete::complete_any_path),
    )]
    script: Option<PathBuf>,

    /// Do not look for a script beside the blob (spec 0271 S1).
    #[arg(long = "no-script", conflicts_with = "script")]
    no_script: bool,

    /// Rows for the script pane, overriding its computed share of the
    /// terminal (spec 0271 S4).
    #[arg(long = "script-height", value_parser = clap::value_parser!(u16).range(1..))]
    script_height: Option<u16>,

    /// Binary protobuf to decode.
    #[arg(add = ArgValueCompleter::new(complete::complete_any_path))]
    blob: PathBuf,
}

/// Batch (non-interactive) subcommands (spec 0123).
#[derive(clap::Subcommand)]
enum Command {
    /// Export one node's rendering (or, for the two descriptor formats,
    /// its synthetic schema) and exit, without entering the interactive
    /// TUI.
    Export {
        /// Field path of the node to export, in positional-path
        /// notation (`/` = document root; `/1/2` = root's 1st child's
        /// 2nd child — same notation the TUI's status line displays per
        /// node).
        path: String,

        /// Previously-saved override collection (spec 0117 §4 YAML, the
        /// same file `:save` writes) to apply before exporting. A
        /// target-hash mismatch against the loaded blob/descriptor-set
        /// is a warning (to stderr), not a hard error — same policy as
        /// the `:restore` TUI command. A missing file or malformed
        /// YAML, unlike the TUI, is a hard error. Required when
        /// `--format` is `descriptor-binary`/`descriptor-prototext`
        /// (spec 0156 G9).
        #[arg(long = "load-overrides")]
        load_overrides: Option<PathBuf>,

        /// Output format. Defaults to `prototext`.
        #[arg(long = "format", value_enum)]
        format: Option<ExtractFormatArg>,

        /// Write to this file instead of stdout.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },

    /// Run every startup phase and exit, without entering the
    /// interactive TUI (spec 0198) — the startup-time benchmark target.
    ///
    /// Takes no options of its own: it is the root command's own
    /// arguments, run for their cost. Everything before the subcommand
    /// dispatch — descriptor load, root-type resolution, decode,
    /// `App::new` — already runs for every mode, so this variant stays
    /// a faithful measurement without listing the phases it covers.
    ///
    /// Named `quit` to match the TUI's own `:quit` (spec 0236 S21).
    Quit,

    /// Apply every step of the loaded script and write what each one
    /// produced, without entering the interactive TUI (spec 0271 S14).
    ///
    /// Which script is loaded is the root command's `--script` (or the
    /// one found beside the blob), so this subcommand has no script
    /// argument of its own.
    Script {
        /// Write to this file instead of stdout.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}

/// Mirrors the TUI's `:export` flags (spec 0156 G2/G8) — a separate
/// type since `extract::ExtractFormat` has no `clap::ValueEnum` derive
/// and, unlike this type, has no variants for the two descriptor
/// formats (those route to `export_descriptor` instead).
#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
enum ExtractFormatArg {
    Binary,
    Prototext,
    DescriptorBinary,
    DescriptorPrototext,
}

impl ExtractFormatArg {
    /// The `extract::ExtractFormat` this argument names, or `None` for
    /// the two descriptor formats, which that enum has no variants for.
    ///
    /// A total function rather than a `From` impl: the conversion is
    /// only defined on half the domain, and a `From` that panics on the
    /// other half is a trap for whoever writes the next `.into()` — the
    /// two descriptor formats route to `export_descriptor`, which the
    /// sole caller has already branched away to before asking.
    fn extract_format(self) -> Option<extract::ExtractFormat> {
        match self {
            Self::Prototext => Some(extract::ExtractFormat::Text),
            Self::Binary => Some(extract::ExtractFormat::Binary),
            Self::DescriptorBinary | Self::DescriptorPrototext => None,
        }
    }

    /// Whether this format exports a *schema* (`export_descriptor`)
    /// rather than a view of the document's own bytes.
    fn is_descriptor(self) -> bool {
        matches!(self, Self::DescriptorBinary | Self::DescriptorPrototext)
    }
}

/// Default `proto_root` from `<descriptor_set-stub>/proto/` when the
/// caller didn't set one explicitly (spec 0155 G2) — `None` (no
/// fallback applied) whenever `cli_proto_root` is already `Some`, or
/// whenever the candidate directory doesn't exist.
fn resolve_proto_root(
    cli_proto_root: Option<PathBuf>,
    descriptor_set: Option<&Path>,
) -> Option<PathBuf> {
    cli_proto_root.or_else(|| {
        let candidate = descriptor_set?.with_extension("").join("proto");
        candidate.is_dir().then_some(candidate)
    })
}

/// What the document render is allowed to emit before it starts folding
/// (spec 0257 S1/S2), or `None` for a whole render.
///
/// Bounded whenever something will drain the stops — the TUI, and `quit`
/// (S4), whose whole purpose is to time what a session pays. `export` is
/// never bounded: nothing there would ever bake, so a budget would write
/// a truncated file (S4, and spec 0256 S3 before it).
///
/// The height comes from `crossterm::terminal::size()`, which is an
/// `ioctl` rather than an escape-sequence probe — so unlike the theme
/// and RGB detection `main` skips for `quit`, asking costs no timeout
/// and can be done on every bounded path.
///
/// Two chrome rows come off it, the same two the first frame will spend:
/// the global command/message row and the main pane's own statusline
/// (spec 0147 G1). Deliberately not `App::document_pane_height()` —
/// `main_area` is written by `render_main_pane` and is
/// `Rect::default()` until the first frame, so asking `App` would return
/// zero.
///
/// And then one row is added back, which is what G2 costs. The budget
/// counts lines *emitted*, and a stop's own header is one of them: the
/// renderer stops at a node when `line_count >= budget` with that node's
/// header already written, so the first stop's header lands on document
/// line `budget` exactly. A pane `budget` rows tall would draw it, and a
/// stop is a row that pops open a moment later. Every later stop is
/// pushed further down than folding pulls it up — a stop contributes two
/// rendered lines and one drawn row — so `budget = pane + 1` puts all of
/// them off the bottom of the first frame, not just the first.
///
/// `FALLBACK_ROWS` when there is no terminal to ask. This is the one
/// place S2 and S4 pull in opposite directions: S2 reasons that a failed
/// `size()` means no interactive session, but `quit` under a benchmark
/// harness has no tty and is exactly the case S4 wants bounded. So the
/// fallback keeps the bound rather than dropping it, and the Background
/// table is what makes the number safe to pick — startup is flat across
/// budgets from 50 to 5000, so this is not a tuning knob.
///
/// Getting it wrong is cheap in both directions: too small and the
/// visible stops are expanded by the first frame itself, through spec
/// 0249 S8's `note_visible_stops`, before the user can act on them; too
/// large and it costs nothing measurable.
fn startup_row_budget(command: &Option<Command>) -> Option<usize> {
    /// A screenful when nobody can tell us what a screen is.
    const FALLBACK_ROWS: usize = 50;
    /// The command/message row and the main pane's statusline.
    const CHROME_ROWS: usize = 2;
    match command {
        None | Some(Command::Quit) => {
            let rows = crossterm::terminal::size()
                .map(|(_, rows)| rows as usize)
                .unwrap_or(FALLBACK_ROWS);
            let pane = rows.saturating_sub(CHROME_ROWS);
            Some((pane + 1).max(tui::App::MIN_EXPAND_ROWS))
        }
        // Both walk the document rather than a screenful, and neither
        // is a startup measurement, so neither pays for a budget it
        // would only have to bake back.
        Some(Command::Export { .. } | Command::Script { .. }) => None,
    }
}

/// `" (N MB)"` for a startup progress message, or `""`.
///
/// The unit is chosen to fit, because the number is the whole point:
/// this suffix exists to say what the wait the message announces is
/// proportional to, and a fixed MB unit truncated everything under a
/// mebibyte to a flat `0 MB` — which reads as either an empty file or a
/// bug, and tells the user nothing either way.
///
/// Best effort (spec 0151 G7): a metadata read failure simply omits the
/// size suffix rather than aborting or erroring, since this is pure UX
/// decoration, not load-bearing for correctness.
fn size_suffix(p: &Path) -> String {
    let Ok(meta) = std::fs::metadata(p) else {
        return String::new();
    };
    let bytes = meta.len();
    if bytes >= 1024 * 1024 {
        format!(" ({} MB)", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!(" ({} KB)", bytes / 1024)
    } else {
        format!(" ({bytes} bytes)")
    }
}

/// Write a batch export to stdout, flushed.
///
/// The flush is the point: Rust's runtime flushes stdout at exit but
/// discards the error, so without this an export into a full disk writes
/// a truncated stream and still exits `0` — and the exit code is the only
/// signal a script piping protolens into a build receives.
fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    out.write_all(bytes)?;
    out.flush()
}

fn main() -> ExitCode {
    // Dynamic shell completion — same model as Cargo/prototext. When
    // PROTOLENS_COMPLETE=<shell> is set, print the completion script and
    // exit.
    CompleteEnv::with_factory(Cli::command)
        .var("PROTOLENS_COMPLETE")
        .complete();

    // Man page generation — same model, and before parsing for the same
    // reason: `blob` is a required positional (spec 0228 S10).
    man::generate_and_exit_if_requested(Cli::command());

    let cli = Cli::parse();

    // Spec 0264: on a machine whose kernel says which CPUs are the fast
    // ones, the thread that draws every frame runs on them. Here rather
    // than at the top of the interactive path so that startup — the
    // longest wait protolens imposes — is covered too. Silent, and a
    // no-op wherever the kernel does not say.
    affinity::apply();

    let descriptor_set = cli.descriptor_set.as_deref();
    if descriptor_set.is_none() && cli.r#type.is_some() {
        eprintln!("error: --type requires --descriptor-set");
        return ExitCode::FAILURE;
    }
    // Here rather than in the `Export` arm below, which is where this
    // check used to live: nothing about it depends on the blob, the
    // descriptor set or the tree, and everything between here and there
    // — the descriptor load, the wrap, the inference sweep — takes
    // seconds on a large set. A flag combination that was never going to
    // be accepted should be refused before the user waits for it.
    if let Some(Command::Export {
        format,
        load_overrides,
        ..
    }) = &cli.command
    {
        if format.is_some_and(ExtractFormatArg::is_descriptor) && load_overrides.is_none() {
            eprintln!(
                "error: --format=descriptor-binary/descriptor-prototext requires \
                 --load-overrides (batch mode has no interactive type-inference loop)"
            );
            return ExitCode::FAILURE;
        }
    }

    // Spec 0271 S1. Loaded here, before the descriptor set: a script
    // that does not parse is a typo in a file the user just wrote, and
    // they should not wait through startup to hear about it. The
    // implicit script is as hard an error as the explicit one — a
    // malformed file that is silently ignored is indistinguishable from
    // one that was never found.
    let script_path = match (&cli.script, cli.no_script) {
        (Some(p), _) => Some(p.clone()),
        (None, true) => None,
        (None, false) => script::Script::beside(&cli.blob),
    };
    let script = match script_path {
        Some(path) => match script::Script::load(&path) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    if script.is_none() {
        if let Some(Command::Script { .. }) = &cli.command {
            eprintln!("error: the `script` subcommand needs a script — none was given or found");
            return ExitCode::FAILURE;
        }
    }

    // Spec 0216 S28/S29: the blob is read (or mapped) and wrapped in the
    // virtual encompassing message here, once, and every byte range the
    // session ever shows is a view of it. A `#@ prototext` text blob is
    // accepted transparently, by encoding it into that same buffer rather
    // than converting it and copying the result in.
    let blob = match Blob::load(&cli.blob, cli.assume_binary, cli.eager_read) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!("error: cannot read '{}': {e}", cli.blob.display());
            return ExitCode::FAILURE;
        }
    };

    if cli.command.is_none() {
        match descriptor_set {
            Some(descriptor_set) => {
                let name = descriptor_set
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| descriptor_set.display().to_string());
                // The sibling `hopcroft.rkyv` scoring graph, if present,
                // is loaded in the same phase but is not named here: it
                // runs about a fifth of the descriptor set's size (4.6 MB
                // against 24.5 MB for googleapis; 209 KB against 1.0 MB
                // for a protoc descriptor pool), so the descriptor size
                // alone already tells the reader what this wait is
                // proportional to. Whether a graph was found shows up in
                // the next two messages anyway — no graph means no
                // inference phase and a raw root.
                eprintln!(
                    "protolens: loading descriptor set '{name}'{}...",
                    size_suffix(descriptor_set)
                );
            }
            None => {
                eprintln!("protolens: no --descriptor-set — decoding without a schema...");
            }
        }
    }

    let ctx_result = match descriptor_set {
        Some(path) => decode::DescriptorContext::load(path),
        None => Ok(decode::DescriptorContext::schemaless()),
    };
    let mut ctx = match ctx_result {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Spec 0197 §S3: the on-demand path was declined, so the user is about
    // to wait seconds that a `reproto` re-run would remove. Printed
    // between the "loading descriptor set" line and the wait it explains,
    // so it is on screen *while* they wait rather than after. The splash
    // pane and the status line carry the same text for a launch nobody was
    // watching stderr for.
    //
    // Batch mode stays silent, like every other progress line above: its
    // stderr is a machine-readable "did anything go wrong" channel, and a
    // performance note is not something going wrong.
    if cli.command.is_none() {
        if let Some(fallback) = &ctx.fallback {
            eprintln!("protolens: warning: {}", fallback.message);
        }
    }

    let root_type = match (cli.raw, cli.r#type.as_deref()) {
        (true, _) => decode::RootType::Raw,
        (false, Some(fqdn)) => decode::RootType::Named(fqdn),
        (false, None) => decode::RootType::Infer,
    };

    // Spec 0168 G1/G4: root-type inference runs here, synchronously, for
    // the TUI as well as the batch paths, and is announced on stderr
    // alongside the messages the phases before it print — the alternate
    // screen is not entered until `tui::run`, so there is no frame to
    // draw into and none is needed.
    //
    // Announced separately from the render below, rather than as one
    // "inferring and decoding" line, because both are multi-second on a
    // large blob and they are wildly unequal: on a 24 MB blob the sweep is
    // a few seconds of a half-minute wait. A single message spanning both
    // makes whichever phase is actually running look like a hang in the
    // other. This is also why `decode` is split into
    // `resolve_root_type_and_arena` + `render_resolved` here instead of
    // being called whole.
    //
    // `--type` is an O(1) pool lookup and `--raw` skips the sweep
    // outright, so neither gets this line (G6).
    // Spec 0198 S2: spelled as a positive list rather than as
    // `!matches!(.., Export)`, so a future subcommand is silent by
    // default and has to opt in — `tests/batch_export.rs` asserts a
    // successful `export` writes nothing at all to stderr, and that is
    // the contract every batch subcommand but `exit` should inherit.
    // `exit` opts in because the phase lines *are* its output.
    let announce = matches!(cli.command, None | Some(Command::Quit));
    let startup_row_budget = startup_row_budget(&cli.command);
    // Spec 0217 S4: the whole session's budget, clamped once here so that
    // `--help`'s promise ("up to N") and what actually runs agree, and so
    // the number stored on `App` is already the honest one.
    let jobs = sweep::effective_jobs(cli.jobs as usize);
    // Spec 0270: the count is the sweep's to report, not this line's to
    // guess. `--jobs` is a ceiling, and since spec 0269 the seating can
    // lower it and the calling thread can join — so `jobs` was the one
    // number here guaranteed *not* to be the answer. `0` is "no sweep
    // ran", which subsumes the `Infer`-and-a-graph test this used to
    // make for itself.
    let announce_sweep = |threads: usize| {
        if announce && threads > 0 {
            eprintln!(
                "protolens: inferring root type{} on {threads} thread{}...",
                size_suffix(&cli.blob),
                if threads == 1 { "" } else { "s" },
            );
        }
    };
    let decoded =
        decode::resolve_root_type_and_arena(&blob, &mut ctx, root_type, jobs, announce_sweep)
            .and_then(|(root_desc, root_candidates, arena)| {
                if announce {
                    eprintln!(
                        "protolens: rendering root node as {}{}...",
                        root_desc
                            .as_ref()
                            .map(|d| d.full_name())
                            .unwrap_or("<raw / no type>"),
                        size_suffix(&cli.blob)
                    );
                }
                decode::render_resolved(
                    Arc::clone(&blob),
                    &mut ctx,
                    root_desc,
                    root_candidates,
                    arena,
                    cli.indent,
                    startup_row_budget,
                )
            });
    let decoded = match decoded {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let blob_label = cli
        .blob
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cli.blob.display().to_string());

    // Theme/color resolution is irrelevant to batch mode's plain stdout/
    // file output (spec 0123 Non-goals) — only resolved, and only probes
    // the terminal, when actually about to enter the TUI.
    //
    // `exit` skips both for a second reason (spec 0198 S3/G4): each
    // writes an escape sequence and waits for a reply under a timeout,
    // so under a benchmark harness with no terminal to answer, what they
    // contribute to a measurement is the timeout rather than any
    // protolens work. Excluding them costs nothing else — the theme is
    // stored by `App::new` and first read when a frame is drawn, and
    // `exit` draws none.
    let theme = match &cli.command {
        None => {
            // Resolved exactly once, before any rendering (spec 0116 §9)
            // — no live re-detection while running.
            let theme = match cli.theme {
                theme::ThemeKind::System => theme::resolve_system(),
                resolved => resolved,
            };
            // Primes the XTGETTCAP RGB probe now, before `tui::run` takes
            // over the terminal with its own crossterm event loop (see
            // `prime_supports_rgb` doc comment).
            eprintln!("protolens: detecting terminal capabilities...");
            theme::prime_supports_rgb();
            theme
        }
        Some(_) => theme::ThemeKind::Dark,
    };

    // The last unannounced startup phase: `App::new` seeds the override
    // collection, seeds the stops the render reported (spec 0257 S3), and
    // runs one `render_overrides` pass. The pass used to be the largest
    // phase of startup; measured on googleapis it is 94 ms, and under
    // spec 0257's budget it walks a screenful rather than the document.
    if announce {
        eprintln!("protolens: indexing {} lines...", decoded.total_lines);
    }

    let proto_root = resolve_proto_root(cli.proto_root.clone(), descriptor_set);
    let mut app = tui::App::new(
        decoded,
        &blob_label,
        cli.blob.clone(),
        cli.indent,
        ctx,
        theme,
        proto_root,
    );
    app.override_preview_byte_budget = cli.override_preview_byte_budget;
    app.sweep_jobs = jobs;
    app.script_height = cli.script_height;
    // Spec 0271 S8: the pane is on screen from the first frame, with
    // navigation on and step 1 already applied. `export` is left out —
    // its output is a view of the document, and a step folds.
    if let (Some(script), None | Some(Command::Script { .. })) = (script, &cli.command) {
        app.set_script(script);
    }

    match cli.command {
        // Spec 0198 S1: every startup phase has already run above; this
        // arm is the whole subcommand.
        Some(Command::Quit) => ExitCode::SUCCESS,
        Some(Command::Script { output }) => {
            write_output(app.script_transcript().as_bytes(), output.as_deref())
        }
        Some(Command::Export {
            path,
            load_overrides,
            format,
            output,
        }) => run_export(
            &mut app,
            &path,
            load_overrides.as_deref(),
            format,
            output.as_deref(),
        ),
        None => {
            // Spec 0221 S4: `App::new`'s own override pass may have
            // refused, and the status line it set for them is about to
            // be unreadable — `warm_up_heat_cues` overwrites it with a
            // progress string before the first frame, and every keypress
            // clears it thereafter (spec 0147 G5). stderr is still ours
            // here (`tui::run` below is what enters the alternate
            // screen) and, unlike the status line, it appends: the
            // phase lines above cannot overwrite these, and the user can
            // scroll back to them after quitting. Same placement as spec
            // 0197 §S3's fallback warning.
            for (_, refusal) in &app.refusals {
                eprintln!("protolens: error: {refusal}");
            }
            if !app.refusals.is_empty() {
                // Replaces the plain count `render_overrides` set, now
                // that there is somewhere to point at for the detail.
                app.message = format!(
                    "{} override(s) refused at startup — see stderr",
                    app.refusals.len()
                );
            }
            if let Err(e) = tui::run(&mut app) {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
            // Spec 0320 S1: do not take this apart. Dropping a baked
            // `App` for `googleapis.desc` is a second of glibc free-list
            // coalescing (91% of it in `malloc_consolidate`/`unlink_chunk`
            // under `Arc::drop_slow`), handing spec 0222's per-slot line
            // strings back to an allocator whose whole address space the
            // kernel is about to reclaim in one act. `tui::run` has
            // already restored the terminal — measured at 17 ms — so
            // every millisecond after it is the user watching a dead
            // process tidy up.
            //
            // Safe because nothing in this chain has anything left to do:
            // the heat worker was shut down inside `tui::run` (spec 0152)
            // and so cannot write to the restored terminal; `Blob`'s
            // mapping is a `munmap` the kernel performs anyway; and
            // `SegmentScan`'s abort-and-join exists to keep a superseded
            // sweep from outliving the data it borrows, of which there is
            // no next borrower. The two file-removing destructors in this
            // crate are `#[cfg(test)]` and never on this path — a leaked
            // temp file would be a real cost and was checked for.
            //
            // Recheck that list before giving `App` a field whose `Drop`
            // does anything observable. Only the success path: a failure
            // returns above and drops normally.
            std::mem::forget(app);
            ExitCode::SUCCESS
        }
    }
}

/// The `export` subcommand, from a fully built `App`.
///
/// Split out of `main` because it is the one part of it that is not
/// startup: everything above the `match` runs for every invocation and
/// has to run in that order, whereas this reads `app` and writes a file.
/// It is also where every one of `main`'s failure exits but two live, so
/// keeping it here is what lets them stay `return`s of an `ExitCode`
/// rather than a flag threaded through the startup sequence.
fn run_export(
    app: &mut tui::App,
    path: &str,
    load_overrides: Option<&Path>,
    format: Option<ExtractFormatArg>,
    output: Option<&Path>,
) -> ExitCode {
    let mut hash_mismatch = false;
    if let Some(overrides_path) = load_overrides {
        match app.load_overrides(&overrides_path.to_string_lossy()) {
            Ok(load) => {
                for w in &load.warnings {
                    eprintln!("warning: --load-overrides: {w}");
                }
                hash_mismatch = load.hash_mismatch;
            }
            Err(e) => {
                eprintln!(
                    "error: --load-overrides '{}': {e}",
                    overrides_path.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    // Spec 0221 S3. Reported here, before anything is written: an
    // override that could not be applied means the export would not be
    // what was asked for, and for `--format=descriptor-binary` — which
    // *requires* `--load-overrides` — that is a wrong schema handed to
    // whatever consumes it. The exit code is the only signal a script
    // piping this into a build receives, so it is a failure and no output
    // file is produced. That is a change from the best-effort export this
    // used to be.
    //
    // Outside the block above because `--load-overrides` is not the only
    // source: `App::new`'s own pass over the overrides the document
    // carries can refuse too, and that refusal makes the export just as
    // wrong.
    //
    // Spec 0198 S2 is not violated: it binds a *successful* batch
    // subcommand to a silent stderr, and this one is not successful.
    if !app.refusals.is_empty() {
        let origin = match load_overrides {
            Some(p) => format!("--load-overrides '{}'", p.display()),
            None => "override".to_string(),
        };
        for (_, refusal) in &app.refusals {
            eprintln!("error: {origin}: {refusal}");
        }
        return ExitCode::FAILURE;
    }

    // `--load-overrides` is already known to be present when this is
    // true — that pairing is checked before startup.
    let is_descriptor = format.is_some_and(ExtractFormatArg::is_descriptor);

    // A hash mismatch is a warning everywhere else, and stays one for the
    // two document formats: those export the document's own bytes, which
    // are what they are whatever the overrides file was written against.
    // The descriptor formats export a *schema* instead, assembled from
    // exactly those overrides — so a collection written against a
    // different blob or a different descriptor set produces a schema that
    // describes neither, and hands it to a tool with no way to notice.
    // Same reasoning, and same exit code, as the refusal check above: no
    // output file is produced.
    if is_descriptor && hash_mismatch {
        eprintln!(
            "error: --load-overrides: the overrides file was written against a \
             different blob or descriptor set, and --format=descriptor-binary/\
             descriptor-prototext exports a schema built from it"
        );
        return ExitCode::FAILURE;
    }

    let Some(idx) = app.resolve_path(path) else {
        eprintln!("error: export path '{path}' does not resolve");
        return ExitCode::FAILURE;
    };

    let bytes = if is_descriptor {
        app.set_cursor(idx);
        let as_prototext = format == Some(ExtractFormatArg::DescriptorPrototext);
        match app.export_descriptor_bytes(as_prototext) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let format = format
            .and_then(ExtractFormatArg::extract_format)
            .unwrap_or(extract::ExtractFormat::Text);
        app.extract_bytes(idx, format)
    };

    write_output(&bytes, output)
}

/// `--output <path>` if given, stdout otherwise.
fn write_output(bytes: &[u8], output: Option<&Path>) -> ExitCode {
    let written = match output {
        Some(out_path) => std::fs::write(out_path, bytes)
            .map_err(|e| format!("cannot write '{}': {e}", out_path.display())),
        None => write_stdout(bytes).map_err(|e| format!("writing to stdout: {e}")),
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_proto_root_keeps_an_explicit_value_regardless_of_the_fallback_dir() {
        let dir = std::env::temp_dir().join("protolens-resolve-proto-root-explicit");
        let explicit = PathBuf::from("/explicit/root");
        assert_eq!(
            resolve_proto_root(Some(explicit.clone()), Some(&dir.join("schema.desc"))),
            Some(explicit)
        );
    }

    #[test]
    fn resolve_proto_root_falls_back_to_the_stub_proto_dir_when_it_exists() {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("protolens-resolve-proto-root-ok-{n}"));
        let proto_dir = base.join("schema").join("proto");
        std::fs::create_dir_all(&proto_dir).unwrap();

        let result = resolve_proto_root(None, Some(&base.join("schema.desc")));

        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(result, Some(proto_dir));
    }

    /// Every unit boundary, and the reason the function is not one
    /// division: the sizes below a mebibyte all used to print `0 MB`, and
    /// a descriptor set of a few hundred kilobytes is an ordinary one.
    #[test]
    fn size_suffix_picks_a_unit_the_number_survives() {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("protolens-size-suffix-{n}"));
        std::fs::create_dir_all(&dir).unwrap();

        for (len, want) in [
            (0usize, " (0 bytes)"),
            (1, " (1 bytes)"),
            (1023, " (1023 bytes)"),
            (1024, " (1 KB)"),
            (1024 * 1024 - 1, " (1023 KB)"),
            (1024 * 1024, " (1 MB)"),
            (3 * 1024 * 1024 + 512 * 1024, " (3 MB)"),
        ] {
            let path = dir.join(format!("{len}.bin"));
            std::fs::write(&path, vec![0u8; len]).unwrap();
            assert_eq!(size_suffix(&path), want, "for {len} bytes");
        }

        // A path that is not there is decoration that says nothing,
        // never a `(0 bytes)` claim about a file nobody could read.
        let absent = size_suffix(&dir.join("absent.bin"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(absent, "");
    }

    #[test]
    fn resolve_proto_root_is_none_when_the_stub_proto_dir_is_missing() {
        let base = std::env::temp_dir().join("protolens-resolve-proto-root-missing");
        assert_eq!(
            resolve_proto_root(None, Some(&base.join("schema.desc"))),
            None
        );
    }

    #[test]
    fn resolve_proto_root_is_none_when_the_stub_proto_path_is_a_file() {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("protolens-resolve-proto-root-file-{n}"));
        let stub_dir = base.join("schema");
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::write(stub_dir.join("proto"), b"").unwrap();

        let result = resolve_proto_root(None, Some(&base.join("schema.desc")));

        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(result, None);
    }
}
