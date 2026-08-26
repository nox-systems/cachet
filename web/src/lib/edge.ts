import { useEffect, useState } from "react";

import { median } from "./series.ts";

// Which of Cloudflare's edges answers this reader, and how far away it
// is. Both are facts about the person reading rather than about the
// deployment, which is what makes them worth showing: placement is
// deliberately unpinned (docs/DEPLOY.md), so the worker runs at the colo
// nearest the client and the edge answering this console is the same one
// answering that laptop's substitutions.

/** How many round trips the median is taken over. */
const WINDOW = 5;

/** How often to take another. */
const EVERY_MS = 60_000;

export type Edge = {
  /** The colo's IATA code, absent where Cloudflare is not in front. */
  readonly colo?: string;
  /** The median round trip in milliseconds, absent until one lands. */
  readonly rttMs?: number;
};

/**
 * Measure the round trip to this deployment's edge.
 *
 * `/nix-cache-info` is the probe because it is the one protocol path that
 * needs no credential and because it is edge-cached, which is what makes
 * the number mean something: a warm narinfo read is an edge hit too, so
 * this is the same round trip a substitution pays. `cache: "no-store"`
 * keeps the browser from answering from its own cache without stopping
 * Cloudflare's from answering from its.
 *
 * A probe that fails says nothing rather than something wrong: the
 * previous samples stand and the reader sees no change.
 */
export const useEdge = (): Edge => {
  const [colo, setColo] = useState<string | undefined>(undefined);
  const [samples, setSamples] = useState<readonly number[]>([]);

  useEffect(() => {
    let live = true;
    const probe = async () => {
      const started = performance.now();
      try {
        const answer = await fetch("/nix-cache-info", { cache: "no-store" });
        const elapsed = performance.now() - started;
        if (!live) return;
        // Cloudflare names the colo in the last segment of every ray id,
        // so this costs the deployment nothing to serve.
        const ray = answer.headers.get("cf-ray");
        const named = ray?.split("-").pop();
        if (named !== undefined && named !== "") setColo(named);
        setSamples((previous) => [...previous, elapsed].slice(-WINDOW));
      } catch {
        // Offline, or the deployment is down. The rest of the console
        // will say so far more usefully than a missing millisecond.
      }
    };
    void probe();
    const timer = window.setInterval(() => void probe(), EVERY_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);

  const rttMs = median(samples);
  return {
    ...(colo === undefined ? {} : { colo }),
    ...(rttMs === undefined ? {} : { rttMs: Math.round(rttMs) }),
  };
};
