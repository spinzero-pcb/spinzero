#!/usr/bin/env python3
"""Reference-side probe for the plan §8.3 differential.

Runs under the *reference parser's* interpreter (the one that can
``import altium_monkey``) and emits the M0 comparison document for one Altium
document on stdout as JSON.  It is deliberately the only file in this repo that
touches the reference parser's API: everything else drives its CLI.

The reference is a development-time oracle.  It is never vendored, never a build
dependency, and nothing here is shipped.

Usage:  python ref_probe.py <file.SchDoc|.PcbDoc>
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from altium_monkey import AltiumOleFile
from altium_monkey.altium_utilities import get_records_in_section

# Primitive streams the reference reads with a per-type scanner rather than the
# prefixed record walk.  Our side frames these as `Blocks(n)`; the comparable
# reference number is the length of the typed collection, so both are reported.
PRIMITIVE_STREAMS = {
    "tracks6": "tracks",
    "arcs6": "arcs",
    "pads6": "pads",
    "vias6": "vias",
    "texts6": "texts",
    "fills6": "fills",
    "regions6": "regions",
}


def stream_paths(ole: AltiumOleFile) -> list[list[str]]:
    return [p for p in ole.listdir(streams=True, storages=False)]


def record_type(rec: dict) -> str:
    """The histogram key, in the same vocabulary our `pcb-extract dump` uses."""
    if rec.get("__BINARY_RECORD__"):
        return f"bin:{rec.get('RECORD', 0)}"
    value = rec.get("RECORD")
    return f"text:{value}" if value is not None else "text:-"


def qualifying(fields: dict[str, str]) -> bool:
    """M0 compares the decoded values of the records that can go wrong quietly:
    a pipe that survived un-escaping, a `%UTF8%` key, or any non-ASCII byte."""
    for key, value in fields.items():
        if key.upper().startswith("%UTF8%"):
            return True
        if "|" in value:
            return True
        if any(ord(ch) > 127 for ch in value):
            return True
    return False


def fold_utf8(fields: dict[str, str]) -> dict[str, str]:
    """Fold `%UTF8%KEY` over `KEY` and upper-case the keys.

    A representation difference we intend (plan §7): the reference keeps both
    keys and picks between them at the call site, we pick at decode time.  What
    survives the fold is the decoded *value*, which is what M0 compares.
    """
    plain: dict[str, str] = {}
    utf8: dict[str, str] = {}
    for key, value in fields.items():
        upper = key.upper()
        if upper.startswith("UNHANDLED"):
            # A `|` chunk with no `=`. The reference keeps it under a synthetic
            # key so it can write the record back; we drop it. Representation.
            continue
        if upper.startswith("%UTF8%"):
            utf8.setdefault(upper[len("%UTF8%") :], value)
        else:
            plain.setdefault(upper, value)
    plain.update(utf8)
    return plain


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: ref_probe.py <file.SchDoc|.PcbDoc>", file=sys.stderr)
        return 2
    path = Path(argv[1])

    streams: dict[str, dict] = {}
    text_fields: dict[str, dict[str, str]] = {}

    with AltiumOleFile(path) as ole:
        for parts in stream_paths(ole):
            name = "/".join(parts)
            try:
                data = ole.openstream(parts)
            except Exception as exc:  # a stream we cannot read is reported, not fatal
                streams[name.lower()] = {"error": str(exc)}
                continue
            try:
                records = get_records_in_section(ole, parts)
            except Exception as exc:
                streams[name.lower()] = {"bytes": len(data), "error": str(exc)}
                continue

            types: dict[str, int] = {}
            text = binary = nonempty = 0
            for index, rec in enumerate(records):
                key = record_type(rec)
                types[key] = types.get(key, 0) + 1
                if rec.get("__BINARY_RECORD__"):
                    binary += 1
                    continue
                text += 1
                fields = {k: v for k, v in rec.items() if not k.startswith("__")}
                if fields:
                    nonempty += 1
                if qualifying(fields):
                    text_fields[f"{name.lower()}#{index}"] = fold_utf8(fields)

            streams[name.lower()] = {
                "bytes": len(data),
                "records": len(records),
                "text": text,
                "binary": binary,
                # A 4-byte counter stream walks as one zero-length "record"
                # under the reference's prefixed walk. Counting the records that
                # actually decoded to something keeps that out of the report.
                "nonempty": nonempty,
                "types": types,
            }

    primitives: dict[str, int] = {}
    if path.suffix.lower() == ".pcbdoc":
        try:
            from altium_monkey import AltiumPcbDoc

            pcb = AltiumPcbDoc.from_file(path)
            for stream, attr in PRIMITIVE_STREAMS.items():
                collection = getattr(pcb, attr, None)
                if collection is not None:
                    primitives[stream] = len(collection)
        except Exception as exc:
            primitives["__error__"] = str(exc)  # type: ignore[assignment]

    json.dump(
        {
            "document": str(path),
            "streams": streams,
            "text_fields": text_fields,
            "primitives": primitives,
        },
        sys.stdout,
        # ASCII-escaped on purpose: the harness reads this over a pipe whose
        # encoding is the platform's, and a µ written as cp1252 would reach it
        # as a replacement character and read as a decoding disagreement.
        ensure_ascii=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
