# SpinZero

Local-first design review for KiCad PCB projects.

Point SpinZero at a project folder: it imports the design with its own
extraction pipeline, snapshots every change, and lets reviewers pin comments
directly to the net, footprint, or trace they're about. When the design moves,
affected comments are automatically flagged for re-check — no more stale PDF
markups. Everything stays on your machine: no cloud, no account, no upload.

![Cross-probing a net from schematic to PCB and anchoring a review comment](assets/spinzero-demo.gif)

*Search a net, cross-probe it from schematic to copper with one key, and pin
review comments directly to the objects they're about.*

**Download:** signed Windows installers are published on the
[Releases page](https://github.com/spinzero-pcb/spinzero/releases)
(the app auto-updates from there).

## How it reads your design files

SpinZero contains its **own, independently written** parser and extraction
pipeline (the `eda-parse-kicad` and `extract` crates under
[`src-tauri/crates/`](src-tauri/crates/)). It does not link, embed, or invoke
any KiCad code — it reads the KiCad file formats directly. Your KiCad source
files are stored untouched in the project's `raw/` store as the system of
record; everything SpinZero derives from them is regenerable.

## Building from source

Prerequisites: [Node.js](https://nodejs.org) (LTS), [Rust](https://rustup.rs)
(≥ 1.95), and on Windows the MSVC build tools (installed by the Rust setup).

```sh
npm install
npm run tauri dev     # hot-reloading dev build
npm run tauri build   # release build → src-tauri/target/release/spinzero.exe
```

Frontend-only checks: `npm run build` (tsc + vite) and `npx vitest run`.
Rust tests: `cargo test --lib` in `src-tauri/`.

A build from source has **telemetry disabled by default**: crash/usage
reporting is compiled in only when a Sentry DSN is provided at build time (see
[`.env.example`](.env.example)), and even official builds honor the in-app
consent toggle (File ▸ Privacy). Updater signing also lives in `.env` —
without it you get a normal unsigned dev build.

## Project layout

| Path | What |
|---|---|
| `src/` | Frontend — React + TypeScript, zustand stores, canvas renderers |
| `src-tauri/src/` | Rust core: raw store, watcher, extraction pipeline, reviews, SQLite index |
| `src-tauri/crates/eda-parse-kicad/` | Parser + in-memory model for KiCad file formats |
| `src-tauri/crates/extract/` | Projects a parsed design into the review bundle (also builds the standalone `pcb-extract` binary) |
| `scripts/` | Release cutting, raw-store recovery, index sanity checks |

## License

SpinZero is free software, licensed under the
[GNU General Public License v3.0 or later](LICENSE) — the same license family
as KiCad. Individual use is free forever; team sync licenses fund development.

## Contributing

Issues and pull requests are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with a minimal KiCad project
that reproduces the problem are especially valuable.
