//! Clean, chunked output for a vector store.
//!
//! Scanned PDFs come out of OCR with page headers and footers on every page, page numbers,
//! words split across lines ("inform-\nation") and ragged whitespace. Mainframe exports come
//! out as records. This module turns both into chunks ready to embed: one JSON object per
//! chunk with a stable id, the text, and metadata (source, page, character span, SHA-256).
//! Identical chunks across documents are stored once and point to every source.

use crate::{field, str_field};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn sha(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_page_number(l: &str) -> bool {
    // Only clear page markers: "Page 3", "Page 3 of 9", "Seite 3", "- 3 -", "3 of 9", "3/9".
    // A bare number ("1500") is data, not a page number.
    let t = l.trim().to_ascii_lowercase();
    let digits = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
    let of = |x: &str| {
        let x = x.trim();
        match x.split_once(" of ").or_else(|| x.split_once('/')) {
            Some((a, b)) => digits(a.trim()) && digits(b.trim()),
            None => false,
        }
    };
    if let Some(rest) = t.strip_prefix("page").or_else(|| t.strip_prefix("seite")) {
        let rest = rest.trim();
        return digits(rest) || of(rest);
    }
    if let Some(mid) = t.strip_prefix('-').and_then(|x| x.strip_suffix('-')) {
        return digits(mid.trim());
    }
    of(&t)
}

/// Normalize a line for boilerplate detection: digits do not count (page 3 vs page 4).
fn shape(l: &str) -> String {
    l.trim()
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .collect::<String>()
        .to_lowercase()
}

/// Split on form feeds, drop lines that repeat on most pages, drop page numbers,
/// join hyphenated line breaks, collapse whitespace. Returns cleaned pages and a report.
pub fn clean_pages(text: &str) -> (Vec<String>, Vec<String>) {
    let pages: Vec<&str> = text.split('\u{c}').collect();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    if pages.len() >= 3 {
        for p in &pages {
            let lines: BTreeSet<String> = p
                .lines()
                .map(shape)
                .filter(|s| s.len() >= 4)
                .collect();
            for s in lines {
                *seen.entry(s).or_default() += 1;
            }
        }
    }
    let boiler: BTreeSet<String> = seen
        .into_iter()
        .filter(|(_, n)| *n * 2 > pages.len())
        .map(|(s, _)| s)
        .collect();
    let mut removed: Vec<String> = boiler.iter().cloned().collect();
    let mut out = Vec::new();
    for p in pages {
        let mut buf = String::new();
        for l in p.lines() {
            if boiler.contains(&shape(l)) {
                continue;
            }
            if is_page_number(l) {
                if !removed.iter().any(|r| r == "<page numbers>") {
                    removed.push("<page numbers>".into());
                }
                continue;
            }
            let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
            if l.is_empty() {
                buf.push_str("\n\n");
                continue;
            }
            if buf.ends_with('-') && l.chars().next().is_some_and(|c| c.is_lowercase()) {
                buf.pop();
                buf.push_str(&l);
            } else {
                if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push(' ');
                }
                buf.push_str(&l);
            }
        }
        let mut s = buf.trim().to_string();
        while s.contains("\n\n\n") {
            s = s.replace("\n\n\n", "\n\n");
        }
        out.push(s);
    }
    (out, removed)
}

/// Split text into chunks of at most `max` chars, breaking at paragraph, then sentence,
/// then word boundaries, with `overlap` chars carried into the next chunk.
pub fn split(text: &str, max: usize, overlap: usize) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;
    while start < b.len() {
        let hard = (start + max).min(b.len());
        let mut end = hard;
        if hard < b.len() {
            let win = &text[start..floor(text, hard)];
            end = win
                .rfind("\n\n")
                .filter(|&i| i > max / 3)
                .or_else(|| win.rfind(". ").filter(|&i| i > max / 3).map(|i| i + 1))
                .or_else(|| win.rfind(' ').filter(|&i| i > 0))
                .map(|i| start + i)
                .unwrap_or(floor(text, hard));
        }
        let end = floor(text, end).max(start + 1).min(b.len());
        let end = ceil(text, end);
        spans.push((start, end));
        if end >= b.len() {
            break;
        }
        let mut next = end.saturating_sub(overlap).max(start + 1);
        next = ceil(text, next);
        // Start the overlap on a word boundary.
        if overlap > 0 {
            if let Some(i) = text[next..end].find(' ') {
                next += i + 1;
            }
        }
        start = next.max(start + 1);
        while start < b.len() && b[start].is_ascii_whitespace() {
            start += 1;
        }
    }
    spans
}
fn floor(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
fn ceil(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// A decoded record becomes "field: value" lines so it embeds as text.
fn record_text(r: &Value) -> String {
    match r.as_object() {
        Some(o) => o
            .iter()
            .map(|(k, v)| format!("{k}: {}", v.as_str().map(String::from).unwrap_or(v.to_string())))
            .collect::<Vec<_>>()
            .join("\n"),
        None => r.to_string(),
    }
}

pub fn chunk_op(i: &Value) -> Result<Value, String> {
    let max = i["max_chars"].as_u64().unwrap_or(800) as usize;
    let overlap = i["overlap_chars"].as_u64().unwrap_or(100) as usize;
    if !(50..=20_000).contains(&max) {
        return Err("max_chars must be 50..=20000".into());
    }
    if overlap * 2 >= max {
        return Err("overlap_chars must be under half of max_chars".into());
    }
    let min = i["min_chars"].as_u64().unwrap_or(1) as usize;
    let mut dropped_short = 0;
    let mut chunks: Vec<Value> = Vec::new();
    let mut by_hash: BTreeMap<String, usize> = BTreeMap::new();
    let mut report = Vec::new();
    let mut dupes = 0;
    for d in field(i, "documents")?.as_array().ok_or("`documents` must be a list")? {
        let id = str_field(d, "id")?;
        let source = d["source"].as_str().unwrap_or(id);
        let pages: Vec<String> = if let Some(t) = d["text"].as_str() {
            let (p, removed) = clean_pages(t);
            report.push(json!({"id": id, "pages": p.len(), "removed_boilerplate": removed}));
            p
        } else if let Some(rs) = d["records"].as_array() {
            report.push(json!({"id": id, "records": rs.len()}));
            rs.iter().map(record_text).collect()
        } else {
            return Err(format!("document {id} needs `text` or `records`"));
        };
        let mut n = 0;
        for (pi, page) in pages.iter().enumerate() {
            // Records stay whole when they fit; a long record is split like text.
            let spans = if d["records"].is_array() && page.len() <= max {
                vec![(0, page.len())]
            } else {
                split(page, max, overlap)
            };
            for (s, e) in spans {
                let text = page[s..e].trim();
                if text.chars().count() < min {
                    dropped_short += 1;
                    continue;
                }
                let h = sha(text);
                if let Some(&k) = by_hash.get(&h) {
                    dupes += 1;
                    if let Some(a) = chunks[k]["metadata"]["also_in"].as_array_mut() {
                        a.push(json!(id));
                    }
                    continue;
                }
                n += 1;
                let unit = if d["records"].is_array() { "record" } else { "page" };
                by_hash.insert(h.clone(), chunks.len());
                chunks.push(json!({
                    "id": format!("{id}#{n}"),
                    "text": text,
                    "metadata": {"source": source, unit: pi + 1, "char_start": s, "char_end": e,
                                  "sha256": h, "also_in": []}
                }));
            }
        }
    }
    let jsonl = chunks.iter().map(|c| c.to_string()).collect::<Vec<_>>().join("\n");
    Ok(json!({"chunks": chunks, "count": chunks.len(), "duplicates_skipped": dupes, "dropped_short": dropped_short,
              "documents": report, "jsonl": jsonl}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_headers_footers_page_numbers_and_hyphens() {
        let t = "ACME BANK CONFIDENTIAL\nLoan file 12\nThe borrower was inform-\ned of the terms.\nPage 1 of 3\u{c}ACME BANK CONFIDENTIAL\nRate is fixed.\nPage 2 of 3\u{c}ACME BANK CONFIDENTIAL\nSigned.\nPage 3 of 3";
        let (p, removed) = clean_pages(t);
        assert_eq!(p[0], "Loan file 12 The borrower was informed of the terms.");
        assert_eq!(p[1], "Rate is fixed.");
        assert!(removed.iter().any(|r| r.contains("acme bank")));
    }

    #[test]
    fn chunks_respect_max_and_cover_text() {
        let text = "Alpha beta gamma. ".repeat(100);
        let spans = split(&text, 200, 40);
        assert!(spans.iter().all(|(s, e)| e - s <= 200));
        assert_eq!(spans[0].0, 0);
        assert_eq!(spans.last().unwrap().1, text.len());
        for w in spans.windows(2) {
            assert!(w[1].0 < w[0].1, "overlap expected");
        }
    }

    #[test]
    fn multibyte_text_does_not_panic() {
        let text = "Größe ünd Maße — 東京 ".repeat(80);
        for (s, e) in split(&text, 64, 10) {
            let _ = &text[s..e];
        }
    }

    #[test]
    fn duplicate_chunks_stored_once_and_records_supported() {
        let r = chunk_op(&json!({"max_chars": 200, "overlap_chars": 20, "documents": [
            {"id": "a", "text": "Same paragraph about the escrow terms and rates."},
            {"id": "b", "text": "Same paragraph about the escrow terms and rates."},
            {"id": "m", "records": [{"CUST-ID": "010042", "CUST-NAME": "ACME FOODS"}]}]}))
        .unwrap();
        assert_eq!(r["count"], 2);
        assert_eq!(r["duplicates_skipped"], 1);
        assert_eq!(r["chunks"][0]["metadata"]["also_in"][0], "b");
        assert_eq!(r["chunks"][1]["text"], "CUST-ID: 010042\nCUST-NAME: ACME FOODS");
    }

    #[test]
    fn bad_sizes_refused() {
        assert!(chunk_op(&json!({"max_chars": 100, "overlap_chars": 60, "documents": []})).is_err());
        assert!(chunk_op(&json!({"max_chars": 10, "documents": []})).is_err());
    }

    #[test]
    fn bare_numbers_kept_and_long_records_split() {
        assert!(!is_page_number("1500"));
        assert!(!is_page_number("of"));
        assert!(is_page_number("Page 2 of 9"));
        assert!(is_page_number("- 4 -"));
        assert!(is_page_number("3/9"));
        let r = chunk_op(&json!({"documents": [{"id": "t", "text": "Invoice\n1500\nUSD total owed"}]})).unwrap();
        assert!(r["chunks"][0]["text"].as_str().unwrap().contains("1500"));
        let r = chunk_op(&json!({"documents": [{"id": "r", "records": [{"name": "x ".repeat(1500)}]}]})).unwrap();
        assert!(r["chunks"].as_array().unwrap().iter().all(|c| c["text"].as_str().unwrap().len() <= 800));
        assert!(r["count"].as_u64().unwrap() > 1);
    }
}
