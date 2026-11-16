// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Python's formatting, as reproto's output has it (spec 0424 S8), each
//! function checked against a table Python generated.

/// `json.dumps(s)`, `ensure_ascii` on: `"`, `\`, and the controls
/// `\b \f \n \r \t` escaped by name; everything else outside space to
/// `~` (the other controls, DEL, all past ASCII) as `\uXXXX`, in UTF-16
/// units, so a surrogate pair past the BMP.
pub fn json_dumps(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if !(' '..='~').contains(&c) => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `syntax._edition_name`: the `Edition` value's name without its
/// `EDITION_` prefix, as Python protobuf 7.34.1 names them; `None` for a
/// value it does not know (where reproto's `Edition.Name` fails).
pub fn edition_name(edition: i32) -> Option<&'static str> {
    Some(match edition {
        0 => "UNKNOWN",
        900 => "LEGACY",
        998 => "PROTO2",
        999 => "PROTO3",
        1000 => "2023",
        1001 => "2024",
        9999 => "UNSTABLE",
        1 => "1_TEST_ONLY",
        2 => "2_TEST_ONLY",
        99997 => "99997_TEST_ONLY",
        99998 => "99998_TEST_ONLY",
        99999 => "99999_TEST_ONLY",
        2147483647 => "MAX",
        _ => return None,
    })
}

/// The code points `str.isprintable()` accepts, from Python 3.13.
mod printable {
    include!("pyfmt/printable.rs");
}

/// `str.isprintable()` for one character.
fn is_printable(c: char) -> bool {
    let cp = c as u32;
    printable::PRINTABLE
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `repr(s)` of a Python `str`: single quotes, or double ones when the
/// text has a single quote and no double one; `\\`, the quote, `\t \n \r`
/// escaped; other non-printable characters as `\xNN`, `\uNNNN` or
/// `\UNNNNNNNN`; printable ones as they are, past ASCII too.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (0x20..0x7f).contains(&cp) => out.push(c),
            c if cp >= 0x7f && is_printable(c) && cp != 0x7f => out.push(c),
            _ if cp < 0x100 => out.push_str(&format!("\\x{cp:02x}")),
            _ if cp < 0x10000 => out.push_str(&format!("\\u{cp:04x}")),
            _ => out.push_str(&format!("\\U{cp:08x}")),
        }
    }
    out.push(quote);
    out
}

/// `repr(f)` (and `str(f)`) of a Python `float`: the shortest digits
/// that read back to `f`, in fixed notation from 1e-4 up to 1e16 (with
/// `.0` for an integral value), in exponent notation otherwise
/// (`1e+16`, `1.5e-05`); `inf`, `-inf`, `nan`.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let sign = if f.is_sign_negative() { "-" } else { "" };
    // `{:e}` writes the shortest digits that round-trip: `d[.ddd]e[-]x`.
    let e = format!("{:e}", f.abs());
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let n = digits.len() as i32;
    let body = if (-4..16).contains(&exp) {
        let decpt = exp + 1;
        if decpt <= 0 {
            format!("0.{}{digits}", "0".repeat((-decpt) as usize))
        } else if decpt >= n {
            format!("{digits}{}.0", "0".repeat((decpt - n) as usize))
        } else {
            let (a, b) = digits.split_at(decpt as usize);
            format!("{a}.{b}")
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let mant = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{esign}{:02}", exp.abs())
    };
    format!("{sign}{body}")
}

/// `text.render_bytes`: a bytes value as a `.proto` string literal,
/// printable ASCII as it is but `"` and `\`, every other byte `\xNN`.
pub fn render_bytes(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for &b in value {
        if (32..=126).contains(&b) && b != b'"' && b != b'\\' {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out.push('"');
    out
}

/// A bytes field's default as reproto reads it:
/// `codecs.decode(s, 'unicode-escape').encode('latin1')`. `s`, a Python
/// `str`, is taken as its UTF-8 bytes, each byte one character; `None`
/// where Python raises (a truncated escape, a character past latin-1).
pub fn bytes_default(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out: Vec<u32> = Vec::with_capacity(b.len());
    let hex = |slice: &[u8]| -> Option<u32> {
        let t = std::str::from_utf8(slice).ok()?;
        if t.len() != slice.len() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(t, 16).ok()
    };
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(u32::from(b[i]));
            i += 1;
            continue;
        }
        let &c = b.get(i + 1)?;
        i += 2;
        match c {
            b'\n' => {}
            b'\\' => out.push(u32::from(b'\\')),
            b'\'' => out.push(u32::from(b'\'')),
            b'"' => out.push(u32::from(b'"')),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'v' => out.push(11),
            b'0'..=b'7' => {
                let mut v = u32::from(c - b'0');
                let mut k = 0;
                while k < 2 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                    v = v * 8 + u32::from(b[i] - b'0');
                    i += 1;
                    k += 1;
                }
                out.push(v);
            }
            b'x' | b'u' | b'U' => {
                let len = match c {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let v = hex(b.get(i..i + len)?)?;
                if v > 0x10_ffff {
                    return None;
                }
                out.push(v);
                i += len;
            }
            other => {
                // An unknown escape keeps its backslash (Python warns).
                out.push(u32::from(b'\\'));
                out.push(u32::from(other));
            }
        }
    }
    out.into_iter().map(|cp| u8::try_from(cp).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::json_dumps;

    /// `(s, json.dumps(s))`, as Python 3.13 gives them.
    const CASES: &[(&str, &str)] = &[
        ("a/b.proto", "\"a/b.proto\""),
        ("q\"b\\s", "\"q\\\"b\\\\s\""),
        ("\n\r\t\u{8}\u{c}", "\"\\n\\r\\t\\b\\f\""),
        ("\u{1}\u{1f}\u{7f}", "\"\\u0001\\u001f\\u007f\""),
        ("é", "\"\\u00e9\""),
        ("日本", "\"\\u65e5\\u672c\""),
        ("🙂", "\"\\ud83d\\ude42\""),
    ];

    mod cases {
        include!("pyfmt/float_cases.rs");
        include!("pyfmt/str_cases.rs");
        include!("pyfmt/bytes_default_cases.rs");
    }

    #[test]
    fn float_repr_is_pythons() {
        for &(bits, expected) in cases::FLOAT_REPR {
            let f = f64::from_bits(bits);
            assert_eq!(super::float_repr(f), expected, "repr({f:e})");
        }
        assert_eq!(super::float_repr(f64::NAN), "nan");
    }

    #[test]
    fn str_repr_is_pythons() {
        for &(s, expected) in cases::STR_REPR {
            assert_eq!(super::str_repr(s), expected, "repr({s:?})");
        }
    }

    #[test]
    fn bytes_default_is_pythons() {
        for &(s, expected) in cases::BYTES_DEFAULT {
            assert_eq!(super::bytes_default(s).as_deref(), Some(expected), "{s:?}");
        }
        for &s in cases::BYTES_DEFAULT_FAILS {
            assert_eq!(super::bytes_default(s), None, "{s:?}");
        }
    }

    #[test]
    fn json_dumps_is_pythons() {
        for &(s, expected) in CASES {
            assert_eq!(json_dumps(s), expected, "json.dumps({s:?})");
        }
    }
}
