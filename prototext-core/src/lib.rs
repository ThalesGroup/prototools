// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

pub mod decoder;
pub mod helpers;
pub mod instantiate;
pub mod schema;
pub mod serialize;

pub use schema::{decode_pool, schema_from_pool, ParsedSchema, SchemaError};

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
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            assume_binary: false,
            include_annotations: false,
            indent: 1,
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
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::DecodeFailed(msg) => write!(f, "decode failed: {msg}"),
            CodecError::TextDecodeFailed(msg) => write!(f, "text decode failed: {msg}"),
            CodecError::NotPrototext => write!(
                f,
                "input is not prototext (missing '#@ prototext:' header); \
                 use 'prototext decode -a' to produce encodable output"
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
pub fn render_as_text(
    data: &[u8],
    schema: Option<&ParsedSchema>,
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
        schema,
        opts.include_annotations,
        opts.indent,
    ))
}

/// Encode a textual prototext payload back to raw protobuf binary wire bytes.
///
/// Encode a prototext payload back to binary protobuf.
///
/// The input must carry the `#@ prototext:` header produced by `render_as_text`.
/// Any other input — plain text, raw binary, or unannotated textproto — is
/// rejected with [`CodecError::NotPrototext`].
pub fn render_as_bytes(data: &[u8], opts: RenderOpts) -> Result<Vec<u8>, CodecError> {
    if opts.assume_binary || !serialize::render_text::is_prototext_text(data) {
        Err(CodecError::NotPrototext)
    } else {
        Ok(serialize::encode_text::encode_text_to_binary(data))
    }
}

/// Parse a compiled `.pb` descriptor into a `ParsedSchema`.
///
/// Re-exported from `schema` for convenience so callers only need to import
/// from the crate root.
pub fn parse_schema(schema_bytes: &[u8], root_message: &str) -> Result<ParsedSchema, SchemaError> {
    schema::parse_schema(schema_bytes, root_message)
}
