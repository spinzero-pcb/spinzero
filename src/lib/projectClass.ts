import type { ProjectClass } from "./types";
import type { BomProfile } from "./findings";

// The project's end application, in ONE place.
//
// It used to be asked twice and stored twice: the import wizard wrote `project.class`
// into project.json, and the BOM review's own "End application" dropdown wrote an
// unrelated `bom_check_profile` into local settings. Two answers to one question, and
// a review could silently run against the one the user had forgotten about.
//
// project.json wins as the home — the end application is a fact about the board, it
// syncs with the project, and the wizard already recorded it. The rules' profile is
// DERIVED from it here rather than stored, so there is nothing left to drift.

export const PROJECT_CLASSES: { value: ProjectClass; label: string; hint: string }[] = [
  { value: "general", label: "General", hint: "Hobby / prototype / no specific standard" },
  // Automotive is two answers, as in the rule pack: which half of the car decides
  // whether a part its maker excludes from braking and steering is a Critical finding.
  {
    value: "automotive-comfort",
    label: "Automotive Infotainment, body and chassis",
    hint: "AEC-Q parts, no driving functions",
  },
  { value: "automotive-safety", label: "Automotive Powertrain/Safety", hint: "ISO 26262 / AEC-Q — driving functions" },
  { value: "commercial", label: "Commercial", hint: "Consumer / IPC Class 2" },
  { value: "medical", label: "Medical", hint: "IEC 60601 / ISO 13485" },
  { value: "industrial", label: "Industrial", hint: "Ruggedized / IPC Class 2–3" },
  { value: "space", label: "Space", hint: "IPC Class 3 / hi-rel" },
];

export function isProjectClass(v: unknown): v is ProjectClass {
  return typeof v === "string" && PROJECT_CLASSES.some((c) => c.value === v);
}

/** A project.json written before automotive was split says `automotive`. It reads as
 *  the stricter half, the same way the rule pack resolves the retired profile, so the
 *  dropdown shows what the review will actually run as. */
export function normalizeClass(v: string | null | undefined): ProjectClass {
  if (v === "automotive") return "automotive-safety";
  return isProjectClass(v) ? v : "general";
}

export function projectClassLabel(v: string | null | undefined): string {
  return PROJECT_CLASSES.find((c) => c.value === normalizeClass(v))?.label ?? "General";
}

/** The seven project classes onto the five rule profiles the rule pack's `config::PROFILES`
 *  ships. `space` reads the hi-rel expectations the industrial profile encodes, and
 *  `general` maps to `commercial` — mapping them down is what lets the app ask the
 *  question once instead of twice.
 *
 *  Nothing here may return `default`. That profile now means **nobody said** and runs
 *  the strictest setting of every rule; the app always has a project class, so it has
 *  always been an answer, and returning the unstated profile for an answered question
 *  would flood a hobby board with AEC-Q findings. `general` and `commercial` are the
 *  same rule set and always were — the old comment on this function said so. */
export function bomProfileForClass(v: string | null | undefined): BomProfile {
  switch (v) {
    case "automotive-comfort":
      return "automotive-comfort";
    // A project that still stores the retired `automotive` does not say which half of
    // the car, and guessing the looser half is the one direction that can hide a
    // driving-function finding. So it gets the safety rules, which matches how
    // `config_for` resolves the retired id.
    case "automotive-safety":
    case "automotive":
      return "automotive-safety";
    case "medical":
      return "medical";
    case "industrial":
    case "space":
      return "industrial";
    default:
      return "commercial";
  }
}
