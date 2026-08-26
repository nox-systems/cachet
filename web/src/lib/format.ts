// How the console writes numbers. Every screen reads from here, so a
// byte count means the same thing on the overview as it does on a run
// report, and a count that grew past a thousand does not change shape.

/** A count, grouped so the eye can read it at a glance. */
export const count = (value: number): string =>
  Math.round(value).toLocaleString("en-US");

/** Bytes, in the units an operator thinks in.
 *
 * Powers of 1024 with the decimal names nix and Cloudflare both print,
 * because those are the numbers a reader will compare this against. One
 * decimal place past a kilobyte: the second one is never the thing
 * anybody is deciding on. */
export const bytes = (value: number): string => {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"] as const;
  let scaled = value;
  let unit = 0;
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024;
    unit += 1;
  }
  return unit === 0
    ? `${Math.round(scaled)} ${units[unit]}`
    : `${scaled.toFixed(1)} ${units[unit]}`;
};

/** A share of a whole, to one decimal place. A denominator of zero is
 *  no share rather than a division by it. */
export const percent = (part: number, whole: number): string =>
  whole <= 0 ? "0%" : `${((part / whole) * 100).toFixed(1)}%`;

/** How many of every hundred, which reads better than a percentage for
 *  a hit rate a person is going to say out loud. */
export const perHundred = (part: number, whole: number): number =>
  whole <= 0 ? 0 : Math.round((part / whole) * 100);

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** A duration, at the coarsest unit that still says something.
 *
 * A collection takes minutes and a countdown takes hours, so the same
 * function has to carry both without printing "0 h 0 m 12 s". */
export const duration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms < 0) return "unknown";
  if (ms < MINUTE) return `${Math.round(ms / 1000)} s`;
  if (ms < HOUR) {
    const minutes = Math.floor(ms / MINUTE);
    const seconds = Math.round((ms % MINUTE) / 1000);
    return seconds === 0 ? `${minutes} min` : `${minutes} min ${seconds} s`;
  }
  const hours = Math.floor(ms / HOUR);
  const minutes = Math.floor((ms % HOUR) / MINUTE);
  return `${hours} h ${minutes} m`;
};

/** A UTC clock, which is what a deployment's header shows. */
export const clock = (ms: number): string => {
  const at = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(at.getUTCHours())}:${pad(at.getUTCMinutes())}:${pad(
    at.getUTCSeconds(),
  )}`;
};

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

/** A day, as an axis tick reads it. */
export const day = (ms: number): string => {
  const at = new Date(ms);
  return `${at.getUTCDate()} ${MONTHS[at.getUTCMonth()] ?? ""}`;
};

/** A day and a time, for a row in a table of runs. */
export const stamp = (ms: number): string => {
  const at = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${day(ms)} ${pad(at.getUTCHours())}:${pad(at.getUTCMinutes())}`;
};

/** A full date, for the one place a person needs to read a deadline. */
export const date = (ms: number): string => {
  const at = new Date(ms);
  return `${at.getUTCDate()} ${MONTHS[at.getUTCMonth()] ?? ""} ${at.getUTCFullYear()}`;
};

/** An hour, as an axis tick reads it. */
export const hour = (ms: number): string =>
  `${String(new Date(ms).getUTCHours()).padStart(2, "0")}:00`;

/** The words the console uses for a counter dimension's value.
 *
 * The wire vocabulary is written for a query; these are written for a
 * person reading a table. An unknown value passes through as itself,
 * because a deployment newer than this console should show its own word
 * rather than a blank. */
export const outcomeLabel = (value: string): string =>
  ({
    edge_hit: "Served from the edge",
    bucket_hit: "Served from the cache",
    miss: "Missing from the cache",
    stored: "Stored",
    answered: "Answered",
  })[value] ?? refusalLabel(value);

const refusalLabel = (value: string): string =>
  /^\d{3}$/.test(value) ? `Refused, ${value}` : value;

/** Whether a counter row is a refusal, which is the only thing on a
 *  screen the signal colour marks. */
export const isRefusal = (value: string): boolean => /^\d{3}$/.test(value);

export const actorLabel = (value: string): string =>
  ({
    ci: "CI",
    laptop: "Laptops",
    browser: "This console",
    anonymous: "Unauthenticated",
  })[value] ?? value;

export const kindLabel = (value: string): string =>
  ({
    narinfo: "Narinfos",
    nar: "NARs",
    part: "Upload parts",
    begin: "Uploads opened",
    complete: "Uploads completed",
    abort: "Uploads abandoned",
    probe: "Probes",
    unknown: "Unclassified",
  })[value] ?? value;
