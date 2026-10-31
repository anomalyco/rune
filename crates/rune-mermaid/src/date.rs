//! Calendar arithmetic for gantt charts, with no time zones: times are milliseconds from
//! 1970-01-01 00:00. Dates read with a dayjs-style `dateFormat` and print with a d3-style
//! `axisFormat`.

const SEC: f64 = 1000.0;
const MIN: f64 = 60.0 * SEC;
const HOUR: f64 = 60.0 * MIN;
pub(crate) const DAY: f64 = 24.0 * HOUR;

/// Days from 1970-01-01 to `y-m-d` (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(year, month, day)` of a day number from [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A time's calendar fields.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fields {
    y: i64,
    mo: u32,
    d: u32,
    h: u32,
    mi: u32,
    s: u32,
    ms: u32,
    /// 0 is Sunday.
    wd: u32,
}

fn fields(t: f64) -> Fields {
    let day = (t / DAY).floor();
    let (y, mo, d) = civil_from_days(day as i64);
    let rest = (t - day * DAY).max(0.0) as u64;
    Fields {
        y,
        mo,
        d,
        h: (rest / HOUR as u64) as u32,
        mi: (rest / MIN as u64 % 60) as u32,
        s: (rest / SEC as u64 % 60) as u32,
        ms: (rest % 1000) as u32,
        wd: (day as i64 + 4).rem_euclid(7) as u32,
    }
}

fn at(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> Option<f64> {
    let ok = (1..=12).contains(&mo) && d >= 1 && d <= days_in_month(y, mo) && h < 24 && mi < 60 && s < 60 && (-9999..=9999).contains(&y);
    ok.then(|| days_from_civil(y, mo, d) as f64 * DAY + h as f64 * HOUR + mi as f64 * MIN + s as f64 * SEC + ms as f64)
}

/// `t` moved by `n` calendar months, keeping its day where the month has it.
fn add_months(t: f64, n: i64) -> f64 {
    let f = fields(t);
    let m0 = f.y * 12 + f.mo as i64 - 1 + n;
    let (y, mo) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
    let d = f.d.min(days_in_month(y, mo));
    at(y, mo, d, f.h, f.mi, f.s, f.ms).unwrap_or(t)
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// `s` read with a dayjs format (`YYYY-MM-DD`, `DD/MM/YYYY HH:mm`, `X`, …), or, failing that,
/// as an ISO date and time.
pub(crate) fn parse(s: &str, format: &str) -> Option<f64> {
    let s = s.trim();
    with_format(s, format)
        .or_else(|| ["YYYY-MM-DDTHH:mm:ss", "YYYY-MM-DD HH:mm:ss", "YYYY-MM-DDTHH:mm", "YYYY-MM-DD HH:mm", "YYYY-MM-DD"].iter().find_map(|f| with_format(s, f)))
}

fn with_format(s: &str, format: &str) -> Option<f64> {
    let (mut y, mut mo, mut d, mut h, mut mi, mut sec, mut ms) = (1970i64, 1u32, 1u32, 0u32, 0u32, 0u32, 0u32);
    let mut pm = None;
    let (f, mut rest) = (format.as_bytes(), s);
    let mut i = 0;
    let number = |rest: &mut &str, max: usize| -> Option<i64> {
        let n = rest.bytes().take(max).take_while(u8::is_ascii_digit).count();
        let v = rest.get(..n).filter(|v| !v.is_empty())?.parse().ok()?;
        *rest = &rest[n..];
        Some(v)
    };
    const TOKENS: [&str; 20] = ["YYYY", "MMMM", "MMM", "SSS", "YY", "MM", "DD", "HH", "hh", "mm", "ss", "Do", "M", "D", "H", "h", "m", "s", "A", "a"];
    while i < f.len() {
        if f[i] == b'[' {
            let end = format[i..].find(']').map_or(f.len(), |e| i + e);
            rest = rest.strip_prefix(&format[i + 1..end])?;
            i = end + 1;
            continue;
        }
        if f[i] == b'X' || f[i] == b'x' {
            let neg = rest.starts_with('-');
            let digits = rest.trim_start_matches('-');
            let n = digits.bytes().take_while(|b| b.is_ascii_digit() || *b == b'.').count();
            let v: f64 = digits[..n].parse().ok()?;
            let v = if neg { -v } else { v } * if f[i] == b'X' { SEC } else { 1.0 };
            return (digits[n..].trim().is_empty() && v.abs() < 2.5e14).then_some(v);
        }
        let Some(tok) = TOKENS.iter().find(|t| format[i..].starts_with(**t)) else {
            rest = rest.strip_prefix(format[i..].chars().next()?)?;
            i += format[i..].chars().next()?.len_utf8();
            continue;
        };
        match *tok {
            "YYYY" => y = number(&mut rest, 4)?,
            "YY" => y = 2000 + number(&mut rest, 2)?,
            "MMMM" | "MMM" => {
                let lower = rest.to_ascii_lowercase();
                let (m, len) = MONTHS.iter().enumerate().find_map(|(k, name)| {
                    let name = name.to_ascii_lowercase();
                    if lower.starts_with(&name) {
                        Some((k, name.len()))
                    } else {
                        lower.starts_with(&name[..3]).then_some((k, 3))
                    }
                })?;
                mo = m as u32 + 1;
                rest = &rest[len..];
            }
            "MM" | "M" => mo = number(&mut rest, 2)? as u32,
            "DD" | "D" => d = number(&mut rest, 2)? as u32,
            "Do" => {
                d = number(&mut rest, 2)? as u32;
                rest = rest.get(2..).filter(|_| rest.starts_with(|c: char| c.is_ascii_alphabetic()))?;
            }
            "HH" | "H" | "hh" | "h" => h = number(&mut rest, 2)? as u32,
            "mm" | "m" => mi = number(&mut rest, 2)? as u32,
            "ss" | "s" => sec = number(&mut rest, 2)? as u32,
            "SSS" => ms = number(&mut rest, 3)? as u32,
            _ => {
                let lower = rest.get(..2)?.to_ascii_lowercase();
                pm = Some(match lower.as_str() {
                    "am" => false,
                    "pm" => true,
                    _ => return None,
                });
                rest = &rest[2..];
            }
        }
        i += tok.len();
    }
    if !rest.trim().is_empty() {
        return None;
    }
    if let Some(pm) = pm {
        h = h % 12 + if pm { 12 } else { 0 };
    }
    at(y, mo, d, h, mi, sec, ms)
}

/// A length of time: fixed milliseconds, or calendar months.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Span {
    Ms(f64),
    Months(f64),
}

impl Span {
    pub(crate) fn after(self, t: f64) -> f64 {
        match self {
            Span::Ms(ms) => t + ms,
            Span::Months(n) => {
                let whole = n.trunc();
                add_months(t, whole as i64) + (n - whole) * 30.0 * DAY
            }
        }
    }
}

/// `3d`, `1.5h`, `2w`, `30m`, `250ms`, `1M` (a month), `1y`.
pub(crate) fn span(s: &str) -> Option<Span> {
    let s = s.trim();
    let n = s.bytes().take_while(|b| b.is_ascii_digit() || *b == b'.').count();
    let v: f64 = s.get(..n).filter(|v| !v.is_empty())?.parse().ok()?;
    if !v.is_finite() || v > 1e6 {
        return None;
    }
    Some(match s[n..].trim() {
        "ms" => Span::Ms(v),
        "s" => Span::Ms(v * SEC),
        "m" => Span::Ms(v * MIN),
        "h" => Span::Ms(v * HOUR),
        "d" => Span::Ms(v * DAY),
        "w" => Span::Ms(v * 7.0 * DAY),
        "M" => Span::Months(v),
        "y" => Span::Months(v * 12.0),
        _ => return None,
    })
}

/// `t` printed with a d3 time format: `%Y %y %m %d %e %H %I %M %S %L %p %b %B %a %A %j %%`, with
/// `-`, `_` or `0` after `%` for no, space or zero padding.
pub(crate) fn format(t: f64, fmt: &str) -> String {
    let f = fields(t);
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut code = chars.next().unwrap_or('%');
        let pad = match code {
            '-' | '_' | '0' => {
                let p = code;
                code = chars.next().unwrap_or('%');
                Some(p)
            }
            _ => None,
        };
        let num = |out: &mut String, v: i64, width: usize, default: char| match pad.unwrap_or(default) {
            '-' => out.push_str(&v.to_string()),
            '_' => out.push_str(&format!("{v:>width$}")),
            _ => out.push_str(&format!("{v:0width$}")),
        };
        let doy = days_from_civil(f.y, f.mo, f.d) - days_from_civil(f.y, 1, 1) + 1;
        match code {
            'Y' => num(&mut out, f.y, 4, '0'),
            'y' => num(&mut out, f.y.rem_euclid(100), 2, '0'),
            'm' => num(&mut out, f.mo as i64, 2, '0'),
            'd' => num(&mut out, f.d as i64, 2, '0'),
            'e' => num(&mut out, f.d as i64, 2, '_'),
            'H' => num(&mut out, f.h as i64, 2, '0'),
            'I' => num(&mut out, ((f.h + 11) % 12 + 1) as i64, 2, '0'),
            'M' => num(&mut out, f.mi as i64, 2, '0'),
            'S' => num(&mut out, f.s as i64, 2, '0'),
            'L' => num(&mut out, f.ms as i64, 3, '0'),
            'j' => num(&mut out, doy, 3, '0'),
            'p' => out.push_str(if f.h < 12 { "AM" } else { "PM" }),
            'b' => out.push_str(&MONTHS[f.mo as usize - 1][..3]),
            'B' => out.push_str(MONTHS[f.mo as usize - 1]),
            'a' => out.push_str(&DAYS[f.wd as usize][..3]),
            'A' => out.push_str(DAYS[f.wd as usize]),
            other => out.push(other),
        }
    }
    out
}

/// A step between axis ticks, aligned to the calendar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Step {
    Ms(f64),
    Days(u32),
    Weeks(u32),
    Months(u32),
}

impl Step {
    /// The step's rough length, for spacing ticks.
    pub(crate) fn len(self) -> f64 {
        match self {
            Step::Ms(ms) => ms,
            Step::Days(n) => n as f64 * DAY,
            Step::Weeks(n) => n as f64 * 7.0 * DAY,
            Step::Months(n) => n as f64 * 30.44 * DAY,
        }
    }

    /// `1day`, `2week`, `6hour`, `1month` (Mermaid's `tickInterval`).
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        let v: u32 = s[..n].parse().ok().filter(|v| (1..=1000).contains(v))?;
        Some(match s[n..].trim() {
            "millisecond" => Step::Ms(v as f64),
            "second" => Step::Ms(v as f64 * SEC),
            "minute" => Step::Ms(v as f64 * MIN),
            "hour" => Step::Ms(v as f64 * HOUR),
            "day" => Step::Days(v),
            "week" => Step::Weeks(v),
            "month" => Step::Months(v),
            _ => return None,
        })
    }

    /// The steps an axis picks from, finest first.
    pub(crate) const LADDER: [Step; 21] = [
        Step::Ms(SEC),
        Step::Ms(5.0 * SEC),
        Step::Ms(15.0 * SEC),
        Step::Ms(30.0 * SEC),
        Step::Ms(MIN),
        Step::Ms(5.0 * MIN),
        Step::Ms(15.0 * MIN),
        Step::Ms(30.0 * MIN),
        Step::Ms(HOUR),
        Step::Ms(3.0 * HOUR),
        Step::Ms(6.0 * HOUR),
        Step::Ms(12.0 * HOUR),
        Step::Days(1),
        Step::Days(2),
        Step::Weeks(1),
        Step::Weeks(2),
        Step::Months(1),
        Step::Months(3),
        Step::Months(6),
        Step::Months(12),
        Step::Months(60),
    ];

    /// Ticks from `lo` to `hi` inclusive, at most `max` of them: midnights for days, Sundays for
    /// weeks, the first of the month for months.
    pub(crate) fn ticks(self, lo: f64, hi: f64, max: usize) -> Vec<f64> {
        let mut t = match self {
            Step::Ms(ms) => (lo / ms).ceil() * ms,
            Step::Days(n) => {
                let mut day = (lo / DAY).ceil() as i64;
                while (civil_from_days(day).2 - 1) % n != 0 {
                    day += 1;
                }
                day as f64 * DAY
            }
            Step::Weeks(_) => {
                let day = (lo / DAY).ceil() as i64;
                (day + (7 - (day + 4).rem_euclid(7)) % 7) as f64 * DAY
            }
            Step::Months(n) => {
                let f = fields(lo);
                let mut m0 = f.y * 12 + f.mo as i64 - 1;
                if at(f.y, f.mo, 1, 0, 0, 0, 0).is_some_and(|first| first < lo) {
                    m0 += 1;
                }
                while m0.rem_euclid(n as i64) != 0 {
                    m0 += 1;
                }
                at(m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1, 1, 0, 0, 0, 0).unwrap_or(lo)
            }
        };
        let mut out = Vec::new();
        while t <= hi + 1e-6 && out.len() < max {
            out.push(t);
            t = match self {
                Step::Ms(ms) => t + ms,
                Step::Weeks(n) => t + n as f64 * 7.0 * DAY,
                Step::Days(n) => {
                    // Back to the 1st at month ends, as d3 does.
                    let next = t + n as f64 * DAY;
                    let (f, g) = (fields(t), fields(next));
                    if g.mo != f.mo && g.d > 1 { at(g.y, g.mo, 1, 0, 0, 0, 0).unwrap_or(next) } else { next }
                }
                Step::Months(n) => add_months(t, n as i64),
            };
        }
        out
    }
}

#[cfg(test)]
#[path = "date_tests.rs"]
mod tests;
