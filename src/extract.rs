//! Labelled-field extraction from OCR / PDF text (invoices, statements, forms).
//!
//! Each target field has label synonyms. A value is taken from the same line after the
//! label, or from the next non-empty line (common in PDF table layouts). Values are then
//! typed (date / money / text) and every result carries the line it came from, so a human
//! can check it. Missing required fields are reported, never guessed.

use crate::csvclean::{date, money};
use crate::field;
use serde_json::{json, Value};

pub fn extract(text: &str, spec: &[Value], day_first: bool) -> Value {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = serde_json::Map::new();
    let mut missing = Vec::new();
    for f in spec {
        let name = f["name"].as_str().unwrap_or("?");
        let kind = f["type"].as_str().unwrap_or("text");
        let labels: Vec<String> = f["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(|s| s.to_ascii_lowercase()))
            .collect();
        let mut hit = None;
        'outer: for (ln, line) in lines.iter().enumerate() {
            let low = line.to_ascii_lowercase();
            for l in &labels {
                let Some(pos) = low.find(l.as_str()) else {
                    continue;
                };
                let rest = line[pos + l.len()..]
                    .trim_start_matches([':', '#', '.', ' ', '\t', '-'])
                    .trim();
                let (raw, src) = if !rest.is_empty() {
                    (rest.to_string(), ln)
                } else if let Some((n, next)) = lines
                    .iter()
                    .enumerate()
                    .skip(ln + 1)
                    .find(|(_, s)| !s.trim().is_empty())
                {
                    (next.trim().to_string(), n)
                } else {
                    continue;
                };
                let typed = match kind {
                    "date" => date(&raw, day_first).map(Value::from),
                    "money" => {
                        let token = raw
                            .split_whitespace()
                            .find(|t| t.chars().any(|c| c.is_ascii_digit()))
                            .unwrap_or("");
                        money(token).map(Value::from)
                    }
                    _ => Some(Value::from(raw.split("  ").next().unwrap_or("").trim())),
                };
                if let Some(v) = typed {
                    hit = Some(json!({"value": v, "line": src + 1, "source": lines[src].trim()}));
                    break 'outer;
                }
            }
        }
        match hit {
            Some(h) => {
                found.insert(name.to_string(), h);
            }
            None if f["required"] == true => missing.push(name.to_string()),
            None => {}
        }
    }
    json!({"fields": found, "missing_required": missing, "complete": missing.is_empty()})
}

pub fn extract_op(i: &Value) -> Result<Value, String> {
    let text = field(i, "text")?
        .as_str()
        .ok_or("`text` must be a string")?;
    let spec = field(i, "fields")?
        .as_array()
        .ok_or("`fields` must be a list")?;
    Ok(extract(
        text,
        spec,
        i["day_first"].as_bool().unwrap_or(false),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_line_and_next_line() {
        let text = "ACME SUPPLY CO\nInvoice No: INV-2024-0042\nDate of issue\n  14.03.2024\nTotal due   EUR 1.250,00\n";
        let spec = json!([
            {"name": "invoice_no", "labels": ["invoice no", "invoice #"], "required": true},
            {"name": "issued", "labels": ["date of issue"], "type": "date"},
            {"name": "total", "labels": ["total due", "amount due"], "type": "money", "required": true},
            {"name": "po", "labels": ["po number"], "required": true}
        ]);
        let r = extract(text, spec.as_array().unwrap(), true);
        assert_eq!(r["fields"]["invoice_no"]["value"], "INV-2024-0042");
        assert_eq!(r["fields"]["issued"]["value"], "2024-03-14");
        assert_eq!(r["fields"]["issued"]["line"], 4);
        assert_eq!(r["fields"]["total"]["value"], "1250.00");
        assert_eq!(r["missing_required"], json!(["po"]));
    }
}
