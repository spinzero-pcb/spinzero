# Contributing to SpinZero

Thanks for your interest! SpinZero is young and moving fast, so a quick note
before you invest significant time.

## Bug reports

The most valuable report includes:

- a **minimal KiCad project** that reproduces the issue — strip it
  down as far as you can;
- what you expected vs. what happened;
- the log file: `%LOCALAPPDATA%\com.spinzero.app\logs\spinzero.log` (Windows).

Never attach a proprietary design you don't have the right to share.

## Pull requests

- **Open an issue first** for anything beyond a small fix, so we can agree on
  the approach before you write code.
- Match the surrounding style; there is no separate style guide — the code is
  the guide. All UI colors come from `src/styles/tokens.css` tokens.
- Keep `src/lib/design.ts` in sync with `src-tauri/src/design.rs` if you touch
  the design model.
- Verify before submitting: `npm run build` (tsc + vite), `npx vitest run`,
  and `cargo test --lib` in `src-tauri/`.

## Project layout

| Path | What |
|---|---|
| `src/` | Frontend — React + TypeScript, zustand stores, canvas renderers |
| `src-tauri/src/` | Rust core: raw store, watcher, extraction pipeline, reviews, SQLite index |
| `src-tauri/crates/eda-parse-kicad/` | Parser + in-memory model for KiCad file formats |
| `src-tauri/crates/extract/` | Projects a parsed design into the review bundle (also builds the standalone `pcb-extract` binary) |
| `scripts/` | Release cutting, raw-store recovery, index sanity checks |

## File-format parsing

The `eda-parse-kicad` crate is written from the file formats themselves
(published S-expression grammars and observed files). To keep the provenance
of this codebase unambiguous, **do not copy code, comments, or identifier
naming from KiCad or any other EDA tool's source** into a contribution —
parser changes must be derivable from the format, not from another
implementation.

## Licence

SpinZero is proprietary. The source is published so you can read it, audit it
and report problems — not so it can be redistributed or reused. `LICENSE` has
the terms.

By contributing you assign us the copyright in your contribution, and you
confirm it is your own work. That is what lets the whole program ship under one
set of terms.
