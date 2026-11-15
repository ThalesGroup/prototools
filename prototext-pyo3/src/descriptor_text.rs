// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// descriptor_text.rs — descriptors in the text format, read back
// (spec 0424 S5, spec 0426), for reproto.
//
// Descriptors in the text format (spec 0427): `read_descriptors` reads
// an input's files in import order, each text parsed with the files read
// before it; `scan_descriptor_text` and `split_descriptor_set` give a
// file's imports and a set's files without parsing;
// `parse_descriptor_text` parses with `descriptor.proto` alone, for what
// must see a file before the input is complete (an FDP plugin).

use std::sync::OnceLock;

use prototext_upb::input::{read_input, Input, Read};
use prototext_upb::text_format::{self, Options};
use prototext_upb::Pool;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::gen_stub_pyfunction;

/// The vendored `descriptor.proto`, alone: the types of a descriptor, and
/// none of the extensions an input declares.
fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(Pool::with_descriptor_proto)
}

/// Unknown names skipped: a field of a newer `descriptor.proto`, an
/// extension the pool does not declare (reported, spec 0427 S4).
const LENIENT: Options = Options {
    allow_unknown_field: true,
    allow_unknown_extension: true,
};

/// Parse a descriptor in the text format into the binary format.
///
/// Parameters
/// ----------
/// text : str
///     A ``FileDescriptorProto`` in the text format, or a
///     ``FileDescriptorSet`` when ``is_set`` is true, as
///     ``protoc --decode`` prints it.
/// is_set : bool, optional
///     Parse ``text`` as a ``FileDescriptorSet``.  Default ``False``.
///
/// Returns
/// -------
/// bytes
///     The serialized message, read with the vendored ``descriptor.proto``
///     alone, for what must see a file before the input is complete (an
///     FDP plugin).  A field printed by number is kept, ``N { … }``
///     length-delimited; a field or an extension by a name
///     ``descriptor.proto`` does not declare is skipped.  Read the input
///     with `read_descriptors` for its options.
///
/// Raises
/// ------
/// ValueError
///     ``"LINE:COLUMN: message"`` when ``text`` does not parse.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (text, is_set=false))]
pub fn parse_descriptor_text<'py>(
    py: Python<'py>,
    text: &str,
    is_set: bool,
) -> PyResult<Bound<'py, PyBytes>> {
    let name = if is_set {
        "google.protobuf.FileDescriptorSet"
    } else {
        "google.protobuf.FileDescriptorProto"
    };
    let def = pool()
        .get_message_by_name(name)
        .expect("the vendored descriptor.proto declares it");
    let bytes = text_format::parse(&def, text, LENIENT)
        .map_err(|e| PyValueError::new_err(format!("{}:{}: {}", e.line, e.column, e.message)))?;
    Ok(PyBytes::new(py, &bytes))
}

/// The name and imports of a ``FileDescriptorProto`` in the text format,
/// from its top-level ``name`` and ``dependency`` fields, without parsing
/// the rest (spec 0427 S1).
///
/// Raises ``ValueError`` (``"LINE:COLUMN: message"``) when a string does
/// not end or the brackets do not match.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn scan_descriptor_text(text: &str) -> PyResult<(String, Vec<String>)> {
    text_format::scan_imports(text).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Each ``file { … }`` of a ``FileDescriptorSet`` in the text format, as
/// the text inside its brackets: one ``FileDescriptorProto`` each.
///
/// Raises ``ValueError`` (``"LINE:COLUMN: message"``) when a string does
/// not end or the brackets do not match.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn split_descriptor_set(text: &str) -> PyResult<Vec<String>> {
    text_format::split_set(text)
        .map(|files| files.into_iter().map(str::to_owned).collect())
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// What became of one text: its bytes, its warnings, and why it does not
/// parse.
type TextRead<'py> = (Option<Bound<'py, PyBytes>>, Vec<String>, Option<String>);

/// Read an input's descriptors in import order (spec 0427).
///
/// Every file goes into one pool, imports first; each text file is
/// parsed when its turn comes, its options by name resolved and by number
/// framed from the files read before it, and parsed once more at the end
/// where that left a value declared nowhere.
///
/// Parameters
/// ----------
/// binary : list[bytes]
///     The input's serialized ``FileDescriptorProto``\ s.
/// text : list[str]
///     The input's ``FileDescriptorProto``\ s in the text format.
/// descriptor_file : str
///     The input's ``descriptor.proto`` (the variant's, e.g.
///     ``net/proto2/proto/descriptor.proto``), whose
///     ``FileDescriptorProto`` the texts are parsed as; the vendored one
///     when no input has that name.
///
/// Returns
/// -------
/// tuple[list[tuple[bytes | None, list[str], str | None]], list[tuple[str, str]]]
///     For each text, in order: its bytes, its warnings (spec 0426 S4,
///     spec 0427 S6), and ``None``; or ``None``, ``[]`` and why it does not
///     parse (``"LINE:COLUMN: message"``). Then ``(file, reason)`` for each
///     file the pool could not hold, even mended.
#[gen_stub_pyfunction]
#[pyfunction]
pub fn read_descriptors<'py>(
    py: Python<'py>,
    binary: Vec<Bound<'py, PyBytes>>,
    text: Vec<String>,
    descriptor_file: &str,
) -> (Vec<TextRead<'py>>, Vec<(String, String)>) {
    let mut inputs: Vec<Input> = binary.iter().map(|b| Input::Binary(b.as_bytes())).collect();
    inputs.extend(text.iter().map(|t| Input::Text(t)));
    let (_, reads, report) = read_input(&inputs, descriptor_file);
    let texts = reads[binary.len()..]
        .iter()
        .map(|r| match r {
            Read::Parsed {
                bytes, warnings, ..
            } => (
                Some(PyBytes::new(py, bytes)),
                warnings.iter().map(|w| w.to_string()).collect(),
                None,
            ),
            Read::Error(e) => (None, Vec::new(), Some(e.to_string())),
            Read::Binary => unreachable!("a text input"),
        })
        .collect();
    (texts, report.skipped)
}
