import { describe, expect, it } from "vitest";

import * as format from "../src/lib/format.ts";

describe("bytes", () => {
  it("names the unit an operator thinks in", () => {
    expect(format.bytes(0)).toBe("0 B");
    expect(format.bytes(512)).toBe("512 B");
    expect(format.bytes(1024)).toBe("1.0 KB");
    expect(format.bytes(2_254_857_830)).toBe("2.1 GB");
    expect(format.bytes(19_112_654_602)).toBe("17.8 GB");
  });

  it("answers a number for a number that is not one", () => {
    // The counters are doubles off a sampled dataset, so a screen can be
    // handed a NaN or a negative by arithmetic upstream of it. A stat
    // tile reading "NaN GB" is worse than one reading zero.
    expect(format.bytes(Number.NaN)).toBe("0 B");
    expect(format.bytes(-1)).toBe("0 B");
    expect(format.bytes(Number.POSITIVE_INFINITY)).toBe("0 B");
  });

  it("stops at the largest unit rather than inventing one", () => {
    expect(format.bytes(1024 ** 6)).toBe("1024.0 PB");
  });
});

describe("count", () => {
  it("groups so a number can be read at a glance", () => {
    expect(format.count(184_212)).toBe("184,212");
    expect(format.count(0)).toBe("0");
    expect(format.count(1_508.4)).toBe("1,508");
  });
});

describe("percent and perHundred", () => {
  it("is no share rather than a division by zero", () => {
    expect(format.percent(5, 0)).toBe("0%");
    expect(format.perHundred(5, 0)).toBe(0);
  });

  it("reads the way the screen says it", () => {
    expect(format.percent(610, 12_480)).toBe("4.9%");
    expect(format.perHundred(11_904, 12_480)).toBe(95);
  });
});

describe("duration", () => {
  it("picks the coarsest unit that still says something", () => {
    expect(format.duration(12_000)).toBe("12 s");
    expect(format.duration(192_000)).toBe("3 min 12 s");
    expect(format.duration(180_000)).toBe("3 min");
    expect(format.duration(85_560_000)).toBe("23 h 46 m");
  });

  it("says so when it does not know", () => {
    expect(format.duration(Number.NaN)).toBe("unknown");
    expect(format.duration(-1)).toBe("unknown");
  });
});

describe("clock and dates", () => {
  it("reads UTC, because that is what the deployment runs on", () => {
    const at = Date.UTC(2026, 7, 25, 5, 14, 2);
    expect(format.clock(at)).toBe("05:14:02");
    expect(format.day(at)).toBe("25 Aug");
    expect(format.stamp(at)).toBe("25 Aug 05:14");
    expect(format.date(at)).toBe("25 Aug 2026");
    expect(format.hour(at)).toBe("05:00");
  });
});

describe("labels", () => {
  it("writes a counter value for a person rather than for a query", () => {
    expect(format.outcomeLabel("edge_hit")).toBe("Served from the edge");
    expect(format.outcomeLabel("miss")).toBe("Missing from the cache");
    expect(format.actorLabel("ci")).toBe("CI");
    expect(format.kindLabel("nar")).toBe("NARs");
  });

  it("marks a refusal by its status", () => {
    expect(format.outcomeLabel("403")).toBe("Refused, 403");
    expect(format.isRefusal("403")).toBe(true);
    expect(format.isRefusal("miss")).toBe(false);
  });

  it("passes an unknown value through as itself", () => {
    // A deployment newer than the console it serves may count something
    // this vocabulary has no word for. Showing its word beats a blank.
    expect(format.outcomeLabel("teleported")).toBe("teleported");
    expect(format.actorLabel("robot")).toBe("robot");
  });
});
