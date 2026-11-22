// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

/// The JSON name protoc gives a field that sets none: its name with every
/// underscore removed and the character after one upper-cased, as
/// `ToJsonName` in protobuf's `descriptor.cc` does (ASCII upper-casing, as
/// there). `foo_bar_baz` is `fooBarBaz`; `_x` is `X`; `x__y` is `xY`.
///
/// Shared by protoback, which omits a `json_name` option equal to it, and
/// git-seriate-fdp, whose canonicalizer drops a `json_name` field equal to
/// it (spec 0453 S4 R1).
pub fn json_name(field_name: &str) -> String {
    let mut out = String::with_capacity(field_name.len());
    let mut upper_next = false;
    for c in field_name.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::json_name;

    #[test]
    fn as_protoc_derives_it() {
        assert_eq!(json_name("foo_bar_baz"), "fooBarBaz");
        assert_eq!(json_name("foo"), "foo");
        assert_eq!(json_name("_x"), "X");
        assert_eq!(json_name("x__y"), "xY");
        assert_eq!(json_name("x_"), "x");
        assert_eq!(json_name("field_1_name"), "field1Name");
        assert_eq!(json_name("FooBar"), "FooBar");
    }
}
