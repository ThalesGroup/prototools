// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

mod colorize;
mod complete;
mod decode;
mod export_descriptor;
mod extract;
mod override_pane;
mod render_cache;
mod theme;
mod tui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};
use clap_complete::engine::ArgValueCompleter;
use clap_complete::CompleteEnv;
use prototext_core::{render_as_bytes, RenderOpts};

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
        env = "PROTOTEXT_DESCRIPTOR_SET",
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
        add = ArgValueCompleter::new(complete::complete_type_names),
    )]
    r#type: Option<String>,

    /// Number of spaces per nesting level in the rendered text.
    #[arg(long = "indent", default_value_t = 2)]
    indent: usize,

    /// Syntax-highlighting color palette (spec 0116 §9). `system` probes
    /// the terminal's actual background once at startup and resolves to
    /// `dark` or `light`.
    #[arg(long = "theme", value_enum, default_value = "system")]
    theme: theme::ThemeKind,

    /// Maximum number of fields/spans a single live-preview override
    /// candidate decode may produce before truncating (spec 0163) —
    /// guards against a structurally mismatched candidate type making
    /// the recursive-descent decoder mis-parse arbitrary bytes into a
    /// pathologically large synthetic tree while the user is just
    /// browsing candidates. A *confirmed* override is always rendered
    /// completely, regardless of this value.
    #[arg(
        long = "override-preview-node-budget",
        default_value_t = tui::App::OVERRIDE_SPLICE_NODE_BUDGET_DEFAULT,
    )]
    override_preview_node_budget: usize,

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

impl From<ExtractFormatArg> for extract::ExtractFormat {
    fn from(f: ExtractFormatArg) -> Self {
        match f {
            ExtractFormatArg::Prototext => extract::ExtractFormat::Text,
            ExtractFormatArg::Binary => extract::ExtractFormat::Binary,
            ExtractFormatArg::DescriptorBinary | ExtractFormatArg::DescriptorPrototext => {
                unreachable!("descriptor formats are routed to export_descriptor, not extract")
            }
        }
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

fn main() -> ExitCode {
    // Dynamic shell completion — same model as Cargo/prototext. When
    // PROTOLENS_COMPLETE=<shell> is set, print the completion script and
    // exit.
    CompleteEnv::with_factory(Cli::command)
        .var("PROTOLENS_COMPLETE")
        .complete();

    let cli = Cli::parse();

    let descriptor_set = cli.descriptor_set.as_deref();
    if descriptor_set.is_none() && cli.r#type.is_some() {
        eprintln!("error: --type requires --descriptor-set");
        return ExitCode::FAILURE;
    }

    let blob = match std::fs::read(&cli.blob) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read '{}': {e}", cli.blob.display());
            return ExitCode::FAILURE;
        }
    };

    // Accept a `#@ prototext` text blob transparently: convert it to raw
    // binary wire bytes first, same as `prototext`'s own
    // `read_descriptor_file` (prototext/src/run.rs). Binary input is
    // returned unchanged (pass-through).
    let blob = match render_as_bytes(&blob, RenderOpts::default()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: '{}': {e}", cli.blob.display());
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
                // Best effort (spec 0151 G7): a metadata read failure simply
                // omits the size suffix rather than aborting or erroring, since
                // this is pure UX decoration, not load-bearing for correctness.
                let size_suffix = |p: &Path| -> String {
                    match std::fs::metadata(p) {
                        Ok(m) => format!(" ({} MB)", m.len() / (1024 * 1024)),
                        Err(_) => String::new(),
                    }
                };
                // Mirrors the sibling-graph check `DescriptorContext::load`
                // itself performs (`decode.rs:87-89`).
                let rkyv_path = descriptor_set.with_extension("").join("hopcroft.rkyv");
                if rkyv_path.exists() {
                    eprintln!(
                        "protolens: loading descriptor set '{name}'{} and scoring graph{}...",
                        size_suffix(descriptor_set),
                        size_suffix(&rkyv_path)
                    );
                } else {
                    eprintln!(
                        "protolens: loading descriptor set '{name}'{}...",
                        size_suffix(descriptor_set)
                    );
                }
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

    // Root-type inference (`score_all` over the whole blob) is deferred to
    // the TUI's background worker when entering the TUI without an
    // explicit `--type` (spec NNNN) — the batch `Extract` path has no
    // event loop to hand it off to, so it always resolves synchronously,
    // same as an explicit `--type`.
    let defer_root_type = cli.command.is_none() && cli.r#type.is_none();
    if cli.command.is_none() && ctx.graph.is_some() && !defer_root_type {
        eprintln!("protolens: resolving root type...");
    }

    let decoded = match decode::decode(
        &blob,
        &mut ctx,
        cli.r#type.as_deref(),
        cli.indent,
        defer_root_type,
    ) {
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
    app.override_splice_node_budget = cli.override_preview_node_budget;

    match cli.command {
        Some(Command::Export {
            path,
            load_overrides,
            format,
            output,
        }) => {
            if let Some(overrides_path) = &load_overrides {
                match app.load_overrides(&overrides_path.to_string_lossy()) {
                    Ok(warnings) => {
                        for w in warnings {
                            eprintln!("warning: --load-overrides: {w}");
                        }
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

            let is_descriptor = matches!(
                format,
                Some(ExtractFormatArg::DescriptorBinary)
                    | Some(ExtractFormatArg::DescriptorPrototext)
            );
            if is_descriptor && load_overrides.is_none() {
                eprintln!(
                    "error: --format=descriptor-binary/descriptor-prototext requires \
                     --load-overrides (batch mode has no interactive type-inference loop)"
                );
                return ExitCode::FAILURE;
            }

            let Some(idx) = app.resolve_path(&path) else {
                eprintln!("error: export path '{path}' does not resolve");
                return ExitCode::FAILURE;
            };

            if is_descriptor {
                app.set_cursor(idx);
                let as_prototext = format == Some(ExtractFormatArg::DescriptorPrototext);
                let bytes = match app.export_descriptor_bytes(as_prototext) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("error: {e}");
                        return ExitCode::FAILURE;
                    }
                };
                match output {
                    Some(out_path) => {
                        if let Err(e) = std::fs::write(&out_path, &bytes) {
                            eprintln!("error: cannot write '{}': {e}", out_path.display());
                            return ExitCode::FAILURE;
                        }
                    }
                    None => {
                        use std::io::Write as _;
                        if let Err(e) = std::io::stdout().write_all(&bytes) {
                            eprintln!("error: writing to stdout: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                return ExitCode::SUCCESS;
            }

            let format = format
                .map(Into::into)
                .unwrap_or(extract::ExtractFormat::Text);

            let bytes = app.extract_bytes(idx, format);
            match output {
                Some(out_path) => {
                    if let Err(e) = std::fs::write(&out_path, &bytes) {
                        eprintln!("error: cannot write '{}': {e}", out_path.display());
                        return ExitCode::FAILURE;
                    }
                }
                None => {
                    use std::io::Write as _;
                    if let Err(e) = std::io::stdout().write_all(&bytes) {
                        eprintln!("error: writing to stdout: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            ExitCode::SUCCESS
        }
        None => {
            if let Err(e) = tui::run(&mut app) {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
    }
}
