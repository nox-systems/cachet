import { useQuery } from "@tanstack/react-query";

import * as api from "../api/client.ts";
import { Bars } from "../components/ui/bars.tsx";
import { Chart } from "../components/ui/chart.tsx";
import {
  Hero,
  Label,
  Panel,
  Prose,
  Tiles,
} from "../components/ui/primitives.tsx";
import {
  CountersUnavailable,
  Failed,
  LoadingScreen,
} from "../components/ui/states.tsx";
import * as format from "../lib/format.ts";
import * as series from "../lib/series.ts";

// What the cache is doing for people, as opposed to for CI. The whole
// screen is one cross-filtered question: reads whose actor is a laptop.
// Without the filter the same numbers would need a query per day.

export const Laptops = () => {
  const daily = useQuery({
    queryKey: ["events", "reads", "day", "week", "laptop"],
    queryFn: () =>
      api.getEvents({
        subject: "reads",
        by: "day",
        window: "week",
        actor: "laptop",
      }),
    retry: false,
  });
  const outcomes = useQuery({
    queryKey: ["events", "reads", "outcome", "week", "laptop"],
    queryFn: () =>
      api.getEvents({
        subject: "reads",
        by: "outcome",
        window: "week",
        actor: "laptop",
      }),
    retry: false,
  });
  const everyone = useQuery({
    queryKey: ["events", "reads", "actor", "week"],
    queryFn: () =>
      api.getEvents({ subject: "reads", by: "actor", window: "week" }),
    retry: false,
  });
  const today = useQuery({
    queryKey: ["events", "reads", "outcome", "day", "laptop"],
    queryFn: () =>
      api.getEvents({
        subject: "reads",
        by: "outcome",
        window: "day",
        actor: "laptop",
      }),
    retry: false,
  });

  if (daily.isPending) return <LoadingScreen />;
  if (daily.error instanceof api.ApiError && daily.error.unavailable) {
    return <CountersUnavailable />;
  }
  if (daily.error !== null) return <Failed message={String(daily.error)} />;

  const points = series.points(daily.data?.rows ?? []);
  const totals = series.total(daily.data?.rows ?? []);
  const busiest = series.busiest(points);
  const allReads = (everyone.data?.rows ?? []).reduce(
    (sum, row) => sum + row.count,
    0,
  );
  const todayTotal = (today.data?.rows ?? []).reduce(
    (sum, row) => sum + row.count,
    0,
  );
  const rows = [...(outcomes.data?.rows ?? [])].sort(
    (left, right) => right.count - left.count,
  );

  return (
    <>
      <div>
        <Label>Laptop reads this week</Label>
        <Hero>{format.count(totals.count)}</Hero>
        <Prose>
          Counted from read events whose actor is a laptop token. Laptops can
          only read, so every path they found was pushed by CI. Reads from CI
          and from this console are on the traffic screen.
        </Prose>
      </div>

      <Tiles
        tiles={[
          { label: "Served to laptops", value: format.bytes(totals.bytes) },
          {
            label: "Share of all reads",
            value: format.percent(totals.count, allReads),
          },
          {
            label: "Busiest day",
            value:
              busiest === undefined || busiest.count === 0
                ? "—"
                : `${format.day(busiest.atMs)}, ${format.count(busiest.count)}`,
            ...(busiest === undefined || busiest.count === 0
              ? { tone: "muted" as const }
              : {}),
          },
          { label: "Reads today", value: format.count(todayTotal) },
        ]}
      />

      <Panel
        title="Laptop reads per day, past 7 days"
        aside={`${format.count(totals.count)} reads · ${format.bytes(
          totals.bytes,
        )} served`}
      >
        <Chart points={points} label={format.day} />
      </Panel>

      <Panel title="Laptop reads by outcome">
        {rows.length === 0 ? (
          <Prose>
            No laptop has read from this deployment in the past week.
          </Prose>
        ) : (
          <Bars
            rows={rows.map((row) => ({
              key: row.dimension,
              name: format.outcomeLabel(row.dimension),
              value: row.count,
              figure: format.count(row.count),
              aside: format.bytes(row.bytes),
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
