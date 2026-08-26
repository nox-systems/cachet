import { describe, expect, it } from "vitest";

import * as series from "../src/lib/series.ts";

const row = (dimension: string, count: number, bytes = 0) => ({
  dimension,
  count,
  bytes,
});

describe("points", () => {
  it("reads epoch seconds as instants", () => {
    expect(series.points([row("1780000000", 5, 50)])).toEqual([
      { atMs: 1_780_000_000_000, count: 5, bytes: 50 },
    ]);
  });

  it("drops a row that is not a bucket", () => {
    // A dimension list answered to a chart's question: the rows are
    // names, not instants, and plotting them at zero would draw a line
    // that looked like real traffic.
    expect(series.points([row("edge_hit", 5)])).toEqual([]);
  });
});

describe("scale", () => {
  it("rounds the top up to a number a person would choose", () => {
    expect(series.scale([1_508, 2_133, 1_902]).top).toBe(2_500);
    expect(series.scale([87, 120, 96]).top).toBe(125);
  });

  it("gives a flat zero series a scale anyway", () => {
    // Without this the line sits at the top of the box, which reads as
    // constant maximum traffic rather than as none.
    expect(series.scale([]).top).toBe(1);
    expect(series.scale([0, 0, 0]).top).toBe(1);
  });
});

describe("path", () => {
  it("draws a series across the box, oldest at the left", () => {
    const drawn = series.path([0, 50, 100], 100, 20, 100);
    expect(drawn.line).toBe("0.00,20.00 50.00,10.00 100.00,0.00");
    expect(drawn.area.startsWith("M 0.00,20")).toBe(true);
    expect(drawn.area.endsWith("Z")).toBe(true);
    expect(drawn.last).toEqual({ x: 100, y: 0 });
  });

  it("clamps a point past the top rather than drawing over the panel", () => {
    const drawn = series.path([500], 100, 20, 100);
    expect(drawn.last.y).toBe(0);
  });

  it("has something to draw for an empty series", () => {
    const drawn = series.path([], 100, 20, 100);
    expect(drawn.line).toBe("");
    expect(drawn.last).toEqual({ x: 0, y: 20 });
  });
});

describe("ticks", () => {
  it("labels every point when there are few", () => {
    expect(series.ticks([1, 2, 3])).toEqual([0, 1, 2]);
  });

  it("thins a long series to its ends and four between", () => {
    const thinned = series.ticks(Array.from({ length: 30 }, (_, i) => i));
    expect(thinned).toHaveLength(6);
    expect(thinned[0]).toBe(0);
    expect(thinned[thinned.length - 1]).toBe(29);
  });
});

describe("nearest", () => {
  it("selects the bucket the pointer is closest to", () => {
    // A reader aims at a date, not at a two-pixel line: anywhere in the
    // half-bucket either side of a point selects it.
    expect(series.nearest(0, 7)).toBe(0);
    expect(series.nearest(1, 7)).toBe(6);
    expect(series.nearest(0.5, 7)).toBe(3);
    expect(series.nearest(0.08, 7)).toBe(0);
    expect(series.nearest(0.09, 7)).toBe(1);
  });

  it("clamps a pointer that left the plot", () => {
    expect(series.nearest(-3, 7)).toBe(0);
    expect(series.nearest(9, 7)).toBe(6);
  });

  it("has one answer for a series of one, and for none", () => {
    expect(series.nearest(0.7, 1)).toBe(0);
    expect(series.nearest(0.7, 0)).toBe(0);
  });
});

describe("total and busiest", () => {
  it("sums a series for the heading it appears in", () => {
    expect(series.total([row("1", 5, 50), row("2", 7, 70)])).toEqual({
      count: 12,
      bytes: 120,
    });
  });

  it("names the busiest bucket, and nothing for none", () => {
    const drawn = series.points([row("1780000000", 5), row("1780086400", 9)]);
    expect(series.busiest(drawn)?.count).toBe(9);
    expect(series.busiest([])).toBeUndefined();
  });
});
