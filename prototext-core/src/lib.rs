// SPDX-FileCopyrightText: 2025, 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025, 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::borrow::Cow;

pub mod helpers;
pub mod instantiate;
pub mod schema;
pub mod serialize;

pub use prost_reflect::MessageDescriptor;
pub use schema::{decode_pool, schema_from_pool, ParsedSchema, SchemaError};
pub use serialize::render_text::{
    build_arena, clear_any_loader, is_prototext_text, set_any_loader, set_ext_loader, AnyLoader,
    Arena, ExtLoader, ExtLoaderGuard, NodeKind, Shape,
};

// ── Public API types ──────────────────────────────────────────────────────────

/// Options controlling how a protobuf binary payload is rendered as text.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    /// When `true`, always treat the input as raw protobuf binary.
    /// When `false`, auto-detect: if the payload already carries the
    /// `#@ prototext:` header it is returned unchanged (zero-copy fast path).
    pub assume_binary: bool,
    /// Emit inline comments with schema field names and types.
    pub include_annotations: bool,
    /// Indentation step in spaces.
    pub indent: usize,
    /// When `true` (default), expand `google.protobuf.Any` fields inline
    /// using the type resolved from `type_url` (spec 0089).
    pub expand_any: bool,
    /// When `true`, suppress fields absent from the schema (unknown fields,
    /// wire-type mismatches).  No effect when no schema is active (raw mode).
    /// Default: `false` (show unknown fields).  (spec 0103)
    pub hide_unknown_fields: bool,
    /// When `true` (default), expand MessageSet groups inline.
    /// Independent of `expand_any`.  (spec 0103)
    pub expand_message_set: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
            expand_any: true,
            hide_unknown_fields: false,
            expand_message_set: true,
        }
    }
}

/// Errors that can occur while decoding or encoding a protobuf payload.
#[non_exhaustive]
#[derive(Debug)]
pub enum CodecError {
    /// The input bytes could not be decoded as a protobuf wire payload.
    DecodeFailed(String),
    /// The input bytes could not be decoded as a textual prototext payload.
    TextDecodeFailed(String),
    /// The input does not carry the `#@ prototext:` header required by `encode`.
    NotPrototext,
    /// The input is larger than `decode_and_render_indexed` can index
    /// (spec 0212 S1): a `NodeSpan`'s offsets are `u32`, and that render's
    /// own output reservation is eight times the input, so a buffer this
    /// size was already fatal before it was refused.
    InputTooLarge { len: usize, max: usize },
    /// The input's maximal wire tree nests as deep as the walk's recursion
    /// cap (spec 0216 S9). At that depth the renderer stops descending and
    /// hands the payload back opaquely, so a structural decomposition built
    /// there would be missing nodes the renderer could later be asked to
    /// draw. Refusing the whole blob is the only safe answer; truncating
    /// the tree is the missing-slot failure the decomposition exists to
    /// rule out.
    InputTooDeep { max: usize },
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::DecodeFailed(msg) => write!(f, "decode failed: {msg}"),
            CodecError::TextDecodeFailed(msg) => write!(f, "text decode failed: {msg}"),
            CodecError::NotPrototext => write!(
                f,
                "input is not prototext (missing '#@ prototext:' header); \
                 use 'prototext decode' to produce encodable output (annotations on by default)"
            ),
            CodecError::InputTooLarge { len, max } => write!(
                f,
                "input is too large to index: {len} bytes, limit {max} \
                 ({} MiB)",
                max / (1024 * 1024)
            ),
            CodecError::InputTooDeep { max } => write!(
                f,
                "input nests too deeply to decompose: the wire depth limit is {max} levels"
            ),
        }
    }
}

impl std::error::Error for CodecError {}

// ── Public API functions ──────────────────────────────────────────────────────

/// Decode a raw protobuf binary payload and render it as protoc-style text.
///
/// When `opts.assume_binary` is `false` and the data already carries the
/// `#@ prototext:` header, it is first encoded back to binary so that the
/// schema-aware decoder can re-render it (e.g. with a different schema or
/// annotation settings).  With `assume_binary: true` the data is always
/// treated as raw binary wire bytes.
///
/// `root_desc` is the already-resolved root message descriptor, if any.
/// Callers that only have a `ParsedSchema` can pass
/// `schema.root_descriptor().as_ref()`.
pub fn render_as_text(
    data: &[u8],
    root_desc: Option<&MessageDescriptor>,
    opts: RenderOpts,
) -> Result<Vec<u8>, CodecError> {
    let binary;
    let wire = if !opts.assume_binary && serialize::render_text::is_prototext_text(data) {
        binary = serialize::encode_text::encode_text_to_binary(data);
        binary.as_slice()
    } else {
        data
    };
    Ok(serialize::render_text::decode_and_render(
        wire,
        root_desc,
        serialize::render_text::DecodeRenderOpts {
            annotations: opts.include_annotations,
            indent_size: opts.indent,
            expand_any: opts.expand_any,
            hide_unknown_fields: opts.hide_unknown_fields,
            expand_message_set: opts.expand_message_set,
            initial_level: 0,
            emit_header: opts.include_annotations,
            // The public API renders whole documents; only `protolens`'s
            // viewport asks to be bounded (spec 0249 S1).
            row_budget: None,
            missing_payload_bytes: None,
        },
    ))
}

/// Encode a textual prototext payload back to raw protobuf binary wire bytes.
///
/// When `opts.assume_binary` is `true`, or the input does not carry the
/// `#@ prototext:` header, the bytes are returned unchanged (pass-through).
/// When the input carries the header, it is decoded from text to binary.
///
/// The pass-through branch **borrows**. It is by far the common one — every
/// already-binary blob takes it — and it has, by definition, nothing to do;
/// returning an owned `Vec` there meant copying the whole input to hand back
/// what the caller already had. On protolens's startup path that was a
/// blob-sized `memcpy` before a single byte had been read (spec 0216 S28).
/// A caller that genuinely needs ownership can still ask for it with
/// `into_owned`, and pays the copy only then.
pub fn render_as_bytes(data: &[u8], opts: RenderOpts) -> Result<Cow<'_, [u8]>, CodecError> {
    if opts.assume_binary || !serialize::render_text::is_prototext_text(data) {
        Ok(Cow::Borrowed(data))
    } else {
        Ok(Cow::Owned(serialize::encode_text::encode_text_to_binary(
            data,
        )))
    }
}

/// Parse a compiled `.pb` descriptor into a `ParsedSchema`.
///
/// Re-exported from `schema` for convenience so callers only need to import
/// from the crate root.
pub fn parse_schema(schema_bytes: &[u8], root_message: &str) -> Result<ParsedSchema, SchemaError> {
    schema::parse_schema(schema_bytes, root_message)
}
