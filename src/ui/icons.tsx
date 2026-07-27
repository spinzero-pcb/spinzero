// Minimal stroke icon set (VS Code codicon style). All icons inherit
// currentColor so tokens drive the palette.

interface IconProps {
  size?: number;
}

function Svg({ size = 16, children }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

/** Circuit board: rounded square with via + traces */
export function IconBoard(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="2" y="2" width="12" height="12" rx="1.5" />
      <circle cx="6" cy="6" r="1.4" />
      <path d="M7.4 6H11M11 6V3.5M6 7.4V11M6 11h3.5" />
    </Svg>
  );
}

/** Copy: two overlapping rounded rectangles (clipboard-free duplicate glyph). */
export function IconCopy(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.3" />
      <path d="M10.5 5.5V4a1.3 1.3 0 0 0-1.3-1.3H4a1.3 1.3 0 0 0-1.3 1.3v5.2A1.3 1.3 0 0 0 4 10.5h1.5" />
    </Svg>
  );
}

/** SpinZero brand mark: a plated via — copper pad, light annular ring, drilled
 *  centre (the “zero”). Copper via the --brand token; the ring/drill use light/
 *  dark surface tokens so the mark reads on the dark app chrome. Mirrors
 *  branding/spinzero-mark-dark.svg. */
export function IconSpinZero({ size = 16 }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 64 64"
      fill="none"
      role="img"
      aria-label="SpinZero"
    >
      <circle cx="32" cy="32" r="27" fill="var(--brand)" />
      <circle cx="32" cy="32" r="19" fill="none" stroke="var(--fg-0)" strokeWidth="3.4" />
      <circle cx="32" cy="32" r="10.8" fill="var(--bg-0)" />
    </svg>
  );
}

/** Review checklist */
export function IconChecklist(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3 4.5l1.2 1.2L6.5 3.4" />
      <path d="M9 4.5h4" />
      <path d="M3 10.5l1.2 1.2 2.3-2.3" />
      <path d="M9 10.5h4" />
    </Svg>
  );
}

/** AI sparkle */
export function IconSparkle(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M8 2.5L9.3 6 13 7.3 9.3 8.6 8 12 6.7 8.6 3 7.3 6.7 6 8 2.5z" />
      <path d="M12.8 11.4l.5 1.3 1.3.5-1.3.5-.5 1.3-.5-1.3-1.3-.5 1.3-.5.5-1.3z" />
    </Svg>
  );
}

/** Settings gear */
export function IconGear(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M12.4 3.6L11 5M5 11l-1.4 1.4" />
    </Svg>
  );
}

/** Folder (open) */
export function IconFolder(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M1.5 3.5h4l1.5 2h7.5v7a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1v-9z" />
    </Svg>
  );
}

/** Chevron — rotate via CSS class for expand/collapse. The glyph is centered in the
 *  viewBox so the rotation doesn't shift it visually (feedback item 12). */
export function IconChevron(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M5.75 3.5L10.25 8 5.75 12.5" />
    </Svg>
  );
}

/** Schematic sheet */
export function IconSheet(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3.5 1.5h6l3 3v10h-9v-13z" />
      <path d="M9.5 1.5v3h3" />
    </Svg>
  );
}

/** PCB copper layers */
export function IconLayers(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M8 2L14 5.5 8 9 2 5.5 8 2z" />
      <path d="M2 8.5L8 12l6-3.5" />
      <path d="M2 11.5L8 15l6-3.5" />
    </Svg>
  );
}

/** BOM table */
export function IconTable(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M2 6.5h12M6.5 6.5V13" />
    </Svg>
  );
}

/** Revision history clock */
export function IconHistory(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2.5 1.5" />
    </Svg>
  );
}

/** Pin (kept revision) */
export function IconPin(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M9.5 2l4.5 4.5-2 .7-2.8 2.8.3 2.5-1.8-1.8L4 14.5 1.5 12l3.8-3.7L3.5 6.5 6 6.2l2.8-2.8.7-1.4z" />
    </Svg>
  );
}

/** Fit to screen — four corner brackets */
export function IconFit(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M2 5.5V2.5h3M14 5.5V2.5h-3M2 10.5v3h3M14 10.5v3h-3" />
    </Svg>
  );
}

/** Refresh */
export function IconRefresh(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
      <path d="M13.5 2.5v2.7h-2.7" />
    </Svg>
  );
}

/** Check mark */
export function IconCheck(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3 8.5l3 3 7-7.5" />
    </Svg>
  );
}

/** Eye (visible) */
export function IconEye(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M1.5 8s2.5-4 6.5-4 6.5 4 6.5 4-2.5 4-6.5 4-6.5-4-6.5-4z" />
      <circle cx="8" cy="8" r="1.8" />
    </Svg>
  );
}

/** Eye with a slash (hidden) */
export function IconEyeOff(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M1.5 8s2.5-4 6.5-4c1.2 0 2.3.36 3.2.87M14.5 8s-2.5 4-6.5 4c-1.2 0-2.3-.36-3.2-.87" />
      <path d="M3 13L13 3" />
    </Svg>
  );
}

/** Comment bubble */
export function IconComment(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2v-7z" />
    </Svg>
  );
}

/** Locate / crosshair target */
export function IconLocate(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2" />
    </Svg>
  );
}

/** Ruler — measure tool (a straight edge with tick marks) */
export function IconRuler(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="2" y="5.5" width="12" height="5" rx="0.5" />
      <path d="M4.5 5.5v2M7 5.5v2.5M9.5 5.5v2M11.5 5.5v2" />
    </Svg>
  );
}

/** Magnifier with a plus — zoom in */
export function IconZoomIn(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="7" cy="7" r="4.3" />
      <path d="M10.3 10.3L14 14" />
      <path d="M7 5.2v3.6M5.2 7h3.6" />
    </Svg>
  );
}

/** Magnifier with a minus — zoom out */
export function IconZoomOut(p: IconProps) {
  return (
    <Svg {...p}>
      <circle cx="7" cy="7" r="4.3" />
      <path d="M10.3 10.3L14 14" />
      <path d="M5.2 7h3.6" />
    </Svg>
  );
}

/** Trash / delete */
export function IconTrash(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M3 4.5h10M6.5 4.5V3h3v1.5M5 4.5l.6 8.5h4.8L11 4.5M6.7 6.8v4M9.3 6.8v4" />
    </Svg>
  );
}

/** Close / cancel: an X. */
export function IconClose(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M4 4l8 8M12 4l-8 8" />
    </Svg>
  );
}

/** Edit / rename — a pencil (nib at bottom-left, eraser ferrule near the tip). */
export function IconEdit(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M2.5 13.5l1-3 7.5-7.5 2 2-7.5 7.5-3 1z" />
      <path d="M9.5 4.5l2 2" />
    </Svg>
  );
}

/** Compare — a framed A│B split view (the side-by-side diff). */
export function IconCompare(p: IconProps) {
  return (
    <Svg {...p}>
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M8 3v10" />
    </Svg>
  );
}

/** Tag / label — a pointed tag with a punch hole. */
export function IconTag(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M2.5 2.5h5.2l5.8 5.8-5.2 5.2-5.8-5.8z" />
      <circle cx="5" cy="5" r="1" />
    </Svg>
  );
}

/** Enter full screen: four corners pointing outward. */
export function IconFullscreen(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M2 5.5V2.5A.5.5 0 0 1 2.5 2H5.5" />
      <path d="M14 5.5V2.5A.5.5 0 0 0 13.5 2H10.5" />
      <path d="M2 10.5v3a.5.5 0 0 0 .5.5H5.5" />
      <path d="M14 10.5v3a.5.5 0 0 1-.5.5H10.5" />
    </Svg>
  );
}

/** Exit full screen: four corners pointing inward. */
export function IconFullscreenExit(p: IconProps) {
  return (
    <Svg {...p}>
      <path d="M5.5 2v3a.5.5 0 0 1-.5.5H2" />
      <path d="M10.5 2v3a.5.5 0 0 0 .5.5H14" />
      <path d="M5.5 14v-3a.5.5 0 0 0-.5-.5H2" />
      <path d="M10.5 14v-3a.5.5 0 0 1 .5-.5H14" />
    </Svg>
  );
}
