import { Schema } from "effect";

import { noteColo } from "../lib/edge.ts";
import * as wire from "./schema.ts";

// The console's one door to the deployment. Every network detail stops
// here: below this module the console speaks in decoded values, and a
// refusal is an ApiError carrying the code the worker chose.

/** A refusal the deployment named, or a shape it answered that this
 *  console could not read. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /** Nobody is signed in. The console shows its sign-in screen for this
   *  rather than an error: it is the ordinary state of a first visit. */
  get unauthenticated(): boolean {
    return this.status === 401;
  }

  /** Signed in, and not an admin. */
  get forbidden(): boolean {
    return this.status === 403;
  }

  /** The deployment counts and cannot report, or a store is down. The
   *  screens that read counters say so rather than showing an error. */
  get unavailable(): boolean {
    return this.status === 503;
  }

  /** No collection has finished yet. */
  get absent(): boolean {
    return this.status === 404;
  }
}

const read = async <A>(
  path: string,
  schema: Schema.Codec<A, unknown, never, never>,
): Promise<A> => {
  const response = await fetch(path, {
    // why: the session cookie is HttpOnly and SameSite=Lax on this
    // origin, so it rides along without the console ever seeing it.
    credentials: "same-origin",
    headers: { accept: "application/json" },
  });
  // why: read off an answer the console was making anyway. Cloudflare
  // names the colo that served a request in the last segment of its ray
  // id, so knowing which edge answers this reader costs no request.
  noteColo(response.headers.get("cf-ray"));
  const body: unknown = await response.json().catch(() => undefined);
  if (!response.ok) {
    const problem = Schema.decodeUnknownOption(wire.Problem)(body);
    throw new ApiError(
      response.status,
      problem._tag === "Some" ? problem.value.code : "unknown",
      problem._tag === "Some"
        ? problem.value.title
        : `the deployment answered ${response.status}`,
    );
  }
  try {
    return Schema.decodeUnknownSync(schema)(body);
  } catch (failure) {
    // A 200 this console cannot read is worth naming as loudly as a
    // refusal: it means the deployment is newer or older than the
    // console served beside it.
    throw new ApiError(
      response.status,
      "unreadable_answer",
      `${path} answered a shape this console does not understand: ${String(failure)}`,
    );
  }
};

export const getConfig = (): Promise<wire.PublicConfig> =>
  read("/api/public/config", wire.PublicConfig);

export const getWhoAmI = (): Promise<wire.WhoAmI> =>
  read("/api/whoami", wire.WhoAmI);

export const getHealth = (): Promise<wire.Health> =>
  read("/api/self/health", wire.Health);

export const getStats = (): Promise<wire.Stats> =>
  read("/api/self/stats", wire.Stats);

export const listGcRuns = (): Promise<wire.GcRunList> =>
  read("/api/self/gc-runs", wire.GcRunList);

export const getGcRun = (runId: string): Promise<wire.GcReport> =>
  read(`/api/self/gc-runs/${encodeURIComponent(runId)}`, wire.GcReport);

/** What a counter question is made of. The worker refuses any value
 *  outside these, so the console never has to. */
export type EventsQuery = {
  readonly subject: wire.Subject;
  readonly by: wire.Dimension;
  readonly window: wire.Window;
  readonly actor?: wire.Actor;
  readonly kind?: wire.Kind;
  readonly outcome?: string;
};

/** The query string one question serializes to, in a fixed order so two
 *  identical questions share a cache entry. */
export const eventsSearch = (query: EventsQuery): string => {
  const search = new URLSearchParams({
    subject: query.subject,
    by: query.by,
    window: query.window,
  });
  if (query.kind !== undefined) search.set("kind", query.kind);
  if (query.outcome !== undefined) search.set("outcome", query.outcome);
  if (query.actor !== undefined) search.set("actor", query.actor);
  return search.toString();
};

export const getEvents = (query: EventsQuery): Promise<wire.StatsEvents> =>
  read(`/api/self/events?${eventsSearch(query)}`, wire.StatsEvents);

/** Sign out, then reload: the session cookie is the only state the
 *  console holds, and the server is what clears it. */
export const signOut = async (): Promise<void> => {
  await fetch("/logout", { method: "POST", credentials: "same-origin" });
};
