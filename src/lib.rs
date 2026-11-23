//! Stage 3 — Legacy Data Extraction & ETL.
//!
//! * `copybook` — parse a COBOL copybook and decode fixed-width EBCDIC mainframe records
//!   (PIC X, zoned decimal with sign overpunch, COMP-3 packed decimal, COMP binary).
//! * `csvclean` — normalize messy CSV exports: delimiter sniffing, quotes, BOM, header
//!   names, dates, money formats (US / EU / accounting negatives), null markers, duplicates.
//! * `reconcile` — match two datasets on a key and report matches, amount breaks and orphans.
//! * `extract` — pull labelled fields (invoice no, dates, totals) out of OCR / PDF text.

mod api;
pub mod copybook;
pub mod cp037;
pub mod csvclean;
pub mod extract;
pub mod reconcile;

pub use api::dispatch;
use serde_json::Value;

pub const OPS: &[&str] = &["decode_mainframe", "normalize_csv", "reconcile", "extract_fields"];

pub(crate) fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    v.get(k).ok_or_else(|| format!("missing field `{k}`"))
}
pub(crate) fn str_field<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    field(v, k)?.as_str().ok_or_else(|| format!("`{k}` must be a string"))
}

pub fn handle(op: &str, input: &Value) -> Result<Value, String> {
    match op {
        "decode_mainframe" => copybook::decode_op(input),
        "normalize_csv" => csvclean::normalize_op(input),
        "reconcile" => reconcile::reconcile_op(input),
        "extract_fields" => extract::extract_op(input),
        _ => Err(format!("unknown op `{op}`; known: {}", OPS.join(", "))),
    }
}
