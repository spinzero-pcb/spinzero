/** KiCad text markup for the Canvas2D net-label overlay. The GPU overlay draws plain
 *  glyph runs, so this is the counterpart of the Rust `render_markup`/`display_text`
 *  (see `extract/src/svg.rs`): it splits a net/label string into consecutive runs,
 *  flags the ones under an over-bar (`~{…}`), and resolves `{token}` character escapes.
 *  KiCad shows e.g. `~{crst}{slash}out2` as an overlined "crst" then "/out2" — emitting
 *  the raw markup instead printed the literal braces and made labels far too wide. */

/** One run of overlay text sharing an over-bar state. */
export interface MarkupRun {
  text: string;
  /** Draw a line over this run (KiCad `~{…}` overbar — active-low nets etc.). */
  over: boolean;
}

/** KiCad `UnescapeString` tokens that turn up in net/label names — most importantly
 *  `{slash}` (a literal `/`, which KiCad escapes as it is the hierarchy separator).
 *  Kept in sync with `unescape_token` in `extract/src/svg.rs`. */
const ESCAPE: Record<string, string> = {
  slash: "/",
  backslash: "\\",
  brace: "{",
  dollar: "$",
  lt: "<",
  gt: ">",
  colon: ":",
  tab: "\t",
  return: "\n",
  dblquote: '"',
};

/** Index of the `}` matching the `{` at `open`, or -1 if unbalanced. */
function matchingBrace(s: string, open: number): number {
  let depth = 0;
  for (let j = open; j < s.length; j++) {
    if (s[j] === "{") depth++;
    else if (s[j] === "}" && --depth === 0) return j;
  }
  return -1;
}

/** Split `text` into over-bar / plain runs, resolving `{token}` escapes. `~{…}`
 *  overlines its body (nestable); `_{…}`/`^{…}` sub/superscript are flattened to plain
 *  runs (the overlay has no baseline shift, matching `display_text`). Adjacent runs
 *  with the same over-bar state are merged; plain input yields a single run. */
export function parseMarkup(text: string): MarkupRun[] {
  const runs: MarkupRun[] = [];
  const push = (t: string, over: boolean) => {
    if (!t) return;
    const last = runs[runs.length - 1];
    if (last && last.over === over) last.text += t;
    else runs.push({ text: t, over });
  };
  const walk = (s: string, over: boolean) => {
    let buf = "";
    let i = 0;
    while (i < s.length) {
      const ch = s[i];
      // Style group: ~{…} (overline), ^{…}/_{…} (flattened sub/superscript).
      if ((ch === "~" || ch === "_" || ch === "^") && s[i + 1] === "{") {
        const close = matchingBrace(s, i + 1);
        if (close >= 0) {
          push(buf, over);
          buf = "";
          walk(s.slice(i + 2, close), over || ch === "~");
          i = close + 1;
          continue;
        }
      }
      // Character escape: {token}. Unknown/unbalanced braces fall through literally.
      if (ch === "{") {
        const close = matchingBrace(s, i);
        if (close >= 0) {
          const rep = ESCAPE[s.slice(i + 1, close)];
          if (rep !== undefined) {
            buf += rep;
            i = close + 1;
            continue;
          }
        }
      }
      buf += ch;
      i++;
    }
    push(buf, over);
  };
  walk(text, false);
  return runs;
}

/** The visible text with markup stripped and escapes resolved (for width fitting). */
export function markupPlain(text: string): string {
  return parseMarkup(text)
    .map((r) => r.text)
    .join("");
}
