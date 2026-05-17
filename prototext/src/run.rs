// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use prototext_core::{
    decode_pool, render_as_bytes, render_as_text, schema_from_pool, CodecError, RenderOpts,
};
use score_graph_lib::score::{
    load::{load_graph, LoadedGraph},
    score_all,
};

use crate::inputs::{expand_path, InputFile};
use crate::lazy_pool::LazyPool;
use prototext_core::instantiate::{generate_message_bytes, InstantiateOpts};

#[cfg(feature = "wkt-db")]
use crate::WKT_GRAPH;
use crate::{Cli, Command, EMBEDDED_DESCRIPTOR};

// ── DescriptorContext ─────────────────────────────────────────────────────────

/// Result of resolving `--descriptor`: a pool for type lookup plus an optional
/// Hopcroft scoring graph.
///
/// When a `<stem>/index.rkyv` sidecar is present, `lazy` holds a `LazyPool`
/// that mmaps the FDS and decodes FDPs on demand; `pool` is `None`.  On the
/// eager path (no sidecar) `pool` holds the fully-decoded pool and `lazy` is
/// `None`.  Always use the `pool()` / `pool_mut()` accessors.
pub struct DescriptorContext {
    /// Populated only on the eager path (no index.rkyv sidecar).
    pool: Option<prost_reflect::DescriptorPool>,
    pub graph: Option<LoadedGraph>,
    pub lazy: Option<LazyPool>,
}

impl DescriptorContext {
    /// The descriptor pool, regardless of which path was taken.
    pub fn pool(&self) -> &prost_reflect::DescriptorPool {
        if let Some(lazy) = &self.lazy {
            &lazy.pool
        } else {
            self.pool.as_ref().unwrap()
        }
    }

    /// Mutable access to the descriptor pool.
    pub fn pool_mut(&mut self) -> &mut prost_reflect::DescriptorPool {
        if let Some(lazy) = &mut self.lazy {
            &mut lazy.pool
        } else {
            self.pool.as_mut().unwrap()
        }
    }

    /// Load a `DescriptorContext` from an optional descriptor path.
    ///
    /// When `path` is `None`, uses the embedded WKT descriptor (eager).
    /// When `path` is `Some(p)` and `<stem>/index.rkyv` exists, opens a
    /// `LazyPool`; otherwise falls back to eager `decode_pool`.
    /// In both `Some` cases checks for `<stem>/hopcroft.rkyv`.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        match path {
            None => {
                let bytes = EMBEDDED_DESCRIPTOR.to_vec();
                #[cfg(feature = "wkt-db")]
                let graph = Some(
                    LoadedGraph::from_static_bytes(WKT_GRAPH)
                        .map_err(|e| format!("wkt graph: {e}"))?,
                );
                #[cfg(not(feature = "wkt-db"))]
                let graph = None;
                let pool = decode_pool(&bytes).map_err(|e| format!("embedded descriptor: {e}"))?;
                Ok(DescriptorContext {
                    pool: Some(pool),
                    graph,
                    lazy: None,
                })
            }
            Some(p) => {
                let stem = p.with_extension("");
                let rkyv_path = stem.join("hopcroft.rkyv");
                let index_path = stem.join("index.rkyv");

                let graph =
                    if rkyv_path.exists() {
                        Some(load_graph(&rkyv_path).map_err(|e| {
                            format!("loading graph '{}': {}", rkyv_path.display(), e)
                        })?)
                    } else {
                        None
                    };

                if index_path.exists() {
                    let lazy = LazyPool::open(p, &index_path)
                        .map_err(|e| format!("opening lazy pool: {e}"))?;
                    Ok(DescriptorContext {
                        pool: None,
                        graph,
                        lazy: Some(lazy),
                    })
                } else {
                    let bytes = read_descriptor_file(p)?;
                    let pool = decode_pool(&bytes).map_err(|e| format!("descriptor: {e}"))?;
                    Ok(DescriptorContext {
                        pool: Some(pool),
                        graph,
                        lazy: None,
                    })
                }
            }
        }
    }
}

/// Read a descriptor file: accepts binary `FileDescriptorSet`, `#@` prototext
/// `FileDescriptorSet`, or a single `FileDescriptorProto`.
fn read_descriptor_file(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
    // The prototext_core parser handles both binary and #@ prototext FDS/FDP
    // transparently via render_as_bytes — but we need raw binary FDS bytes for
    // decode_pool.  If the file starts with the #@ magic, decode it first.
    if bytes.starts_with(b"#@") {
        let opts = RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
        };
        render_as_bytes(&bytes, opts).map_err(|e: CodecError| {
            format!("decoding prototext descriptor '{}': {}", path.display(), e)
        })
    } else {
        Ok(bytes)
    }
}

// ── Type inference helpers ────────────────────────────────────────────────────

/// Maximum number of tied type names shown per ambiguous file in warnings.
const MAX_AMBIGUOUS_TYPES: usize = 10;

/// Result of auto-inference: the winning FQDN and its score breakdown.
pub struct InferredType {
    pub fqdn: String,
    pub score: i64,
    pub matches: u64,
    pub unknowns: u64,
    pub mismatches: u64,
    pub non_canonical: u64,
}

/// Outcome of attempting to infer the message type of a protobuf blob.
pub enum InferOutcome {
    /// A unique winner was found.
    Unique(InferredType),
    /// Multiple types tied at the top score; contains the tied FQDNs
    /// (lexicographically sorted, capped at MAX_AMBIGUOUS_TYPES).
    Ambiguous(Vec<String>),
}

/// Score `pb_bytes` against `graph` and return the inference outcome,
/// or a hard error (e.g. all entries vetoed, or encoding failure).
pub fn infer_type(pb_bytes: &[u8], graph: &LoadedGraph) -> Result<InferOutcome, String> {
    let binary_buf;
    let pb_bytes = {
        let opts = RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
        };
        binary_buf = render_as_bytes(pb_bytes, opts)
            .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))?;
        binary_buf.as_slice()
    };

    let mut results = score_all(pb_bytes, graph);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(&b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(&b.fqdn)),
    });

    if results.is_empty() {
        return Err("schema DB is empty; cannot infer message type".into());
    }
    let non_vetoed: Vec<_> = results.iter().filter(|r| !r.vetoed).collect();
    if non_vetoed.is_empty() {
        return Err("all entries vetoed; cannot infer message type".into());
    }

    let top_score = non_vetoed[0].score();
    let tied: Vec<_> = non_vetoed
        .iter()
        .filter(|r| r.score() == top_score)
        .collect();

    if tied.len() > 1 {
        let mut names: Vec<String> = tied.iter().map(|r| r.fqdn.clone()).collect();
        names.sort();
        names.truncate(MAX_AMBIGUOUS_TYPES);
        return Ok(InferOutcome::Ambiguous(names));
    }

    let winner = &non_vetoed[0];
    Ok(InferOutcome::Unique(InferredType {
        fqdn: winner.fqdn.clone(),
        score: top_score,
        matches: winner.matches,
        unknowns: winner.unknowns,
        mismatches: winner.mismatches,
        non_canonical: winner.non_canonical,
    }))
}

/// Insert `# Type:` and `# Score:` comment lines (plus a blank line) after the
/// magic first line of rendered prototext output.  The magic line itself is
/// left unmodified.
fn inject_matched_annotation(text: &[u8], inferred: &InferredType) -> Vec<u8> {
    let score_str = if inferred.score == i64::MIN {
        "-inf".to_string()
    } else {
        inferred.score.to_string()
    };
    let insert = format!(
        "# Type: {}\n# Score: {}  (matched: {}, unknown: {}, mismatches: {}, non_canonical: {})\n\n",
        inferred.fqdn,
        score_str,
        inferred.matches,
        inferred.unknowns,
        inferred.mismatches,
        inferred.non_canonical,
    );
    if let Some(nl) = text.iter().position(|&b| b == b'\n') {
        let mut out = text[..=nl].to_vec();
        out.extend_from_slice(insert.as_bytes());
        out.extend_from_slice(&text[nl + 1..]);
        out
    } else {
        text.to_vec()
    }
}

// ── Per-file processing ───────────────────────────────────────────────────────

pub fn process(
    data: &[u8],
    decode: bool,
    assume_binary: bool,
    schema: Option<&prototext_core::ParsedSchema>,
    annotations: bool,
) -> Result<Vec<u8>, String> {
    let opts = RenderOpts {
        assume_binary,
        include_annotations: annotations,
        indent: 1,
    };
    if decode {
        render_as_text(data, schema, opts).map_err(|e: CodecError| e.to_string())
    } else {
        render_as_bytes(data, opts).map_err(|e: CodecError| e.to_string())
    }
}

// ── Output helpers ────────────────────────────────────────────────────────────

/// Compute the output path for a batch-mode file.
pub fn output_path_for(f: &InputFile, in_place: bool, output_root: Option<&PathBuf>) -> PathBuf {
    if let Some(root) = output_root {
        root.join(&f.rel)
    } else if in_place {
        f.abs.clone()
    } else {
        unreachable!("output_path_for called without in_place or output_root")
    }
}

/// Write `data` to `explicit_output` path, or to stdout when `None`.
pub fn write_output(data: &[u8], explicit_output: Option<&Path>) -> Result<(), String> {
    if let Some(path) = explicit_output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating '{}': {}", parent.display(), e))?;
        }
        std::fs::write(path, data).map_err(|e| format!("writing '{}': {}", path.display(), e))
    } else {
        io::stdout()
            .write_all(data)
            .map_err(|e| format!("writing stdout: {}", e))
    }
}

// ── Schema listing helpers ────────────────────────────────────────────────────

pub fn list_schemas_one(
    pb_bytes: &[u8],
    graph: &LoadedGraph,
    path_label: &str,
    top: Option<usize>,
    out: &mut dyn Write,
) -> Result<(), String> {
    let binary_buf;
    let pb_bytes = {
        let opts = RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
        };
        binary_buf = render_as_bytes(pb_bytes, opts)
            .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))?;
        binary_buf.as_slice()
    };

    let mut results = score_all(pb_bytes, graph);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(&b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(&b.fqdn)),
    });

    let non_vetoed: Vec<_> = results.iter().filter(|r| !r.vetoed).collect();

    let to_print: Vec<_> = match top {
        Some(n) if n > 0 => non_vetoed[..n.min(non_vetoed.len())].to_vec(),
        _ => {
            if non_vetoed.is_empty() {
                vec![]
            } else {
                let top_score = non_vetoed[0].score();
                let mut tied: Vec<_> = non_vetoed
                    .iter()
                    .filter(|r| r.score() == top_score)
                    .copied()
                    .collect();
                tied.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
                tied
            }
        }
    };

    #[derive(Serialize)]
    struct Entry<'a> {
        path: &'a str,
        types: Vec<&'a str>,
    }
    let entry = Entry {
        path: path_label,
        types: to_print.iter().map(|r| r.fqdn.as_str()).collect(),
    };
    let yaml = serde_yaml::to_string(&[entry]).map_err(|e| format!("serializing YAML: {}", e))?;
    out.write_all(yaml.as_bytes())
        .map_err(|e| format!("writing list: {}", e))
}

// ── Top-level run ─────────────────────────────────────────────────────────────

pub fn run(cli: Cli) -> Result<(), String> {
    // Resolve the descriptor once up front.
    let mut desc_ctx = DescriptorContext::load(cli.descriptor.as_deref())?;

    match cli.command {
        // ── decode ────────────────────────────────────────────────────────────
        Command::Decode {
            r#type,
            in_place,
            assume_binary,
            annotations,
            output_root: cmd_output_root,
            paths,
        } => {
            let effective_annotations = annotations;
            let output_root = cli.output_root.or(cmd_output_root);

            validate_input_root_absolute(&cli.input_root, &paths)?;
            validate_roots_not_same(&cli.input_root, &output_root)?;

            let auto_infer = r#type.is_none();
            if auto_infer && desc_ctx.graph.is_none() {
                return Err(if cli.descriptor.is_some() {
                    "decode auto-inference requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                        .into()
                } else {
                    "decode auto-inference requires --descriptor with a sibling \
                     hopcroft.rkyv, or a wkt-db-enabled build"
                        .into()
                });
            }

            run_decode(
                &mut desc_ctx,
                r#type.as_deref(),
                in_place,
                assume_binary,
                effective_annotations,
                cli.strict,
                &cli.output,
                output_root.as_ref(),
                &cli.input_root,
                &paths,
            )
        }

        // ── encode ────────────────────────────────────────────────────────────
        Command::Encode {
            in_place,
            output_root: cmd_output_root,
            paths,
        } => {
            let output_root = cli.output_root.or(cmd_output_root);

            validate_input_root_absolute(&cli.input_root, &paths)?;
            validate_roots_not_same(&cli.input_root, &output_root)?;

            run_encode(
                in_place,
                &cli.output,
                output_root.as_ref(),
                &cli.input_root,
                &paths,
            )
        }

        // ── list-schemas ──────────────────────────────────────────────────────
        Command::ListSchemas { top, paths } => {
            let graph = desc_ctx.graph.as_ref().ok_or_else(|| {
                if cli.descriptor.is_some() {
                    "list-schemas requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                } else {
                    "list-schemas requires --descriptor with a sibling hopcroft.rkyv, \
                     or a wkt-db-enabled build"
                }
            })?;
            if let Some(lazy) = &mut desc_ctx.lazy {
                lazy.load_all().map_err(|e| format!("loading index: {e}"))?;
            }
            run_list_schemas(graph, top, &cli.input_root, &paths)
        }

        // ── instantiate-schema ────────────────────────────────────────────────
        Command::InstantiateSchema {
            types,
            seed,
            max_depth,
            max_repeated,
            p_optional,
        } => {
            if types.len() > 1 && cli.output_root.is_none() {
                return Err("multiple types require --output-root (-O)".into());
            }
            let opts = InstantiateOpts {
                seed,
                max_depth,
                max_repeated,
                p_optional,
                quiet: cli.quiet,
            };
            for type_name in &types {
                let lookup = type_name.trim_start_matches('.');
                if let Some(lazy) = &mut desc_ctx.lazy {
                    lazy.get_message(lookup)
                        .map_err(|e| format!("loading type '{lookup}': {e}"))?;
                }
                let out: Option<PathBuf> = if let Some(ref root) = cli.output_root {
                    let rel = lookup.replace('.', "/");
                    let mut p = root.join(rel);
                    p.set_extension("pb");
                    Some(p)
                } else {
                    cli.output.clone()
                };
                run_instantiate_schema(desc_ctx.pool().clone(), lookup, &opts, out.as_deref())?;
            }
            Ok(())
        }

        // ── score ─────────────────────────────────────────────────────────────
        Command::Score {
            r#type,
            assume_binary,
            paths,
        } => {
            let graph = desc_ctx.graph.as_ref().ok_or_else(|| {
                if cli.descriptor.is_some() {
                    "score requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                } else {
                    "score requires --descriptor with a sibling hopcroft.rkyv, \
                     or a wkt-db-enabled build"
                }
            })?;
            run_score(graph, &r#type, assume_binary, &cli.input_root, &paths)
        }
    }
}

// ── Validation helpers ────────────────────────────────────────────────────────

fn validate_input_root_absolute(
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    if input_root.is_some() {
        for raw in paths {
            if Path::new(raw).is_absolute() {
                return Err(format!(
                    "absolute path '{}' is not allowed when --input-root is given",
                    raw
                ));
            }
        }
    }
    Ok(())
}

fn validate_roots_not_same(
    input_root: &Option<PathBuf>,
    output_root: &Option<PathBuf>,
) -> Result<(), String> {
    if let (Some(ir), Some(or)) = (input_root, output_root) {
        let ir_canon = std::fs::canonicalize(ir)
            .map_err(|e| format!("--input-root '{}': {}", ir.display(), e))?;
        let or_canon = std::fs::canonicalize(or).unwrap_or_else(|_| or.clone());
        if ir_canon == or_canon {
            return Err(
                "--input-root and --output-root resolve to the same directory; \
                 use --in-place instead"
                    .into(),
            );
        }
    }
    Ok(())
}

// ── decode handler ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_decode(
    desc_ctx: &mut DescriptorContext,
    type_name: Option<&str>,
    in_place: bool,
    assume_binary: bool,
    annotations: bool,
    strict: bool,
    output: &Option<PathBuf>,
    output_root: Option<&PathBuf>,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    let auto_infer = type_name.is_none();

    // Build schema if a type was given explicitly.
    let schema: Option<prototext_core::ParsedSchema> = if let Some(t) = type_name {
        let lookup = t.trim_start_matches('.');
        if let Some(lazy) = &mut desc_ctx.lazy {
            lazy.get_message(lookup)
                .map_err(|e| format!("loading type '{lookup}': {e}"))?;
        }
        Some(
            schema_from_pool(desc_ctx.pool().clone(), lookup)
                .map_err(|e| format!("descriptor: {}", e))?,
        )
    } else {
        None
    };

    let base = input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if paths.is_empty() {
        // stdin path
        if in_place {
            return Err("--in-place cannot be used with stdin input".into());
        }
        if output_root.is_some() {
            return Err("--output-root cannot be used with stdin input".into());
        }
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;

        if auto_infer {
            let graph = desc_ctx.graph.as_ref().unwrap(); // checked earlier
            match infer_type(&data, graph)? {
                InferOutcome::Ambiguous(tied) => {
                    let mut rep = InferFailureReporter::new();
                    rep.report_ambiguous("<stdin>", &tied);
                    std::process::exit(if strict { 1 } else { 0 });
                }
                InferOutcome::Unique(inferred) => {
                    let lookup = inferred.fqdn.trim_start_matches('.');
                    if let Some(lazy) = &mut desc_ctx.lazy {
                        lazy.get_message(lookup)
                            .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
                    }
                    let infer_schema = schema_from_pool(desc_ctx.pool().clone(), lookup)
                        .map_err(|e| format!("descriptor: {}", e))?;
                    let raw_out =
                        process(&data, true, assume_binary, Some(&infer_schema), annotations)?;
                    let out = inject_matched_annotation(&raw_out, &inferred);
                    write_output(&out, output.as_deref())?;
                    return Ok(());
                }
            }
        }

        let out = process(&data, true, assume_binary, schema.as_ref(), annotations)?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    // Expand paths.
    let all_files = expand_all_paths(paths, &base)?;

    // Single file, no batch flags — write to stdout / --output.
    if all_files.len() == 1 && !in_place && output_root.is_none() {
        let f = &all_files[0];
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;

        if auto_infer {
            let graph = desc_ctx.graph.as_ref().unwrap();
            match infer_type(&data, graph)? {
                InferOutcome::Ambiguous(tied) => {
                    let mut rep = InferFailureReporter::new();
                    rep.report_ambiguous(&f.abs.display().to_string(), &tied);
                    std::process::exit(if strict { 1 } else { 0 });
                }
                InferOutcome::Unique(inferred) => {
                    let lookup = inferred.fqdn.trim_start_matches('.');
                    if let Some(lazy) = &mut desc_ctx.lazy {
                        lazy.get_message(lookup)
                            .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
                    }
                    let infer_schema = schema_from_pool(desc_ctx.pool().clone(), lookup)
                        .map_err(|e| format!("descriptor: {}", e))?;
                    let raw_out =
                        process(&data, true, assume_binary, Some(&infer_schema), annotations)?;
                    let out = inject_matched_annotation(&raw_out, &inferred);
                    write_output(&out, output.as_deref())?;
                    return Ok(());
                }
            }
        }

        let out = process(&data, true, assume_binary, schema.as_ref(), annotations)?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    // Batch mode.
    if !in_place && output_root.is_none() {
        return Err("multiple input files require --in-place (-i) or --output-root (-O)".into());
    }

    if auto_infer {
        return run_batch_infer(
            all_files,
            assume_binary,
            annotations,
            strict,
            in_place,
            output_root,
            desc_ctx,
        );
    }

    run_batch(
        all_files,
        true,
        assume_binary,
        schema.as_ref(),
        annotations,
        in_place,
        output_root,
    )
}

// ── encode handler ────────────────────────────────────────────────────────────

fn run_encode(
    in_place: bool,
    output: &Option<PathBuf>,
    output_root: Option<&PathBuf>,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    let base = input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    if paths.is_empty() {
        if in_place {
            return Err("--in-place cannot be used with stdin input".into());
        }
        if output_root.is_some() {
            return Err("--output-root cannot be used with stdin input".into());
        }
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;
        let out = process(&data, false, false, None, false)?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    let all_files = expand_all_paths(paths, &base)?;

    if all_files.len() == 1 && !in_place && output_root.is_none() {
        let f = &all_files[0];
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
        let out = process(&data, false, false, None, false)?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    if !in_place && output_root.is_none() {
        return Err("multiple input files require --in-place (-i) or --output-root (-O)".into());
    }

    run_batch(all_files, false, false, None, false, in_place, output_root)
}

// ── list-schemas handler ──────────────────────────────────────────────────────

fn run_list_schemas(
    graph: &LoadedGraph,
    top: Option<usize>,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    let base = input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut out = io::stdout();

    if paths.is_empty() {
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;
        return list_schemas_one(&data, graph, "<stdin>", top, &mut out);
    }

    let all_files = expand_all_paths(paths, &base)?;
    for f in &all_files {
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
        let label = f.abs.display().to_string();
        list_schemas_one(&data, graph, &label, top, &mut out)?;
    }
    Ok(())
}

// ── score handler ─────────────────────────────────────────────────────────────

fn run_score(
    graph: &LoadedGraph,
    type_name: &str,
    assume_binary: bool,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    let base = input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    #[derive(Serialize)]
    #[serde(untagged)]
    enum ScoreEntry {
        Scored {
            path: String,
            score: i64,
            matches: u64,
            unknowns: u64,
            mismatches: u64,
            non_canonical: u64,
        },
        Vetoed {
            path: String,
            vetoed: bool,
        },
    }

    let score_one = |data: &[u8]| -> Result<(bool, i64, u64, u64, u64, u64), String> {
        let binary = render_as_bytes(
            data,
            RenderOpts {
                assume_binary,
                include_annotations: false,
                indent: 1,
            },
        )
        .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))?;
        let results = score_all(&binary, graph);
        let result = results
            .iter()
            .find(|r| r.fqdn == type_name || r.fqdn == format!(".{}", type_name))
            .ok_or_else(|| format!("type '{}' not found in scoring graph", type_name))?;
        Ok((
            result.vetoed,
            result.score(),
            result.matches,
            result.unknowns,
            result.mismatches,
            result.non_canonical,
        ))
    };

    let mut entries: Vec<ScoreEntry> = Vec::new();

    if paths.is_empty() {
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;
        let (vetoed, score, matches, unknowns, mismatches, non_canonical) = score_one(&data)?;
        if vetoed {
            entries.push(ScoreEntry::Vetoed {
                path: "<stdin>".into(),
                vetoed: true,
            });
        } else {
            entries.push(ScoreEntry::Scored {
                path: "<stdin>".into(),
                score,
                matches,
                unknowns,
                mismatches,
                non_canonical,
            });
        }
    } else {
        let all_files = expand_all_paths(paths, &base)?;
        for f in &all_files {
            let data = std::fs::read(&f.abs)
                .map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
            let label = f.abs.display().to_string();
            let (vetoed, score, matches, unknowns, mismatches, non_canonical) = score_one(&data)?;
            if vetoed {
                entries.push(ScoreEntry::Vetoed {
                    path: label,
                    vetoed: true,
                });
            } else {
                entries.push(ScoreEntry::Scored {
                    path: label,
                    score,
                    matches,
                    unknowns,
                    mismatches,
                    non_canonical,
                });
            }
        }
    }

    let yaml = serde_yaml::to_string(&entries).map_err(|e| format!("serializing YAML: {}", e))?;
    io::stdout()
        .write_all(yaml.as_bytes())
        .map_err(|e| format!("writing stdout: {}", e))
}

// ── inference failure reporter ────────────────────────────────────────────────

/// Stateful reporter that prints a single heading on the first failure, then
/// emits each entry on-the-go as it is discovered.
///
/// Output format:
/// ```text
/// warning: type inference issues:
/// - path: foo.pb
///   types:
///   - TypeA
///   - TypeB
/// - path: bar.pb
///   types: []
///   error: all entries vetoed
/// ```
struct InferFailureReporter {
    heading_printed: bool,
    had_hard_error: bool,
    had_warning: bool,
}

impl InferFailureReporter {
    fn new() -> Self {
        Self {
            heading_printed: false,
            had_hard_error: false,
            had_warning: false,
        }
    }

    fn report_ambiguous(&mut self, path: &str, tied: &[String]) {
        self.ensure_heading();
        self.had_warning = true;
        eprintln!("- path: {path}");
        eprintln!("  types:");
        for fqdn in tied {
            eprintln!("  - {fqdn}");
        }
    }

    fn report_error(&mut self, path: &str, error: &str) {
        self.ensure_heading();
        self.had_hard_error = true;
        eprintln!("- path: {path}");
        eprintln!("  types: []");
        eprintln!("  error: {error}");
    }

    fn ensure_heading(&mut self) {
        if !self.heading_printed {
            eprintln!("warning: type inference issues:");
            self.heading_printed = true;
        }
    }

    /// Compute the appropriate exit code.
    /// - 0: no failures, or warnings without --strict
    /// - 1: hard errors, or warnings with --strict
    fn exit_code(&self, strict: bool) -> i32 {
        if self.had_hard_error || (self.had_warning && strict) {
            1
        } else {
            0
        }
    }
}

// ── batch auto-infer ──────────────────────────────────────────────────────────

fn run_batch_infer(
    all_files: Vec<InputFile>,
    assume_binary: bool,
    annotations: bool,
    strict: bool,
    in_place: bool,
    output_root: Option<&PathBuf>,
    desc_ctx: &mut DescriptorContext,
) -> Result<(), String> {
    // Detect output collisions eagerly.
    {
        let mut seen: HashMap<PathBuf, PathBuf> = HashMap::new();
        for f in &all_files {
            let out_path = output_path_for(f, in_place, output_root);
            if let Some(prev_abs) = seen.get(&out_path) {
                return Err(format!(
                    "output collision: '{}' and '{}' both map to '{}'",
                    prev_abs.display(),
                    f.abs.display(),
                    out_path.display()
                ));
            }
            seen.insert(out_path, f.abs.clone());
        }
    }

    let graph = desc_ctx.graph.as_ref().unwrap(); // guaranteed by caller

    // First pass: read + infer every file.
    // Failures are reported on-the-go; successes are collected with their data
    // so the second pass can use &mut desc_ctx without re-reading.
    let mut reporter = InferFailureReporter::new();
    let mut successes: Vec<(InputFile, Vec<u8>, InferredType)> = Vec::new();

    for f in all_files {
        let data = match std::fs::read(&f.abs) {
            Ok(d) => d,
            Err(e) => {
                reporter.report_error(&f.abs.display().to_string(), &e.to_string());
                continue;
            }
        };
        match infer_type(&data, graph) {
            Err(e) => reporter.report_error(&f.abs.display().to_string(), &e),
            Ok(InferOutcome::Ambiguous(tied)) => {
                reporter.report_ambiguous(&f.abs.display().to_string(), &tied)
            }
            Ok(InferOutcome::Unique(inferred)) => successes.push((f, data, inferred)),
        }
    }

    // Second pass: process successful files (needs &mut desc_ctx for lazy loading).
    let mut had_hard_error = false;
    for (f, data, inferred) in &successes {
        let lookup = inferred.fqdn.trim_start_matches('.');
        let schema = match (|| {
            if let Some(lazy) = &mut desc_ctx.lazy {
                lazy.get_message(lookup)
                    .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
            }
            schema_from_pool(desc_ctx.pool().clone(), lookup)
                .map_err(|e| format!("descriptor: {}", e))
        })() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: '{}': {}", f.abs.display(), e);
                had_hard_error = true;
                continue;
            }
        };
        let raw_out = match process(data, true, assume_binary, Some(&schema), annotations) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("error: '{}': {}", f.abs.display(), e);
                had_hard_error = true;
                continue;
            }
        };
        let out = inject_matched_annotation(&raw_out, inferred);
        let dest = output_path_for(f, in_place, output_root);
        if let Err(e) = write_output(&out, Some(&dest)) {
            eprintln!("error: {}", e);
            had_hard_error = true;
        }
    }

    let code = if had_hard_error {
        1
    } else {
        reporter.exit_code(strict)
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

// ── batch helper ──────────────────────────────────────────────────────────────

fn run_batch(
    all_files: Vec<InputFile>,
    decode: bool,
    assume_binary: bool,
    schema: Option<&prototext_core::ParsedSchema>,
    annotations: bool,
    in_place: bool,
    output_root: Option<&PathBuf>,
) -> Result<(), String> {
    // Detect output collisions eagerly.
    {
        let mut seen: HashMap<PathBuf, PathBuf> = HashMap::new();
        for f in &all_files {
            let out_path = output_path_for(f, in_place, output_root);
            if let Some(prev_abs) = seen.get(&out_path) {
                return Err(format!(
                    "output collision: '{}' and '{}' both map to '{}'",
                    prev_abs.display(),
                    f.abs.display(),
                    out_path.display()
                ));
            }
            seen.insert(out_path, f.abs.clone());
        }
    }

    // In-place: sponge semantics — read all files before writing any.
    let file_data: Vec<(InputFile, Option<Vec<u8>>)> = if in_place {
        let mut v = Vec::with_capacity(all_files.len());
        for f in all_files {
            let data = std::fs::read(&f.abs)
                .map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
            v.push((f, Some(data)));
        }
        v
    } else {
        all_files.into_iter().map(|f| (f, None)).collect()
    };

    let mut had_error = false;
    for (f, preread) in file_data {
        let data: Vec<u8> = if let Some(d) = preread {
            d
        } else {
            match std::fs::read(&f.abs) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: reading '{}': {}", f.abs.display(), e);
                    had_error = true;
                    continue;
                }
            }
        };

        let dest = output_path_for(&f, in_place, output_root);
        match process(&data, decode, assume_binary, schema, annotations) {
            Ok(out) => {
                if let Err(e) = write_output(&out, Some(&dest)) {
                    eprintln!("error: {}", e);
                    had_error = true;
                }
            }
            Err(e) => {
                eprintln!("error: '{}': {}", f.abs.display(), e);
                had_error = true;
            }
        }
    }

    if had_error {
        std::process::exit(1);
    }
    Ok(())
}

// ── Path expansion helper ─────────────────────────────────────────────────────

fn expand_all_paths(paths: &[String], base: &Path) -> Result<Vec<InputFile>, String> {
    let mut all_files: Vec<InputFile> = Vec::new();
    let mut expand_errors: Vec<String> = Vec::new();

    for raw in paths {
        match expand_path(raw, base) {
            Ok(files) => all_files.extend(files),
            Err(e) => expand_errors.push(e),
        }
    }

    if !expand_errors.is_empty() {
        for e in &expand_errors {
            eprintln!("error: {}", e);
        }
        std::process::exit(1);
    }

    Ok(all_files)
}

// ── instantiate-schema handler ────────────────────────────────────────────────

fn run_instantiate_schema(
    pool: prost_reflect::DescriptorPool,
    type_name: &str,
    opts: &InstantiateOpts,
    output: Option<&Path>,
) -> Result<(), String> {
    let fqdn = if type_name.starts_with('.') {
        type_name.to_string()
    } else {
        format!(".{}", type_name)
    };
    let lookup_name = fqdn.trim_start_matches('.');

    let schema = schema_from_pool(pool, lookup_name).map_err(|e| format!("descriptor: {}", e))?;

    let msg_desc = schema
        .root_descriptor()
        .ok_or_else(|| format!("type '{}' not found in descriptor", lookup_name))?;

    let binary = generate_message_bytes(&msg_desc, opts);

    let render_opts = RenderOpts {
        assume_binary: true,
        include_annotations: true,
        indent: 1,
    };
    let text_bytes = render_as_text(&binary, Some(&schema), render_opts)
        .map_err(|e: CodecError| e.to_string())?;

    let text = String::from_utf8(text_bytes)
        .map_err(|e| format!("generated text is not valid UTF-8: {}", e))?;
    let out = insert_hints(&text, &fqdn, opts.seed);

    write_output(out.as_bytes(), output)
}

fn insert_hints(text: &str, fqdn: &str, seed: i64) -> String {
    let mut lines = text.splitn(2, '\n');
    let magic = lines.next().unwrap_or("");
    let rest = lines.next().unwrap_or("");
    format!("{}\n# type: {}\n# seed: {}\n{}", magic, fqdn, seed, rest)
}
