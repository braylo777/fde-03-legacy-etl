# fde-03-legacy-etl

**Forward Deployed Engineering — Stage 3 of 12: Legacy Data Extraction & ETL**

Turn mainframe records, messy CSV exports and document text into clean, typed, reconciled data, and flag every value it could not trust.

**Also built as Project 3 of 12, "Unstructured Legacy Ingestor",** from the companion [12 FDE projects post](https://x.com/suraj_sharma14/status/2125197762752806998).

**Live demo:** https://fde-03-legacy-etl.braylo777.workers.dev — the Rust core runs in your browser as WebAssembly. No server, no tracking.

## Why this matters

Most enterprise AI projects stall on data, not models. The customer's truth lives in a COBOL system that exports EBCDIC files, in CSVs from five ERPs with five date formats, and in scanned PDFs. Before an agent can answer a question about an invoice, someone has to extract, normalize and reconcile that data, and prove the totals still match. This repo does those steps with exact decimal handling and an issue list instead of silent guesses.

## What it does

- COBOL copybook parser: PIC X / 9 / S9 V9, COMP-3 packed decimal, COMP binary, zoned sign overpunch, OCCURS, FILLER, sequence columns
- EBCDIC (code page 037) to Unicode, full 256-byte table
- CSV cleanup: delimiter sniffing, RFC 4180 quotes, BOM, header names, US/EU dates, US/EU/accounting money, null markers, duplicate rows, per-row issue list
- Reconciliation in integer cents: matches, amount breaks with tolerance, orphans both sides, duplicate keys, control totals
- Labelled field extraction from OCR / PDF text with source line for each value and a missing-required list
- Vector-store chunker: strips OCR page headers, footers and page numbers, joins hyphen breaks, splits at paragraph / sentence / word, overlap, SHA-256 dedupe, JSONL with source and page metadata

## What it does not do (yet)

- No OCR engine or PDF parser here: feed it text from Tesseract, AWS Textract or pdftotext
- No live mainframe connector (DB2 / IMS / VSAM over the network); it decodes the exported files
- No Airflow or dbt project: each op is a pure step you can call from an Airflow task or a dbt Python model
- Other EBCDIC code pages (e.g. 500, 1047) and COMP-1/COMP-2 floats
- No embedding call or vector DB write: the JSONL output is what you send to your embedder and store

## Operations

Every operation takes JSON in and gives JSON out. The CLI, the tests and the demo page all use the same entry point.

| Operation | What it does |
|---|---|
| `decode_mainframe` | Three 32-byte EBCDIC records from a VSAM export, decoded with the COBOL copybook. Balances are COMP-3 packed decimals (one is negative). |
| `normalize_csv` | Semicolon delimiter, BOM, quoted delimiter, EU amounts, mixed date formats, a duplicate row and two bad values. Bad values stay as text and land in the issue list. |
| `reconcile` | Mainframe ledger vs. new ERP. One rounding difference inside the 1-cent tolerance, one real break, one orphan each side. |
| `extract_fields` | Text from an OCR pass over a scanned invoice. Each value has its source line. The PO number is required but absent, so it is reported, not guessed. |
| `chunk_for_vectors` | Scanned loan file (OCR text, 3 pages) plus decoded mainframe records become clean chunks: headers, footers and page numbers removed, hyphen breaks joined, one JSONL line per chunk with source and page metadata. |

## Run it

You need Rust (stable). No Node, no npm.

```bash
git clone https://github.com/braylo777/fde-03-legacy-etl
cd fde-03-legacy-etl
cargo test
cargo run -- ops
```

Try the first demo example from the command line:

```bash
python3 -c 'import json;print(json.dumps(json.load(open("web/examples.json"))["ops"][0]["input"]))' | cargo run -q -- decode_mainframe
```

## Build and deploy the demo

```bash
./build.sh    # tests, then compiles the core to web/app.wasm
./deploy.sh   # serves web/ as a Cloudflare Workers static-assets site (needs a Cloudflare login)
```

`web/` is a plain static site (`index.html`, `app.js`, `app.wasm`). It has no build step, so any static host works.

## Layout

```
src/lib.rs        operation list and router
src/api.rs        JSON entry point + the 3 WASM exports (alloc, dealloc, run)
src/main.rs       CLI
src/*.rs          one module per capability, each with unit tests
tests/            integration tests (every demo example must pass)
web/              demo page; examples.json drives both the page and this README
```

## Design choices

- **Rust, few dependencies.** Only `serde`, `serde_json`, `sha2` and `hmac`, pinned to exact versions with a committed `Cargo.lock`.
- **Deterministic.** Time, randomness and keys are inputs, so every result can be replayed in a test or an audit.
- **Fail closed.** Unknown input, missing fields or an unmatched policy gives an error or a denial, never a silent pass.

## Part of a series

This is one of 12 repos that work through a public Forward Deployed Engineering roadmap ([12 stages](https://x.com/suraj_sharma14/status/2124827808535048566), [12 projects](https://x.com/suraj_sharma14/status/2125197762752806998)). All 12: [github.com/braylo777?q=fde-](https://github.com/braylo777?tab=repositories&q=fde-).

## License

MIT — see [LICENSE](LICENSE). Built by [Braylo SI](https://braylo.si).
