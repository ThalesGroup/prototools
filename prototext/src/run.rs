// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use prototext_core::{parse_schema, render_as_bytes, render_as_text, CodecError, RenderOpts};

use crate::inputs::{expand_path, InputFile};
use prototext_core::instantiate::{generate_message_bytes, InstantiateOpts};

use crate::{Cli, Command, EMBEDDED_DESCRIPTOR};

// ── Schema loading ────────────────────────────────────────────────────────────

pub fn load_schema(
    descriptor: Option<&PathBuf>,
    type_name: Option<&String>,
) -> Result<Option<prototext_core::ParsedSchema>, String> {
    match (descriptor, type_name) {
        (None, None) => Ok(None),
        (None, Some(t)) => {
            let schema = parse_schema(EMBEDDED_DESCRIPTOR, t)
                .map_err(|e| format!("embedded descriptor: {}", e))?;
            Ok(Some(schema))
        }
        (Some(_), None) => Err("--descriptor requires --type".into()),
        (Some(d), Some(t)) => {
            let bytes = std::fs::read(d)
                .map_err(|e| format!("cannot read descriptor '{}': {}", d.display(), e))?;
            let schema = parse_schema(&bytes, t).map_err(|e| format!("descriptor parse: {}", e))?;
            Ok(Some(schema))
        }
    }
}

// ── Per-file processing ───────────────────────────────────────────────────────

pub fn process(
    data: &[u8],
    decode: bool,
    schema: Option<&prototext_core::ParsedSchema>,
    annotations: bool,
) -> Result<Vec<u8>, String> {
    let opts = RenderOpts {
        assume_binary: decode,
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

/// Compute the output path for a file in batch mode.
/// Precondition: `cli.in_place || cli.output_root.is_some()`.
pub fn output_path_for(f: &InputFile, cli: &Cli) -> PathBuf {
    if let Some(root) = &cli.output_root {
        root.join(&f.rel)
    } else {
        // --in-place: overwrite the original file (sponge semantics).
        f.abs.clone()
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

// ── Top-level run ─────────────────────────────────────────────────────────────

pub fn run(cli: Cli) -> Result<(), String> {
    // ── Subcommand dispatch ───────────────────────────────────────────────────
    if let Some(Command::InstantiateSchema {
        r#type,
        seed,
        max_depth,
        max_repeated,
        p_optional,
    }) = cli.command
    {
        return run_instantiate_schema(
            cli.descriptor.as_ref(),
            &r#type,
            seed,
            max_depth,
            max_repeated,
            p_optional,
            cli.quiet,
            cli.output.as_deref(),
        );
    }

    // ── Validate mode ─────────────────────────────────────────────────────────
    if !cli.decode && !cli.encode {
        return Err("one of --decode (-d) or --encode (-e) is required".into());
    }
    let decode = cli.decode;
    // clap `overrides_with` ensures at most one of the two flags is set in
    // argv, but the bool fields still reflect their defaults when absent.
    // The effective value is: annotations=true unless --no-annotations was given.
    let annotations = !cli.no_annotations;

    // ── Validate absolute paths when --input-root is given ────────────────────
    if cli.input_root.is_some() {
        for raw in &cli.paths {
            if Path::new(raw).is_absolute() {
                return Err(format!(
                    "absolute path '{}' is not allowed when --input-root is given",
                    raw
                ));
            }
        }
    }

    // ── Validate --input-root and --output-root not the same dir ──────────────
    if let (Some(ir), Some(or)) = (&cli.input_root, &cli.output_root) {
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

    // ── Load schema ───────────────────────────────────────────────────────────
    let schema = load_schema(cli.descriptor.as_ref(), cli.r#type.as_ref())?;
    let schema_ref = schema.as_ref();

    // ── Resolve base dir ──────────────────────────────────────────────────────
    let base = cli
        .input_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // ── Stdin path ────────────────────────────────────────────────────────────
    if cli.paths.is_empty() {
        if cli.in_place {
            return Err("--in-place cannot be used with stdin input".into());
        }
        if cli.output_root.is_some() {
            return Err("--output-root cannot be used with stdin input".into());
        }
        let mut data = Vec::new();
        io::stdin()
            .read_to_end(&mut data)
            .map_err(|e| format!("reading stdin: {}", e))?;
        let out = process(&data, decode, schema_ref, annotations)?;
        write_output(&out, cli.output.as_deref())?;
        return Ok(());
    }

    // ── Expand positional paths ───────────────────────────────────────────────
    let mut all_files: Vec<InputFile> = Vec::new();
    let mut expand_errors: Vec<String> = Vec::new();

    for raw in &cli.paths {
        match expand_path(raw, &base) {
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

    // ── Single file (no batch flags) ──────────────────────────────────────────
    if all_files.len() == 1 && !cli.in_place && cli.output_root.is_none() {
        let f = &all_files[0];
        let data =
            std::fs::read(&f.abs).map_err(|e| format!("reading '{}': {}", f.abs.display(), e))?;
        let out = process(&data, decode, schema_ref, annotations)?;
        write_output(&out, cli.output.as_deref())?;
        return Ok(());
    }

    // ── Batch mode ────────────────────────────────────────────────────────────
    if !cli.in_place && cli.output_root.is_none() {
        return Err("multiple input files require --in-place (-i) or --output-root (-O)".into());
    }

    // Detect output collisions eagerly.
    {
        let mut seen: HashMap<PathBuf, &InputFile> = HashMap::new();
        for f in &all_files {
            let out_path = output_path_for(f, &cli);
            if let Some(prev) = seen.get(&out_path) {
                return Err(format!(
                    "output collision: '{}' and '{}' both map to '{}'",
                    prev.abs.display(),
                    f.abs.display(),
                    out_path.display()
                ));
            }
            seen.insert(out_path, f);
        }
    }

    // In-place: read all files into memory before writing any of them (sponge
    // semantics — safe when input and output are the same path).
    // Each entry is (file, Some(preread_bytes)) for in-place, (file, None) otherwise.
    let file_data: Vec<(InputFile, Option<Vec<u8>>)> = if cli.in_place {
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

        let dest = output_path_for(&f, &cli);
        match process(&data, decode, schema_ref, annotations) {
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

// ── instantiate-schema ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_instantiate_schema(
    descriptor: Option<&PathBuf>,
    type_name: &str,
    seed: i64,
    max_depth: usize,
    max_repeated: usize,
    p_optional: f64,
    quiet: bool,
    output: Option<&Path>,
) -> Result<(), String> {
    // Normalize type name: strip leading dot for descriptor lookup, but keep
    // the dotted form for the ground_truth hint comment.
    let fqdn = if type_name.starts_with('.') {
        type_name.to_string()
    } else {
        format!(".{}", type_name)
    };
    let lookup_name = fqdn.trim_start_matches('.');

    // Load the descriptor bytes.
    let desc_bytes: Vec<u8> = if let Some(path) = descriptor {
        std::fs::read(path)
            .map_err(|e| format!("cannot read descriptor '{}': {}", path.display(), e))?
    } else {
        EMBEDDED_DESCRIPTOR.to_vec()
    };

    // Locate the message descriptor.
    let schema =
        parse_schema(&desc_bytes, lookup_name).map_err(|e| format!("descriptor parse: {}", e))?;

    let opts = InstantiateOpts {
        seed,
        max_depth,
        max_repeated,
        p_optional,
        quiet,
    };

    // Locate the root MessageDescriptor (guaranteed present — parse_schema
    // returns MessageNotFound if the type is absent).
    let msg_desc = schema
        .root_descriptor()
        .ok_or_else(|| format!("type '{}' not found in descriptor", lookup_name))?;

    // Generate binary protobuf bytes.
    let binary = generate_message_bytes(&msg_desc, &opts);

    // Decode to #@ prototext.
    let render_opts = RenderOpts {
        assume_binary: true,
        include_annotations: true,
        indent: 1,
    };
    let text_bytes = render_as_text(&binary, Some(&schema), render_opts)
        .map_err(|e: CodecError| e.to_string())?;

    // Insert hint comments after the magic line.
    let text = String::from_utf8(text_bytes)
        .map_err(|e| format!("generated text is not valid UTF-8: {}", e))?;
    let out = insert_hints(&text, &fqdn, seed);

    write_output(out.as_bytes(), output)
}

/// Insert `# ground_truth:` and `# seed:` comment lines after the first line
/// (the `#@ prototext:` magic line).
fn insert_hints(text: &str, fqdn: &str, seed: i64) -> String {
    let mut lines = text.splitn(2, '\n');
    let magic = lines.next().unwrap_or("");
    let rest = lines.next().unwrap_or("");
    format!(
        "{}\n# ground_truth: {}\n# seed: {}\n{}",
        magic, fqdn, seed, rest
    )
}
