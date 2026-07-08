#!/usr/bin/env python3
"""Cut a SpinZero release: stamp the version, build+sign, assemble latest.json, commit+tag.

Usage:
    python scripts/release.py 0.0.3 --notes "What changed, shown in the in-app update banner"

Steps (publish to GitHub is deliberately NOT automated -- the command is printed at the end):
  1. Preflight  - semver format, greater than current version, clean git tree
  2. Stamp      - src-tauri/Cargo.toml (single source of truth; tauri.conf.json inherits it)
                  + package.json/package-lock.json via `npm version`
  3. Build      - `npm run tauri build` (loads .env, signs the updater artifact)
  4. Manifest   - latest.json next to the installer, signature read from the .sig
  5. Commit+tag - version-bump commit and `v<version>` tag in this repo (not pushed)
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO / "src-tauri" / "Cargo.toml"
BUNDLE_DIR = REPO / "src-tauri" / "target" / "release" / "bundle" / "nsis"
RELEASES_REPO = "spinzero-pcb/spinzero"

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


def die(msg: str) -> "NoReturn":
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    print(f"$ {' '.join(cmd)}")
    # shell=True so npm/git resolve through PATH shims on Windows
    return subprocess.run(subprocess.list2cmdline(cmd), shell=True, cwd=REPO, **kw)


def capture(cmd: list[str]) -> str:
    r = run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        die(f"`{' '.join(cmd)}` failed:\n{r.stderr.strip()}")
    return r.stdout.strip()


def current_version() -> str:
    m = re.search(r'^version\s*=\s*"([^"]+)"', CARGO_TOML.read_text(encoding="utf-8"), re.M)
    if not m:
        die(f"could not find a version line in {CARGO_TOML}")
    return m.group(1)


def semver_key(v: str) -> tuple[int, int, int]:
    m = SEMVER.match(v)
    if not m:
        die(f"version {v!r} is not plain semver (expected X.Y.Z)")
    return tuple(int(p) for p in m.groups())


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("version", help="new version, plain semver X.Y.Z (no v prefix)")
    ap.add_argument("--notes", required=True,
                    help="release notes; shown to users in the in-app update banner")
    args = ap.parse_args()
    version: str = args.version

    # -- 1. preflight ---------------------------------------------------------
    old = current_version()
    if semver_key(version) <= semver_key(old):
        die(f"new version {version} must be greater than current {old}")
    dirty = capture(["git", "status", "--porcelain"])
    if dirty:
        die("git tree is dirty -- commit or stash first:\n" + dirty)

    installer = BUNDLE_DIR / f"SpinZero_{version}_x64-setup.exe"
    sig_file = installer.with_name(installer.name + ".sig")

    # -- 2. stamp -------------------------------------------------------------
    print(f"\n== stamping {old} -> {version}")
    text = CARGO_TOML.read_text(encoding="utf-8")
    text, n = re.subn(r'^version\s*=\s*"[^"]+"', f'version = "{version}"', text, count=1, flags=re.M)
    if n != 1:
        die(f"failed to rewrite version in {CARGO_TOML}")
    CARGO_TOML.write_text(text, encoding="utf-8")
    if run(["npm", "version", version, "--no-git-tag-version"]).returncode != 0:
        die("npm version failed")

    # -- 3. build (signs via .env; Cargo.lock picks up the new version) --------
    print(f"\n== building {version}")
    if run(["npm", "run", "tauri", "build"]).returncode != 0:
        die("build failed -- the version bump is left uncommitted; "
            "`git checkout -- .` to undo it")

    if not installer.exists():
        die(f"expected installer not found: {installer}")
    if not sig_file.exists():
        die(f"signature not found: {sig_file} -- was the build signed? (check .env)")

    # -- 4. latest.json --------------------------------------------------------
    manifest = {
        "version": version,
        "notes": args.notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc)
                    .isoformat(timespec="seconds").replace("+00:00", "Z"),
        "platforms": {
            "windows-x86_64": {
                "signature": sig_file.read_text(encoding="utf-8").strip(),
                "url": f"https://github.com/{RELEASES_REPO}/releases/download/"
                       f"v{version}/{installer.name}",
            }
        },
    }
    latest = BUNDLE_DIR / "latest.json"
    latest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {latest}")

    # -- 5. commit + tag -------------------------------------------------------
    print(f"\n== committing and tagging v{version}")
    if run(["git", "add", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock",
            "package.json", "package-lock.json"]).returncode != 0:
        die("git add failed")
    if run(["git", "commit", "-m", f"release: v{version}"]).returncode != 0:
        die("git commit failed")
    if run(["git", "tag", f"v{version}"]).returncode != 0:
        die("git tag failed")

    # -- done -------------------------------------------------------------------
    print(f"""
== release v{version} built and tagged (nothing pushed/published)

artifacts:
  {installer}
  {sig_file}
  {latest}

to publish, push this repo and create the GitHub release:
  git push && git push origin v{version}
  gh release create v{version} --repo {RELEASES_REPO} --title "SpinZero {version}" \\
      --notes "{args.notes}" "{installer}" "{sig_file}" "{latest}"
(gh auth: extract the GCM token in bash first -- see .claude/memory/dist-release.md)
""")


if __name__ == "__main__":
    main()
