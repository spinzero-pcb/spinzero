# `schemas/` — the public review contracts

These files are the pivot between every SpinZero review surface: the free
in-app deterministic BOM check, the paid review engine (`spinzero-private`),
the CLI, and any future CI integration. They are public and versioned; both
repos consume them rather than each defining their own shape.

| File | What it pins |
|---|---|
| `findings-1.3.json` | `findings.json` — the one output contract every review producer emits. |
| `findings-1.2.json` | The version before the `Not verified` severity. Kept for readers. |
| `findings-1.1.json` | The version before `column_mapping` and `execution`. Kept for readers. |
| `findings-1.0.json` | The retired five-level severity / four-level confidence version. No producer emits it; kept so a document already sitting in a project's review inbox still reads. |
| `bundle-1.0.json` | The review bundle — every file a detailed review may upload, and by omission everything it may not. |
| `mcp-tools-1.0.json` | The MCP harness's tool surface: what a customer's own agent may call, in what order, and how a refusal is phrased. |
| `mcp-setup-1.0.json` | The two files behind `SpinZero --setup <dir>`: the review setup an MCP review server asks the user to confirm, and the answer the app writes back. |

## Consumers

- Coverage gaps are findings. A part the review could not check emits a finding at
  severity `Not verified`, anchored to its BOM row, alongside the `bom_audit` prose
  that says the same thing in the pipeline's terms. Before 1.3 they were prose only,
  so nothing downstream could count them, place them on a row, or record that somebody
  had dealt with one. A `Not verified` finding is the ABSENCE of a claim: it must not
  be presented as a defect found on the board.
- The free tier's rule pack, `bom-rules`, emits `findings.json` v1.1 with
  `pipeline: "bom-rules"` and `confidence: "Unvalidated"`. It is a separate program
  the installer puts beside the app, not a crate in this repository; `bomrules.rs`
  says where the app looks for it.
- Rust: `src-tauri/src/findings.rs` is this schema as the app reads it — from the rule
  pack, from the paid service, and from a project's review inbox.
- TypeScript: `src/lib/findings.ts` mirrors the schema for the UI.
- The app ingests **both** tiers through one path (`bomcheck.rs` →
  `reviews.rs`), matching on `fingerprint`.
- The paid engine (`spinzero-private/engine`) mirrors the findings types in
  `src/contracts.ts` and pins them against this file in its own test suite. Its
  `bom-detailed` stage 2 shells out to the same `bom-rules` binary, so both
  tiers produce identical fingerprints and a paid finding refines the free-tier
  comment in place instead of filing a second one.
- The bundle spec is enforced on **both** sides: the app builds exactly this file
  set (`src-tauri/src/reviewbundle.rs`) and shows it to the user before upload; the
  service rejects any file the spec does not name.
- `mcp-tools-1.0.json` is **generated** from the server's own tool definitions
  (`spinzero-private/mcp/scripts/emit-schema.mjs`), and its CI check fails if the two
  drift. A hand-transcribed copy of a tool description is a second description to keep
  in step with the first, and the stale one is always the one strangers read.

## The MCP tool contract, and why a closed server publishes one

The SpinZero MCP server is not open source. Its tool surface is, because the two
questions a reader has about a harness are answerable without its source: *what will
it let a model do*, and *what will it refuse*. `mcp-tools-1.0.json` answers both.

The shape is deliberately coarse. There is no tool for the deterministic layer —
bundle validation, distributor lookup, datasheet collection, the rule pack, the
BOM-versus-distributor cross-check, document assembly. Those are not refused; they are
**absent**, which is what makes them impossible to skip or reimplement rather than
merely discouraged. They run inside `spinzero_start_review`, before the client sees
anything.

The contract version moves when a tool's name, arguments or refusal semantics change —
never when the review's quality does.

## Changing the schema

`schema_version` is the compatibility gate. Additive, optional fields keep
the version; anything a consumer could choke on gets a new version and a new file
(`findings-1.3.json`), with the old one kept for readers. That is why 1.1 exists:
collapsing severity to two levels and confidence to three is a value a 1.0 reader
would not recognise. It is also why 1.3 exists: `Not verified` is a third severity,
and a 1.2 reader handed one would either drop the finding or file it as a defect. The
app reads every version and normalises on ingest (`comment_severity` in
`bomcheck.rs`).
