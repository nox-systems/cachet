import { Schema } from "effect";

// The wire, decoded. Every response the console reads passes through one
// of these before any screen sees it, so a deployment that answers a
// shape this console does not understand fails at the boundary with the
// field named rather than three components later with `undefined`.
//
// The vocabularies below are the worker's own closed enums
// (cachet-core/src/stats.rs and stats_query.rs). They are written out
// rather than imported because the API is defined in Rust; the console
// lane decodes recorded worker answers with these schemas, which is what
// keeps the two in step.

export const Subject = Schema.Literals(["reads", "writes", "probes"]);
export type Subject = typeof Subject.Type;

export const Dimension = Schema.Literals([
  "kind",
  "outcome",
  "actor",
  "repository",
  "reference",
  "hour",
  "day",
]);
export type Dimension = typeof Dimension.Type;

export const Window = Schema.Literals(["day", "week", "month"]);
export type Window = typeof Window.Type;

export const Actor = Schema.Literals(["ci", "laptop", "browser", "anonymous"]);
export type Actor = typeof Actor.Type;

export const Kind = Schema.Literals([
  "narinfo",
  "nar",
  "part",
  "begin",
  "complete",
  "abort",
  "probe",
  "unknown",
]);
export type Kind = typeof Kind.Type;

export const PublicConfig = Schema.Struct({
  oauthClientId: Schema.String,
  orgs: Schema.Array(Schema.String),
  host: Schema.String,
  publicKey: Schema.String,
  deployment: Schema.String,
  version: Schema.String,
  buildSha: Schema.optional(Schema.String),
  fontCss: Schema.optional(Schema.String),
});
export type PublicConfig = typeof PublicConfig.Type;

export const WhoAmI = Schema.Struct({
  login: Schema.String,
  admin: Schema.Boolean,
  credential: Schema.Literals(["browser", "laptop", "ci"]),
  expiresAtMs: Schema.optional(Schema.Number),
});
export type WhoAmI = typeof WhoAmI.Type;

export const Health = Schema.Struct({
  status: Schema.Literals(["healthy", "degraded", "unknown"]),
  nextCollectionAtMs: Schema.optional(Schema.Number),
  latestRunId: Schema.optional(Schema.String),
  latestFinishedAtMs: Schema.optional(Schema.Number),
  gate: Schema.optional(Schema.String),
});
export type Health = typeof Health.Type;

export const StatsRow = Schema.Struct({
  dimension: Schema.String,
  count: Schema.Number,
  bytes: Schema.Number,
});
export type StatsRow = typeof StatsRow.Type;

export const StatsEvents = Schema.Struct({
  subject: Schema.String,
  dimension: Schema.String,
  window: Schema.String,
  filters: Schema.Struct({
    kind: Schema.optional(Schema.String),
    outcome: Schema.optional(Schema.String),
    actor: Schema.optional(Schema.String),
  }),
  rows: Schema.Array(StatsRow),
});
export type StatsEvents = typeof StatsEvents.Type;

export const GcRunList = Schema.Struct({
  runs: Schema.Array(Schema.String),
  nextCursor: Schema.optional(Schema.String),
});
export type GcRunList = typeof GcRunList.Type;

export const GcReport = Schema.Struct({
  runId: Schema.String,
  startedAtMs: Schema.Number,
  finishedAtMs: Schema.Number,
  inventoryPaths: Schema.Number,
  activeLeases: Schema.Number,
  markedPaths: Schema.Number,
  unreadableDeep: Schema.Number,
  narinfosDeleted: Schema.Number,
  narsDeleted: Schema.Number,
  bytesFreed: Schema.Number,
  uploadsAborted: Schema.Number,
  // why: nullable rather than optional. The worker streams this body
  // verbatim from the bucket and the stored document writes `gate: null`
  // on a run that finished, so a schema expecting an absent key would
  // refuse every clean run.
  gate: Schema.optional(Schema.NullOr(Schema.String)),
});
export type GcReport = typeof GcReport.Type;

export const Stats = Schema.Struct({
  basedOnRunId: Schema.String,
  inventoryPaths: Schema.Number,
  narinfosDeleted: Schema.Number,
  narsDeleted: Schema.Number,
  bytesFreed: Schema.Number,
  // Same null as the report this is projected from.
  gate: Schema.optional(Schema.NullOr(Schema.String)),
  finishedAtMs: Schema.Number,
});
export type Stats = typeof Stats.Type;

/// The worker's RFC 9457 refusal, which every route answers with.
export const Problem = Schema.Struct({
  status: Schema.Number,
  title: Schema.String,
  code: Schema.String,
});
export type Problem = typeof Problem.Type;
