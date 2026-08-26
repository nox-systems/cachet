import { useSyncExternalStore } from "react";

// Which of Cloudflare's edges answered this reader, and how long the
// console took to start arriving from it. Both are facts about the
// person reading rather than about the deployment, which is what makes
// them worth showing: placement is deliberately unpinned
// (docs/DEPLOY.md), so the worker runs at the colo nearest the client and
// the edge that served this console is the edge that serves that
// laptop's substitutions.
//
// Neither costs a request. The colo rides the `cf-ray` header Cloudflare
// puts on every response, read off answers the console was making
// anyway; the timing is the browser's own record of the navigation that
// loaded this page.

let colo: string | undefined;
const listeners = new Set<() => void>();

/** Record the colo named by a response the console already made. */
export const noteColo = (ray: string | null): void => {
  const named = ray?.split("-").pop();
  if (named === undefined || named === "" || named === colo) return;
  colo = named;
  for (const listener of listeners) listener();
};

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

/**
 * The console's own time to first byte.
 *
 * The browser's record of the navigation that loaded this page, so it is
 * the round trip the reader actually waited through rather than a probe
 * standing in for one. It does not change while the page is open, which
 * is correct: it describes one load, and re-measuring would describe a
 * request nobody made.
 */
export const ttfbMs = (): number | undefined => {
  const [entry] = performance.getEntriesByType(
    "navigation",
  ) as PerformanceNavigationTiming[];
  if (entry === undefined) return undefined;
  const elapsed = entry.responseStart - entry.requestStart;
  return elapsed > 0 ? Math.round(elapsed) : undefined;
};

export type Edge = {
  /** The colo's IATA code, absent where Cloudflare is not in front. */
  readonly colo?: string;
  /** The console's time to first byte, in milliseconds. */
  readonly ttfbMs?: number;
};

export const useEdge = (): Edge => {
  const seen = useSyncExternalStore(
    subscribe,
    () => colo,
    () => undefined,
  );
  const ttfb = ttfbMs();
  return {
    ...(seen === undefined ? {} : { colo: seen }),
    ...(ttfb === undefined ? {} : { ttfbMs: ttfb }),
  };
};
