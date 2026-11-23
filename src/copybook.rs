//! COBOL copybook parsing and EBCDIC record decoding.
//!
//! Supported clauses: `PIC X(n)`, `PIC 9(n)`, `PIC S9(n)V9(m)` (zoned, sign overpunch in the
//! last byte), `COMP-3` (packed decimal), `COMP` / `BINARY` (big-endian two's complement),
//! `OCCURS n TIMES` on elementary items, and `FILLER`. Group items (no PIC) are skipped.
//! Numbers are returned as decimal strings so no precision is lost.

use crate::{cp037::CP037, field, str_field};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Alpha,
    Zoned,
    Packed,
    Binary,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub digits: usize, // total digits (int + frac) for numerics, chars for alpha
    pub scale: usize,  // digits after the implied decimal point (V)
    pub signed: bool,
    pub len: usize, // bytes in the record
    pub occurs: usize,
}

/// Expand `9(5)V99` style pictures into (int digits, frac digits, alpha?).
fn picture(p: &str) -> Result<(usize, usize, bool, bool), String> {
    let p = p.to_ascii_uppercase();
    let (mut int, mut frac, mut alpha, mut signed, mut after_v) =
        (0usize, 0usize, false, false, false);
    let chars: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut count = 1;
        if i + 1 < chars.len() && chars[i + 1] == '(' {
            let end = p[i + 2..].find(')').ok_or("unclosed ( in PIC")? + i + 2;
            count = p[i + 2..end]
                .parse()
                .map_err(|_| "bad repeat count in PIC")?;
            i = end;
        }
        match c {
            'S' => signed = true,
            'V' => after_v = true,
            '9' if after_v => frac += count,
            '9' => int += count,
            'X' | 'A' => {
                alpha = true;
                int += count
            }
            _ => return Err(format!("unsupported PIC symbol `{c}`")),
        }
        i += 1;
    }
    Ok((int, frac, alpha, signed))
}

pub fn parse(copybook: &str) -> Result<Vec<Field>, String> {
    // Statements end with a period; columns 1-6 / 73-80 are often sequence numbers.
    let cleaned: String = copybook
        .lines()
        .filter(|l| !l.trim_start().starts_with('*'))
        .map(|l| {
            if l.len() > 6 && l.is_char_boundary(6) && l[..6].chars().all(|c| c.is_ascii_digit())
            {
                &l[6..]
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut fields = Vec::new();
    for stmt in cleaned.split('.') {
        let toks: Vec<&str> = stmt.split_whitespace().collect();
        if toks.len() < 2 || toks[0].parse::<u32>().is_err() {
            continue;
        }
        let up: Vec<String> = toks.iter().map(|t| t.to_ascii_uppercase()).collect();
        let Some(pi) = up.iter().position(|t| t == "PIC" || t == "PICTURE") else {
            continue;
        };
        let pic = toks.get(pi + 1).ok_or("PIC without picture")?;
        let (int, frac, alpha, signed) = picture(pic)?;
        let usage = up
            .iter()
            .find(|t| t.starts_with("COMP") || *t == "BINARY" || *t == "PACKED-DECIMAL");
        let kind = match (alpha, usage.map(String::as_str)) {
            (true, _) => Kind::Alpha,
            (false, Some("COMP-3")) | (false, Some("PACKED-DECIMAL")) => Kind::Packed,
            (false, Some("COMP")) | (false, Some("COMP-4")) | (false, Some("BINARY")) => {
                Kind::Binary
            }
            (false, Some(u)) => return Err(format!("usage {u} not supported")),
            (false, None) => Kind::Zoned,
        };
        let digits = int + frac;
        let len = match kind {
            Kind::Alpha | Kind::Zoned => digits,
            Kind::Packed => digits / 2 + 1,
            Kind::Binary => match digits {
                1..=4 => 2,
                5..=9 => 4,
                _ => 8,
            },
        };
        let occurs = up
            .iter()
            .position(|t| t == "OCCURS")
            .and_then(|o| up.get(o + 1))
            .and_then(|n| n.parse().ok())
            .unwrap_or(1);
        fields.push(Field {
            name: toks[1].to_string(),
            kind,
            digits,
            scale: frac,
            signed,
            len,
            occurs,
        });
    }
    if fields.is_empty() {
        return Err("no elementary fields with PIC found".into());
    }
    Ok(fields)
}

fn with_scale(neg: bool, digits: &str, scale: usize) -> String {
    let d = digits.trim_start_matches('0');
    let d = if d.len() <= scale {
        format!("{}{}", "0".repeat(scale + 1 - d.len()), d)
    } else {
        d.to_string()
    };
    let s = if scale > 0 {
        format!("{}.{}", &d[..d.len() - scale], &d[d.len() - scale..])
    } else {
        d
    };
    let zero = s.chars().all(|c| c == '0' || c == '.');
    if neg && !zero {
        format!("-{s}")
    } else {
        s
    }
}

pub fn decode_field(f: &Field, b: &[u8]) -> Result<Value, String> {
    match f.kind {
        Kind::Alpha => {
            let s: String = b
                .iter()
                .map(|&x| char::from_u32(CP037[x as usize] as u32).unwrap_or('?'))
                .collect();
            Ok(json!(s.trim_end()))
        }
        Kind::Zoned => {
            let mut digits = String::new();
            let mut neg = false;
            for (i, &x) in b.iter().enumerate() {
                let (zone, d) = (x >> 4, x & 0x0f);
                if d > 9 {
                    return Err(format!("{}: bad zoned digit 0x{x:02x}", f.name));
                }
                if i == b.len() - 1 && zone == 0x0D {
                    neg = true; // sign overpunch: C/F = +, D = -
                } else if zone != 0x0F && !(i == b.len() - 1 && zone == 0x0C) {
                    return Err(format!("{}: bad zone 0x{x:02x}", f.name));
                }
                digits.push((b'0' + d) as char);
            }
            Ok(json!(with_scale(neg, &digits, f.scale)))
        }
        Kind::Packed => {
            let mut digits = String::new();
            for (i, &x) in b.iter().enumerate() {
                let (hi, lo) = (x >> 4, x & 0x0f);
                if hi > 9 {
                    return Err(format!("{}: bad packed nibble", f.name));
                }
                digits.push((b'0' + hi) as char);
                if i < b.len() - 1 {
                    if lo > 9 {
                        return Err(format!("{}: bad packed nibble", f.name));
                    }
                    digits.push((b'0' + lo) as char);
                } else if !matches!(lo, 0x0C | 0x0D | 0x0F) {
                    return Err(format!("{}: bad packed sign nibble 0x{lo:x}", f.name));
                }
            }
            let neg = b.last().map(|x| x & 0x0f == 0x0D).unwrap_or(false);
            Ok(json!(with_scale(neg, &digits, f.scale)))
        }
        Kind::Binary => {
            let mut n: i64 = if f.signed && b[0] & 0x80 != 0 { -1 } else { 0 };
            for &x in b {
                n = (n << 8) | x as i64;
            }
            Ok(json!(with_scale(
                n < 0,
                &n.unsigned_abs().to_string(),
                f.scale
            )))
        }
    }
}

pub fn record_len(fields: &[Field]) -> usize {
    fields.iter().map(|f| f.len * f.occurs).sum()
}

pub fn decode_record(fields: &[Field], rec: &[u8]) -> Result<Value, String> {
    if rec.len() != record_len(fields) {
        return Err(format!(
            "record is {} bytes, copybook says {}",
            rec.len(),
            record_len(fields)
        ));
    }
    let mut out = Map::new();
    let mut pos = 0;
    for f in fields {
        let mut vals = Vec::new();
        for _ in 0..f.occurs {
            vals.push(decode_field(f, &rec[pos..pos + f.len])?);
            pos += f.len;
        }
        if f.name.eq_ignore_ascii_case("FILLER") {
            continue;
        }
        let key = f.name.to_ascii_lowercase().replace('-', "_");
        out.insert(
            key,
            if f.occurs == 1 {
                vals.remove(0)
            } else {
                Value::Array(vals)
            },
        );
    }
    Ok(Value::Object(out))
}

pub fn hex(s: &str) -> Result<Vec<u8>, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Err("hex must have an even number of digits".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "bad hex".to_string()))
        .collect()
}

pub fn decode_op(i: &Value) -> Result<Value, String> {
    let fields = parse(str_field(i, "copybook")?)?;
    let data = hex(str_field(i, "hex")?)?;
    let n = record_len(&fields);
    if data.len() % n != 0 {
        return Err(format!(
            "data is {} bytes, not a multiple of the {n}-byte record",
            data.len()
        ));
    }
    let mut records = Vec::new();
    let mut errors = Vec::new();
    for (idx, rec) in data.chunks(n).enumerate() {
        match decode_record(&fields, rec) {
            Ok(r) => records.push(r),
            Err(e) => errors.push(json!({"record": idx, "error": e})),
        }
    }
    let layout: Vec<Value> = fields
        .iter()
        .map(|f| json!({"name": f.name, "type": format!("{:?}", f.kind), "bytes": f.len, "scale": f.scale, "occurs": f.occurs}))
        .collect();
    let _ = field(i, "hex")?;
    Ok(json!({"record_length": n, "layout": layout, "records": records, "errors": errors}))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CB: &str = "       01 CUST-REC.
           05 CUST-ID     PIC 9(5).
           05 CUST-NAME   PIC X(6).
           05 BALANCE     PIC S9(5)V99 COMP-3.
           05 VISITS      PIC S9(4) COMP.
           05 FILLER      PIC X(1).
           05 DELTA       PIC S9(3)V9.";

    #[test]
    fn layout() {
        let f = parse(CB).unwrap();
        assert_eq!(f.len(), 6);
        assert_eq!(
            f[2],
            Field {
                name: "BALANCE".into(),
                kind: Kind::Packed,
                digits: 7,
                scale: 2,
                signed: true,
                len: 4,
                occurs: 1
            }
        );
        assert_eq!(record_len(&f), 5 + 6 + 4 + 2 + 1 + 4);
    }

    #[test]
    fn decodes_all_types() {
        let f = parse(CB).unwrap();
        // 00042 | "SMITH " | -12345.67 packed | 300 | filler | -012.5 zoned overpunch
        let rec = hex("F0F0F0F4F2 E2D4C9E3C840 1234567D 012C 40 F0F1F2D5").unwrap();
        let r = decode_record(&f, &rec).unwrap();
        assert_eq!(r["cust_id"], "42");
        assert_eq!(r["cust_name"], "SMITH");
        assert_eq!(r["balance"], "-12345.67");
        assert_eq!(r["visits"], "300");
        assert_eq!(r["delta"], "-12.5");
        assert!(r.get("filler").is_none());
    }

    #[test]
    fn negative_binary_and_small_scale() {
        let f = Field {
            name: "X".into(),
            kind: Kind::Binary,
            digits: 4,
            scale: 2,
            signed: true,
            len: 2,
            occurs: 1,
        };
        assert_eq!(decode_field(&f, &[0xFF, 0xFE]).unwrap(), "-0.02");
    }

    #[test]
    fn bad_sign_nibble_is_an_error() {
        let f = parse("05 A PIC 9(3) COMP-3.").unwrap();
        assert!(decode_field(&f[0], &[0x12, 0x3A]).is_err());
    }

    #[test]
    fn wrong_length_is_an_error() {
        let f = parse(CB).unwrap();
        assert!(decode_record(&f, &[0x40; 5]).is_err());
    }

    #[test]
    fn occurs() {
        let f = parse("05 Q PIC 9(2) OCCURS 3 TIMES.").unwrap();
        let r = decode_record(&f, &hex("F0F1F0F2F9F9").unwrap()).unwrap();
        assert_eq!(r["q"], json!(["1", "2", "99"]));
    }
}
