import { useEffect, useRef, useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useSelectionStore } from "../../stores/selectionStore";
import type { DesignIndexes } from "../../lib/design";

// Contact-sheet of all sheets (spec WS6) — the secondary "picker" tier, reached by
// zooming out past the sheet edge or the overview key. U4 crunch-time thumbnails
// don't exist yet, so sheets are rasterized client-side once per revision: keyed by
// the indexes object identity, the cache dies with the revision that made it.
const thumbCache = new WeakMap<DesignIndexes, Map<number, string>>();

const THUMB_W = 560;

async function rasterize(svgText: string): Promise<string> {
  const blob = new Blob([svgText], { type: "image/svg+xml" });
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    await new Promise<void>((res, rej) => {
      img.onload = () => res();
      img.onerror = () => rej(new Error("svg rasterize failed"));
      img.src = url;
    });
    const ratio = img.naturalHeight / Math.max(1, img.naturalWidth);
    const canvas = document.createElement("canvas");
    canvas.width = THUMB_W;
    canvas.height = Math.max(1, Math.round(THUMB_W * ratio));
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png");
  } finally {
    URL.revokeObjectURL(url);
  }
}

export function Overview({ onPick }: { onPick: (num: number) => void }) {
  const indexes = useDesignStore((s) => s.indexes);
  const getSheetSvg = useDesignStore((s) => s.getSheetSvg);
  const currentSheet = useSelectionStore((s) => s.currentSheet);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const rootRef = useRef<HTMLDivElement>(null);

  // The overlay lives inside the canvas stage, whose NATIVE wheel/pointer handlers
  // pan/zoom the sheet underneath. React's synthetic stopPropagation fires too late
  // for those, so the events are stopped natively at the overlay boundary.
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const stop = (e: Event) => e.stopPropagation();
    const evs = ["wheel", "pointerdown", "pointermove", "pointerup"] as const;
    for (const ev of evs) el.addEventListener(ev, stop);
    return () => {
      for (const ev of evs) el.removeEventListener(ev, stop);
    };
  }, []);

  useEffect(() => {
    if (!indexes) return;
    let cancelled = false;
    let cache = thumbCache.get(indexes);
    if (!cache) {
      cache = new Map();
      thumbCache.set(indexes, cache);
    }
    const have = cache;
    setThumbs(new Map(have));
    (async () => {
      for (const sheet of indexes.sheets) {
        if (cancelled) return;
        if (!sheet.svg || have.has(sheet.num)) continue;
        try {
          have.set(sheet.num, await rasterize(await getSheetSvg(sheet.num)));
          if (!cancelled) setThumbs(new Map(have));
        } catch {
          // Sheet failed to rasterize — its card stays a name-only tile.
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [indexes, getSheetSvg]);

  if (!indexes) return null;
  return (
    <div className="overview" ref={rootRef}>
      <div className="overview-grid">
        {indexes.sheets
          .filter((s) => s.svg)
          .map((s) => (
            <button
              key={s.num}
              className={`overview-card ${s.num === currentSheet ? "active" : ""}`}
              onClick={() => onPick(s.num)}
              title={s.name}
            >
              {thumbs.get(s.num) ? (
                <img src={thumbs.get(s.num)} alt={s.name} draggable={false} />
              ) : (
                <div className="overview-empty">rendering…</div>
              )}
              <span className="overview-name">
                {s.name} <span className="dim mono">p{s.num}</span>
              </span>
            </button>
          ))}
      </div>
    </div>
  );
}
