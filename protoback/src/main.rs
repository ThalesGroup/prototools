// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback: reproto in Rust, on upb (spec 0424). Step 4 (spec 0428):
//! loading, the pool, phases 1 to 6, and the graph dump.

mod affinity;
mod alloc;
mod cli;
mod complete;
mod console;
mod db;
mod descriptor_import;
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
mod moves;
mod pathmatch;
mod phase1;
mod phase2;
mod phase4;
mod phase5;
mod phase6;
mod plugin;
mod profile;
mod render;
mod report;
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

/// Spec 0447 S8, both tools' words.
const VARIANTS_GONE: &str =
    "--proto-variant: variants are gone (spec 0447); use --profile, --descriptor and --target";
const REPROTO_VARIANT_IGNORED: &str =
    "$REPROTO_VARIANT is ignored: variants are gone (spec 0447); use --profile";

/// A `-C` file's lists: reproto's `filter_config.load` (`seed:`,
/// `prune:`), and protoback's `move:` (spec 0445 S1). Its `variants:`
/// sections went with variants (spec 0447 S8): an unknown key.
#[derive(Default)]
struct FilterConfig {
    seeds: Vec<String>,
    prunes: Vec<String>,
    moves: Vec<moves::Move>,
}

/// The `-C` file.
fn filter_config(path: &Path) -> Result<FilterConfig, String> {
    let p = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{p}: {e}"))?;
    let raw: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{p}: invalid YAML: {e}"))?;
    let map = match raw {
        serde_yaml::Value::Null => serde_yaml::Mapping::new(),
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err(format!("{p}: filter-config must be a YAML mapping")),
    };
    let mut config = FilterConfig::default();
    lists(
        &map,
        &format!("{p}"),
        &["seed", "prune", "move"],
        &mut config,
    )?;
    Ok(config)
}

/// One mapping's `seed:`, `prune:` and `move:` into `out`, any key not in
/// `allowed` refused.
fn lists(
    map: &serde_yaml::Mapping,
    at: &str,
    allowed: &[&str],
    out: &mut FilterConfig,
) -> Result<(), String> {
    let mut unknown: Vec<String> = map
        .keys()
        .filter_map(|k| k.as_str())
        .filter(|k| !allowed.contains(k))
        .map(str::to_string)
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!(
            "{at}: unknown filter-config key(s): {}",
            unknown.join(", ")
        ));
    }
    let strings = |key: &str| -> Result<Vec<String>, String> {
        match map.get(key) {
            None | Some(serde_yaml::Value::Null) => Ok(Vec::new()),
            Some(serde_yaml::Value::Sequence(s)) => s
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or(()))
                .collect::<Result<_, _>>()
                .map_err(|()| format!("{at}: '{key}' must be a list of strings")),
            Some(_) => Err(format!("{at}: '{key}' must be a list of strings")),
        }
    };
    out.seeds.extend(strings("seed")?);
    out.prunes.extend(strings("prune")?);
    let bad = || format!("{at}: 'move' must be a list of OLD: TARGET or OLD: [TARGET, ...]");
    match map.get("move") {
        None | Some(serde_yaml::Value::Null) => {}
        Some(serde_yaml::Value::Sequence(entries)) => {
            for entry in entries {
                let serde_yaml::Value::Mapping(m) = entry else {
                    return Err(bad());
                };
                let mut it = m.iter();
                let (Some((old, target)), None) = (it.next(), it.next()) else {
                    return Err(bad());
                };
                let old = old.as_str().ok_or_else(bad)?.to_string();
                let targets: Vec<String> = match target {
                    serde_yaml::Value::String(t) => vec![t.clone()],
                    serde_yaml::Value::Sequence(ts) if !ts.is_empty() => ts
                        .iter()
                        .map(|t| t.as_str().map(str::to_string).ok_or_else(bad))
                        .collect::<Result<_, _>>()?,
                    _ => return Err(bad()),
                };
                out.moves.push(moves::Move { old, targets });
            }
        }
        Some(_) => return Err(bad()),
    }
    Ok(())
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

fn usage(message: &str) -> u8 {
    report::error(message);
    eprintln!(
        "Usage: protoback [OPTIONS] DESCRIPTOR_FILES\n\
         Try 'protoback --help' for help.\n\nError: {message}"
    );
    2
}

/// A failure that ends the run, said on stderr as `print` does, and kept
/// for the report (spec 0446 S1).
fn failed(message: &str, print: impl FnOnce(&str)) -> u8 {
    report::error(message);
    print(message);
    1
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
    // Matches kept: a profile fills in only what the command line left at
    // its default (spec 0447 S6).
    let matches = <Cli as clap::CommandFactory>::command().get_matches();
    let cli = match <Cli as clap::FromArgMatches>::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    let explicit = |id: &str| {
        matches!(
            matches.value_source(id),
            Some(clap::parser::ValueSource::CommandLine | clap::parser::ValueSource::EnvVariable)
        )
    };
    let explicit: std::collections::HashSet<&'static str> = [
        "descriptor",
        "target",
        "editions",
        "import_descriptor",
        "emit_descriptor",
        "no_emit_descriptor",
    ]
    .into_iter()
    .filter(|id| explicit(id))
    .collect();
    let code = protoback(cli, &explicit);
    report::finish(code);
    ExitCode::from(code)
}

/// The run: its exit status, or, at its end, the process ended without
/// freeing what it built (`exit_without_freeing`), the report written
/// first. Every exit goes through `report::finish` (spec 0446 S1).
fn protoback(mut cli: Cli, explicit: &std::collections::HashSet<&'static str>) -> u8 {
    console::init(cli.quiet);
    // --report-out: a previous report removed now, so that none outlives
    // this run, whatever ends it (spec 0446 S1).
    if let Some(path) = &cli.report_out {
        if let Err(e) = report::start(path) {
            return usage(&e);
        }
    }
    // --fail-on's codes, each one of spec 0438's (S4: else a usage error).
    if let Some(code) = cli.fail_on.iter().find(|c| !events::is_code(c)) {
        return usage(&format!(
            "Invalid value for '--fail-on': unknown warning code '{code}'"
        ));
    }
    // Spec 0447 S6: the profiles, under the command line: an option it
    // leaves at its default takes the profiles' value; lists accumulate.
    let mut profile_folders = Vec::new();
    if !cli.profile.is_empty() {
        let prof = match profile::load(&cli.profile) {
            Ok(p) => p,
            Err(e) => return usage(&e),
        };
        let unset = |id: &str| !explicit.contains(id);
        if let (Some(v), true) = (prof.strings.get("descriptor"), unset("descriptor")) {
            cli.descriptor = Some(v.clone());
        }
        if let (Some(v), true) = (prof.strings.get("target"), unset("target")) {
            cli.target = <cli::Target as clap::ValueEnum>::from_str(v, false).unwrap_or(cli.target);
        }
        if let (Some(v), true) = (prof.strings.get("editions"), unset("editions")) {
            cli.editions =
                <cli::Editions as clap::ValueEnum>::from_str(v, false).unwrap_or(cli.editions);
        }
        if let (Some(&v), true) = (
            prof.bools.get("import_descriptor"),
            unset("import_descriptor"),
        ) {
            cli.import_descriptor = v;
        }
        if let (Some(&v), true) = (
            prof.bools.get("emit_descriptor"),
            unset("emit_descriptor") && unset("no_emit_descriptor"),
        ) {
            cli.no_emit_descriptor = !v;
        }
        let mut builtin: Vec<UseVariant> = prof
            .lists
            .get("builtin")
            .into_iter()
            .flatten()
            .filter_map(|v| <UseVariant as clap::ValueEnum>::from_str(v, false).ok())
            .collect();
        builtin.append(&mut cli.builtin);
        cli.builtin = builtin;
        let mut fail_on: Vec<String> = prof.lists.get("fail_on").cloned().unwrap_or_default();
        if let Some(code) = fail_on.iter().find(|c| !events::is_code(c)) {
            return usage(&format!(
                "Invalid value for '--fail-on': unknown warning code '{code}'"
            ));
        }
        fail_on.append(&mut cli.fail_on);
        cli.fail_on = fail_on;
        profile_folders = prof.folders;
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

    let mut variant = match Variant::load(None) {
        Ok(v) => v,
        Err(e) => return usage(&e),
    };
    // Spec 0447 S1: --descriptor names the descriptor; else it is inferred
    // from the input, once loaded.
    if let Some(d) = &cli.descriptor {
        variant.descriptor_proto = d.clone();
    }
    let infer_descriptor = cli.descriptor.is_none();
    report::variant(&variant.name);
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
    if !cli.use_variant.is_empty() {
        events.warn("W18", "--use-variant is deprecated; use --builtin");
    }
    // Spec 0447 S8: variants are gone.
    if cli.keep_descriptor_path {
        events.warn(
            "W18",
            "--keep-descriptor-path is deprecated; use --target keep",
        );
    }
    if cli.proto_variant.is_some() {
        return usage(VARIANTS_GONE);
    }
    if std::env::var_os("REPROTO_VARIANT").is_some() && !cli.quiet {
        events.attention(REPROTO_VARIANT_IGNORED);
    }
    if cli.target != cli::Target::Keep && cli.keep_descriptor_path {
        let name = format!("{:?}", cli.target).to_lowercase();
        return usage(&format!(
            "--target {name} and --keep-descriptor-path (--target keep) are exclusive"
        ));
    }
    if let Err(e) = stub_check(&cli, out_repo.as_deref()) {
        return usage(&e);
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
    let mut config = FilterConfig::default();
    if let Some(path) = &cli.filter_config {
        match filter_config(path) {
            Ok(c) => config = c,
            Err(e) => return usage(&e),
        }
    }
    let mut all_moves = Vec::new();
    for value in &cli.moves {
        match moves::parse_cli(value) {
            Ok(m) => all_moves.push(m),
            Err(e) => return usage(&e),
        }
    }
    all_moves.extend(config.moves);
    let all_seeds: Vec<String> = cli.seeds.iter().cloned().chain(config.seeds).collect();
    let all_stumps: Vec<String> = cli.stumps.iter().cloned().chain(config.prunes).collect();
    let ((seed_fqdns, seed_paths), (prune_fqdns, prune_paths)) =
        match (partition(&all_seeds), partition(&all_stumps)) {
            (Ok(s), Ok(p)) => (s, p),
            (Err(e), _) | (_, Err(e)) => return usage(&e),
        };

    let mut blobs = HashMap::default();
    for root in &roots {
        if root.is_file() && load::blob_members(&mut blobs, root).is_empty() {
            return failed(
                &format!(
                    "Error: -I {} holds no FileDescriptorProto; a file given to -I is read as a blob of descriptors.",
                    root.display()
                ),
                |m| eprintln!("{m}"),
            );
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
        Err(message) => return failed(&message, |m| eprintln!("{m}")),
    };
    // The CPUs this process may use, counted before the main thread is
    // confined to the fast ones (`available_parallelism` follows the
    // mask); then the confinement, the plugin's process already started
    // on the whole mask (spec 0444 S3).
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    affinity::apply();
    // Rayon's global pool, which phases 1 and 2 use for their per-file
    // work (spec 0443 P8): --jobs threads, each first given every CPU, the
    // main thread's confinement being inherited.
    let jobs = cli.jobs.unwrap_or(cpus);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .start_handler(|_| affinity::widen())
        .build_global();

    // --builtin, and --use-variant, its deprecated spelling.
    let builtin: Vec<UseVariant> = cli
        .use_variant
        .iter()
        .chain(&cli.builtin)
        .copied()
        .collect();
    let fallback = fallback_protos(&builtin, &variant);
    // Spec 0447 S2: every other built-in copy fills in what the input
    // lacks, in the same order (leaves first), unless --no-builtin.
    let fill_in: Vec<String> = if cli.no_builtin {
        Vec::new()
    } else {
        fallback_protos(&[UseVariant::All], &variant)
            .into_iter()
            .filter(|p| !fallback.contains(p))
            .collect()
    };
    let moves = moves::Moves::new(all_moves, &prune_fqdns, &mut events);
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
        fill_in_protos: fill_in,
        infer_descriptor,
        profile_folders,
        target: cli.target,
        import_descriptor: cli.import_descriptor,
        descriptor_error: None,
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
        edition_names: Default::default(),
        visibility_defaults: Default::default(),
        seed_nodes: BTreeSet::new(),
        schema_db_extra_nodes: Vec::new(),
        plugin,
        moves,
    };
    // The graph dump's record starts with the run, as reproto's recorder.
    run.events.record();
    let mut topo = topology::Topology::default();
    // reproto's collector is flushed in a `finally`: before the CLI prints
    // an error the phases raised, after one they printed themselves.
    phase1::run(&mut run, &mut topo);
    if let Some(failure) = run.plugin.as_ref().and_then(plugin::Plugin::failure) {
        run.events.flush();
        return failed(&failure.to_string(), |m| eprintln!("{m}"));
    }
    if let Some(e) = run.descriptor_error.take() {
        run.events.flush();
        return failed(&format!("Error: {e}"), |m| eprintln!("{m}"));
    }
    // No file loaded: reproto's run stops here, successfully (`if not
    // topo.files: return None`), before descriptor.proto is looked for.
    if topo.files.is_empty() {
        run.events.flush();
        return finish(&run.events, &cli.fail_on);
    }
    if let Err(fatal) = phase2::run(&mut run, &mut topo) {
        if let Some(detail) = fatal.detail() {
            // reproto's `cli_error`, before the CLI's `Error:`.
            console::error(&detail);
        }
        run.events.flush();
        return failed(&format!("Error: {}", fatal.message()), |m| eprintln!("{m}"));
    }
    if let Some(failure) = run.plugin.as_ref().and_then(plugin::Plugin::failure) {
        run.events.flush();
        return failed(&failure.to_string(), |m| eprintln!("{m}"));
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
        return finish(&run.events, &cli.fail_on);
    }
    phase4::run(&mut run, &mut topo);
    phase5::run(&mut run, &topo);
    phase6::run(&mut run);

    if let Some(dump_path) = dump_path {
        let written = std::fs::write(&dump_path, dump::write(&run, &topo));
        run.events.flush();
        if let Err(e) = written {
            return failed(&format!("Error: {}: {e}", dump_path.display()), |m| {
                eprintln!("{m}")
            });
        }
        return finish(&run.events, &cli.fail_on);
    }

    // Spec 0437: two files rewritten onto one output path.
    let keep = cli.keep_descriptor_path;
    match render::resolve_rewrite_collisions(&mut run.graph, &run.variant, keep) {
        Ok(warnings) => warnings
            .iter()
            .for_each(|(code, w)| run.events.warn(code, w)),
        Err(errors) => {
            run.events.flush();
            return failed(&format!("Error: {}", errors.join("\n")), |m| {
                eprintln!("{m}")
            });
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
    // The setup rendering reads, built side by side when there is more
    // than one thread (spec 0443 P5): the pool loads then the options
    // pool, which reads them; the symbol table; each node's definition.
    let build_pools = || {
        let loads = render::pool_loads(&run.db, &loaded);
        let options = render::OptionsPool::new(&run.db, &run.variant, &loads);
        (loads, options)
    };
    let build_names = || {
        render::names::SymbolTable::build(
            run.db
                .files
                .iter()
                .map(|f| prototext_upb::descriptor_access::FileDescriptorProto(f.as_ref())),
            &render::names::Renaming {
                variant: &run.variant,
                keep_descriptor: cli.keep_descriptor_path,
            },
        )
    };
    let build_defs = || render::find_defs(&run.graph, &run.db);
    let ((loads, options), names, defs) = if jobs > 1 {
        std::thread::scope(|s| {
            let names = s.spawn(|| {
                affinity::widen();
                build_names()
            });
            let defs = s.spawn(|| {
                affinity::widen();
                build_defs()
            });
            let pools = build_pools();
            // A panic in either is the run's, as it would be on one thread.
            let names = names
                .join()
                .unwrap_or_else(|e| std::panic::resume_unwind(e));
            let defs = defs.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
            (pools, names, defs)
        })
    } else {
        (build_pools(), build_names(), build_defs())
    };
    let options = match options {
        Ok(options) => options,
        Err(reason) => {
            // The pool refused the variant's descriptor.proto: refused as a
            // corrupted one is, its reason under --debug (reproto's
            // `DescriptorProtoUnbuildableError`).
            if cli.debug {
                console::warning(&format!("'{}': {reason}", run.variant.descriptor_proto));
            }
            run.events.flush();
            return failed(
                "Error: descriptor.proto could not be built into a descriptor pool; \
                 it is not a valid descriptor.proto",
                |m| eprintln!("{m}"),
            );
        }
    };
    let shared = render::Shared {
        defs: &defs,
        graph: &run.graph,
        topo: &topo,
        variant: &run.variant,
        edition_defaults: &run.edition_defaults,
        edition_names: &run.edition_names,
        exports: None,
        options_fqns: render::options_fqns(&run.db, &run.variant),
        keep_variant_descriptor: cli.keep_descriptor_path,
        emit_binary: cli.emit_binary,
        emit_scoring_yaml: cli.emit_scoring_yaml || cli.emit_scoring_graphs,
        emit_extension_ranges: cli.emit_extension_ranges,
        source_info: !cli.no_source_info,
        force_proto2_output: cli.force_proto2_output,
        force_proto2_for_editions: cli.force_proto2_for_editions
            || cli.prost_workaround
            || cli.editions == cli::Editions::Proto2,
        redact_comments: cli.redact_comments,
        redact_orphans: cli.redact_orphans,
        loads: &loads,
        options: &options,
        schema_db: schema_db.is_some(),
        debug: cli.debug,
        names: &names,
    };
    // --export-used (spec 0458): what to export, read from the files to
    // render.
    let exports = cli
        .export_used
        .then(|| render::exports(&shared, &run.visibility_defaults));
    let shared = render::Shared {
        exports: exports.as_ref(),
        ..shared
    };
    let output = render::Output {
        out_repo,
        dry_run: cli.dry_run,
        write_variant_descriptor: !cli.no_emit_descriptor,
        jobs,
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
            report::error(&format!("Error: {e}"));
            console::error(&format!("Error: {e}"));
            console::error("Cannot continue due to I/O error.");
            run.events.flush();
            return 1;
        }
        Err(render::Failure::Fatal(message)) => {
            report::error(&message);
            console::error(&message);
            run.events.flush();
            return 1;
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
    report::finish(code);
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
    if let Some(p) = &proto_out {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(proto_out)
}

/// Spec 0155 G1: -O may not land inside --schema-db-out's stub directory
/// (reserved for hopcroft.rkyv/index.rkyv) except as that stub's immediate
/// 'proto' child. Checked where reproto checks it: after the deprecation
/// warnings, `-O` already created.
fn stub_check(cli: &Cli, proto_out: Option<&Path>) -> Result<(), String> {
    let schema_db = cli
        .build_schema_db
        .clone()
        .or(cli.build_schema_db_deprecated.clone());
    if let (Some(db), Some(out)) = (&schema_db, proto_out) {
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
    Ok(())
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
