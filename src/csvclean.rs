//! Messy CSV normalization.

use crate::str_field;
use serde_json::{json, Map, Value};

pub fn sniff_delimiter(text: &str) -> char {
    let sample: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();
    let mut best = (',', 0usize);
    for d in [',', ';', '\t', '|'] {
        let counts: Vec<usize> = sample.iter().map(|l| split_line(l, d).len()).collect();
        let first = *counts.first().unwrap_or(&0);
        // Prefer a delimiter that gives the same, >1 column count on every line.
        if first > 1 && counts.iter().all(|&c| c == first) && first > best.1 {
            best = (d, first);
        }
    }
    best.0
}

/// RFC 4180-style split of one line: quotes, doubled quotes, delimiters inside quotes.
pub fn split_line(line: &str, d: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => q = !q,
            c if c == d && !q => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// "Invoice #", "  Customer Name " -> "invoice_no", "customer_name"
pub fn header(h: &str) -> String {
    let h = h.trim().replace('#', " no ").replace('%', " pct ");
    let mut s = String::new();
    for c in h.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    s.trim_matches('_').to_string()
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn ymd(y: i32, m: u32, d: u32) -> Option<String> {
    let dim = [
        31,
        if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&m) || d == 0 || d > dim[(m - 1) as usize] || !(1900..=2100).contains(&y)
    {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// Dates to ISO 8601. `day_first` decides 03/04/2024 (EU = 3 April, US = 4 March).
pub fn date(s: &str, day_first: bool) -> Option<String> {
    let s = s.trim();
    let parts: Vec<&str> = s
        .split(['/', '-', '.', ' ', ','])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let num = |p: &str| p.parse::<u32>().ok();
    let month_name = |p: &str| {
        MONTHS
            .iter()
            .position(|m| p.to_ascii_lowercase().starts_with(m))
            .map(|i| i as u32 + 1)
    };
    if parts[0].len() == 4 {
        return ymd(
            num(parts[0])? as i32,
            num(parts[1]).or(month_name(parts[1]))?,
            num(parts[2])?,
        );
    }
    if let Some(m) = month_name(parts[0]) {
        return ymd(num(parts[2])? as i32, m, num(parts[1])?); // Jan 5 2024
    }
    if let Some(m) = month_name(parts[1]) {
        return ymd(num(parts[2])? as i32, m, num(parts[0])?); // 5 Jan 2024
    }
    let (a, b, mut y) = (num(parts[0])?, num(parts[1])?, num(parts[2])? as i32);
    if parts[2].len() == 2 {
        y += if y < 70 { 2000 } else { 1900 };
    }
    let (d, m) = if a > 12 {
        (a, b)
    } else if b > 12 {
        (b, a)
    } else if day_first {
        (a, b)
    } else {
        (b, a)
    };
    ymd(y, m, d)
}

/// Money to a plain decimal string: "$1,234.50" "(12.00)" "1.234,50 €" "-7" "12.5-".
pub fn money(s: &str) -> Option<String> {
    let t = s.trim();
    let neg = (t.starts_with('(') && t.ends_with(')')) || t.starts_with('-') || t.ends_with('-');
    let core: String = t
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    if core.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // Reject things that are clearly not money (letters other than currency codes).
    let letters: String = t.chars().filter(|c| c.is_alphabetic()).collect();
    if !letters.is_empty()
        && !["USD", "EUR", "GBP", "CAD", "AUD"].contains(&letters.to_ascii_uppercase().as_str())
    {
        return None;
    }
    let (last_dot, last_comma) = (core.rfind('.'), core.rfind(','));
    let normalized = match (last_dot, last_comma) {
        (Some(d), Some(c)) if c > d => core.replace('.', "").replace(',', "."), // 1.234,50
        (Some(_), Some(_)) => core.replace(',', ""),                            // 1,234.50
        (None, Some(c)) if core.len() - c - 1 <= 2 => core.replace(',', "."),   // 12,50
        (None, Some(_)) => core.replace(',', ""),                               // 1,234
        _ => core,
    };
    if normalized.matches('.').count() > 1 {
        return None;
    }
    Some(if neg {
        format!("-{normalized}")
    } else {
        normalized
    })
}

const NULLS: [&str; 8] = ["", "n/a", "na", "null", "none", "-", "--", "#n/a"];

pub fn normalize(
    text: &str,
    day_first: bool,
    date_cols: &[String],
    money_cols: &[String],
) -> Value {
    let text = text.trim_start_matches('\u{feff}');
    let d = sniff_delimiter(text);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let headers: Vec<String> = lines
        .next()
        .map(|l| split_line(l, d).iter().map(|h| header(h)).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut issues = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut dups = 0;
    for (n, line) in lines.enumerate() {
        let cells = split_line(line, d);
        if cells.len() != headers.len() {
            issues.push(json!({"row": n + 2, "issue": format!("expected {} columns, got {}", headers.len(), cells.len())}));
            continue;
        }
        let mut obj = Map::new();
        for (h, raw) in headers.iter().zip(cells.iter()) {
            let v = raw.trim();
            let val = if NULLS.contains(&v.to_ascii_lowercase().as_str()) {
                Value::Null
            } else if date_cols.contains(h) {
                match date(v, day_first) {
                    Some(iso) => json!(iso),
                    None => {
                        issues.push(json!({"row": n + 2, "column": h, "issue": format!("unparsed date `{v}`")}));
                        json!(v)
                    }
                }
            } else if money_cols.contains(h) {
                match money(v) {
                    Some(m) => json!(m),
                    None => {
                        issues.push(json!({"row": n + 2, "column": h, "issue": format!("unparsed amount `{v}`")}));
                        json!(v)
                    }
                }
            } else {
                json!(v.split_whitespace().collect::<Vec<_>>().join(" "))
            };
            obj.insert(h.clone(), val);
        }
        let row = Value::Object(obj);
        if !seen.insert(row.to_string()) {
            dups += 1;
            continue;
        }
        rows.push(row);
    }
    let delim = match d {
        '\t' => "tab".to_string(),
        c => c.to_string(),
    };
    json!({"delimiter": delim, "columns": headers, "rows": rows, "duplicates_removed": dups, "issues": issues})
}

pub fn normalize_op(i: &Value) -> Result<Value, String> {
    let s = |k: &str| -> Vec<String> {
        i[k].as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(header)).collect())
            .unwrap_or_default()
    };
    Ok(normalize(
        str_field(i, "csv")?,
        i["day_first"].as_bool().unwrap_or(false),
        &s("date_columns"),
        &s("money_columns"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delimiter_and_quotes() {
        assert_eq!(sniff_delimiter("a;b;c\n1;\"x;y\";3"), ';');
        assert_eq!(
            split_line("1,\"Smith, \"\"Jo\"\"\",3", ','),
            vec!["1", "Smith, \"Jo\"", "3"]
        );
    }
    #[test]
    fn headers() {
        assert_eq!(header("  Invoice # "), "invoice_no");
        assert_eq!(header("Customer  Name"), "customer_name");
    }
    #[test]
    fn dates() {
        assert_eq!(date("03/04/2024", false).unwrap(), "2024-03-04");
        assert_eq!(date("03/04/2024", true).unwrap(), "2024-04-03");
        assert_eq!(date("25.12.23", false).unwrap(), "2023-12-25");
        assert_eq!(date("Jan 5, 2024", false).unwrap(), "2024-01-05");
        assert_eq!(date("2024-02-29", false).unwrap(), "2024-02-29");
        assert_eq!(date("2023-02-29", false), None);
    }
    #[test]
    fn money_formats() {
        assert_eq!(money("$1,234.50").unwrap(), "1234.50");
        assert_eq!(money("(12.00)").unwrap(), "-12.00");
        assert_eq!(money("1.234,50 EUR").unwrap(), "1234.50");
        assert_eq!(money("12,5").unwrap(), "12.5");
        assert_eq!(money("1,234").unwrap(), "1234");
        assert_eq!(money("12,50").unwrap(), "12.50");
        assert_eq!(money("450-").unwrap(), "-450");
        assert_eq!(money("call me"), None);
    }
    #[test]
    fn full_normalize() {
        let r = normalize("\u{feff}Name|Due Date|Amount\nAcme| 01/02/2024 |$1,000.00\nAcme| 01/02/2024 |$1,000.00\nBeta|N/A|bad\n", false, &["due_date".into()], &["amount".into()]);
        assert_eq!(r["delimiter"], "|");
        assert_eq!(r["duplicates_removed"], 1);
        assert_eq!(r["rows"][0]["due_date"], "2024-01-02");
        assert_eq!(r["rows"][1]["due_date"], Value::Null);
        assert_eq!(r["issues"].as_array().unwrap().len(), 1);
    }
}
