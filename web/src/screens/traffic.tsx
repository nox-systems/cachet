import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import * as stylex from "@stylexjs/stylex";

import * as api from "../api/client.ts";
import type { Subject, Window } from "../api/schema.ts";
import { Bars } from "../components/ui/bars.tsx";
import { Chart } from "../components/ui/chart.tsx";
import { Panel, Prose } from "../components/ui/primitives.tsx";
import {
  CountersUnavailable,
  Failed,
  LoadingScreen,
} from "../components/ui/states.tsx";
import { Segmented } from "../components/ui/segmented.tsx";
import * as format from "../lib/format.ts";
import * as series from "../lib/series.ts";
import { space } from "../styles/tokens.stylex.ts";

// Every read, write, and probe the deployment counted, over time and by
// outcome. The two controls are the two the query surface offers, and
// they live in the URL so a view is a link someone can send.

const styles = stylex.create({
  controls: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: space.s4,
  },
});

const SUBJECTS = [
  { value: "reads", label: "Reads" },
  { value: "writes", label: "Writes" },
  { value: "probes", label: "Probes" },
] as const satisfies readonly { value: Subject; label: string }[];

const WINDOWS = [
  { value: "day", label: "Day" },
  { value: "week", label: "Week" },
  { value: "month", label: "Month" },
] as const satisfies readonly { value: Window; label: string }[];

/** The bucket a window is drawn at. Hourly rows only fit inside a day;
 *  the worker refuses the other pairs, so the console never asks. */
export const bucketFor = (window: Window) =>
  window === "day" ? ("hour" as const) : ("day" as const);

export type TrafficSearch = { subject: Subject; window: Window };

export const Traffic = () => {
  const search = useSearch({ from: "/traffic" });
  const navigate = useNavigate({ from: "/traffic" });
  const { subject, window } = search;
  const by = bucketFor(window);

  const line = useQuery({
    queryKey: ["events", subject, by, window],
    queryFn: () => api.getEvents({ subject, by, window }),
    retry: false,
  });
  const outcomes = useQuery({
    queryKey: ["events", subject, "outcome", window],
    queryFn: () => api.getEvents({ subject, by: "outcome", window }),
    retry: false,
  });

  if (line.isPending) return <LoadingScreen />;
  if (line.error instanceof api.ApiError && line.error.unavailable) {
    return <CountersUnavailable />;
  }
  if (line.error !== null) return <Failed message={String(line.error)} />;

  const points = series.points(line.data?.rows ?? []);
  const totals = series.total(line.data?.rows ?? []);
  const rows = [...(outcomes.data?.rows ?? [])].sort(
    (left, right) => right.count - left.count,
  );
  const served = rows
    .filter(
      (row) => row.dimension === "edge_hit" || row.dimension === "bucket_hit",
    )
    .reduce((sum, row) => sum + row.count, 0);
  const outcomeTotal = rows.reduce((sum, row) => sum + row.count, 0);
  const noun = subject === "probes" ? "probes" : subject;

  return (
    <>
      <div {...stylex.props(styles.controls)}>
        <Segmented
          label="What to count"
          options={SUBJECTS}
          value={subject}
          onChange={(next) =>
            void navigate({
              search: (old: TrafficSearch) => ({ ...old, subject: next }),
            })
          }
        />
        <Segmented
          label="How far back"
          options={WINDOWS}
          value={window}
          onChange={(next) =>
            void navigate({
              search: (old: TrafficSearch) => ({ ...old, window: next }),
            })
          }
        />
      </div>

      <Panel
        title={`${format.kindLabel(noun) === noun ? capitalize(noun) : capitalize(noun)} per ${by}, past ${windowLabel(window)}`}
        aside={`${format.count(totals.count)} ${noun} · ${format.bytes(
          totals.bytes,
        )}`}
      >
        <Chart
          points={points}
          label={by === "hour" ? format.hour : format.day}
        />
      </Panel>

      <Panel
        title={`${capitalize(noun)} by outcome`}
        aside={
          subject === "reads" && outcomeTotal > 0
            ? `${format.percent(served, outcomeTotal)} cache hit`
            : undefined
        }
      >
        {rows.length === 0 ? (
          <Prose>Nothing counted in this window yet.</Prose>
        ) : (
          <Bars
            rows={rows.map((row) => ({
              key: row.dimension,
              name: format.outcomeLabel(row.dimension),
              value: row.count,
              figure: format.count(row.count),
              aside: row.bytes > 0 ? format.bytes(row.bytes) : "",
              ...(format.isRefusal(row.dimension)
                ? { tone: "signal" as const }
                : {}),
            }))}
          />
        )}
      </Panel>
    </>
  );
};

const capitalize = (value: string) =>
  `${value.slice(0, 1).toUpperCase()}${value.slice(1)}`;

const windowLabel = (window: Window) =>
  ({ day: "24 hours", week: "7 days", month: "30 days" })[window];
