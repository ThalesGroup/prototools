// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The protobuf text format, parsed into the binary format (spec 0424 S5).
//!
//! [`parse`] reads text to protobuf.dev's Text Format Language
//! Specification and encodes it straight into the binary format, guided by
//! a pool's definitions, as `protoc --encode` does. The bytes decode into a
//! [`DynamicMessage`](crate::DynamicMessage), or anywhere else.
//!
//! **Fields by number.** `protoc --decode` prints a field it does not know
//! by number, and the parser keeps it, as protoc printed it: a decimal
//! number is a varint, `0x` with 8 or 16 hex digits a fixed32 or a fixed64,
//! a string a length-delimited field, `N { … }` a group of the same kind of
//! fields. protoc prints that way a group, a length-delimited message, and
//! a string whose bytes happen to read as a message. The parser frames
//! each one by the declaration of its number in the pool it parses with,
//! and takes the likely framing, with a [`Warning`], where there is none
//! (spec 0426). An input's descriptors are read in import order, with a
//! pool of the files read before, which declares their options
//! ([`crate::input`], spec 0427).
//!
//! **Unknown names** (a field name, or an `[extension]`, the type does not
//! have) are skipped, value and all, when [`Options`] allows it, and are
//! an error otherwise: unlike a number, a name carries no wire type.

use std::collections::HashMap;
use std::fmt;

use crate::wire::{
    put_len, put_tag, put_varint, END_GROUP, FIXED32, FIXED64, LEN, START_GROUP, VARINT,
};
use crate::{Cardinality, Field, Kind, Message, Pool};

/// What to do with names the type does not know.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Skip a field name the type does not have.
    pub allow_unknown_field: bool,
    /// Skip an `[extension]` the pool does not have for the type.
    pub allow_unknown_extension: bool,
}

/// A syntax or type error, where it was found (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for TextError {}

/// What the pool could not settle (spec 0426 S4, spec 0427 S4), in a
/// message of type `message_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// W1: `N { … }` by number, declared nowhere; read as
    /// length-delimited.
    Undeclared { message_type: String, number: u32 },
    /// W2: a string or bytes protoc printed as a message; rebuilt taking
    /// its sub-fields for groups.
    StringAsMessage { message_type: String, number: u32 },
    /// W3: an `[extension]` declared nowhere; skipped.
    UnknownExtension { message_type: String, name: String },
    /// W4: a field name the message's type does not declare; skipped
    /// (spec 0436: a name is never dropped unsaid).
    UnknownField { message_type: String, name: String },
}

impl Warning {
    /// Its code (spec 0438): `T1` to `T4`.
    pub fn code(&self) -> &'static str {
        match self {
            Warning::Undeclared { .. } => "T1",
            Warning::StringAsMessage { .. } => "T2",
            Warning::UnknownExtension { .. } => "T3",
            Warning::UnknownField { .. } => "T4",
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::Undeclared {
                message_type,
                number,
            } => write!(
                f,
                "field {number} of {message_type} is declared nowhere in the input: \
                 read as length-delimited"
            ),
            Warning::StringAsMessage {
                message_type,
                number,
            } => write!(
                f,
                "field {number} of {message_type} is a string protoc printed as a \
                 message: rebuilt taking its sub-fields for groups (decode the text \
                 with the files that declare the options to avoid it)"
            ),
            Warning::UnknownExtension { message_type, name } => write!(
                f,
                "extension [{name}] of {message_type} is declared nowhere in the input: \
                 skipped"
            ),
            Warning::UnknownField { message_type, name } => write!(
                f,
                "field {name} of {message_type} is declared nowhere in the input: skipped"
            ),
        }
    }
}

/// What [`parse_reporting`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub bytes: Vec<u8>,
    pub warnings: Vec<Warning>,
}

/// Unknown names skipped: a field of a newer `descriptor.proto`, an
/// extension the pool does not declare (reported, spec 0427 S4).
pub const LENIENT: Options = Options {
    allow_unknown_field: true,
    allow_unknown_extension: true,
};

/// A `FileDescriptorProto` in the text format, or a `FileDescriptorSet`
/// when `is_set`, into the binary format, read with the vendored
/// `descriptor.proto` alone and [`LENIENT`]ly: for what must see a file
/// before the input is complete (an FDP plugin, specs 0369 and 0431). A
/// field printed by number is kept; a field or an extension by a name
/// `descriptor.proto` does not declare is skipped.
pub fn parse_descriptor(text: &str, is_set: bool) -> Result<Vec<u8>, TextError> {
    let name = if is_set {
        "google.protobuf.FileDescriptorSet"
    } else {
        "google.protobuf.FileDescriptorProto"
    };
    let def = Pool::vendored()
        .get_message_by_name(name)
        .expect("the vendored descriptor.proto declares it");
    parse(&def, text, LENIENT)
}

/// Parse `text` as a message of type `def`, into the binary format.
pub fn parse(def: &Message, text: &str, options: Options) -> Result<Vec<u8>, TextError> {
    parse_reporting(def, text, options).map(|p| p.bytes)
}

/// [`parse`], with what the pool could not settle.
pub fn parse_reporting(def: &Message, text: &str, options: Options) -> Result<Parsed, TextError> {
    let tokens = lex(text)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        options,
        end: end_position(text),
        warnings: Vec::new(),
    };
    let mut frame = Frame::default();
    p.fields(def, &mut frame, None)?;
    if p.pos < p.tokens.len() {
        return Err(p.error_here("unexpected token"));
    }
    Ok(Parsed {
        bytes: frame.finish(),
        warnings: p.warnings,
    })
}

// ── Skimming ────────────────────────────────────────────────────────────────
//
// What a set's split and a file's imports need, its top level, without
// tokens: brackets, comments and strings skipped at the byte level, three
// times faster than lexing (spec 0427 S1).

/// The 1-based line and column of byte `at` of `text`.
fn position(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    (line, before[line_start..].chars().count() + 1)
}

fn skim_error(text: &str, at: usize, message: &str) -> TextError {
    let (line, column) = position(text, at);
    TextError {
        line,
        column,
        message: message.into(),
    }
}

/// The end of the string starting at `i`, past its closing quote.
fn skip_string(b: &[u8], i: usize) -> Option<usize> {
    let quote = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'\n' => return None,
            c if c == quote => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Whitespace and comments from `i` on skipped.
fn skip_blank(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => i += 1,
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    i
}

/// Walk `text`'s top level: `visit(name, i, depth)` for each identifier,
/// `i` just past it, and `bracket(open, i, depth)` for each bracket, the
/// depth before it. Strings and comments are skipped.
fn skim(
    text: &str,
    mut visit: impl FnMut(&str, usize, usize) -> Result<Option<usize>, TextError>,
    mut bracket: impl FnMut(bool, usize, usize),
) -> Result<(), TextError> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                i = skip_string(b, i).ok_or_else(|| skim_error(text, i, "unterminated string"))?
            }
            b'{' | b'<' => {
                bracket(true, i, depth);
                depth += 1;
                i += 1;
            }
            b'}' | b'>' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| skim_error(text, i, "unbalanced brackets"))?;
                bracket(false, i, depth);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if let Some(next) = visit(&text[start..i], i, depth)? {
                    i = next;
                }
            }
            c if c.is_ascii_digit() => {
                // A number, `1e5` and `0x1f` included: not an identifier.
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Err(skim_error(text, b.len(), "unbalanced brackets"));
    }
    Ok(())
}

/// Each `file { … }` of `text`, a `FileDescriptorSet` in the text
/// format, as the slice of `text` inside its brackets, in order: one
/// `FileDescriptorProto` each, to read on its own (spec 0424 S7).
pub fn split_set(text: &str) -> Result<Vec<&str>, TextError> {
    field_bodies(text, "file")
}

/// The body of each top-level `field { … }` (or `field: < … >`) of `text`,
/// in order: the slice of `text` inside its brackets. A `field` with a
/// scalar value is not among them.
pub fn field_bodies<'t>(text: &'t str, field: &str) -> Result<Vec<&'t str>, TextError> {
    let b = text.as_bytes();
    let mut bodies = Vec::new();
    let mut open: Option<usize> = None;
    let after_field: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
    skim(
        text,
        |name, i, depth| {
            if depth == 0 && name == field {
                after_field.set(Some(i));
            }
            Ok(None)
        },
        |opening, i, depth| {
            if depth == 0 {
                if opening {
                    // The field, an optional `:`, then the bracket.
                    let follows = after_field.take().is_some_and(|f| {
                        let j = skip_blank(b, f);
                        let j = if b.get(j) == Some(&b':') {
                            skip_blank(b, j + 1)
                        } else {
                            j
                        };
                        j == i
                    });
                    open = follows.then_some(i + 1);
                } else if let Some(start) = open.take() {
                    bodies.push(&text[start..i]);
                }
            }
        },
    )?;
    Ok(bodies)
}

/// The name and imports of `text`, a `FileDescriptorProto` in the text
/// format, from its top-level `name`, `dependency` and `option_dependency`
/// fields (an `import option`, edition 2024: a file to load too), without
/// parsing the rest (spec 0427 S1). An error where a string does not end,
/// or the brackets do not match.
pub fn scan_imports(text: &str) -> Result<(String, Vec<String>), TextError> {
    let b = text.as_bytes();
    let mut name = None;
    let mut imports = Vec::new();
    skim(
        text,
        |field, i, depth| {
            if depth != 0 || !matches!(field, "name" | "dependency" | "option_dependency") {
                return Ok(None);
            }
            // `field: "…" "…"`, adjacent strings joined.
            let mut j = skip_blank(b, i);
            if b.get(j) == Some(&b':') {
                j = skip_blank(b, j + 1);
            }
            let mut value = Vec::new();
            let mut any = false;
            while matches!(b.get(j), Some(b'"' | b'\'')) {
                let (s, next) = lex_string(b, j).map_err(|m| skim_error(text, j, &m))?;
                value.extend(s);
                any = true;
                j = skip_blank(b, next);
            }
            if !any {
                return Ok(None);
            }
            let value = String::from_utf8_lossy(&value).into_owned();
            if field == "name" {
                name = Some(value);
            } else if !imports.contains(&value) {
                imports.push(value);
            }
            Ok(Some(j))
        },
        |_, _, _| {},
    )?;
    Ok((name.unwrap_or_default(), imports))
}

/// Message `name` (`FileDescriptorProto`, `FileDescriptorSet`) of the
/// package of `pool`'s `descriptor_file` (spec 0427 S3): `proto2.…` for a
/// `descriptor.proto` under `package proto2`, `google.protobuf.…` when the pool
/// has no such file. When the pool does not declare it (a variant's
/// `descriptor.proto` may declare the options messages alone), the
/// vendored `descriptor.proto`'s: its text then parses, its options
/// unresolved. `None` only for a `name` neither declares.
pub fn descriptor_type(pool: &Pool, descriptor_file: &str, name: &str) -> Option<Message> {
    let package = pool
        .file(descriptor_file)
        .map_or("google.protobuf".to_string(), |f| {
            f.package_name().to_string()
        });
    pool.get_message_by_name(&format!("{package}.{name}"))
        .or_else(|| pool.get_message_by_name(&format!("google.protobuf.{name}")))
        .or_else(|| Pool::vendored().get_message_by_name(&format!("google.protobuf.{name}")))
}

// ── Lexer ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    /// An integer literal, as written (decimal, octal or hex).
    Int(String),
    /// A float literal, as written, `f` suffix removed.
    Float(String),
    /// One string literal, unescaped.
    Str(Vec<u8>),
    Sym(char),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
    column: usize,
}

fn end_position(text: &str) -> (usize, usize) {
    let line = text.matches('\n').count() + 1;
    let column = text.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

fn lex(text: &str) -> Result<Vec<Token>, TextError> {
    let b = text.as_bytes();
    let (mut i, mut line, mut line_start) = (0usize, 1usize, 0usize);
    let mut out = Vec::new();
    let err = |line: usize, col: usize, m: &str| TextError {
        line,
        column: col,
        message: m.to_string(),
    };
    while i < b.len() {
        let c = b[i];
        let start = i;
        let column = || text[line_start..start].chars().count() + 1;
        match c {
            b'\n' => {
                i += 1;
                line += 1;
                line_start = i;
            }
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => i += 1,
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' | b'}' | b'<' | b'>' | b'[' | b']' | b':' | b',' | b';' | b'-' | b'/' | b'.'
                if !(c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) =>
            {
                out.push(Token {
                    tok: Tok::Sym(c as char),
                    line,
                    column: column(),
                });
                i += 1;
            }
            b'"' | b'\'' => {
                let (bytes, next) = lex_string(b, i).map_err(|m| err(line, column(), &m))?;
                out.push(Token {
                    tok: Tok::Str(bytes),
                    line,
                    column: column(),
                });
                i = next;
            }
            c if c.is_ascii_digit() || c == b'.' => {
                let (tok, next) = lex_number(b, i).map_err(|m| err(line, column(), &m))?;
                out.push(Token {
                    tok,
                    line,
                    column: column(),
                });
                i = next;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Token {
                    tok: Tok::Ident(text[start..i].to_string()),
                    line,
                    column: column(),
                });
            }
            _ => return Err(err(line, column(), "unexpected character")),
        }
    }
    Ok(out)
}

fn lex_number(b: &[u8], start: usize) -> Result<(Tok, usize), String> {
    let mut i = start;
    let text = |i: usize| String::from_utf8_lossy(&b[start..i]).into_owned();
    let ident_follows = |i: usize| {
        b.get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
    };
    if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        let digits = i;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i == digits || ident_follows(i) {
            return Err("malformed hexadecimal number".into());
        }
        return Ok((Tok::Int(text(i)), i));
    }
    let mut float = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if b.get(i) == Some(&b'.') {
        float = true;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            float = true;
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    let literal = text(i);
    if matches!(b.get(i), Some(b'f' | b'F')) {
        i += 1;
        float = true;
    }
    if ident_follows(i) {
        return Err("malformed number".into());
    }
    Ok((
        if float {
            Tok::Float(literal)
        } else {
            Tok::Int(literal)
        },
        i,
    ))
}

/// A string literal from `b[start]`, its quote, to the matching quote.
fn lex_string(b: &[u8], start: usize) -> Result<(Vec<u8>, usize), String> {
    let quote = b[start];
    let mut i = start + 1;
    let mut out = Vec::new();
    loop {
        let Some(&c) = b.get(i) else {
            return Err("unterminated string".into());
        };
        i += 1;
        match c {
            b'\n' => return Err("newline in a string".into()),
            c if c == quote => return Ok((out, i)),
            b'\\' => {
                let Some(&e) = b.get(i) else {
                    return Err("unterminated string".into());
                };
                i += 1;
                match e {
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'v' => out.push(0x0b),
                    b'?' => out.push(b'?'),
                    b'\\' | b'\'' | b'"' => out.push(e),
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        for _ in 0..2 {
                            match b.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        if v > 0xff {
                            return Err("octal escape out of range".into());
                        }
                        out.push(v as u8);
                    }
                    b'x' | b'X' => {
                        let mut v = 0u32;
                        let mut n = 0;
                        while n < 2 && b.get(i).is_some_and(u8::is_ascii_hexdigit) {
                            v = v * 16 + (b[i] as char).to_digit(16).unwrap();
                            i += 1;
                            n += 1;
                        }
                        if n == 0 {
                            return Err("\\x without hex digits".into());
                        }
                        out.push(v as u8);
                    }
                    b'u' | b'U' => {
                        let n = if e == b'u' { 4 } else { 8 };
                        let digits = b
                            .get(i..i + n)
                            .filter(|d| d.iter().all(u8::is_ascii_hexdigit))
                            .ok_or("malformed unicode escape")?;
                        let v = u32::from_str_radix(std::str::from_utf8(digits).unwrap(), 16)
                            .map_err(|_| "malformed unicode escape")?;
                        i += n;
                        let ch = char::from_u32(v).ok_or("unicode escape out of range")?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err(format!("unknown escape \\{}", e as char)),
                }
            }
            c => out.push(c),
        }
    }
}

// ── Encoder ─────────────────────────────────────────────────────────────────

/// The fields of one message, in text order. A packed field's values are
/// gathered into one record, where the field first appears.
#[derive(Default)]
struct Frame {
    records: Vec<Record>,
    packed: HashMap<u32, usize>,
    /// Singular fields set, to refuse a second value, and the oneof each
    /// set field belongs to, to refuse a second member.
    singular: HashMap<u32, ()>,
    oneofs: HashMap<String, String>,
}

enum Record {
    Raw(Vec<u8>),
    Packed(u32, Vec<u8>),
}

impl Frame {
    fn raw(&mut self, bytes: Vec<u8>) {
        self.records.push(Record::Raw(bytes));
    }

    fn packed(&mut self, number: u32) -> &mut Vec<u8> {
        let i = *self.packed.entry(number).or_insert_with(|| {
            self.records.push(Record::Packed(number, Vec::new()));
            self.records.len() - 1
        });
        match &mut self.records[i] {
            Record::Packed(_, payload) => payload,
            Record::Raw(_) => unreachable!(),
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        for r in self.records {
            match r {
                Record::Raw(b) => out.extend(b),
                Record::Packed(n, payload) => put_len(&mut out, n, &payload),
            }
        }
        out
    }
}

// ── Parser ──────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    options: Options,
    end: (usize, usize),
    warnings: Vec<Warning>,
}

/// How `N { … }` is framed inside a value of no known type.
#[derive(Clone, Copy)]
enum Raw {
    /// Inside a string protoc printed as a message: as protoc read it, a
    /// group (spec 0426 S3).
    Group,
    /// Inside a value declared nowhere: length-delimited (0426 S2).
    Len,
}

/// A scalar value as written: its sign and token.
struct Written {
    negative: bool,
    tok: Tok,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn error_at(&self, pos: usize, message: impl Into<String>) -> TextError {
        let (line, column) = self
            .tokens
            .get(pos)
            .map_or(self.end, |t| (t.line, t.column));
        TextError {
            line,
            column,
            message: message.into(),
        }
    }

    fn error_here(&self, message: impl Into<String>) -> TextError {
        self.error_at(self.pos, message)
    }

    fn try_sym(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, c: char) -> Result<(), TextError> {
        if self.try_sym(c) {
            Ok(())
        } else {
            Err(self.error_here(format!("expected \"{c}\"")))
        }
    }

    fn ident(&mut self) -> Result<String, TextError> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            _ => Err(self.error_here("expected an identifier")),
        }
    }

    /// `{` or `<`, returning the matching end.
    fn open(&mut self) -> Result<char, TextError> {
        if self.try_sym('{') {
            Ok('}')
        } else if self.try_sym('<') {
            Ok('>')
        } else {
            Err(self.error_here("expected \"{\" or \"<\""))
        }
    }

    fn at_message_start(&self) -> bool {
        matches!(self.peek(), Some(Tok::Sym('{' | '<')))
    }

    /// Fields of `def` until `end` (or the end of input when `None`).
    fn fields(
        &mut self,
        def: &Message,
        frame: &mut Frame,
        end: Option<char>,
    ) -> Result<(), TextError> {
        loop {
            match (self.peek(), end) {
                (None, None) => return Ok(()),
                (None, Some(e)) => return Err(self.error_here(format!("expected \"{e}\""))),
                (Some(Tok::Sym(c)), Some(e)) if *c == e => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => {}
            }
            self.field(def, frame)?;
            if !self.try_sym(',') {
                self.try_sym(';');
            }
        }
    }

    fn field(&mut self, def: &Message, frame: &mut Frame) -> Result<(), TextError> {
        let start = self.pos;
        if self.try_sym('[') {
            if def.full_name() == "google.protobuf.Any" && self.looks_like_type_url() {
                return self.any(def, frame);
            }
            let name = self.dotted_name()?;
            self.expect_sym(']')?;
            let ext = def
                .pool()
                .get_extension_by_name(name.trim_start_matches('.'))
                .filter(|e| e.containing_message() == *def);
            return match ext {
                Some(f) => self.known_field(&f, frame, start),
                None if self.options.allow_unknown_extension => {
                    self.warnings.push(Warning::UnknownExtension {
                        message_type: def.full_name().to_string(),
                        name: name.trim_start_matches('.').to_string(),
                    });
                    self.skip_field_value()
                }
                None => Err(self.error_at(
                    start,
                    format!("extension \"{name}\" of \"{}\" not found", def.full_name()),
                )),
            };
        }
        match self.peek().cloned() {
            Some(Tok::Int(n)) => {
                self.pos += 1;
                let number = parse_decimal_u32(&n)
                    .ok_or_else(|| self.error_at(start, "malformed field number"))?;
                let mut out = Vec::new();
                self.by_number(def, number, &mut out)?;
                frame.raw(out);
                Ok(())
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                let field = def.get_field_by_name(&name).or_else(|| {
                    // A group goes by its type's name.
                    def.get_field_by_name(&name.to_ascii_lowercase())
                        .filter(|f| {
                            f.is_group_like()
                                && matches!(f.kind(), Kind::Message(m) if m.name() == name)
                        })
                });
                match field {
                    Some(f) => self.known_field(&f, frame, start),
                    None if self.options.allow_unknown_field => {
                        self.warnings.push(Warning::UnknownField {
                            message_type: def.full_name().to_string(),
                            name,
                        });
                        self.skip_field_value()
                    }
                    None => Err(self.error_at(
                        start,
                        format!(
                            "message type \"{}\" has no field named \"{name}\"",
                            def.full_name()
                        ),
                    )),
                }
            }
            _ => Err(self.error_here("expected a field name")),
        }
    }

    fn dotted_name(&mut self) -> Result<String, TextError> {
        let mut name = String::new();
        if self.try_sym('.') {
            name.push('.');
        }
        name.push_str(&self.ident()?);
        while self.try_sym('.') {
            name.push('.');
            name.push_str(&self.ident()?);
        }
        Ok(name)
    }

    /// Inside `[`: whether a `/` comes before the `]`.
    fn looks_like_type_url(&self) -> bool {
        self.tokens[self.pos..]
            .iter()
            .take_while(|t| t.tok != Tok::Sym(']'))
            .any(|t| t.tok == Tok::Sym('/'))
    }

    /// `[prefix/pkg.Type] { … }` in a `google.protobuf.Any`.
    fn any(&mut self, def: &Message, frame: &mut Frame) -> Result<(), TextError> {
        let start = self.pos;
        let mut url = String::new();
        loop {
            match self.peek().cloned() {
                Some(Tok::Sym(']')) => {
                    self.pos += 1;
                    break;
                }
                Some(Tok::Ident(s) | Tok::Int(s) | Tok::Float(s)) => {
                    url.push_str(&s);
                    self.pos += 1;
                }
                Some(Tok::Sym(c @ ('.' | '/' | '-'))) => {
                    url.push(c);
                    self.pos += 1;
                }
                _ => return Err(self.error_here("malformed type URL")),
            }
        }
        let type_name = url.rsplit('/').next().unwrap_or_default().to_string();
        let packed = def
            .pool()
            .get_message_by_name(&type_name)
            .ok_or_else(|| self.error_at(start, format!("type \"{type_name}\" not found")))?;
        self.try_sym(':');
        let end = self.open()?;
        let mut inner = Frame::default();
        self.fields(&packed, &mut inner, Some(end))?;
        let mut out = Vec::new();
        put_len(&mut out, 1, url.as_bytes());
        put_len(&mut out, 2, &inner.finish());
        frame.raw(out);
        Ok(())
    }

    /// A field the type knows: its value or values.
    fn known_field(&mut self, f: &Field, frame: &mut Frame, start: usize) -> Result<(), TextError> {
        let repeated = f.cardinality() == Cardinality::Repeated;
        if !repeated {
            if frame.singular.insert(f.number(), ()).is_some() {
                return Err(self.error_at(start, format!("field \"{}\" set twice", f.name())));
            }
            if let Some(o) = f.containing_oneof() {
                if let Some(other) = frame
                    .oneofs
                    .insert(o.name().to_string(), f.name().to_string())
                {
                    if other != f.name() {
                        return Err(self.error_at(
                            start,
                            format!(
                                "fields \"{other}\" and \"{}\" of oneof \"{}\"",
                                f.name(),
                                o.name()
                            ),
                        ));
                    }
                }
            }
        }
        let is_message = matches!(f.kind(), Kind::Message(_));
        if is_message {
            self.try_sym(':');
        } else {
            self.expect_sym(':')?;
        }
        if repeated && self.try_sym('[') {
            if !self.try_sym(']') {
                loop {
                    self.value(f, frame)?;
                    if self.try_sym(']') {
                        break;
                    }
                    self.expect_sym(',')?;
                }
            }
            return Ok(());
        }
        self.value(f, frame)
    }

    /// One value of `f`, encoded into `frame`.
    fn value(&mut self, f: &Field, frame: &mut Frame) -> Result<(), TextError> {
        let n = f.number();
        if let Kind::Message(sub) = f.kind() {
            let end = self.open()?;
            let mut inner = Frame::default();
            self.fields(&sub, &mut inner, Some(end))?;
            let payload = inner.finish();
            let mut out = Vec::new();
            if f.is_group() {
                put_tag(&mut out, n, START_GROUP);
                out.extend(payload);
                put_tag(&mut out, n, END_GROUP);
            } else {
                put_len(&mut out, n, &payload);
            }
            frame.raw(out);
            return Ok(());
        }
        let start = self.pos;
        let written = self.scalar()?;
        let (wire_type, payload) =
            encode_scalar(f, &written).map_err(|m| self.error_at(start, m))?;
        if f.cardinality() == Cardinality::Repeated && f.is_packed() && wire_type != LEN {
            frame.packed(n).extend(payload);
        } else {
            let mut out = Vec::new();
            put_tag(&mut out, n, wire_type);
            out.extend(payload);
            frame.raw(out);
        }
        Ok(())
    }

    /// A scalar as written: a sign and a token, adjacent strings joined.
    fn scalar(&mut self) -> Result<Written, TextError> {
        let negative = self.try_sym('-');
        let tok = match self.peek().cloned() {
            Some(Tok::Str(mut s)) => {
                self.pos += 1;
                while let Some(Tok::Str(more)) = self.peek() {
                    s.extend_from_slice(more);
                    self.pos += 1;
                }
                Tok::Str(s)
            }
            Some(t @ (Tok::Int(_) | Tok::Float(_) | Tok::Ident(_))) => {
                self.pos += 1;
                t
            }
            _ => return Err(self.error_here("expected a value")),
        };
        if negative && matches!(tok, Tok::Str(_)) {
            return Err(self.error_at(self.pos - 1, "a string cannot be negative"));
        }
        Ok(Written { negative, tok })
    }

    /// Whether `N` is followed by a message, `{ … }` or `: { … }`, rather
    /// than by a scalar; the `:` is consumed.
    fn message_follows(&mut self) -> bool {
        self.at_message_start() || (self.try_sym(':') && self.at_message_start())
    }

    /// Field `number` of a message of type `def`, given by number (spec
    /// 0426, 0427 S5): a scalar as protoc printed it, `N { … }` framed as
    /// the field or extension of that number is declared in the pool.
    fn by_number(
        &mut self,
        def: &Message,
        number: u32,
        out: &mut Vec<u8>,
    ) -> Result<(), TextError> {
        if !self.message_follows() {
            return self.wire_scalar(number, out);
        }
        let end = self.open()?;
        let declared = def.get_field(number).or_else(|| def.get_extension(number));
        let message_type = || def.full_name().to_string();
        match declared.as_ref().map(|f| (f, f.kind())) {
            Some((f, Kind::Message(sub))) => {
                let mut inner = Frame::default();
                self.fields(&sub, &mut inner, Some(end))?;
                let payload = inner.finish();
                if f.is_group() {
                    put_tag(out, number, START_GROUP);
                    out.extend(payload);
                    put_tag(out, number, END_GROUP);
                } else {
                    put_len(out, number, &payload);
                }
            }
            Some((_, Kind::String | Kind::Bytes)) => {
                let (body, nested) = self.raw_body(end, Raw::Group)?;
                if nested {
                    self.warnings.push(Warning::StringAsMessage {
                        message_type: message_type(),
                        number,
                    });
                }
                put_len(out, number, &body);
            }
            Some(_) => {
                // A scalar printed as a message: kept as protoc read it.
                let (body, _) = self.raw_body(end, Raw::Group)?;
                put_tag(out, number, START_GROUP);
                out.extend(body);
                put_tag(out, number, END_GROUP);
            }
            None => {
                self.warnings.push(Warning::Undeclared {
                    message_type: message_type(),
                    number,
                });
                let (body, _) = self.raw_body(end, Raw::Len)?;
                put_len(out, number, &body);
            }
        }
        Ok(())
    }

    /// Fields by number until `end`, in a value of no known type, `N { … }`
    /// framed as `raw` says: the bytes, and whether a non-empty group is
    /// among them, at any depth.
    fn raw_body(&mut self, end: char, raw: Raw) -> Result<(Vec<u8>, bool), TextError> {
        let mut out = Vec::new();
        let mut nested = false;
        loop {
            if self.try_sym(end) {
                return Ok((out, nested));
            }
            let start = self.pos;
            let number = match self.peek() {
                Some(Tok::Int(n)) => parse_decimal_u32(n),
                _ => None,
            }
            .ok_or_else(|| {
                self.error_at(start, "expected a field number in a value of unknown type")
            })?;
            self.pos += 1;
            if self.message_follows() {
                let inner_end = self.open()?;
                let (body, deeper) = self.raw_body(inner_end, raw)?;
                match raw {
                    Raw::Group => {
                        nested |= !body.is_empty() || deeper;
                        put_tag(&mut out, number, START_GROUP);
                        out.extend(body);
                        put_tag(&mut out, number, END_GROUP);
                    }
                    Raw::Len => put_len(&mut out, number, &body),
                }
            } else {
                self.wire_scalar(number, &mut out)?;
            }
            if !self.try_sym(',') {
                self.try_sym(';');
            }
        }
    }

    /// A scalar by number, as protoc printed it: a decimal number is a
    /// varint, `0x` and 8 or 16 hex digits a fixed32 or a fixed64, a string
    /// length-delimited.
    fn wire_scalar(&mut self, number: u32, out: &mut Vec<u8>) -> Result<(), TextError> {
        let start = self.pos;
        let written = self.scalar()?;
        match &written.tok {
            Tok::Str(s) => put_len(out, number, s),
            Tok::Int(t) if !written.negative && (t.starts_with("0x") || t.starts_with("0X")) => {
                let digits = &t[2..];
                let v = u64::from_str_radix(digits, 16)
                    .map_err(|_| self.error_at(start, "malformed number"))?;
                match digits.len() {
                    8 => {
                        put_tag(out, number, FIXED32);
                        out.extend((v as u32).to_le_bytes());
                    }
                    16 => {
                        put_tag(out, number, FIXED64);
                        out.extend(v.to_le_bytes());
                    }
                    _ => {
                        return Err(self.error_at(
                            start,
                            "a field by number takes 0x and 8 or 16 hex digits (fixed32, fixed64)",
                        ))
                    }
                }
            }
            Tok::Int(t) => {
                let v = parse_integer(t, written.negative)
                    .ok_or_else(|| self.error_at(start, "malformed integer"))?;
                put_tag(out, number, VARINT);
                put_varint(out, v as u64);
            }
            _ => {
                return Err(self.error_at(
                    start,
                    "a field by number takes a number, a string or a message",
                ))
            }
        }
        Ok(())
    }

    /// Skip what follows an unknown name: an optional `:`, then a message,
    /// a list, or a value.
    fn skip_field_value(&mut self) -> Result<(), TextError> {
        self.try_sym(':');
        if self.try_sym('[') {
            if !self.try_sym(']') {
                loop {
                    self.skip_value()?;
                    if self.try_sym(']') {
                        break;
                    }
                    self.expect_sym(',')?;
                }
            }
            Ok(())
        } else {
            self.skip_value()
        }
    }

    fn skip_value(&mut self) -> Result<(), TextError> {
        if self.at_message_start() {
            let end = self.open()?;
            loop {
                if self.try_sym(end) {
                    return Ok(());
                }
                if self.peek().is_none() {
                    return Err(self.error_here(format!("expected \"{end}\"")));
                }
                if self.try_sym('[') {
                    while !self.try_sym(']') {
                        if self.peek().is_none() {
                            return Err(self.error_here("expected \"]\""));
                        }
                        self.pos += 1;
                    }
                } else {
                    match self.peek() {
                        Some(Tok::Ident(_) | Tok::Int(_)) => self.pos += 1,
                        _ => return Err(self.error_here("expected a field name")),
                    }
                }
                self.skip_field_value()?;
                if !self.try_sym(',') {
                    self.try_sym(';');
                }
            }
        }
        self.scalar().map(|_| ())
    }
}

// ── Values ──────────────────────────────────────────────────────────────────

fn parse_decimal_u32(t: &str) -> Option<u32> {
    if t.len() > 1 && t.starts_with('0') {
        return None;
    }
    t.parse().ok()
}

/// An integer literal (decimal, `0` octal, `0x` hex) with its sign, as an
/// i128 wide enough for every range check.
fn parse_integer(t: &str, negative: bool) -> Option<i128> {
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i128::from_str_radix(h, 16).ok()?
    } else if t.len() > 1 && t.starts_with('0') {
        i128::from_str_radix(&t[1..], 8).ok()?
    } else {
        t.parse().ok()?
    };
    Some(if negative { -v } else { v })
}

fn parse_float(w: &Written) -> Option<f64> {
    let v = match &w.tok {
        Tok::Float(t) => t.parse::<f64>().ok()?,
        Tok::Int(t) => parse_integer(t, false)? as f64,
        Tok::Ident(t) => match t.to_ascii_lowercase().as_str() {
            "inf" | "infinity" => f64::INFINITY,
            "nan" => f64::NAN,
            _ => return None,
        },
        _ => return None,
    };
    Some(if w.negative { -v } else { v })
}

/// One scalar of `f`'s type: its wire type and encoded payload (the tag
/// excluded).
fn encode_scalar(f: &Field, w: &Written) -> Result<(u32, Vec<u8>), String> {
    let kind = f.kind();
    let mut out = Vec::new();
    let integer = |lo: i128, hi: i128| -> Result<i128, String> {
        let Tok::Int(t) = &w.tok else {
            return Err(format!("expected an integer for \"{}\"", f.name()));
        };
        let v = parse_integer(t, w.negative).ok_or("malformed integer")?;
        if v < lo || v > hi {
            return Err(format!("integer out of range for \"{}\"", f.name()));
        }
        Ok(v)
    };
    let wire_type = match &kind {
        Kind::Int32 => {
            put_varint(
                &mut out,
                integer(i32::MIN.into(), i32::MAX.into())? as i64 as u64,
            );
            VARINT
        }
        Kind::Int64 => {
            put_varint(
                &mut out,
                integer(i64::MIN.into(), i64::MAX.into())? as i64 as u64,
            );
            VARINT
        }
        Kind::Uint32 => {
            put_varint(&mut out, integer(0, u32::MAX.into())? as u64);
            VARINT
        }
        Kind::Uint64 => {
            put_varint(&mut out, integer(0, u64::MAX.into())? as u64);
            VARINT
        }
        Kind::Sint32 => {
            let v = integer(i32::MIN.into(), i32::MAX.into())? as i32;
            put_varint(&mut out, u64::from(((v << 1) ^ (v >> 31)) as u32));
            VARINT
        }
        Kind::Sint64 => {
            let v = integer(i64::MIN.into(), i64::MAX.into())? as i64;
            put_varint(&mut out, ((v << 1) ^ (v >> 63)) as u64);
            VARINT
        }
        Kind::Fixed32 => {
            out.extend((integer(0, u32::MAX.into())? as u32).to_le_bytes());
            FIXED32
        }
        Kind::Sfixed32 => {
            out.extend((integer(i32::MIN.into(), i32::MAX.into())? as i32).to_le_bytes());
            FIXED32
        }
        Kind::Fixed64 => {
            out.extend((integer(0, u64::MAX.into())? as u64).to_le_bytes());
            FIXED64
        }
        Kind::Sfixed64 => {
            out.extend((integer(i64::MIN.into(), i64::MAX.into())? as i64).to_le_bytes());
            FIXED64
        }
        Kind::Bool => {
            let v = match (&w.tok, w.negative) {
                (Tok::Ident(t), false) if matches!(t.as_str(), "true" | "True" | "t") => true,
                (Tok::Ident(t), false) if matches!(t.as_str(), "false" | "False" | "f") => false,
                (Tok::Int(t), false) if t == "1" => true,
                (Tok::Int(t), false) if t == "0" => false,
                _ => return Err(format!("expected a bool for \"{}\"", f.name())),
            };
            out.push(u8::from(v));
            VARINT
        }
        Kind::Float => {
            let v =
                parse_float(w).ok_or_else(|| format!("expected a float for \"{}\"", f.name()))?;
            out.extend((v as f32).to_le_bytes());
            FIXED32
        }
        Kind::Double => {
            let v =
                parse_float(w).ok_or_else(|| format!("expected a double for \"{}\"", f.name()))?;
            out.extend(v.to_le_bytes());
            FIXED64
        }
        Kind::String | Kind::Bytes => {
            let Tok::Str(s) = &w.tok else {
                return Err(format!("expected a string for \"{}\"", f.name()));
            };
            put_varint(&mut out, s.len() as u64);
            out.extend_from_slice(s);
            LEN
        }
        Kind::Enum(e) => {
            let number = match (&w.tok, w.negative) {
                (Tok::Ident(name), false) => e
                    .values()
                    .find(|v| v.name() == name)
                    .map(|v| v.number())
                    .ok_or_else(|| {
                        format!("enum \"{}\" has no value named \"{name}\"", e.full_name())
                    })?,
                (Tok::Int(t), negative) => {
                    let v = parse_integer(t, negative).ok_or("malformed integer")?;
                    let v = i32::try_from(v).map_err(|_| "enum value out of range".to_string())?;
                    if e.is_closed() && e.get_value(v).is_none() {
                        return Err(format!("enum \"{}\" has no value {v}", e.full_name()));
                    }
                    v
                }
                _ => return Err(format!("expected an enum value for \"{}\"", f.name())),
            };
            put_varint(&mut out, i64::from(number) as u64);
            VARINT
        }
        Kind::Message(_) => unreachable!("messages are not scalars"),
    };
    Ok((wire_type, out))
}
