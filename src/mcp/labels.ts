import type { SqlWarning, WriteKind } from "./api";

/** What a write does, under its statement type in the approval dialog. */
export const WRITE_KIND_LABEL: Record<WriteKind, string> = {
  dml: "Changes data",
  ddl: "Changes the schema",
  privileges: "Changes users or privileges",
  procedural: "Runs procedural code",
  sideEffectFunction: "Calls a function that changes something",
  lockingRead: "Reads and locks rows",
  other: "Not a plain read",
  unparsed: "IdeDB couldn't analyze this statement",
};

/** Why a write deserves a second look. */
export const WARNING_LABEL: Record<SqlWarning, string> = {
  noWhereClause: "No WHERE clause: every row of the table is affected.",
  dropsOrTruncates: "Drops or truncates: the data is destroyed.",
};
