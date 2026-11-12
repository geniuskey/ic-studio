//! SPICE style engineering number parsing and formatting (`1k`, `180n`, `2.2meg`).

/// Parse a SPICE number such as `10k`, `1.5u`, `3meg`, `1e-9`, `100nF`.
pub fn parse_spice(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    // Find the longest numeric prefix.
    let bytes = lower.as_bytes();
    let mut end = 0;
    let mut seen_e = false;
    while end < bytes.len() {
        let c = bytes[end] as char;
        let ok = c.is_ascii_digit()
            || c == '.'
            || ((c == '+' || c == '-') && (end == 0 || bytes[end - 1] == b'e'))
            || (c == 'e'
                && !seen_e
                && end > 0
                && bytes.get(end + 1).is_some_and(|n| n.is_ascii_digit() || *n == b'-' || *n == b'+'));
        if !ok {
            break;
        }
        if c == 'e' {
            seen_e = true;
        }
        end += 1;
    }
    let num: f64 = lower[..end].parse().ok()?;
    let suffix = &lower[end..];
    let mult = if suffix.starts_with("meg") {
        1e6
    } else if suffix.starts_with("mil") {
        25.4e-6
    } else {
        match suffix.chars().next() {
            None => 1.0,
            Some('t') => 1e12,
            Some('g') => 1e9,
            Some('k') => 1e3,
            Some('m') => 1e-3,
            Some('u') | Some('µ') => 1e-6,
            Some('n') => 1e-9,
            Some('p') => 1e-12,
            Some('f') => 1e-15,
            Some('a') => 1e-18,
            // Unit letters without scale (e.g. "1V", "5s", "2A", "10Hz")
            Some(c) if c.is_ascii_alphabetic() => 1.0,
            _ => return None,
        }
    };
    Some(num * mult)
}

/// Format a value with an engineering suffix, e.g. `1.5e-9` -> `1.5n`.
pub fn format_eng(v: f64, unit: &str) -> String {
    if v == 0.0 {
        return format!("0{unit}");
    }
    if !v.is_finite() {
        return format!("{v}");
    }
    const PREFIXES: [(f64, &str); 11] = [
        (1e12, "T"),
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
        (1e-18, "a"),
    ];
    let a = v.abs();
    for (scale, p) in PREFIXES {
        if a >= scale * 0.999_999 {
            let x = v / scale;
            // 4 significant digits
            let decimals = (3 - x.abs().log10().floor() as i32).clamp(0, 3) as usize;
            let s = format!("{x:.decimals$}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            return format!("{s}{p}{unit}");
        }
    }
    format!("{v:.3e}{unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(parse_spice("1k"), Some(1e3));
        assert_eq!(parse_spice("2.2meg"), Some(2.2e6));
        assert!((parse_spice("180n").unwrap() - 180e-9).abs() < 1e-18);
        assert_eq!(parse_spice("1e-9"), Some(1e-9));
        assert_eq!(parse_spice("5V"), Some(5.0));
        assert_eq!(parse_spice("-3.3"), Some(-3.3));
        assert!((parse_spice("10pF").unwrap() - 10e-12).abs() < 1e-24);
        assert_eq!(parse_spice("abc"), None);
    }

    #[test]
    fn format() {
        assert_eq!(format_eng(1.5e-9, "s"), "1.5ns");
        assert_eq!(format_eng(1000.0, "Ω"), "1kΩ");
        assert_eq!(format_eng(0.0, "V"), "0V");
    }
}
