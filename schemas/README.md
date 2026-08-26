# `schemas/` — the public review contracts

These files are the pivot between every SpinZero review surface: the free
in-app deterministic BOM check, the paid review engine (`spinzero-private`),
the CLI, and any future CI integration. They are public and versioned; both
repos consume them rather than each defining their own shape.

| File | What it pins |
|---|---|
| `findings-1.1.json` | `findings.json` — the one output contract every review producer emits. |
| `findings-1.0.json` | The retired five-level severity / four-level confidence version. No producer emits it; kept so a document already sitting in a project's review inbox still reads. |
| `bundle-1.0.json` | The review bundle — every file a detailed review may upload, and by omission everything it may not. |
| `rule-fixtures/` | Golden BOM fixtures + expected rule hits, pinning the Rust `bom-rules` crate. |

## Consumers

- Rust: `src-tauri/crates/bom-rules` emits `findings.json` v1.1 with
  `pipeline: "bom-rules"` and `confidence: "Unvalidated"`.
- TypeScript: `src/lib/findings.ts` mirrors the schema for the UI.
- The app ingests **both** tiers through one path (`bomcheck.rs` →
  `reviews.rs`), matching on `fingerprint`.
- The paid engine (`spinzero-private/engine`) mirrors the findings types in
  `src/contracts.ts` and pins them against this file in its own test suite. Its
  `bom-detailed` stage 2 shells out to the `bom-rules` binary in this repo, so both
  tiers produce identical fingerprints and a paid finding refines the free-tier
  comment in place instead of filing a second one.
- The bundle spec is enforced on **both** sides: the app builds exactly this file
  set (`src-tauri/src/reviewbundle.rs`) and shows it to the user before upload; the
  service rejects any file the spec does not name.

## Changing the schema

`schema_version` is the compatibility gate. Additive, optional fields keep
the version; anything a consumer could choke on gets a new version and a new file
(`findings-1.2.json`), with the old one kept for readers. That is why 1.1 exists:
collapsing severity to two levels and confidence to three is a value a 1.0 reader
would not recognise. The app reads both and normalises on ingest
(`comment_severity` in `bomcheck.rs`).

## Rule fixtures

Each fixture is a pair:

- `<name>.csv` — a BOM CSV, small and hand-written so every row exists to
  trigger (or deliberately not trigger) a specific rule.
- `<name>.expected.json` — `{ "profile": …, "expect_rules": [...],
  "forbid_rules": [...] }`, asserting which rule ids must and must not fire.

The Rust side runs them in `bom-rules`' test suite (`cargo test -p bom-rules`).
