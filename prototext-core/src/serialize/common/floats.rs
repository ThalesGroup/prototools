// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Float / double formatting ─────────────────────────────────────────────────
// Mirrors format_double_like_text_format / format_float_like_text_format.
//
// The Python functions delegate to protobuf's C++ text_format for exact
// compatibility.  The Rust versions replicate the same algorithm:
//
//   double: Python str(float64) shortest round-trip.
//   float:  ToShortestFloat — try 6..=9 significant digits until f32 round-trips.

pub fn format_double(v: f64) -> String {
    if v.is_nan() {
        let bits = v.to_bits();
        return if bits == f64::NAN.to_bits() {
            "nan".to_owned()
        } else {
            format!("nan(0x{:016x})", bits)
        };
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }

    // Match Python's str(float) behavior: use scientific notation when
    // the exponent is < -4 or >= 16 (matching Python's default float repr).
    // This ensures compatibility with Python's text_format output.
    let abs_v = v.abs();
    let use_scientific = abs_v >= 1e16 || (abs_v != 0.0 && abs_v < 1e-4);

    let s = if use_scientific {
        // Use scientific notation, then convert to Python style
        format!("{:e}", v)
    } else {
        // Use default formatting (which may already include .0 for integers)
        format!("{}", v)
    };
    python_exponent_style(&s)
}

pub fn format_float(v: f32) -> String {
    if v.is_nan() {
        let bits = v.to_bits();
        return if bits == f32::NAN.to_bits() {
            "nan".to_owned()
        } else {
            format!("nan(0x{:08x})", bits)
        };
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    for prec in 6usize..=9 {
        let s = format!("{:.prec$e}", v, prec = prec - 1);
        let g_str = rust_sci_to_g_style(&s, prec);
        if let Ok(reparsed) = g_str.parse::<f32>() {
            if reparsed == v {
                return python_exponent_style(&g_str);
            }
        }
    }
    let s = format!("{:.8e}", v);
    python_exponent_style(&rust_sci_to_g_style(&s, 9))
}

/// Protoc-style double: try 15 significant digits (%g), fall back to 17.
///
/// Mirrors Python's `_format_floating_point_like_protoc(short_precision=15, long_precision=17)`:
/// `f'{value:.15g}'`, falling back to `f'{value:.17g}'` if needed for exact round-trip.
pub fn format_double_protoc(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    let s15 = format!("{:.14e}", v);
    let g15 = rust_sci_to_g_style(&s15, 15);
    if let Ok(r) = g15.parse::<f64>() {
        if r == v {
            return protoc_exponent_style(&g15);
        }
    }
    let s17 = format!("{:.16e}", v);
    protoc_exponent_style(&rust_sci_to_g_style(&s17, 17))
}

/// Protoc-style float: try 6 significant digits (%g), fall back to 9.
///
/// Mirrors Python's `_format_floating_point_like_protoc(short_precision=6, long_precision=9)`
/// with the vanilla protoc (Google C++) exact bit-level round-trip check:
/// re-parse the 6g string as f32 and compare bits, falling back to 9g only when
/// the bit patterns differ.  This replaces the former approximate `1e-7` tolerance (D3 fix).
pub fn format_float_protoc(v: f32) -> String {
    if v.is_nan() {
        return "nan".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    let s6 = format!("{:.5e}", v);
    let g6 = rust_sci_to_g_style(&s6, 6);
    if let Ok(r) = g6.parse::<f32>() {
        if r.to_bits() == v.to_bits() {
            return protoc_exponent_style(&g6);
        } // D3: exact bit check
    }
    let s9 = format!("{:.8e}", v);
    protoc_exponent_style(&rust_sci_to_g_style(&s9, 9))
}

/// Format exponent notation for protoc style (Python `%g`).
///
/// Normalises scientific notation to Python style (`e+01` → `e+01`, `e-04` → `e-04`).
/// Unlike `python_exponent_style`, does NOT append `.0` for whole numbers —
/// Python's `%g` format already omits trailing decimal points.
fn protoc_exponent_style(s: &str) -> String {
    if let Some(e_pos) = s.find('e') {
        let mantissa = &s[..e_pos];
        let exp_part = &s[e_pos + 1..];
        let (sign, digits) = if let Some(rest) = exp_part.strip_prefix('-') {
            ("-", rest)
        } else if let Some(rest) = exp_part.strip_prefix('+') {
            ("+", rest)
        } else {
            ("+", exp_part)
        };
        let digits: String = digits.trim_start_matches('0').to_owned();
        let digits = if digits.is_empty() {
            "0".to_owned()
        } else {
            digits
        };
        let formatted_exp = if digits.len() < 2 {
            format!("0{}", digits)
        } else {
            digits
        };
        format!("{}e{}{}", mantissa, sign, formatted_exp)
    } else {
        // No scientific notation — return as-is (do NOT add ".0")
        s.to_owned()
    }
}

fn python_exponent_style(s: &str) -> String {
    if let Some(e_pos) = s.find('e') {
        let mantissa = &s[..e_pos];
        let exp_part = &s[e_pos + 1..];
        let (sign, digits) = if let Some(rest) = exp_part.strip_prefix('-') {
            ("-", rest)
        } else if let Some(rest) = exp_part.strip_prefix('+') {
            ("+", rest)
        } else {
            ("+", exp_part)
        };
        let digits: String = digits.trim_start_matches('0').to_owned();
        let digits = if digits.is_empty() {
            "0".to_owned()
        } else {
            digits
        };
        let formatted_exp = if digits.len() < 2 {
            format!("0{}", digits)
        } else {
            digits
        };
        format!("{}e{}{}", mantissa, sign, formatted_exp)
    } else if !s.contains('.') && !s.contains('n') && !s.contains('i') {
        format!("{}.0", s)
    } else {
        s.to_owned()
    }
}

fn rust_sci_to_g_style(rust_sci: &str, prec: usize) -> String {
    let (mantissa_str, exp_str) = if let Some(pos) = rust_sci.find('e') {
        (&rust_sci[..pos], &rust_sci[pos + 1..])
    } else {
        return rust_sci.to_owned();
    };
    let exp: i32 = exp_str
        .parse()
        .expect("rust_sci_to_g_style: malformed exponent from Rust float formatter");
    let is_neg = mantissa_str.starts_with('-');
    // digits_str contains only ASCII decimal digits; char count == byte count.
    let digits_str = mantissa_str.trim_start_matches('-').replace('.', "");
    let sig_digits = digits_str.as_bytes();

    if exp >= -4 && exp < prec as i32 {
        // Keep decimal_pos as i32: (exp + 1) is negative when exp < -1, and
        // casting a negative i32 to usize would silently wrap to near-usize::MAX,
        // causing a capacity overflow panic in the .repeat() calls below.
        let decimal_pos = exp + 1; // i32; ≤ 0 when exp ∈ {-4,-3,-2,-1}
        let mut result = String::new();
        if is_neg {
            result.push('-');
        }
        if decimal_pos <= 0 {
            // exp < 0: output is "0.000…XYZ" with (-exp-1) leading zeros.
            // (-exp-1) as usize is safe here: exp ∈ [-4..-1] → value ∈ [0..3].
            result.push('0');
            result.push('.');
            result.push_str(&"0".repeat((-exp - 1) as usize));
            for d in sig_digits {
                result.push(*d as char);
            }
        } else if decimal_pos as usize >= sig_digits.len() {
            // decimal_pos > 0 here, so the cast to usize is safe.
            for d in sig_digits {
                result.push(*d as char);
            }
            result.push_str(&"0".repeat(decimal_pos as usize - sig_digits.len()));
            // No trailing ".0" — Python %g strips the decimal point entirely
            // for whole numbers (e.g. 1.0f32 → "1", not "1.0").
        } else {
            for (i, d) in sig_digits.iter().enumerate() {
                if i == decimal_pos as usize {
                    result.push('.');
                }
                result.push(*d as char);
            }
        }
        trim_trailing_zeros_after_dot(&mut result);
        result
    } else {
        let mut result = String::new();
        if is_neg {
            result.push('-');
        }
        if sig_digits.is_empty() {
            result.push('0');
        } else {
            result.push(sig_digits[0] as char);
            if sig_digits.len() > 1 {
                result.push('.');
                for d in &sig_digits[1..] {
                    result.push(*d as char);
                }
                trim_trailing_zeros_after_dot(&mut result);
            }
        }
        if exp >= 0 {
            result.push_str(&format!("e+{:02}", exp));
        } else {
            result.push_str(&format!("e-{:02}", -exp));
        }
        result
    }
}

fn trim_trailing_zeros_after_dot(s: &mut String) {
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_double_protoc — negative-exponent decimal form ─────────────────
    //
    // Regression for capacity-overflow panic in rust_sci_to_g_style.
    // Values with |v| ∈ [1e-4, 1e-2) have exponents -4, -3, -2.  Before the
    // fix, `decimal_pos = (exp + 1) as usize` wrapped a negative i32 to
    // near-usize::MAX, making `.repeat()` panic with a capacity overflow.

    #[test]
    fn double_protoc_exp_minus4() {
        // exp = -4 was the deepest broken case (decimal_pos → usize::MAX - 2).
        assert_eq!(format_double_protoc(1e-4_f64), "0.0001");
        assert_eq!(format_double_protoc(-1e-4_f64), "-0.0001");
        assert_eq!(format_double_protoc(5.5e-4_f64), "0.00055");
    }

    #[test]
    fn double_protoc_exp_minus3() {
        assert_eq!(format_double_protoc(1e-3_f64), "0.001");
        assert_eq!(format_double_protoc(-1.5e-3_f64), "-0.0015");
    }

    #[test]
    fn double_protoc_exp_minus2() {
        assert_eq!(format_double_protoc(1e-2_f64), "0.01");
        assert_eq!(format_double_protoc(3.75e-2_f64), "0.0375");
    }

    // ── Adjacent exponents that were always correct (boundary sanity) ─────────

    #[test]
    fn double_protoc_exp_minus1() {
        assert_eq!(format_double_protoc(1e-1_f64), "0.1");
        assert_eq!(format_double_protoc(2.5e-1_f64), "0.25");
    }

    #[test]
    fn double_protoc_exp_zero_and_positive() {
        assert_eq!(format_double_protoc(1.0_f64), "1");
        assert_eq!(format_double_protoc(1.5_f64), "1.5");
        assert_eq!(format_double_protoc(123.456_f64), "123.456");
    }

    #[test]
    fn double_protoc_scientific_below_threshold() {
        // exp = -5: below the -4 threshold, stays in scientific form.
        assert_eq!(format_double_protoc(1e-5_f64), "1e-05");
    }

    #[test]
    fn double_protoc_special_values() {
        assert_eq!(format_double_protoc(f64::NAN), "nan");
        assert_eq!(format_double_protoc(f64::INFINITY), "inf");
        assert_eq!(format_double_protoc(f64::NEG_INFINITY), "-inf");
    }

    // ── format_float_protoc — same broken range applies to f32 ────────────────

    #[test]
    fn float_protoc_exp_minus4() {
        assert_eq!(format_float_protoc(1e-4_f32), "0.0001");
        assert_eq!(format_float_protoc(5.5e-4_f32), "0.00055");
    }

    #[test]
    fn float_protoc_exp_minus3() {
        assert_eq!(format_float_protoc(1.5e-3_f32), "0.0015");
    }

    #[test]
    fn float_protoc_exp_minus2() {
        assert_eq!(format_float_protoc(1e-2_f32), "0.01");
    }
}
