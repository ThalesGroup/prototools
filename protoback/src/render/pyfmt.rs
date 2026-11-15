// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Python's formatting, as reproto's output has it (spec 0424 S8).

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

    #[test]
    fn json_dumps_is_pythons() {
        for &(s, expected) in CASES {
            assert_eq!(json_dumps(s), expected, "json.dumps({s:?})");
        }
    }
}
