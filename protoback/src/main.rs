// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback: reproto in Rust, on upb (spec 0424). Step 4 (spec 0428):
//! loading, the pool, phases 1 to 6, and the graph dump.

mod alloc;
mod cli;
mod complete;
mod console;
mod db;
mod dump;
mod events;
mod features;
mod features_dump;
mod fuzz;
mod graph;
mod hash;
mod instantiate;
mod load;
mod man;
mod pathmatch;
mod phase1;
mod phase2;
mod phase4;
mod phase5;
mod phase6;
mod plugin;
mod render;
mod run;
mod schema_db;
mod scoring;
mod select;
mod topology;
mod variant;

use crate::hash::HashMap;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, UseVariant};
use events::Events;
use pathmatch::PathPatterns;
use run::Run;
use variant::Variant;

const FQDN_PREFIXES: &[&str] = &["file", "desc", "enum", "serv", "meth", "fdsc"];

/// reproto's `_normalise_fqdn`: a known prefix, and, except for `file:`,
/// `.` written `/` without a leading one.
fn normalise_fqdn(s: &str) -> Result<String, String> {
    let Some((prefix, rest)) = s.split_once(':') else {
        return Err(format!(
            "Invalid FQDN '{s}': must start with a prefix ({}).",
            FQDN_PREFIXES.join(", ")
        ));
    };
    if !FQDN_PREFIXES.contains(&prefix) {
        return Err(format!(
            "Invalid FQDN prefix '{prefix}' in '{s}': must be one of {}.",
            FQDN_PREFIXES.join(", ")
        ));
    }
    if prefix == "file" {
        return Ok(s.to_string());
    }
    Ok(format!(
        "{prefix}:{}",
        rest.replace('.', "/").trim_start_matches('/')
    ))
}

/// reproto's `_partition`: FQDN patterns (normalised) and path patterns.
fn partition(values: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut fqdns, mut paths) = (Vec::new(), Vec::new());
    for v in values {
        match v.split_once(':') {
            Some(("path", rest)) => paths.push(rest.to_string()),
            Some((prefix, _)) if FQDN_PREFIXES.contains(&prefix) => fqdns.push(normalise_fqdn(v)?),
            Some((prefix, _)) => {
                let mut valid: Vec<&str> = FQDN_PREFIXES.to_vec();
                valid.push("path");
                return Err(format!(
                    "'{v}': '{prefix}:' is not a recognized prefix ({}); if this is meant to be a \
                     filesystem path, use the 'path:' prefix",
                    valid.join(", ")
                ));
            }
            None => paths.push(v.clone()),
        }
    }
    Ok((fqdns, paths))
}

/// reproto's `filter_config.load`: the seed and prune lists of the file.
fn filter_config(path: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let p = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{p}: {e}"))?;
    let raw: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{p}: invalid YAML: {e}"))?;
    let map = match raw {
        serde_yaml::Value::Null => serde_yaml::Mapping::new(),
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err(format!("{p}: filter-config must be a YAML mapping")),
    };
    let mut unknown: Vec<String> = map
        .keys()
        .filter_map(|k| k.as_str())
        .filter(|k| *k != "seed" && *k != "prune")
        .map(str::to_string)
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!(
            "{p}: unknown filter-config key(s): {}",
            unknown.join(", ")
        ));
    }
    let list = |key: &str| -> Result<Vec<String>, String> {
        match map.get(key) {
            None | Some(serde_yaml::Value::Null) => Ok(Vec::new()),
            Some(serde_yaml::Value::Sequence(s)) => s
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or(()))
                .collect::<Result<_, _>>()
                .map_err(|()| format!("{p}: '{key}' must be a list of strings")),
            Some(_) => Err(format!("{p}: '{key}' must be a list of strings")),
        }
    };
    Ok((list("seed")?, list("prune")?))
}

/// The fallbacks `--use-variant` asks for, in reproto's order.
fn fallback_protos(use_variant: &[UseVariant], variant: &Variant) -> Vec<String> {
    let wanted = |u: UseVariant| use_variant.contains(&UseVariant::All) || use_variant.contains(&u);
    let mut out = Vec::new();
    if wanted(UseVariant::Descriptor) {
        out.push(variant.descriptor_proto.clone());
    }
    for &u in UseVariant::WELL_KNOWN {
        if wanted(u) {
            out.push(variant.well_known(&format!("google/protobuf/{}.proto", u.name())));
        }
    }
    out
}

fn usage(message: &str) -> ExitCode {
    eprintln!(
        "Usage: protoback [OPTIONS] DESCRIPTOR_FILES\n\
         Try 'protoback --help' for help.\n\nError: {message}"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    // `protoback instantiate`, reproto-instantiate-schema (spec 0432 S1):
    // a subcommand of its own options. A descriptor file named
    // `instantiate` is given as `./instantiate`.
    // PROTOBACK_COMPLETE=<shell>: the completion script, or the candidates
    // (spec 0433 S7); bash completes from the raw line (spec 0406).
    clap_complete::CompleteEnv::with_factory(<Cli as clap::CommandFactory>::command)
        .var("PROTOBACK_COMPLETE")
        .shells(prototools_complete::shells())
        .complete();
    // PROTOBACK_GEN_MAN=<dir>: the man pages (spec 0433 S7).
    man::generate_and_exit_if_requested();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("instantiate") {
        let mut sub = vec!["protoback instantiate".to_string()];
        sub.extend(args.into_iter().skip(2));
        return instantiate::main(sub);
    }
    let cli = Cli::parse();
    console::init(cli.quiet);
    // --fail-on's codes, each one of spec 0438's (S4: else a usage error).
    if let Some(code) = cli.fail_on.iter().find(|c| !events::is_code(c)) {
        return usage(&format!(
            "Invalid value for '--fail-on': unknown warning code '{code}'"
        ));
    }
    // First: every warning from here on is counted (spec 0438 S4); the
    // deprecated options, in reproto's order.
    let mut events = Events::new(cli.detailed_warnings);
    let deprecated: [(bool, &str); 3] = [
        (
            cli.output_root_deprecated.is_some(),
            "--output-root is deprecated; use --proto-out",
        ),
        (
            cli.build_schema_db_deprecated.is_some(),
            "--build-schema-db is deprecated; use --schema-db-out",
        ),
        (
            cli.emit_scoring_html_deprecated.is_some(),
            "--emit-scoring-html is deprecated; use --scoring-html-out",
        ),
    ];
    for (_, message) in deprecated.iter().filter(|(set, _)| *set) {
        events.warn("W18", message);
    }
    let dump_path = cli.dump_graph.clone();
    let out_repo = match proto_out(&cli) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };

    let mut roots: Vec<PathBuf> = cli
        .pb_path
        .iter()
        .chain(&cli.pb_path_deprecated)
        .cloned()
        .collect();
    for root in &roots {
        if !root.exists() {
            return usage(&format!(
                "Invalid value for '-I' / '--desc-root': Path '{}' does not exist.",
                root.display()
            ));
        }
    }
    if roots.is_empty() {
        roots.push(PathBuf::from("."));
    }
    let pb_files = if cli.pb_files.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.pb_files.clone()
    };

    let variant = match Variant::load(cli.proto_variant.as_deref()) {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    for key in &variant.unread_keys {
        events.warn(
            "W17",
            &format!(
                "variant '{}': key '{key}' is not read; it has no effect",
                variant.name
            ),
        );
    }
    // The plugin options: one at most (spec 0369 S3, spec 0431).
    if cli.fdp_plugin.is_some() && cli.phase2_plugin_deprecated.is_some() {
        return usage(
            "--fdp-plugin and --phase2-plugin are mutually exclusive; \
             --phase2-plugin is deprecated, use --fdp-plugin alone",
        );
    }
    if cli.fdp_plugin_cmd.is_some()
        && (cli.fdp_plugin.is_some() || cli.phase2_plugin_deprecated.is_some())
    {
        return usage("--fdp-plugin-cmd and --fdp-plugin are mutually exclusive");
    }
    if cli.phase2_plugin_deprecated.is_some() {
        events.warn(
            "W18",
            "--phase2-plugin is deprecated; use --fdp-plugin \
             (protoback calls phase2_plugin(ctx, fdp) with ctx None)",
        );
    }
    if cli.emit_scoring_graphs {
        events.warn(
            "W18",
            "--emit-scoring-graphs is deprecated; use --emit-scoring-yaml",
        );
    }
    if cli.emit_pyvis.is_some() {
        events.warn("W18", "--emit-pyvis is deprecated; use --scoring-html-out");
    }
    // reproto's `--hide` check: `desc:` patterns only, normalised as
    // seeds and prunes are. show.py then matches them against bare FQDNs
    // (spec 0430 S8).
    let mut hide = Vec::new();
    for h in &cli.pyvis_hide {
        if h.split_once(':').map_or("", |(p, _)| p) != "desc" {
            return usage(&format!(
                "--hide only accepts desc: patterns (got {})",
                render::pyfmt::str_repr(h)
            ));
        }
        match normalise_fqdn(h) {
            Ok(n) => hide.push(n),
            Err(e) => return usage(&e),
        }
    }
    let (mut config_seeds, mut config_prunes) = (Vec::new(), Vec::new());
    if let Some(path) = &cli.filter_config {
        match filter_config(path) {
            Ok((s, p)) => (config_seeds, config_prunes) = (s, p),
            Err(e) => return usage(&e),
        }
    }
    let all_seeds: Vec<String> = cli.seeds.iter().cloned().chain(config_seeds).collect();
    let all_stumps: Vec<String> = cli.stumps.iter().cloned().chain(config_prunes).collect();
    let ((seed_fqdns, seed_paths), (prune_fqdns, prune_paths)) =
        match (partition(&all_seeds), partition(&all_stumps)) {
            (Ok(s), Ok(p)) => (s, p),
            (Err(e), _) | (_, Err(e)) => return usage(&e),
        };

    let mut blobs = HashMap::default();
    for root in &roots {
        if root.is_file() && load::blob_members(&mut blobs, root).is_empty() {
            eprintln!(
                "Error: -I {} holds no FileDescriptorProto; a file given to -I is read as a blob of descriptors.",
                root.display()
            );
            return ExitCode::from(1);
        }
    }

    // The plugin, started once and ready before any file is read (spec
    // 0431 S2).
    let source = match (
        &cli.fdp_plugin,
        &cli.phase2_plugin_deprecated,
        &cli.fdp_plugin_cmd,
    ) {
        (Some(file), _, _) => Some(plugin::Source::Python {
            file: file.display().to_string(),
            legacy: false,
        }),
        (_, Some(file), _) => Some(plugin::Source::Python {
            file: file.display().to_string(),
            legacy: true,
        }),
        (_, _, Some(cmd)) => Some(plugin::Source::Command(cmd.clone())),
        _ => None,
    };
    let plugin = match source.map(plugin::Plugin::start).transpose() {
        Ok(p) => p,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(1);
        }
    };

    let fallback = fallback_protos(&cli.use_variant, &variant);
    let mut run = Run {
        roots,
        pb_files,
        prunings: prune_fqdns.clone(),
        pruned_fqdns: prune_fqdns.into_iter().collect(),
        pruned_file_names: BTreeSet::new(),
        pruned_paths: PathPatterns::new(&prune_paths),
        seed_fqdns,
        path_seeds: PathPatterns::new(&seed_paths),
        path_seed_fqdns: BTreeSet::new(),
        fallback_protos: fallback,
        variant,
        keep_duplicates: cli.keep_duplicates,
        build_schema_db: cli.build_schema_db.is_some() || cli.build_schema_db_deprecated.is_some(),
        go_root: cli.go_root.clone(),
        quiet: cli.quiet,
        debug: cli.debug,
        debug_fqdn: cli.debug_fqdn,
        events,
        blobs,
        db: db::Database::default(),
        graph: graph::Graph::default(),
        edition_defaults: Default::default(),
        seed_nodes: BTreeSet::new(),
        schema_db_extra_nodes: Vec::new(),
        plugin,
    };
    // The graph dump's record starts with the run, as reproto's recorder.
    run.events.record();
    let mut topo = topology::Topology::default();
    // reproto's collector is flushed in a `finally`: before the CLI prints
    // an error the phases raised, after one they printed themselves.
    phase1::run(&mut run, &mut topo);
    if let Some(failure) = run.plugin.as_ref().and_then(plugin::Plugin::failure) {
        run.events.flush();
        eprintln!("{failure}");
        return ExitCode::from(1);
    }
    // No file loaded: reproto's run stops here, successfully (`if not
    // topo.files: return None`), before descriptor.proto is looked for.
    if topo.files.is_empty() {
        run.events.flush();
        return ExitCode::from(finish(&run.events, &cli.fail_on));
    }
    if let Err(fatal) = phase2::run(&mut run, &mut topo) {
        if let Some(detail) = fatal.detail() {
            // reproto's `cli_error`, before the CLI's `Error:`.
            console::error(&detail);
        }
        run.events.flush();
        eprintln!("Error: {}", fatal.message());
        return ExitCode::from(1);
    }
    if let Some(failure) = run.plugin.as_ref().and_then(plugin::Plugin::failure) {
        run.events.flush();
        eprintln!("{failure}");
        return ExitCode::from(1);
    }
    let phase = console::phase(
        "Building type graph",
        console::Label::Always,
        if cli.quiet {
            0
        } else {
            topo.files.len() as u64
        },
    );
    run.graph = graph::run(
        &topo,
        &run.db,
        cli.force_proto2_output,
        &run.edition_defaults,
    );
    phase.done();
    // --dump-resolved-features: the file's resolved features, then the run
    // stops (reproto's `_dump_resolved_features_yaml`, spec 0433 G4).
    if let Some(name) = &cli.dump_resolved_features {
        if let Err(warning) = features_dump::print(&run, name) {
            run.events.warn("W14", &warning);
        }
        run.events.flush();
        return ExitCode::from(finish(&run.events, &cli.fail_on));
    }
    phase4::run(&mut run, &mut topo);
    phase5::run(&mut run, &topo);
    phase6::run(&mut run);

    if let Some(dump_path) = dump_path {
        let written = std::fs::write(&dump_path, dump::write(&run, &topo));
        run.events.flush();
        if let Err(e) = written {
            eprintln!("Error: {}: {e}", dump_path.display());
            return ExitCode::from(1);
        }
        return ExitCode::from(finish(&run.events, &cli.fail_on));
    }

    // Spec 0437: two files rewritten onto one output path.
    let keep = cli.keep_descriptor_path;
    match render::resolve_rewrite_collisions(&mut run.graph, &run.variant, keep) {
        Ok(warnings) => warnings.iter().for_each(|w| run.events.warn("W13", w)),
        Err(errors) => {
            run.events.flush();
            eprintln!("Error: {}", errors.join("\n"));
            return ExitCode::from(1);
        }
    }

    let schema_db = cli
        .build_schema_db
        .clone()
        .or(cli.build_schema_db_deprecated.clone());
    // reproto's pool loads the summoned files as phase 7 renders them,
    // then the database's WKT dependencies as it renders those (spec 0430
    // S1, S4).
    let mut loaded = render::summoned_files(&run.graph);
    if schema_db.is_some() {
        loaded.extend(run.schema_db_extra_nodes.iter().map(|&i| {
            run.graph.nodes[i]
                .fqdn
                .strip_prefix("file:")
                .unwrap_or_default()
                .to_string()
        }));
    }
    let loads = render::pool_loads(&run.db, &loaded);
    let options = match render::OptionsPool::new(&run.db, &run.variant, &loads) {
        Ok(options) => options,
        Err(reason) => {
            // The pool refused the variant's descriptor.proto: refused as a
            // corrupted one is, its reason under --debug (reproto's
            // `DescriptorProtoUnbuildableError`).
            if cli.debug {
                console::warning(&format!("'{}': {reason}", run.variant.descriptor_proto));
            }
            run.events.flush();
            eprintln!(
                "Error: descriptor.proto could not be built into a descriptor pool; \
                 it is not a valid descriptor.proto"
            );
            return ExitCode::from(1);
        }
    };
    let names = render::names::SymbolTable::build(
        run.db
            .files
            .iter()
            .map(|f| prototext_upb::descriptor_access::FileDescriptorProto(f.as_ref())),
        &render::names::Renaming {
            variant: &run.variant,
            keep_descriptor: cli.keep_descriptor_path,
        },
    );
    let defs = render::find_defs(&run.graph, &run.db);
    let shared = render::Shared {
        defs: &defs,
        graph: &run.graph,
        topo: &topo,
        variant: &run.variant,
        edition_defaults: &run.edition_defaults,
        options_fqns: render::options_fqns(&run.db, &run.variant),
        keep_variant_descriptor: cli.keep_descriptor_path,
        emit_binary: cli.emit_binary,
        emit_scoring_yaml: cli.emit_scoring_yaml || cli.emit_scoring_graphs,
        emit_extension_ranges: cli.emit_extension_ranges,
        source_info: !cli.no_source_info,
        force_proto2_output: cli.force_proto2_output,
        force_proto2_for_editions: cli.force_proto2_for_editions || cli.prost_workaround,
        redact_comments: cli.redact_comments,
        redact_orphans: cli.redact_orphans,
        loads: &loads,
        options: &options,
        schema_db: schema_db.is_some(),
        debug: cli.debug,
        names: &names,
    };
    let output = render::Output {
        out_repo,
        dry_run: cli.dry_run,
        write_variant_descriptor: !cli.no_emit_descriptor,
        jobs: cli.jobs.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
        }),
        schema_db,
        quiet: cli.quiet,
        extra_nodes: run.schema_db_extra_nodes.clone(),
        scoring_html: cli
            .emit_scoring_html
            .clone()
            .or(cli.emit_scoring_html_deprecated.clone())
            .or(cli.emit_pyvis.clone()),
        with_leaf_nodes: cli.with_leaf_nodes,
        hide,
    };
    let outcome = render::run(&shared, &output, &mut run.events);
    match outcome {
        Ok(()) => {}
        Err(render::Failure::Io(e)) => {
            console::error(&format!("Error: {e}"));
            console::error("Cannot continue due to I/O error.");
            run.events.flush();
            return ExitCode::from(1);
        }
        Err(render::Failure::Fatal(message)) => {
            console::error(&message);
            run.events.flush();
            return ExitCode::from(1);
        }
    }
    run.events.flush();
    let code = finish(&run.events, &cli.fail_on);
    // The plugin's process ended and reaped (its stdin closed), and the
    // standard streams flushed; then the process exits without freeing
    // what it built, which the system takes back at once: the symbol
    // table, the graph and the run were ~0.9 s of frees on googleapis
    // (spec 0443 P2). Every file was written and closed by then; no other
    // destructor has an effect outside the process.
    drop(run.plugin.take());
    exit_without_freeing(code)
}

/// `code`, without running the destructors of what `main` holds: every
/// output written, the streams flushed first.
fn exit_without_freeing(code: u8) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(i32::from(code))
}

/// A run done, its output written: exit status 3 when a `--fail-on` code
/// occurred, after saying which (spec 0438 S4), `-q` or not.
fn finish(events: &Events, fail_on: &[String]) -> u8 {
    match events.failed_on(fail_on) {
        Some(failed) => {
            eprintln!("Error: --fail-on: {failed}");
            3
        }
        None => 0,
    }
}

/// reproto's output root: `-O` (or its deprecated spelling), or, with
/// `--schema-db-out` and no `--no-proto-out`, that file's stem's `proto`
/// child; required unless nothing is written. Created at once, as reproto
/// creates it before phase 1.
fn proto_out(cli: &Cli) -> Result<Option<PathBuf>, String> {
    let mut proto_out = cli.proto_out.clone().or(cli.output_root_deprecated.clone());
    if cli.no_proto_out && proto_out.is_some() {
        return Err("--no-proto-out contradicts '-O' / '--proto-out'; drop one.".into());
    }
    let schema_db = cli
        .build_schema_db
        .clone()
        .or(cli.build_schema_db_deprecated.clone());
    if proto_out.is_none() && !cli.no_proto_out {
        if let Some(db) = &schema_db {
            proto_out = Some(db.with_extension("").join("proto"));
        }
    }
    let output_only = cli.no_proto_out
        || cli.dry_run
        || cli.emit_scoring_html.is_some()
        || cli.emit_scoring_html_deprecated.is_some()
        || cli.dump_resolved_features.is_some()
        || cli.dump_graph.is_some();
    if proto_out.is_none() && !output_only {
        return Err("Missing option '-O' / '--proto-out'.\n\
             Required when generating .proto output.  Alternatives:\n  \
             - Add -O DIR                   to write .proto files to DIR\n  \
             - Add --schema-db-out FILE     to write a schema DB, and .proto\n                                 \
             files under FILE's 'proto' child\n  \
             - Add --no-proto-out           to write no .proto files\n  \
             - Add --scoring-html-out FILE  to write HTML graphs only\n  \
             - Add --dry-run                to run without writing any files"
            .into());
    }
    // Spec 0155 G1: -O may not land inside --schema-db-out's stub directory
    // (reserved for hopcroft.rkyv/index.rkyv) except as that stub's
    // immediate 'proto' child.
    if let (Some(db), Some(out)) = (&schema_db, &proto_out) {
        let stub_dir = resolve(&db.with_extension(""));
        if let Ok(rel) = resolve(out).strip_prefix(&stub_dir) {
            if rel != Path::new("proto") {
                return Err(format!(
                    "-O/--proto-out ({}) is inside the --schema-db-out stub directory \
                     ({}); only an immediate 'proto' child directory is allowed there \
                     \u{2014} it otherwise holds hopcroft.rkyv/index.rkyv.",
                    py_str(out),
                    stub_dir.display()
                ));
            }
        }
    }
    if let Some(p) = &proto_out {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(proto_out)
}

/// Python's `Path.resolve()` (non-strict): absolute, the longest existing
/// prefix with its links resolved, the rest appended, `.` and `..` folded.
fn resolve(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out = PathBuf::new();
    let mut existing = true;
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => {
                out.push(c);
                if existing {
                    match out.canonicalize() {
                        Ok(real) => out = real,
                        Err(_) => existing = false,
                    }
                }
            }
        }
    }
    out
}

/// `str(Path(p))`: `.` components and a trailing `/` dropped.
fn py_str(p: &Path) -> String {
    let s: PathBuf = p.components().filter(|c| *c != Component::CurDir).collect();
    if s.as_os_str().is_empty() {
        ".".into()
    } else {
        s.display().to_string()
    }
}
