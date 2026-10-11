// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// lib.rs — PyO3 module definition for `prototext_codec`.
//
// Exposes functions and one opaque handle class to Python.

// `cargo test` links this library as its own test binary, which runs
// Python and so links libpython; the extension itself does not. See build.rs
// (spec 0402 S6).
#[cfg(test)]
include!(concat!(env!("OUT_DIR"), "/libpython.rs"));

use prototext_core::schema::SchemaError;
use prototext_core::{schema, serialize};

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCapsule};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pyfunction};

use schema::{parse_schema, ParsedSchema};
use serialize::encode_text::encode_text_to_binary;
use serialize::render_text::{
    clear_any_loader, decode_and_render, is_prototext_text, set_any_loader, DecodeRenderOpts,
};

// ── SchemaHandle ──────────────────────────────────────────────────────────────

/// Opaque handle to a parsed protobuf schema.
///
/// Created once per unique schema via `register_schema()`.
/// Lifetime managed by Python reference counting.
#[gen_stub_pyclass]
#[pyclass]
pub struct SchemaHandle {
    inner: Arc<ParsedSchema>,
}

// Name used to tag the PyCapsule so consumers can verify they received the
// right kind of capsule (guards against accidental misuse with unrelated capsules).
pub const SCHEMA_CAPSULE_NAME: &std::ffi::CStr = c"prototext_codec.SchemaHandle.ParsedSchema";

#[pymethods]
impl SchemaHandle {
    /// Return an opaque `PyCapsule` wrapping a cloned `Arc<ParsedSchema>`.
    ///
    /// --- Why capsules? ---
    /// PyO3 registers `#[pyclass]` types per shared library (.so).  A
    /// `SchemaHandle` created by `prototext_codec.so` cannot be downcast inside
    /// `protosets.so` because each .so has its own copy of the type-registration
    /// vtable and the type IDs do not match.
    ///
    /// A `PyCapsule` sidesteps this: it is an opaque Python object that carries
    /// an owned Rust value of any `T: 'static + Send`.  Any extension can call
    /// `schema_capsule()` on the Python side and receive a capsule it can
    /// unwrap — no type-registry lookup involved.
    ///
    /// --- Lifetime & reference counting ---
    /// `PyCapsule::new` takes ownership of the `Arc` clone.  PyO3's internal
    /// destructor drops it when Python garbage-collects the capsule, decrementing
    /// the `Arc` refcount.  The original `SchemaHandle` (still held by Python)
    /// also owns a clone, so the `ParsedSchema` data lives until *both* the
    /// handle and the capsule are collected.
    fn schema_capsule<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        let arc: Arc<ParsedSchema> = Arc::clone(&self.inner);
        PyCapsule::new_with_value(py, arc, SCHEMA_CAPSULE_NAME)
    }
}

impl SchemaHandle {
    /// Reconstruct a `SchemaHandle` from a `PyCapsule` produced by
    /// `schema_capsule()`.
    ///
    /// Called from other extension crates that cannot downcast the Python
    /// `SchemaHandle` object directly because PyO3's type registry is per-`.so`.
    ///
    /// # Safety
    ///
    /// `unsafe` is required here because the pointer returned by
    /// `PyCapsuleMethods::pointer_checked()` is an untyped `NonNull<c_void>`:
    /// PyO3 cannot verify at compile time that the `T` stored in the capsule
    /// matches the `T` we cast it to. The correctness argument is:
    ///   1. The capsule was created by `schema_capsule()` above, which stored
    ///      an `Arc<ParsedSchema>` — the exact type we read back.
    ///   2. `pointer_checked()`'s name check (`SCHEMA_CAPSULE_NAME`) provides a
    ///      runtime identity check that guards against accidentally passing an
    ///      unrelated capsule.
    ///   3. The `Arc` inside the capsule is still live (the capsule holds a
    ///      strong reference), so the pointer is valid for the duration of this
    ///      call.
    ///   4. We clone the `Arc` (incrementing the refcount) rather than moving
    ///      it out, so the capsule's own refcount is not disturbed.
    pub fn from_capsule(capsule: &Bound<'_, PyCapsule>) -> PyResult<SchemaHandle> {
        let ptr = capsule.pointer_checked(Some(SCHEMA_CAPSULE_NAME))?;
        // SAFETY: the name check inside `pointer_checked()` above confirms this
        // capsule was produced by `schema_capsule()`, which stored an
        // `Arc<ParsedSchema>`. The Arc is still alive because the capsule holds
        // it. We borrow the reference only long enough to clone the Arc; the
        // capsule continues to own the original.
        let arc: Arc<ParsedSchema> =
            unsafe { Arc::clone(&*(ptr.as_ptr() as *const Arc<ParsedSchema>)) };
        Ok(SchemaHandle { inner: arc })
    }

    /// Build a `SchemaHandle` directly from parsed schema bytes and a root
    /// message name.  Equivalent to `register_schema()` but usable from Rust
    /// without a Python runtime.
    pub fn from_bytes(schema_bytes: &[u8], root_message: &str) -> Result<Self, SchemaError> {
        let parsed = parse_schema(schema_bytes, root_message)?;
        Ok(SchemaHandle {
            inner: Arc::new(parsed),
        })
    }

    /// Rust-to-Rust rendering path — decode `data` and render as protoc-style
    /// text.  When `assume_binary` is false and the data already carries the
    /// `#@ prototext:` header the bytes are returned unchanged (zero-copy fast
    /// path).
    pub fn render_as_text(
        &self,
        data: &[u8],
        assume_binary: bool,
        include_annotations: bool,
        indent: usize,
    ) -> Vec<u8> {
        if !assume_binary && is_prototext_text(data) {
            return data.to_vec();
        }
        let root_desc = self.inner.root_descriptor();
        // Install a JIT loader (spec 0099/0106 S2) so Any/MessageSet types
        // discovered during rendering resolve against the full schema pool,
        // not just the root message's directly-reachable types.
        set_any_loader(Box::new({
            let schema = Arc::clone(&self.inner);
            move |fqdn: &str| schema.get_descriptor(fqdn).map(Arc::new)
        }));
        let out = decode_and_render(
            data,
            root_desc.as_ref(),
            DecodeRenderOpts {
                annotations: include_annotations,
                indent_size: indent,
                emit_header: include_annotations,
                ..Default::default()
            },
        );
        clear_any_loader();
        out
    }
}

// ── API functions ─────────────────────────────────────────────────────────────

/// Parse and register a protobuf schema for use with `format_as_text()`.
///
/// Parameters
/// ----------
/// schema_data : bytes
///     A serialised ``FileDescriptorProto`` or ``FileDescriptorSet``, in
///     either raw binary wire format **or** textual prototext format (i.e.
///     starting with ``b"#@ prototext:"``, as produced by
///     ``format_as_text()``).  The format is auto-detected and normalised to
///     binary before the schema is parsed.
///     Pass ``b""`` together with ``root_message=""`` to create a no-schema
///     handle (all fields decoded as generic wire types).
/// root_message : str
///     Fully-qualified message name, e.g. ``"my.package.MyMessage"``.
///     The leading dot is optional.
///
/// Returns
/// -------
/// SchemaHandle
///     Opaque handle; pass to `format_as_text()`.
#[gen_stub_pyfunction]
#[pyfunction]
fn register_schema(schema_data: Bound<'_, PyBytes>, root_message: &str) -> PyResult<SchemaHandle> {
    let raw = schema_data.as_bytes();
    let decoded;
    let schema_bytes: &[u8] = if is_prototext_text(raw) {
        decoded = encode_text_to_binary(raw);
        &decoded
    } else {
        raw
    };
    let parsed = parse_schema(schema_bytes, root_message)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    Ok(SchemaHandle {
        inner: Arc::new(parsed),
    })
}

/// Decode a protobuf payload and render it as protoc-style text.
///
/// Parameters
/// ----------
/// data : bytes
///     The input payload.  When ``assume_binary`` is ``False`` (default) the
///     function auto-detects the format: if the payload starts with the
///     ``#@ prototext:`` magic header it is already textual and returned
///     unchanged (zero-copy fast path).  Otherwise it is treated as raw
///     protobuf binary and decoded+rendered in a single pass.
///     When ``assume_binary=True`` the auto-detection step is skipped and the
///     payload is always decoded as raw protobuf binary.
/// schema : SchemaHandle, optional
///     Schema handle from `register_schema()`.
/// assume_binary : bool, optional
///     Skip format auto-detection and treat ``data`` as raw protobuf binary.
///     Default ``False``.
/// include_annotations : bool, optional
///     Emit inline comments with schema field names and types.  Default
///     ``False``.
/// indent : int, optional
///     Indentation step in spaces used by the protoc text renderer.
///     Default ``1``.
///
/// Returns
/// -------
/// bytes
///     Protoc-style text representation of the decoded message, encoded as
///     UTF-8 bytes.  The fast path returns the original ``data`` object
///     unchanged (zero copy).
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (data, schema=None, assume_binary=false, include_annotations=false, indent=1))]
fn format_as_text<'py>(
    py: Python<'py>,
    data: Bound<'py, PyBytes>,
    schema: Option<&SchemaHandle>,
    assume_binary: bool,
    include_annotations: bool,
    indent: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    let indent_size = indent.max(0) as usize;
    let raw = data.as_bytes();

    if !assume_binary && is_prototext_text(raw) {
        return Ok(data);
    }

    let root_desc = schema.and_then(|sh| sh.inner.root_descriptor());
    if let Some(sh) = schema {
        // Install a JIT loader (spec 0099/0106 S2) so Any/MessageSet types
        // discovered during rendering resolve against the full schema pool,
        // not just the root message's directly-reachable types.
        set_any_loader(Box::new({
            let schema = Arc::clone(&sh.inner);
            move |fqdn: &str| schema.get_descriptor(fqdn).map(Arc::new)
        }));
    }
    let rendered = decode_and_render(
        raw,
        root_desc.as_ref(),
        DecodeRenderOpts {
            annotations: include_annotations,
            indent_size,
            emit_header: include_annotations,
            ..Default::default()
        },
    );
    if schema.is_some() {
        clear_any_loader();
    }
    Ok(PyBytes::new(py, &rendered))
}

/// Decode a protobuf payload and return it as raw binary wire bytes.
///
/// Parameters
/// ----------
/// data : bytes
///     The input payload.  Interpretation depends on ``assume_binary``.
/// assume_binary : bool, optional
///     When ``True``, ``data`` is already a raw protobuf binary wire payload
///     and is returned unchanged (passthrough — prototext is bit-level
///     faithful and never normalises the wire encoding).
///     When ``False`` (default), the format is auto-detected:
///
///   * If `data` starts with `b"#@ prototext:"` it is textual prototext
///     produced by `format_as_text`; it is decoded line-by-line and
///     re-encoded directly to binary wire bytes.
///   * Otherwise `data` is returned unchanged.
///
/// Returns
/// -------
/// bytes
///     Raw protobuf binary wire bytes.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (data, assume_binary=false))]
fn format_as_bytes<'py>(
    py: Python<'py>,
    data: Bound<'py, PyBytes>,
    assume_binary: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let raw = data.as_bytes();

    if assume_binary {
        // Already binary — return unchanged (bit-faithful passthrough).
        return Ok(data);
    }

    // Auto-detect: text prototext → decode to wire bytes; raw binary → pass through.
    let decoded;
    let bytes: &[u8] = if is_prototext_text(raw) {
        decoded = encode_text_to_binary(raw);
        &decoded
    } else {
        raw
    };
    Ok(PyBytes::new(py, bytes))
}

// ── Python module ─────────────────────────────────────────────────────────────

/// Rust extension for lossless protobuf decoding.
#[pymodule]
fn prototext_codec_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(register_schema, m)?)?;
    m.add_function(wrap_pyfunction!(format_as_text, m)?)?;
    m.add_function(wrap_pyfunction!(format_as_bytes, m)?)?;
    m.add_class::<SchemaHandle>()?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
///
/// Uses std::env::var (runtime) rather than env!() (compile-time) so that the
/// binary works correctly when Cargo reuses it from a prior build's artifact
/// cache (different sandbox paths under Crane/Nix).
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set when running prototext_post_build"),
    );
    let mut info =
        pyo3_stub_gen::StubInfo::from_pyproject_toml(manifest_dir.join("pyproject.toml"))?;
    // Generate aside, into target/stubs/<crate dir>/, never over the
    // committed stub; the Nix build compares the two (spec 0409 S2).
    let crate_dir = manifest_dir
        .file_name()
        .expect("CARGO_MANIFEST_DIR has a final component");
    info.python_root = manifest_dir.join("../target/stubs").join(crate_dir);
    Ok(info)
}
