"""Quick sanity check of an index.sqlite produced by the app."""
import sqlite3
import sys

db = sys.argv[1]
conn = sqlite3.connect(db)
for table in ["revisions", "components", "nets", "pins", "sheets", "layers", "bom_lines", "search_fts"]:
    n = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    print(f"{table:12} {n}")
print("\nsample component R12:")
for row in conn.execute("SELECT designator, value, mpn, sheet FROM components WHERE designator='R12'"):
    print(" ", row)
print("\nR12 nets:")
for row in conn.execute(
    "SELECT n.name, p.pin_number FROM pins p JOIN nets n ON n.id=p.net_id "
    "JOIN components c ON c.id=p.component_id WHERE c.designator='R12'"
):
    print(" ", row)
print("\nFTS 'VDRV':")
for row in conn.execute("SELECT kind, ref FROM search_fts WHERE search_fts MATCH '\"VDRV\"*' LIMIT 5"):
    print(" ", row)
