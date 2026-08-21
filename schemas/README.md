# `schemas/` — the public review contracts

These files are the pivot between every SpinZero review surface: the free
in-app deterministic BOM check, the paid review engine (`spinzero-private`),
the CLI, and any future CI integration. They are public and versioned; both
repos consume them rather than each defining their own shape.

| File | What it pins |
|---|---|
| `findings-1.0.json` | `findings.json` — the one output contract every review producer emits. |
| `rule-fixtures/` | Golden BOM fixtures + expected rule hits. Both rule runtimes (the Rust `bom-rules` crate here, the Python `run_bom.py` in the engine) must agree on them. |

## Consumers

- Rust: `src-tauri/crates/bom-rules` emits `findings.json` v1.0 with
  `pipeline: "bom-rules"` and `confidence: "Unvalidated"`.
- TypeScript: `src/lib/findings.ts` mirrors the schema for the UI.
- The app ingests **both** tiers through one path (`bomcheck.rs` →
  `reviews.rs`), matching on `fingerprint`.

## Changing the schema

`schema_version` is the compatibility gate. Additive, optional fields keep
`1.0`; anything a consumer could choke on gets a new version and a new file
(`findings-1.1.json`), with the old one kept for readers.

## Rule fixtures

Each fixture is a pair:

- `<name>.csv` — a BOM CSV, small and hand-written so every row exists to
  trigger (or deliberately not trigger) a specific rule.
- `<name>.expected.json` — `{ "profile": …, "expect_rules": [...],
  "forbid_rules": [...] }`, asserting which rule ids must and must not fire.

The Rust side runs them in `bom-rules`' test suite (`cargo test -p bom-rules`).
