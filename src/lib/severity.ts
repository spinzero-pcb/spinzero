import type { CommentSeverity } from "./types";

// The comment severities, in one place. `unverified` is a coverage gap from a detailed
// review: the review could not check the part, so it claims nothing about the board.
// It used to land on "info" and read as a remark about a part nobody had looked at.

/** Least to most severe, for the pickers. */
export const SEVERITIES: CommentSeverity[] = ["info", "minor", "unverified", "major", "critical"];

/** What a person reads for each level. */
export const SEVERITY_LABEL: Record<CommentSeverity, string> = {
  info: "info",
  minor: "minor",
  unverified: "not verified",
  major: "major",
  critical: "critical",
};

/** Higher is worse. The row marker and the review list show the worst comment first. */
export const SEVERITY_WEIGHT: Record<CommentSeverity, number> = {
  info: 0,
  minor: 1,
  unverified: 2,
  major: 3,
  critical: 4,
};
