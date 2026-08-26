import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import * as wire from "../src/api/schema.ts";

// The console decodes every answer before a screen sees it, so these are
// the shapes it believes the deployment serves. The bodies below are the
// worker's own, copied from what cachet-api serializes; a field the
// worker renames fails here rather than three components later as an
// `undefined` on screen.

const decode = <A>(
  schema: Schema.Codec<A, unknown, never, never>,
  body: unknown,
) => Schema.decodeUnknownSync(schema)(body);

describe("PublicConfig", () => {
  it("reads what /api/public/config serves", () => {
    const config = decode(wire.PublicConfig, {
      oauthClientId: "id",
      orgs: ["nox-systems"],
      host: "cachet.example.com",
      publicKey: "cachet.example.com-1:AAAA",
      deployment: "production",
      version: "0.1.0",
    });
    expect(config.host).toBe("cachet.example.com");
    // The two optional fields are absent rather than null on a build
    // that stamps no commit and licenses no fonts.
    expect(config.buildSha).toBeUndefined();
    expect(config.fontCss).toBeUndefined();
  });

  it("refuses a body missing a field the console reads", () => {
    expect(() =>
      decode(wire.PublicConfig, { orgs: [], host: "h", publicKey: "k" }),
    ).toThrow();
  });
});

describe("WhoAmI", () => {
  it("reads a browser session with its expiry", () => {
    const who = decode(wire.WhoAmI, {
      login: "shivansh",
      admin: true,
      credential: "browser",
      expiresAtMs: 1_789_000_000_000,
    });
    expect(who.admin).toBe(true);
    expect(who.expiresAtMs).toBe(1_789_000_000_000);
  });

  it("reads a laptop, which names no expiry", () => {
    const who = decode(wire.WhoAmI, {
      login: "shivansh",
      admin: false,
      credential: "laptop",
    });
    expect(who.expiresAtMs).toBeUndefined();
  });

  it("refuses a credential class the console has no screen for", () => {
    expect(() =>
      decode(wire.WhoAmI, {
        login: "x",
        admin: false,
        credential: "carrier pigeon",
      }),
    ).toThrow();
  });
});

describe("Health", () => {
  it("reads a deployment that has never collected", () => {
    const health = decode(wire.Health, {
      status: "unknown",
      nextCollectionAtMs: 1_787_806_800_000,
    });
    expect(health.status).toBe("unknown");
    expect(health.latestRunId).toBeUndefined();
  });

  it("reads a run that tripped a gate", () => {
    const health = decode(wire.Health, {
      status: "degraded",
      nextCollectionAtMs: 1,
      latestRunId: "1780000000000-0123456789abcdef",
      latestFinishedAtMs: 2,
      gate: "sweep_fraction_exceeded",
    });
    expect(health.gate).toBe("sweep_fraction_exceeded");
  });
});

describe("StatsEvents", () => {
  it("reads a filtered series", () => {
    const events = decode(wire.StatsEvents, {
      subject: "reads",
      dimension: "day",
      window: "week",
      filters: { actor: "laptop" },
      rows: [{ dimension: "1780000000", count: 610, bytes: 1_300 }],
    });
    expect(events.filters.actor).toBe("laptop");
    expect(events.rows[0]?.count).toBe(610);
  });

  it("reads an unfiltered answer, whose filters object is empty", () => {
    const events = decode(wire.StatsEvents, {
      subject: "reads",
      dimension: "outcome",
      window: "week",
      filters: {},
      rows: [],
    });
    expect(events.filters.actor).toBeUndefined();
  });
});

describe("GcReport", () => {
  it("reads a report the worker streams verbatim from the bucket", () => {
    const report = decode(wire.GcReport, {
      runId: "1780000000000-0123456789abcdef",
      startedAtMs: 1_780_000_000_000,
      finishedAtMs: 1_780_000_012_345,
      inventoryPaths: 4_213,
      activeLeases: 7,
      markedPaths: 4_102,
      unreadableDeep: 0,
      narinfosDeleted: 111,
      narsDeleted: 98,
      bytesFreed: 8_123_456_789,
      uploadsAborted: 2,
      gate: null,
    });
    expect(report.inventoryPaths).toBe(4_213);
    // The worker writes `gate: null` on a clean run and omits nothing,
    // so the console has to read a null as "no gate".
    expect(report.gate ?? undefined).toBeUndefined();
  });
});

describe("Problem", () => {
  it("reads the refusal every route answers with", () => {
    const problem = decode(wire.Problem, {
      type: "about:blank",
      status: 403,
      title: "not an admin of this deployment",
      code: "forbidden_admin",
    });
    expect(problem.code).toBe("forbidden_admin");
  });
});
