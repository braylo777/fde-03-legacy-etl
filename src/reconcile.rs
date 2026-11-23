//! Reconcile two datasets (e.g. mainframe ledger vs. new system) on a key.
//! Amounts are compared in integer cents to avoid float drift.

use crate::{field, str_field};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn cents(v: &Value) -> Option<i64> {
    let s = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        _ => return None,
    };
    let neg = s.starts_with('-');
    let s = s.trim_start_matches('-');
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > 2 && frac[2..].chars().any(|c| c != '0') {
        return None; // sub-cent precision: refuse rather than round silently
    }
    let frac2 = format!("{:0<2}", &frac[..frac.len().min(2)]);
    let c = int.parse::<i64>().ok()? * 100 + frac2.parse::<i64>().ok()?;
    Some(if neg { -c } else { c })
}

fn fmt(c: i64) -> String {
    format!(
        "{}{}.{:02}",
        if c < 0 { "-" } else { "" },
        c.abs() / 100,
        c.abs() % 100
    )
}

pub fn reconcile_op(i: &Value) -> Result<Value, String> {
    let key = str_field(i, "key")?;
    let amt = str_field(i, "amount")?;
    let tol = i["tolerance_cents"].as_i64().unwrap_or(0);
    let index = |k: &str| -> Result<(BTreeMap<String, Value>, Vec<String>), String> {
        let mut m = BTreeMap::new();
        let mut dup = Vec::new();
        for r in field(i, k)?
            .as_array()
            .ok_or(format!("`{k}` must be a list"))?
        {
            let id = match &r[key] {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(format!("a `{k}` row has no `{key}`")),
            };
            if m.insert(id.clone(), r.clone()).is_some() {
                dup.push(id);
            }
        }
        Ok((m, dup))
    };
    let (left, dl) = index("left")?;
    let (right, dr) = index("right")?;
    let (mut matched, mut breaks, mut only_l, mut only_r) = (0, vec![], vec![], vec![]);
    let (mut tl, mut tr) = (0i64, 0i64);
    for (k, l) in &left {
        let lc = cents(&l[amt]).ok_or(format!("left {k}: bad amount {}", l[amt]))?;
        tl += lc;
        match right.get(k) {
            None => only_l.push(json!({"key": k, "amount": fmt(lc)})),
            Some(r) => {
                let rc = cents(&r[amt]).ok_or(format!("right {k}: bad amount {}", r[amt]))?;
                if (lc - rc).abs() <= tol {
                    matched += 1;
                } else {
                    breaks.push(
                        json!({"key": k, "left": fmt(lc), "right": fmt(rc), "diff": fmt(lc - rc)}),
                    );
                }
            }
        }
    }
    for (k, r) in &right {
        let rc = cents(&r[amt]).ok_or(format!("right {k}: bad amount {}", r[amt]))?;
        tr += rc;
        if !left.contains_key(k) {
            only_r.push(json!({"key": k, "amount": fmt(rc)}));
        }
    }
    let clean = breaks.is_empty()
        && only_l.is_empty()
        && only_r.is_empty()
        && dl.is_empty()
        && dr.is_empty();
    Ok(json!({
        "reconciled": clean, "matched": matched, "amount_breaks": breaks,
        "only_in_left": only_l, "only_in_right": only_r,
        "duplicate_keys": {"left": dl, "right": dr},
        "totals": {"left": fmt(tl), "right": fmt(tr), "diff": fmt(tl - tr)}
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cents_parsing() {
        assert_eq!(cents(&json!("12.5")), Some(1250));
        assert_eq!(cents(&json!("-0.07")), Some(-7));
        assert_eq!(cents(&json!(3)), Some(300));
        assert_eq!(cents(&json!("1.005")), None);
        assert_eq!(cents(&json!("1.000")), Some(100));
    }
    #[test]
    fn finds_all_break_kinds() {
        let r = reconcile_op(&json!({"key": "id", "amount": "amt", "tolerance_cents": 1,
            "left": [{"id": "A", "amt": "10.00"}, {"id": "B", "amt": "5.00"}, {"id": "C", "amt": "1.00"}, {"id": "E", "amt": "2.00"}],
            "right": [{"id": "A", "amt": "10.01"}, {"id": "B", "amt": "6.00"}, {"id": "D", "amt": "9.00"}, {"id": "E", "amt": 2}]}))
        .unwrap();
        assert_eq!(r["matched"], 2);
        assert_eq!(r["amount_breaks"][0]["diff"], "-1.00");
        assert_eq!(r["only_in_left"][0]["key"], "C");
        assert_eq!(r["only_in_right"][0]["key"], "D");
        assert_eq!(r["reconciled"], false);
    }
}
