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
  { value: "automotive", label: "Automotive", hint: "ISO 26262 / AEC-Q — functional safety" },
  { value: "commercial", label: "Commercial", hint: "Consumer / IPC Class 2" },
  { value: "medical", label: "Medical", hint: "IEC 60601 / ISO 13485" },
  { value: "industrial", label: "Industrial", hint: "Ruggedized / IPC Class 2–3" },
  { value: "space", label: "Space", hint: "IPC Class 3 / hi-rel" },
];

export function isProjectClass(v: unknown): v is ProjectClass {
  return typeof v === "string" && PROJECT_CLASSES.some((c) => c.value === v);
}

export function projectClassLabel(v: string | null | undefined): string {
  return PROJECT_CLASSES.find((c) => c.value === v)?.label ?? "General";
}

/** The six project classes onto the four rule profiles `bom_rules::config::PROFILES`
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
    // The app's project class does not say which half of the car, and guessing the
    // looser half is the one direction that can hide a driving-function finding. So an
    // "automotive" project gets the safety rules until someone says otherwise, which
    // matches how `config_for` resolves the retired id.
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
