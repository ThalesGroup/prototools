// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use prototext_core::serialize::render_text::EXTRA_HEADER;
use prototext_core::{
    clear_any_loader, decode_pool, is_prototext_text, render_as_bytes, render_as_text,
    set_any_loader, AnyLoader, CodecError, RenderOpts,
};
use prototext_graph::score::{
    load::{load_graph, LoadedGraph},
    score_all, score_one, ScoringOpts,
};

use crate::inputs::{expand_path, InputFile};
use crate::lazy_pool::LazyPool;

#[cfg(feature = "wkt-db")]
use crate::WKT_GRAPH;
use crate::{Cli, Command, EMBEDDED_DESCRIPTOR};

// ── DescriptorContext ─────────────────────────────────────────────────────────

/// Result of resolving `--descriptor-set`: a pool for type lookup plus an optional
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
            expand_any: false,
            ..RenderOpts::default()
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
    /// Multiple types tied at the top score; contains the tied entries
    /// (lexicographically sorted, capped at MAX_AMBIGUOUS_TYPES).
    Ambiguous(Vec<InferredType>),
}

/// Score `pb_bytes` against `graph` and return the inference outcome,
/// or a hard error (e.g. all entries vetoed, or encoding failure).
pub fn infer_type(
    pb_bytes: &[u8],
    graph: &LoadedGraph,
    scoring_opts: &ScoringOpts,
) -> Result<InferOutcome, String> {
    let binary_buf;
    let pb_bytes = {
        let opts = RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
            expand_any: false,
            ..RenderOpts::default()
        };
        binary_buf = render_as_bytes(pb_bytes, opts)
            .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))?;
        binary_buf.as_slice()
    };

    let mut results = score_all(pb_bytes, graph, scoring_opts);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(b.fqdn)),
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
        let mut ambiguous: Vec<InferredType> = tied
            .iter()
            .map(|r| InferredType {
                fqdn: r.fqdn.to_owned(),
                score: r.score(),
                matches: r.matches,
                unknowns: r.unknowns,
                mismatches: r.mismatches,
                non_canonical: r.non_canonical,
            })
            .collect();
        ambiguous.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
        ambiguous.truncate(MAX_AMBIGUOUS_TYPES);
        return Ok(InferOutcome::Ambiguous(ambiguous));
    }

    let winner = &non_vetoed[0];
    Ok(InferOutcome::Unique(InferredType {
        fqdn: winner.fqdn.to_owned(),
        score: top_score,
        matches: winner.matches,
        unknowns: winner.unknowns,
        mismatches: winner.mismatches,
        non_canonical: winner.non_canonical,
    }))
}

/// Format the `# Type:` / `# Score:` inference header (with trailing blank line).
fn inferred_header(inferred: &InferredType) -> String {
    let score_str = if inferred.score == i64::MIN {
        "-inf".to_string()
    } else {
        inferred.score.to_string()
    };
    format!(
        "# Type: {}\n# Score: {}  (matched: {}, unknown: {}, mismatches: {}, non_canonical: {})\n\n",
        inferred.fqdn,
        score_str,
        inferred.matches,
        inferred.unknowns,
        inferred.mismatches,
        inferred.non_canonical,
    )
}

/// Write a YAML type-entry block (used by both `InferFailureReporter` and
/// `list_schemas_one`).  When `detailed_score` is true the four sub-dimensions
/// are included; otherwise only `type` and `score` are emitted.
fn write_type_entry(w: &mut dyn Write, indent: &str, t: &InferredType, detailed_score: bool) {
    let _ = writeln!(w, "{indent}- type: {}", t.fqdn);
    let _ = writeln!(w, "{indent}  score: {}", t.score);
    if detailed_score {
        let _ = writeln!(w, "{indent}  matched: {}", t.matches);
        let _ = writeln!(w, "{indent}  unknown: {}", t.unknowns);
        let _ = writeln!(w, "{indent}  mismatches: {}", t.mismatches);
        let _ = writeln!(w, "{indent}  non_canonical: {}", t.non_canonical);
    }
}

// ── Per-file processing ───────────────────────────────────────────────────────

pub fn process(
    data: &[u8],
    decode: bool,
    root_desc: Option<&prost_reflect::MessageDescriptor>,
    opts: RenderOpts,
) -> Result<Vec<u8>, String> {
    if decode {
        render_as_text(data, root_desc, opts).map_err(|e: CodecError| e.to_string())
    } else {
        // encode path: require the `#@ prototext:` header
        if !is_prototext_text(data) {
            return Err(CodecError::NotPrototext.to_string());
        }
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
    detailed_score: bool,
    scoring_opts: &ScoringOpts,
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut results = score_all(pb_bytes, graph, scoring_opts);
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(b.fqdn)),
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
                tied.sort_by(|a, b| a.fqdn.cmp(b.fqdn));
                tied
            }
        }
    };

    let entries: Vec<InferredType> = to_print
        .iter()
        .map(|r| InferredType {
            fqdn: r.fqdn.to_owned(),
            score: r.score(),
            matches: r.matches,
            unknowns: r.unknowns,
            mismatches: r.mismatches,
            non_canonical: r.non_canonical,
        })
        .collect();

    writeln!(out, "- path: {path_label}").map_err(|e| format!("writing list: {}", e))?;
    writeln!(out, "  types:").map_err(|e| format!("writing list: {}", e))?;
    for t in &entries {
        write_type_entry(out, "  ", t, detailed_score);
    }
    Ok(())
}

// ── Top-level run ─────────────────────────────────────────────────────────────

pub fn run(mut cli: Cli) -> Result<(), String> {
    // Deprecation shim for old env var name.
    if std::env::var_os("PROTOTEXT_DESCRIPTOR_SET").is_none() {
        if let Some(val) = std::env::var_os("PROTOTEXT_DEFAULT_DESCRIPTOR") {
            if !cli.quiet {
                eprintln!(
                    "warning: PROTOTEXT_DEFAULT_DESCRIPTOR is deprecated; \
                     use PROTOTEXT_DESCRIPTOR_SET"
                );
            }
            // Fall back to the old var when the new one is absent and the
            // flag was not supplied on the command line.
            if cli.descriptor.is_none() {
                cli.descriptor = Some(PathBuf::from(val));
            }
        }
    }

    // Resolve the descriptor once up front.
    let mut desc_ctx = DescriptorContext::load(cli.descriptor.as_deref())?;

    match cli.command {
        // ── decode ────────────────────────────────────────────────────────────
        Command::Decode {
            r#type,
            raw,
            in_place,
            assume_binary,
            no_annotations,
            detailed_score,
            relax_ranges,
            no_expand_any,
            no_expand_message_set,
            hide_unknown_fields,
            strict,
            paths,
        } => {
            let annotations = !no_annotations;
            let output_root = cli.output_root.clone();
            let scoring_opts = ScoringOpts {
                strict_ranges: !relax_ranges,
                expand_any: !no_expand_any,
            };

            validate_input_root_absolute(&cli.input_root, &paths)?;
            validate_not_in_place_and_output_root(in_place, &output_root)?;
            validate_roots_not_same(&cli.input_root, &output_root)?;

            let auto_infer = r#type.is_none() && !raw;
            if auto_infer && desc_ctx.graph.is_none() {
                return Err(if cli.descriptor.is_some() {
                    "decode auto-inference requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                        .into()
                } else {
                    "decode auto-inference requires --descriptor-set with a sibling \
                     hopcroft.rkyv, or a wkt-db-enabled build"
                        .into()
                });
            }

            run_decode(
                &mut desc_ctx,
                r#type.as_deref(),
                raw,
                in_place,
                assume_binary,
                annotations,
                !no_expand_any,
                hide_unknown_fields,
                !no_expand_message_set,
                detailed_score,
                &scoring_opts,
                strict,
                &cli.output,
                output_root.as_ref(),
                &cli.input_root,
                &paths,
            )
        }

        // ── encode ────────────────────────────────────────────────────────────
        Command::Encode { in_place, paths } => {
            let output_root = cli.output_root.clone();

            validate_input_root_absolute(&cli.input_root, &paths)?;
            validate_not_in_place_and_output_root(in_place, &output_root)?;
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
        Command::ListSchemas {
            top,
            assume_binary,
            detailed_score,
            relax_ranges,
            no_expand_any,
            paths,
        } => {
            let graph = desc_ctx.graph.as_ref().ok_or_else(|| {
                if cli.descriptor.is_some() {
                    "list-schemas requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                } else {
                    "list-schemas requires --descriptor-set with a sibling hopcroft.rkyv, \
                     or a wkt-db-enabled build"
                }
            })?;
            let scoring_opts = ScoringOpts {
                strict_ranges: !relax_ranges,
                expand_any: !no_expand_any,
            };
            run_list_schemas(
                graph,
                top,
                assume_binary,
                detailed_score,
                &scoring_opts,
                &cli.input_root,
                &paths,
            )
        }

        // ── score ─────────────────────────────────────────────────────────────
        Command::Score {
            r#type,
            assume_binary,
            relax_ranges,
            no_expand_any,
            paths,
        } => {
            let graph = desc_ctx.graph.as_ref().ok_or_else(|| {
                if cli.descriptor.is_some() {
                    "score requires a DB-backed descriptor \
                     (no hopcroft.rkyv found alongside the descriptor file)"
                } else {
                    "score requires --descriptor-set with a sibling hopcroft.rkyv, \
                     or a wkt-db-enabled build"
                }
            })?;
            let scoring_opts = ScoringOpts {
                strict_ranges: !relax_ranges,
                expand_any: !no_expand_any,
            };
            run_score(
                graph,
                &r#type,
                assume_binary,
                &scoring_opts,
                &cli.input_root,
                &paths,
            )
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

fn validate_not_in_place_and_output_root(
    in_place: bool,
    output_root: &Option<PathBuf>,
) -> Result<(), String> {
    if in_place && output_root.is_some() {
        return Err("--in-place and --output-root are mutually exclusive".into());
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

// ── Root type resolution ──────────────────────────────────────────────────────

/// Resolve `lookup` to a `MessageDescriptor` directly from `desc_ctx`'s pool
/// (spec 0106 S4).  The caller is responsible for having already loaded the
/// type on the lazy path (`lazy.get_message(lookup)`) before calling this.
///
/// On a miss, the error names the closest match in the pool by edit
/// distance rather than dumping every message name (as
/// `schema_from_pool`/`parse_schema` in `prototext-core` do).
fn resolve_root_desc(
    desc_ctx: &DescriptorContext,
    lookup: &str,
) -> Result<prost_reflect::MessageDescriptor, String> {
    desc_ctx.pool().get_message_by_name(lookup).ok_or_else(|| {
        let closest = desc_ctx
            .pool()
            .all_messages()
            .min_by_key(|m| strsim::levenshtein(m.full_name(), lookup));
        match closest {
            Some(m) => format!(
                "type '{lookup}' not found (did you mean '{}'?)",
                m.full_name()
            ),
            None => format!("type '{lookup}' not found"),
        }
    })
}

// ── Any JIT loader ────────────────────────────────────────────────────────────

/// Install a JIT loader for `google.protobuf.Any` type resolution (spec 0099).
///
/// On the lazy-pool path, `lazy.get_message(fqdn)` loads the FDP on demand.
/// On the eager-pool path, the type is either already in the pool or absent.
/// In both cases the loader then returns the descriptor from the shared pool.
///
/// The caller must call `clear_any_loader()` after rendering completes.
fn install_any_loader(desc_ctx: &mut DescriptorContext) {
    // SAFETY: `desc_ctx` outlives every rendering call that uses this loader.
    // The loader is cleared by `clear_any_loader()` before the caller that
    // holds `desc_ctx` returns, so the raw pointer is never dangling.
    let ctx_ptr: *mut DescriptorContext = desc_ctx as *mut DescriptorContext;
    let loader: AnyLoader = Box::new(move |key: &str| {
        let ctx = unsafe { &mut *ctx_ptr };
        // Both paths follow the same pattern:
        //   1. JIT-load the relevant FDP into lazy.pool (no-op on eager path).
        //   2. Look up the descriptor from ctx.pool() and return it.
        //
        // ctx.pool() always returns &lazy.pool on the lazy path, so after the
        // JIT-load mutates lazy.pool it is the authoritative source.  We must
        // NOT use a pre-existing MessageDescriptor for the post-load lookup:
        // prost-reflect uses Arc::make_mut when adding FDPs, which forks the
        // pool Arc whenever a clone exists (e.g. in ParsedSchema), making any
        // descriptor obtained before the load blind to newly-registered symbols.

        if let Some(slash) = key.rfind('/') {
            // MessageSet extension sentinel: "extendee_fqdn/field_number" (spec 0100 §5.2).
            if let Ok(number) = key[slash + 1..].parse::<u32>() {
                let extendee = &key[..slash];
                if let Some(lazy) = ctx.lazy.as_mut() {
                    let _ = lazy.get_extension(extendee, number);
                }
                return ctx
                    .pool()
                    .get_message_by_name(extendee)
                    .and_then(|ed| ed.get_extension(number))
                    .and_then(|ext| {
                        if let prost_reflect::Kind::Message(inner) = ext.kind() {
                            Some(std::sync::Arc::new(inner))
                        } else {
                            None
                        }
                    });
            }
        }
        // Normal Any path: key is a FQDN.
        if let Some(lazy) = ctx.lazy.as_mut() {
            let _ = lazy.get_message(key);
        }
        ctx.pool().get_message_by_name(key).map(std::sync::Arc::new)
    });
    set_any_loader(loader);
}

// ── decode handler ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_decode(
    desc_ctx: &mut DescriptorContext,
    type_name: Option<&str>,
    raw: bool,
    in_place: bool,
    assume_binary: bool,
    annotations: bool,
    expand_any: bool,
    hide_unknown_fields: bool,
    expand_message_set: bool,
    detailed_score: bool,
    scoring_opts: &ScoringOpts,
    strict: bool,
    output: &Option<PathBuf>,
    output_root: Option<&PathBuf>,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    // --raw: bypass all schema / inference logic and render field numbers +
    // wire types directly.  No descriptor set required.
    if raw {
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
            let out = process(
                &data,
                true,
                None,
                RenderOpts {
                    assume_binary,
                    include_annotations: annotations,
                    indent: 1,
                    ..RenderOpts::default()
                },
            )?;
            write_output(&out, output.as_deref())?;
        } else {
            let all_files = expand_all_paths(paths, &base)?;
            if all_files.len() == 1 && !in_place && output_root.is_none() {
                let f = &all_files[0];
                let data = std::fs::read(&f.abs)
                    .map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
                let out = process(
                    &data,
                    true,
                    None,
                    RenderOpts {
                        assume_binary,
                        include_annotations: annotations,
                        indent: 1,
                        ..RenderOpts::default()
                    },
                )?;
                write_output(&out, output.as_deref())?;
            } else {
                if !in_place && output_root.is_none() {
                    return Err(
                        "multiple input files require --in-place (-i) or --output-root (-O)".into(),
                    );
                }
                run_batch(
                    all_files,
                    true,
                    None,
                    RenderOpts {
                        assume_binary,
                        include_annotations: annotations,
                        indent: 1,
                        ..RenderOpts::default()
                    },
                    in_place,
                    output_root,
                )?;
            }
        }
        return Ok(());
    }

    let auto_infer = type_name.is_none();

    // Render options for all schema-based decode calls in this function.
    let decode_opts = RenderOpts {
        assume_binary,
        include_annotations: annotations,
        indent: 1,
        expand_any,
        hide_unknown_fields,
        expand_message_set,
    };

    // Resolve the root descriptor if a type was given explicitly.
    let root_desc: Option<prost_reflect::MessageDescriptor> = if let Some(t) = type_name {
        let lookup = t.trim_start_matches('.');
        if let Some(lazy) = &mut desc_ctx.lazy {
            lazy.get_message(lookup)
                .map_err(|e| format!("loading type '{lookup}': {e}"))?;
        }
        Some(resolve_root_desc(desc_ctx, lookup)?)
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
            match infer_type(&data, graph, scoring_opts)? {
                InferOutcome::Ambiguous(tied) => {
                    let mut rep = InferFailureReporter::new();
                    rep.report_ambiguous("<stdin>", &tied, detailed_score);
                    std::process::exit(if strict { 1 } else { 0 });
                }
                InferOutcome::Unique(inferred) => {
                    let lookup = inferred.fqdn.trim_start_matches('.');
                    if let Some(lazy) = &mut desc_ctx.lazy {
                        lazy.get_message(lookup)
                            .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
                    }
                    let infer_desc = resolve_root_desc(desc_ctx, lookup)?;
                    EXTRA_HEADER.with(|h| *h.borrow_mut() = inferred_header(&inferred));
                    install_any_loader(desc_ctx);
                    let out = process(&data, true, Some(&infer_desc), decode_opts.clone());
                    clear_any_loader();
                    EXTRA_HEADER.with(|h| h.borrow_mut().clear());
                    write_output(&out?, output.as_deref())?;
                    return Ok(());
                }
            }
        }

        install_any_loader(desc_ctx);
        let out = process(&data, true, root_desc.as_ref(), decode_opts.clone());
        clear_any_loader();
        write_output(&out?, output.as_deref())?;
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
            match infer_type(&data, graph, scoring_opts)? {
                InferOutcome::Ambiguous(tied) => {
                    let mut rep = InferFailureReporter::new();
                    rep.report_ambiguous(&f.abs.display().to_string(), &tied, detailed_score);
                    std::process::exit(if strict { 1 } else { 0 });
                }
                InferOutcome::Unique(inferred) => {
                    let lookup = inferred.fqdn.trim_start_matches('.');
                    if let Some(lazy) = &mut desc_ctx.lazy {
                        lazy.get_message(lookup)
                            .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
                    }
                    let infer_desc = resolve_root_desc(desc_ctx, lookup)?;
                    EXTRA_HEADER.with(|h| *h.borrow_mut() = inferred_header(&inferred));
                    install_any_loader(desc_ctx);
                    let out = process(&data, true, Some(&infer_desc), decode_opts.clone());
                    clear_any_loader();
                    EXTRA_HEADER.with(|h| h.borrow_mut().clear());
                    write_output(&out?, output.as_deref())?;
                    return Ok(());
                }
            }
        }

        install_any_loader(desc_ctx);
        let out = process(&data, true, root_desc.as_ref(), decode_opts.clone());
        clear_any_loader();
        write_output(&out?, output.as_deref())?;
        return Ok(());
    }

    // Batch mode.
    if !in_place && output_root.is_none() {
        return Err("multiple input files require --in-place (-i) or --output-root (-O)".into());
    }

    if auto_infer {
        return run_batch_infer(
            all_files,
            decode_opts,
            &BatchInferOpts {
                scoring_opts,
                detailed_score,
                strict,
            },
            in_place,
            output_root,
            desc_ctx,
        );
    }

    run_batch(
        all_files,
        true,
        root_desc.as_ref(),
        decode_opts,
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
        let out = process(&data, false, None, RenderOpts::default())?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    let all_files = expand_all_paths(paths, &base)?;

    if all_files.len() == 1 && !in_place && output_root.is_none() {
        let f = &all_files[0];
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
        let out = process(&data, false, None, RenderOpts::default())?;
        write_output(&out, output.as_deref())?;
        return Ok(());
    }

    if !in_place && output_root.is_none() {
        return Err("multiple input files require --in-place (-i) or --output-root (-O)".into());
    }

    run_batch(
        all_files,
        false,
        None,
        RenderOpts::default(),
        in_place,
        output_root,
    )
}

// ── list-schemas handler ──────────────────────────────────────────────────────

fn run_list_schemas(
    graph: &LoadedGraph,
    top: Option<usize>,
    assume_binary: bool,
    detailed_score: bool,
    scoring_opts: &ScoringOpts,
    input_root: &Option<PathBuf>,
    paths: &[String],
) -> Result<(), String> {
    let base = input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut out = io::stdout();

    let read_data = |raw: &[u8]| -> Result<Vec<u8>, String> {
        let opts = RenderOpts {
            assume_binary,
            include_annotations: false,
            indent: 1,
            expand_any: false,
            ..RenderOpts::default()
        };
        render_as_bytes(raw, opts)
            .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))
    };

    if paths.is_empty() {
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;
        let binary = read_data(&data)?;
        return list_schemas_one(
            &binary,
            graph,
            "<stdin>",
            top,
            detailed_score,
            scoring_opts,
            &mut out,
        );
    }

    let all_files = expand_all_paths(paths, &base)?;
    for f in &all_files {
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
        let binary = read_data(&data)?;
        let label = f.abs.display().to_string();
        list_schemas_one(
            &binary,
            graph,
            &label,
            top,
            detailed_score,
            scoring_opts,
            &mut out,
        )?;
    }
    Ok(())
}

// ── score handler ─────────────────────────────────────────────────────────────

fn run_score(
    graph: &LoadedGraph,
    type_name: &str,
    assume_binary: bool,
    scoring_opts: &ScoringOpts,
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

    let score_input = |data: &[u8]| -> Result<(bool, i64, u64, u64, u64, u64), String> {
        let binary = render_as_bytes(
            data,
            RenderOpts {
                assume_binary,
                include_annotations: false,
                indent: 1,
                expand_any: false,
                ..RenderOpts::default()
            },
        )
        .map_err(|e: CodecError| format!("encoding prototext to binary: {}", e))?;
        let result = score_one(&binary, type_name, graph, scoring_opts)
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
        let (vetoed, score, matches, unknowns, mismatches, non_canonical) = score_input(&data)?;
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
            let (vetoed, score, matches, unknowns, mismatches, non_canonical) = score_input(&data)?;
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

    fn report_ambiguous(&mut self, path: &str, tied: &[InferredType], detailed_score: bool) {
        self.ensure_heading();
        self.had_warning = true;
        let mut stderr = io::stderr();
        let _ = writeln!(stderr, "- path: {path}");
        let _ = writeln!(stderr, "  types:");
        for t in tied {
            write_type_entry(&mut stderr, "  ", t, detailed_score);
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

struct BatchInferOpts<'a> {
    scoring_opts: &'a ScoringOpts,
    detailed_score: bool,
    strict: bool,
}

fn run_batch_infer(
    all_files: Vec<InputFile>,
    opts: RenderOpts,
    infer: &BatchInferOpts<'_>,
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
        match infer_type(&data, graph, infer.scoring_opts) {
            Err(e) => reporter.report_error(&f.abs.display().to_string(), &e),
            Ok(InferOutcome::Ambiguous(tied)) => {
                reporter.report_ambiguous(&f.abs.display().to_string(), &tied, infer.detailed_score)
            }
            Ok(InferOutcome::Unique(inferred)) => successes.push((f, data, inferred)),
        }
    }

    // Second pass: process successful files (needs &mut desc_ctx for lazy loading).
    let mut had_hard_error = false;
    for (f, data, inferred) in &successes {
        let lookup = inferred.fqdn.trim_start_matches('.');
        let root_desc = match (|| {
            if let Some(lazy) = &mut desc_ctx.lazy {
                lazy.get_message(lookup)
                    .map_err(|e| format!("loading inferred type '{lookup}': {e}"))?;
            }
            resolve_root_desc(desc_ctx, lookup)
        })() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: '{}': {}", f.abs.display(), e);
                had_hard_error = true;
                continue;
            }
        };
        EXTRA_HEADER.with(|h| *h.borrow_mut() = inferred_header(inferred));
        install_any_loader(desc_ctx);
        let raw_out = process(data, true, Some(&root_desc), opts.clone());
        clear_any_loader();
        EXTRA_HEADER.with(|h| h.borrow_mut().clear());
        let raw_out = match raw_out {
            Ok(o) => o,
            Err(e) => {
                eprintln!("error: '{}': {}", f.abs.display(), e);
                had_hard_error = true;
                continue;
            }
        };
        let dest = output_path_for(f, in_place, output_root);
        if let Err(e) = write_output(&raw_out, Some(&dest)) {
            eprintln!("error: {}", e);
            had_hard_error = true;
        }
    }

    let code = if had_hard_error {
        1
    } else {
        reporter.exit_code(infer.strict)
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
    root_desc: Option<&prost_reflect::MessageDescriptor>,
    opts: RenderOpts,
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
        match process(&data, decode, root_desc, opts.clone()) {
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
