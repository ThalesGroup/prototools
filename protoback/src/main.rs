// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! protoback: reproto in Rust, on upb (spec 0424). Step 4 (spec 0428):
//! loading, the pool, phases 1 to 6, and the graph dump.

mod cli;
mod db;
mod dump;
mod events;
mod graph;
mod load;
mod pathmatch;
mod phase1;
mod phase2;
mod run;
mod topology;
mod variant;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
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
    eprintln!("Usage: protoback [OPTIONS] [DESCRIPTOR_FILES]...\n\nError: {message}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(dump_path) = cli.dump_graph.clone() else {
        return usage("protoback renders nothing yet (spec 0424 step 5): run it with --dump-graph");
    };
    let _ = cli.later_options();

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
    let (mut config_seeds, mut config_prunes) = (Vec::new(), Vec::new());
    if let Some(path) = &cli.filter_config {
        match filter_config(path) {
            Ok((s, p)) => (config_seeds, config_prunes) = (s, p),
            Err(e) => return usage(&e),
        }
    }
    let all_seeds: Vec<String> = cli.seeds.iter().cloned().chain(config_seeds).collect();
    let all_stumps: Vec<String> = cli.stumps.iter().cloned().chain(config_prunes).collect();
    // FQDN seeds are validated here; phase 5 reads them (milestone 4).
    let ((_, seed_paths), (prune_fqdns, prune_paths)) =
        match (partition(&all_seeds), partition(&all_stumps)) {
            (Ok(s), Ok(p)) => (s, p),
            (Err(e), _) | (_, Err(e)) => return usage(&e),
        };

    let mut blobs = HashMap::new();
    for root in &roots {
        if root.is_file() && load::blob_members(&mut blobs, root).is_empty() {
            eprintln!(
                "Error: -I {} holds no FileDescriptorProto; a file given to -I is read as a blob of descriptors.",
                root.display()
            );
            return ExitCode::from(1);
        }
    }

    let fallback = fallback_protos(&cli.use_variant, &variant);
    let mut run = Run {
        roots,
        pb_files,
        prunings: prune_fqdns.clone(),
        pruned_fqdns: prune_fqdns.into_iter().collect(),
        pruned_file_names: BTreeSet::new(),
        pruned_paths: PathPatterns::new(&prune_paths),
        path_seeds: PathPatterns::new(&seed_paths),
        path_seed_fqdns: BTreeSet::new(),
        fallback_protos: fallback,
        variant,
        keep_duplicates: cli.keep_duplicates,
        go_root: cli.go_root.clone(),
        quiet: cli.quiet,
        debug: cli.debug,
        debug_fqdn: cli.debug_fqdn,
        events: Events::default(),
        blobs,
        db: db::Database::default(),
        graph: graph::Graph::default(),
        edition_defaults: Default::default(),
    };
    let mut topo = topology::Topology::default();
    phase1::run(&mut run, &mut topo);
    if let Err(fatal) = phase2::run(&mut run, &mut topo) {
        if let Some(detail) = fatal.detail() {
            eprintln!("{detail}");
        }
        eprintln!("Error: {}", fatal.message());
        return ExitCode::from(1);
    }
    run.graph = graph::run(
        &topo,
        &run.db,
        cli.force_proto2_output,
        &run.edition_defaults,
    );

    if let Err(e) = std::fs::write(&dump_path, dump::write(&run, &topo)) {
        eprintln!("Error: {}: {e}", dump_path.display());
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
