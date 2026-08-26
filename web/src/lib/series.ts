import type { StatsRow } from "../api/schema.ts";

// Turning a counter answer into something a chart can draw. All of it is
// arithmetic over the rows the worker already gap-filled, so the console
// lane tests it without a browser.

/** One point on a line: when, how much, and how many bytes. */
export type Point = {
  readonly atMs: number;
  readonly count: number;
  readonly bytes: number;
};

/** Read a bucketed answer as points in time.
 *
 * The worker answers a series with each row's dimension the bucket's
 * first instant in epoch seconds (docs/DEPLOY.md), so this is the one
 * place that knows to multiply. A row whose dimension is not a number is
 * dropped rather than plotted at zero: it means the answer was a
 * dimension list and the caller asked the wrong question. */
export const points = (rows: readonly StatsRow[]): Point[] =>
  rows
    .map((row) => ({
      atMs: Number(row.dimension) * 1000,
      count: row.count,
      bytes: row.bytes,
    }))
    .filter((point) => Number.isFinite(point.atMs));

/** The axis a line is drawn against.
 *
 * The top is rounded up to something a person would choose, so the
 * gridline labels read as round numbers rather than as the maximum of
 * the data. A flat series of zeros still gets a scale, because a chart
 * that collapses to a single line at the top is a chart claiming the
 * traffic was constant rather than absent. */
export const scale = (values: readonly number[]): { top: number } => {
  const peak = values.reduce((most, value) => Math.max(most, value), 0);
  if (peak <= 0) return { top: 1 };
  const magnitude = 10 ** Math.floor(Math.log10(peak));
  for (const step of [1, 1.25, 1.5, 2, 2.5, 3, 4, 5, 7.5, 10]) {
    if (peak <= step * magnitude) return { top: step * magnitude };
  }
  return { top: 10 * magnitude };
};

/** The SVG coordinates of a series inside a box.
 *
 * Returned as strings ready for `points` and `d` attributes: the caller
 * draws, and everything numeric happened here where it can be tested. */
export const path = (
  values: readonly number[],
  width: number,
  height: number,
  top: number,
): { line: string; area: string; last: { x: number; y: number } } => {
  if (values.length === 0) {
    return { line: "", area: "", last: { x: 0, y: height } };
  }
  const step = values.length === 1 ? 0 : width / (values.length - 1);
  const coords = values.map((value, index) => ({
    x: values.length === 1 ? width : index * step,
    // why: clamped. A bucket past the axis top would draw above the
    // chart and over the panel's own heading.
    y: height - Math.min(1, Math.max(0, value / top)) * height,
  }));
  const line = coords
    .map(({ x, y }) => `${x.toFixed(2)},${y.toFixed(2)}`)
    .join(" ");
  const first = coords[0] ?? { x: 0, y: height };
  const last = coords[coords.length - 1] ?? { x: width, y: height };
  const area = `M ${first.x.toFixed(2)},${height} L ${line.replaceAll(
    " ",
    " L ",
  )} L ${last.x.toFixed(2)},${height} Z`;
  return { line, area, last };
};

/** The ticks an axis shows.
 *
 * At most six, evenly spaced, always including the first and the last,
 * because a 30-day series with 30 labels is a smear. */
export const ticks = <T>(items: readonly T[], most = 6): number[] => {
  if (items.length <= most) return items.map((_, index) => index);
  const stride = (items.length - 1) / (most - 1);
  return Array.from({ length: most }, (_, index) => Math.round(index * stride));
};

/** Which bucket a pointer sitting `fraction` of the way across the plot
 *  is nearest to.
 *
 * Nearest rather than containing, so a reader aims at a date rather than
 * at a two-pixel line: anywhere in the half-bucket either side of a
 * point selects it, and both ends stay reachable from the plot's very
 * edges. A fraction outside the plot clamps rather than wrapping. */
export const nearest = (fraction: number, count: number): number => {
  if (count <= 1) return 0;
  const clamped = Math.min(1, Math.max(0, fraction));
  return Math.round(clamped * (count - 1));
};

/** A total across a series, which every chart's heading carries. */
export const total = (
  rows: readonly StatsRow[],
): { count: number; bytes: number } =>
  rows.reduce(
    (sum, row) => ({
      count: sum.count + row.count,
      bytes: sum.bytes + row.bytes,
    }),
    { count: 0, bytes: 0 },
  );

/** The busiest point in a series, which the laptops screen names. */
export const busiest = (series: readonly Point[]): Point | undefined =>
  series.reduce<Point | undefined>(
    (most, point) =>
      most === undefined || point.count > most.count ? point : most,
    undefined,
  );
