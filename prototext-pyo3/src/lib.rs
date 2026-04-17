// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// lib.rs — PyO3 module definition for `prototext_codec`.
//
// Exposes functions and one opaque handle class to Python.

use prototext_core::schema::SchemaError;
use prototext_core::{schema, serialize};

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCapsule};
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pyfunction};

use schema::{parse_schema, ParsedSchema};
use serialize::encode_text::encode_text_to_binary;
use serialize::render_text::{decode_and_render, is_prototext_text};

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
        PyCapsule::new(py, arc, Some(SCHEMA_CAPSULE_NAME.to_owned()))
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
    /// `unsafe` is required here because `PyCapsuleMethods::reference::<T>()`
    /// is an unsafe function: PyO3 cannot verify at compile time that the `T`
    /// stored in the capsule matches the `T` we request.  The correctness
    /// argument is:
    ///   1. The capsule was created by `schema_capsule()` above, which stored
    ///      an `Arc<ParsedSchema>` — the exact type we read back.
    ///   2. The name tag (`SCHEMA_CAPSULE_NAME`) provides a runtime identity
    ///      check that guards against accidentally passing an unrelated capsule.
    ///   3. The `Arc` inside the capsule is still live (the capsule holds a
    ///      strong reference), so the pointer is valid for the duration of this
    ///      call.
    ///   4. We clone the `Arc` (incrementing the refcount) rather than moving
    ///      it out, so the capsule's own refcount is not disturbed.
    pub fn from_capsule(capsule: &Bound<'_, PyCapsule>) -> PyResult<SchemaHandle> {
        let name = capsule.name()?.unwrap_or(c"");
        if name != SCHEMA_CAPSULE_NAME {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "expected capsule name {:?}, got {:?}",
                SCHEMA_CAPSULE_NAME, name,
            )));
        }
        // SAFETY: the name check above confirms this capsule was produced by
        // `schema_capsule()`, which stored an `Arc<ParsedSchema>`.  The Arc is
        // still alive because the capsule holds it.  We borrow the reference
        // only long enough to clone the Arc; the capsule continues to own the
        // original.
        let arc: Arc<ParsedSchema> =
            unsafe { Arc::clone(capsule.reference::<Arc<ParsedSchema>>()) };
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
        decode_and_render(data, Some(self.inner.as_ref()), include_annotations, indent)
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

    let parsed_schema = schema.map(|sh| sh.inner.as_ref());
    let rendered = decode_and_render(raw, parsed_schema, include_annotations, indent_size);
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
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    pyo3_stub_gen::StubInfo::from_project_root(
        "prototext_codec_lib".to_string(),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    )
}
