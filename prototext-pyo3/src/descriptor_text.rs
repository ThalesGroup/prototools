// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// descriptor_text.rs — descriptors in the text format, read back
// (spec 0424 S5, spec 0426), for reproto.
//
// Two passes (spec 0427): `parse_descriptor_text` parses the text of a
// `FileDescriptorProto` or a `FileDescriptorSet` with `descriptor.proto`
// alone, which gives each file's imports and declarations;
// `DescriptorPool`, built from every file of the input, parses each text
// file again, its options resolved by the input's declarations.

use std::sync::OnceLock;

use prototext_upb::text_format::{self, Options};
use prototext_upb::{Message, Pool};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pyfunction, gen_stub_pymethods};

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
///     alone: the first pass of spec 0427, which gives a file's imports
///     and declarations.  A field printed by number is kept, ``N { … }``
///     length-delimited; a field or an extension by a name
///     ``descriptor.proto`` does not declare is skipped.  Parse the file
///     again with a `DescriptorPool` of the input for its options.
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

/// The pool of an input's files, to parse its text files with in a
/// second pass (spec 0427).
///
/// Built from every file of the input, binary as it is, text as
/// `parse_descriptor_text` gave it; `parse_file_text` then parses a text
/// file again, its options by name resolved and by number framed from the
/// input's declarations.
#[gen_stub_pyclass]
#[pyclass]
pub struct DescriptorPool {
    file_type: Message,
    skipped: Vec<(String, String)>,
}

#[gen_stub_pymethods]
#[pymethods]
impl DescriptorPool {
    /// Parameters
    /// ----------
    /// files : list[bytes]
    ///     Every file of the input, serialized ``FileDescriptorProto``\ s,
    ///     in any order: they are loaded imports first.
    /// descriptor_file : str
    ///     The input's ``descriptor.proto`` (the variant's, e.g.
    ///     ``net/proto2/proto/descriptor.proto``): its ``FileDescriptorProto``
    ///     is the type text files are parsed as.  The vendored one is
    ///     added when no file of ``files`` has that name.
    #[new]
    fn new(files: Vec<Bound<'_, PyBytes>>, descriptor_file: &str) -> PyResult<Self> {
        let mut set = Vec::new();
        for f in &files {
            put_len(&mut set, 1, f.as_bytes());
        }
        let (pool, report) = text_format::load_input(&set, descriptor_file);
        // The vendored descriptor.proto declares it: never an error, but
        // an extension module must not panic.
        let file_type = text_format::descriptor_type(&pool, descriptor_file, "FileDescriptorProto")
            .ok_or_else(|| PyValueError::new_err("no FileDescriptorProto to parse with"))?;
        Ok(DescriptorPool {
            file_type,
            skipped: report.skipped,
        })
    }

    /// ``(file, reason)`` for each file that did not load: its
    /// declarations are not in the pool.
    #[getter]
    fn skipped(&self) -> Vec<(String, String)> {
        self.skipped.clone()
    }

    /// Parse ``text``, one ``FileDescriptorProto`` in the text format, with
    /// the pool.
    ///
    /// Returns the serialized file, and one warning per value the pool
    /// could not settle (spec 0426 S4, spec 0427 S4).  Raises
    /// ``ValueError`` (``"LINE:COLUMN: message"``) when ``text`` does not
    /// parse.
    fn parse_file_text<'py>(
        &self,
        py: Python<'py>,
        text: &str,
    ) -> PyResult<(Bound<'py, PyBytes>, Vec<String>)> {
        let parsed = text_format::parse_reporting(&self.file_type, text, LENIENT)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let warnings = parsed.warnings.iter().map(|w| w.to_string()).collect();
        Ok((PyBytes::new(py, &parsed.bytes), warnings))
    }
}

fn put_len(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    let mut put_varint = |mut v: u64| {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    };
    put_varint((u64::from(number) << 3) | 2);
    put_varint(payload.len() as u64);
    out.extend_from_slice(payload);
}
