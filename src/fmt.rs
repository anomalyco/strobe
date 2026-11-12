//! Formatting shared by modules below `app` (which re-exports it in `app::fmt`).

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

/// The unit a measured time shows in (SPEC §7.1 "Times"): microseconds unless the user picks
/// milliseconds or seconds (`--unit`, `"unit"` in strobe.json, MCP's `unit`). Only what people read
/// changes: every stored and `--json` number stays in its metric's unit (a `ms` metric in ms).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeUnit {
    #[default]
    Us,
    Ms,
    S,
}

impl TimeUnit {
    pub const NAMES: &'static [&'static str] = &["us", "ms", "s"];

    /// `us` (or `µs`), `ms` or `s`.
    pub fn parse(s: &str) -> Option<TimeUnit> {
        match s.trim() {
            "us" | "\u{b5}s" | "\u{3bc}s" => Some(TimeUnit::Us),
            "ms" => Some(TimeUnit::Ms),
            "s" => Some(TimeUnit::S),
            _ => None,
        }
    }

    /// The name `--unit` and strobe.json take: `us`, `ms`, `s`.
    pub fn as_str(self) -> &'static str {
        match self {
            TimeUnit::Us => "us",
            TimeUnit::Ms => "ms",
            TimeUnit::S => "s",
        }
    }

    /// The unit as printed: `µs` (U+00B5, which the card's font has; `us` where the terminal is not
    /// UTF-8), `ms`, `s`.
    pub fn label(self, ascii: bool) -> &'static str {
        match self {
            TimeUnit::Us if ascii => "us",
            TimeUnit::Us => "\u{b5}s",
            TimeUnit::Ms => "ms",
            TimeUnit::S => "s",
        }
    }

    /// This unit's amount in one millisecond.
    pub fn per_ms(self) -> f64 {
        match self {
            TimeUnit::Us => 1000.0,
            TimeUnit::Ms => 1.0,
            TimeUnit::S => 0.001,
        }
    }

    /// The most decimals a time shows in this unit: 0.01 µs, 0.0001 ms (0.1 µs), 0.000001 s (1 µs).
    /// A nonzero time below half of that prints as `<0.01 µs`, never as 0.
    fn finest(self) -> usize {
        match self {
            TimeUnit::Us => 2,
            TimeUnit::Ms => 4,
            TimeUnit::S => 6,
        }
    }
}

/// How times print: the unit, and whether the output is plain ASCII (`us` and `-`: a terminal whose
/// locale is not UTF-8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FmtCtx {
    pub unit: TimeUnit,
    pub ascii: bool,
}

impl FmtCtx {
    fn pack(self) -> u8 {
        (match self.unit {
            TimeUnit::Us => 0,
            TimeUnit::Ms => 1,
            TimeUnit::S => 2,
        }) | if self.ascii { 4 } else { 0 }
    }

    fn unpack(b: u8) -> FmtCtx {
        let unit = match b & 3 {
            1 => TimeUnit::Ms,
            2 => TimeUnit::S,
            _ => TimeUnit::Us,
        };
        FmtCtx { unit, ascii: b & 4 != 0 }
    }
}

/// The process's context (a CLI command's, set once before it runs, so its threads print alike).
static DEFAULT: AtomicU8 = AtomicU8::new(0);

thread_local! {
    /// One invocation's own context: the daemon runs several MCP calls at once, each on its thread.
    static LOCAL: Cell<Option<FmtCtx>> = const { Cell::new(None) };
}

/// How times print here: this thread's invocation's context, else the process's.
pub fn ctx() -> FmtCtx {
    LOCAL.with(Cell::get).unwrap_or_else(|| FmtCtx::unpack(DEFAULT.load(Ordering::Relaxed)))
}

/// The context an invocation on this thread entered (`scope`), if one did.
pub fn scoped() -> Option<FmtCtx> {
    LOCAL.with(Cell::get)
}

pub fn set_default(c: FmtCtx) {
    DEFAULT.store(c.pack(), Ordering::Relaxed);
}

/// A context entered on this thread; dropping it puts the one before back.
pub struct CtxScope(Option<FmtCtx>);

impl Drop for CtxScope {
    fn drop(&mut self) {
        LOCAL.with(|l| l.set(self.0));
    }
}

/// Enters `c` on this thread until the returned scope drops (a thread a command starts enters its
/// command's: `let c = fmt::ctx();` before, `let _f = fmt::scope(c);` in it).
pub fn scope(c: FmtCtx) -> CtxScope {
    CtxScope(LOCAL.with(|l| l.replace(Some(c))))
}

/// Whether a locale (`LC_ALL`, else `LC_CTYPE`, else `LANG`, as POSIX picks it) asks for something
/// other than UTF-8: `C`, `POSIX`, `en_US.ISO-8859-1`. Unset is taken as UTF-8 (a program started
/// from a GUI or an IDE has none, and its output is UTF-8). Windows writes to its console in UTF-16
/// whatever the code page and UTF-8 to a pipe or file, so there only an explicit `LC_ALL=C` (or
/// `LANG=C`) asks for ASCII, as on POSIX (Windows PowerShell 5.1 reads a pipe as its OEM code page).
pub fn ascii_locale() -> bool {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match get("LC_ALL").or_else(|| get("LC_CTYPE")).or_else(|| get("LANG")) {
        None => false,
        Some(l) => {
            let l = l.to_ascii_lowercase();
            !(l.contains("utf-8") || l.contains("utf8"))
        }
    }
}

/// The minus sign: U+2212, or `-` in ASCII output.
pub fn minus() -> &'static str {
    if ctx().ascii { "-" } else { "\u{2212}" }
}

/// The active time unit as printed (`µs`), for a column header or an axis.
pub fn time_label() -> &'static str {
    let c = ctx();
    c.unit.label(c.ascii)
}

/// `1234567` → `1,234,567`: an integer's digits in groups of three; a fraction after them as it is.
pub fn group(digits: &str) -> String {
    let (int, frac) = digits.split_once('.').map_or((digits, None), |(i, f)| (i, Some(f)));
    let (sign, int) = int.strip_prefix('-').map_or(("", int), |i| ("-", i));
    let mut out = String::with_capacity(int.len() + int.len() / 3 + 2);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

/// Decimals a time of size `x` (≥ 0, in the shown unit) prints with: every whole digit and at least
/// three significant ones (`738,147`, `42.0`, `1.23`, `0.738`, `0.0420`), at most `finest`. Taken
/// again after rounding, so 99.96 prints `100`, not `100.0`.
fn time_digits(x: f64, finest: usize) -> usize {
    let d = |x: f64| -> usize {
        if x >= 100.0 {
            0
        } else if x >= 10.0 {
            1
        } else if x >= 1.0 {
            2
        } else {
            (3..=finest.max(3)).find(|&d| x * 10f64.powi(d as i32) >= 100.0).unwrap_or(finest).min(finest)
        }
    };
    let first = d(x);
    let rounded: f64 = format!("{x:.first$}").parse().unwrap_or(x);
    d(rounded).min(first)
}

/// `x` (≥ 0, in the shown unit) at `d` decimals with its digits grouped; `<0.01` (the unit's finest
/// step) for a nonzero one that would print as 0 (SPEC §1 rule 1: a time that was measured is never
/// shown as nothing).
fn time_body(x: f64, d: usize, finest: usize) -> String {
    let s = format!("{x:.d$}");
    if s.chars().all(|c| c == '0' || c == '.') {
        let step = format!("{:.finest$}", 10f64.powi(-(finest as i32)));
        return format!("<{step}");
    }
    group(&s)
}

fn time_parts(ms: f64, d: Option<usize>, signed: bool) -> String {
    if !ms.is_finite() {
        return "—".into();
    }
    let c = ctx();
    // Exactly 0 is the only time that prints as 0: one that scaling took under the smallest double
    // (5e-324 ms in s) is still nonzero (`<0.000001 s`), and one it took past the largest prints `—`.
    if ms == 0.0 {
        return format!("0 {}", c.unit.label(c.ascii));
    }
    let x = (ms * c.unit.per_ms()).abs();
    if !x.is_finite() {
        return "—".into();
    }
    let d = d.unwrap_or_else(|| time_digits(x, c.unit.finest()));
    let body = if x >= EXACT { sci(x) } else { time_body(x, d, c.unit.finest()) };
    let sign = if ms < 0.0 {
        minus()
    } else if ms > 0.0 && signed {
        "+"
    } else {
        ""
    };
    format!("{sign}{body} {}", c.unit.label(c.ascii))
}

/// A measured time given in milliseconds, in the active unit: `738,147 µs`, `42.0 µs`, `0.40 µs`,
/// `<0.01 µs`, `0 µs`; `738 ms`, `0.0420 ms`; `18.3 s`, `0.738 s` (`time_digits`). A negative one
/// takes the minus sign.
pub fn time(ms: f64) -> String {
    time_parts(ms, None, false)
}

/// A difference of times in milliseconds, signed: `+1,234 µs`, `−42.0 µs`, `+<0.01 µs`; exactly 0 is
/// `0 µs`, with no sign.
pub fn time_signed(ms: f64) -> String {
    time_parts(ms, None, true)
}

/// Times printed together (an estimate and its interval's ends, a check's `[lowest … highest]`): each
/// as it prints alone (`+3.20 µs [−1.10 … +12.4 µs]`). Shared decimals would print a small end with
/// fewer digits than its own (0.05 ms beside 250 ms read `0.1 ms`) or a large one with digits it does
/// not have (`12.40`).
pub fn times(ms: &[f64], signed: bool) -> Vec<String> {
    ms.iter().map(|v| time_parts(*v, None, signed)).collect()
}

/// A time's number alone (an axis tick under a header that names the unit): `250,000`.
pub fn time_number(ms: f64) -> String {
    let t = time(ms);
    t.rsplit_once(' ').map_or(t.clone(), |(n, _)| n.to_string())
}

/// 2^53: from here a double no longer holds every integer, so whole digits past it would be the
/// binary number's, not the value's (a count of 1e307 once printed `9,223,372,036,854,775,807`, a cast
/// that saturated).
pub const EXACT: f64 = 9_007_199_254_740_992.0;

/// A number too large to print exactly (`|v| ≥ 2^53`) in scientific notation with three significant
/// digits: `1.00e307`, `-4.50e16` (Rust's `{:.2e}`: the exact decimal rounded, ties to even; no `+`).
pub fn sci(v: f64) -> String {
    format!("{v:.2e}")
}

/// `text` without a minus sign when every digit it shows is 0 (`-0 B`, `-0.0`, `−0 instr` → `0 B`,
/// `0.0`, `0 instr`): a value that rounds to nothing has no direction, and nothing strobe prints
/// reads as minus zero.
pub fn no_minus_zero(text: String) -> String {
    let zero = !text.chars().any(|c| c.is_ascii_digit() && c != '0');
    match text.strip_prefix('-').or_else(|| text.strip_prefix('\u{2212}')) {
        Some(rest) if zero => rest.to_string(),
        _ => text,
    }
}

/// `v` at `d` decimals with its sign, none on what prints as 0: `+1.5`, `−0.3`, `0.0` (never `+0.0` or
/// `−0.0`).
pub fn signed_fixed(v: f64, d: usize) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let body = format!("{:.d$}", v.abs());
    if !body.chars().any(|c| c.is_ascii_digit() && c != '0') {
        body
    } else if v < 0.0 {
        format!("{}{body}", minus())
    } else {
        format!("+{body}")
    }
}

/// Bytes in binary units with three significant digits at most and at least the unit's whole number:
/// `512 B`, `1.4 KB`, `12 KB`, `3.40 MB`, `34.5 MB`, `1.25 GB` (`—` when non-finite). Whole KB once
/// printed 1.4 KB as `1 KB`.
pub fn bytes(v: f64) -> String {
    no_minus_zero(bytes_of(v))
}

fn bytes_of(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    if v.abs() >= EXACT {
        return format!("{} B", sci(v));
    }
    let a = v.abs();
    let scaled = |div: f64, unit: &str| {
        let x = v / div;
        let digits = if x.abs() >= 100.0 {
            0
        } else if x.abs() >= 10.0 {
            1
        } else {
            2
        };
        format!("{x:.digits$} {unit}")
    };
    if a >= 1024.0 * 1024.0 * 1024.0 {
        scaled(1024.0 * 1024.0 * 1024.0, "GB")
    } else if a >= 1024.0 * 1024.0 {
        scaled(1024.0 * 1024.0, "MB")
    } else if a >= 1024.0 {
        let x = v / 1024.0;
        if x.abs() >= 10.0 { format!("{x:.0} KB") } else { format!("{x:.1} KB") }
    } else {
        format!("{v:.0} B")
    }
}

/// Decimals a number of this size needs to show its first significant digit, at least `min` and at
/// most `max`: 0.03 needs 2, 0.004 needs 3, 12 needs none. 0, and a value too small to show at `max`
/// (an interval end of 8.67·10⁻¹⁸ from the Walsh averages' rounding), need `min`: they print as 0.
pub fn digits_for(v: f64, min: usize, max: usize) -> usize {
    let a = v.abs();
    if !a.is_finite() || a == 0.0 {
        return min;
    }
    // The decimal that shows it, after rounding: 0.096 shows as 0.1 at one decimal.
    let d = (0..=max).find(|&d| (a * 10f64.powi(d as i32)).round() >= 1.0).unwrap_or(min);
    d.clamp(min, max)
}

/// Decimals an interval `[lo, hi]` (in the unit it prints in) needs so its width shows its significant
/// digits, at most `max` (SPEC §9.3 "Digits"), as the Particle Data Group rounds an uncertainty: two
/// when the width's leading digits are 100 to 354 (1.6 points: tenths; 3 points: tenths), else one
/// (0.8: tenths; 6: whole numbers). One digit alone printed a 1.6-point interval `[+1% … +2%]`.
pub fn width_digits(lo: f64, hi: f64, max: usize) -> usize {
    let w = (hi - lo).abs();
    if !w.is_finite() || w == 0.0 {
        return 0;
    }
    let exp = w.log10().floor();
    let lead = w / 10f64.powf(exp - 2.0);
    let sig = if lead < 354.5 { 2.0 } else { 1.0 };
    ((sig - 1.0 - exp).max(0.0) as usize).min(max)
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
    // A fraction past f64's range once it is a percentage (1e307 → inf) is no number to print.
    if !p.is_finite() {
        return "—".into();
    }
    let body = if p.abs() >= EXACT { sci(p.abs()) } else { format!("{:.digits$}", p.abs()) };
    let zero = (p.abs() * 10f64.powi(digits as i32)).round() == 0.0;
    let sign = if zero {
        ""
    } else if p < 0.0 {
        minus()
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
/// "Digits"): all three at the decimals that show the width's significant digits (`width_digits`: a
/// 166-point interval in whole percent, a 0.8- or 1.6-point one in tenths); a number that would round
/// to 0 there keeps its own first digit, so a decided interval never prints an end of 0.0%. Without
/// an interval, the estimate's usual digits (`pct`). `+5.0% [+4.6% … +5.4%]`, `+1.0% [+0.4% … +2.0%]`,
/// `+0.030% [+0.020% … +0.040%]`, `+7% [−51% … +116%]`.
pub fn pct_ci(est: f64, lo: f64, hi: f64) -> (String, String, String) {
    if !(lo.is_finite() && hi.is_finite()) || hi == lo {
        return (pct(est), pct(lo), pct(hi));
    }
    let common = width_digits(lo * 100.0, hi * 100.0, 3);
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
/// `no worse (≤1.5%)`, `same (±0.006)`, `same (±520 µs)`): a fraction for ratio metrics, else `abs`
/// in `unit` (a time in the active unit, `time`), never rounded to a different bound (±0.3% was
/// printed ±0%, 1.5% as 2%) nor to 0 (`same (±0 ms)` from a bound of 0.04 ms).
pub fn bound(fraction: f64, relative: bool, abs: f64, unit: &str) -> String {
    let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if relative {
        let p = fraction * 100.0;
        return format!("{}%", trim(format!("{p:.*}", digits_for(p, 0, 3).max(2))));
    }
    if unit == "bytes" {
        return bytes(abs);
    }
    if unit == "ms" {
        return time(abs);
    }
    let n = trim(format!("{abs:.*}", digits_for(abs, 0, 4).max(2)));
    match unit {
        "%" => format!("{n} pp"),
        _ => n,
    }
}
