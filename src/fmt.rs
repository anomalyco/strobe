//! Formatting shared by modules below `app` (which re-exports it in `app::fmt`).

/// Bytes in binary units: `512 B`, `12 KB`, `3.4 MB`, `1.25 GB` (`—` when non-finite).
pub fn bytes(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let a = v.abs();
    if a >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", v / (1024.0 * 1024.0 * 1024.0))
    } else if a >= 1024.0 * 1024.0 {
        format!("{:.1} MB", v / (1024.0 * 1024.0))
    } else if a >= 1024.0 {
        format!("{:.0} KB", v / 1024.0)
    } else {
        format!("{v:.0} B")
    }
}

/// Decimals a number of this size needs to show its first significant digit, at least `min` and at
/// most `max`: 0.03 needs 2, 0.004 needs 3, 12 needs none. 0 needs `min`.
pub fn digits_for(v: f64, min: usize, max: usize) -> usize {
    let a = v.abs();
    if !a.is_finite() || a == 0.0 {
        return min;
    }
    // The decimal that shows it, after rounding: 0.096 shows as 0.1 at one decimal.
    let d = (0..=max).find(|&d| (a * 10f64.powi(d as i32)).round() >= 1.0).unwrap_or(max);
    d.clamp(min, max)
}

/// Decimals an interval `[lo, hi]` (in the unit it prints in) needs so its width shows its first
/// significant digit, at most `max` (SPEC §9.3 "Digits"): a 3-point-wide interval prints whole numbers,
/// 0.2 tenths, 0.02 hundredths.
pub fn width_digits(lo: f64, hi: f64, max: usize) -> usize {
    let w = (hi - lo).abs();
    if !w.is_finite() || w == 0.0 {
        return 0;
    }
    ((-w.log10().floor()).max(0.0) as usize).min(max)
}

/// The usual decimals of a percentage: none from 10%, one from 0.1%, and below that as many as show its
/// first significant digit (up to three), so a nonzero change never prints as 0.0%.
fn pct_default(p: f64) -> usize {
    if p.abs() >= 10.0 { 0 } else { digits_for(p, 1, 3) }
}

/// A fraction as a signed percentage with `digits` decimals: −7.7%, +12%, 0.0% (no sign on what
/// rounds to zero).
pub fn pct_with(r: f64, digits: usize) -> String {
    if !r.is_finite() {
        return "—".into();
    }
    let p = r * 100.0;
    let body = format!("{:.digits$}", p.abs());
    let zero = (p.abs() * 10f64.powi(digits as i32)).round() == 0.0;
    let sign = if zero {
        ""
    } else if p < 0.0 {
        "−"
    } else {
        "+"
    };
    format!("{sign}{body}%")
}

/// A relative change as a signed percentage, e.g. −7.7%, +12%, +0.03%.
pub fn pct(r: f64) -> String {
    pct_with(r, pct_default(r * 100.0))
}

/// An estimate and its interval as percentages whose precision follows the interval's width (SPEC §9.3
/// "Digits"): all three at the same decimals, the width's first significant digit or the estimate's
/// usual ones, whichever is finer; an end that would round to 0 keeps its own first digit, so a decided
/// interval never prints an end of 0.0%. `+15.2% [+15.1% … +15.3%]`, `+0.03% [+0.02% … +0.04%]`.
pub fn pct_ci(est: f64, lo: f64, hi: f64) -> (String, String, String) {
    let common = width_digits(lo * 100.0, hi * 100.0, 3).max(pct_default(est * 100.0));
    let own = |x: f64| pct_with(x, common.max(if x == 0.0 { 0 } else { digits_for(x * 100.0, 0, 3) }));
    (own(est), own(lo), own(hi))
}

/// A percentage printed beside a threshold it is compared with (`+15.2% > 15%`): with enough decimals
/// that the two do not read the same, up to three.
pub fn pct_vs(r: f64, threshold: f64) -> String {
    let mut d = pct_default(r * 100.0);
    while d < 3 && r != threshold && (r * 100.0 * 10f64.powi(d as i32)).round() == (threshold * 100.0 * 10f64.powi(d as i32)).round() {
        d += 1;
    }
    pct_with(r, d)
}

/// An equivalence bound or non-inferiority margin as it prints in a verdict (`same (±2%)`,
/// `no worse (≤1.5%)`, `same (±0.006)`): a fraction for ratio metrics, else `abs` in `unit`, never
/// rounded to a different bound (±0.3% was printed ±0%, 1.5% as 2%).
pub fn bound(fraction: f64, relative: bool, abs: f64, unit: &str) -> String {
    let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if relative {
        let p = fraction * 100.0;
        return format!("{}%", trim(format!("{p:.*}", digits_for(p, 0, 3).max(2))));
    }
    if unit == "bytes" {
        return bytes(abs);
    }
    let n = trim(format!("{abs:.*}", digits_for(abs, 0, 4).max(2)));
    match unit {
        "ms" => format!("{n} ms"),
        "%" => format!("{n} pp"),
        _ => n,
    }
}
