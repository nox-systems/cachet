import { useQuery } from "@tanstack/react-query";

import * as api from "../api/client.ts";
import { Bars } from "../components/ui/bars.tsx";
import { Chart } from "../components/ui/chart.tsx";
import {
  Hero,
  Intro,
  Label,
  Panel,
  Prose,
  Row,
  Tiles,
} from "../components/ui/primitives.tsx";
import {
  CountersUnavailable,
  Failed,
  LoadingScreen,
  NoRunsYet,
} from "../components/ui/states.tsx";
import * as format from "../lib/format.ts";
import * as series from "../lib/series.ts";

// What the deployment holds, and what put it there. The hero is the
// inventory the last collection counted, because that is the one number
// that answers "how big is this cache" without qualification.

export const Overview = () => {
  const stats = useQuery({
    queryKey: ["stats"],
    queryFn: api.getStats,
    retry: false,
  });
  const runs = useQuery({
    queryKey: ["gc-runs"],
    queryFn: api.listGcRuns,
    retry: false,
  });
  const reads = useQuery({
    queryKey: ["events", "reads", "day", "week"],
    queryFn: () =>
      api.getEvents({ subject: "reads", by: "day", window: "week" }),
    retry: false,
  });
  const writes = useQuery({
    queryKey: ["events", "writes", "repository", "week"],
    queryFn: () =>
      api.getEvents({ subject: "writes", by: "repository", window: "week" }),
    retry: false,
  });

  if (stats.isPending || runs.isPending) return <LoadingScreen />;

  const failure = stats.error;
  if (failure instanceof api.ApiError && failure.absent) {
    return <NoRunsYet />;
  }
  if (failure !== null) {
    return <Failed message={String(failure)} />;
  }

  const report = stats.data;
  const readRows = reads.data?.rows ?? [];
  const writeRows = [...(writes.data?.rows ?? [])].sort(
    (left, right) => right.count - left.count,
  );
  const readTotals = series.total(readRows);
  const writeTotals = series.total(writeRows);
  const countersFailed =
    reads.error instanceof api.ApiError && reads.error.unavailable;

  return (
    <>
      <Intro>
        <div>
          <Label>Paths in the cache</Label>
          <Hero>{format.count(report?.inventoryPaths ?? 0)}</Hero>
        </div>
        <Prose>
          Counted by the last collection, which finished{" "}
          {report === undefined ? "" : format.stamp(report.finishedAtMs)} UTC.
        </Prose>
      </Intro>

      <Tiles
        tiles={[
          {
            label: "Freed by last GC",
            value: format.bytes(report?.bytesFreed ?? 0),
          },
          {
            label: "Kept reports",
            value: format.count(runs.data?.runs.length ?? 0),
          },
          {
            label: "Reads this week",
            value: countersFailed ? "—" : format.count(readTotals.count),
            ...(countersFailed ? { tone: "muted" as const } : {}),
          },
          {
            label: "Writes this week",
            value: countersFailed ? "—" : format.count(writeTotals.count),
            ...(countersFailed ? { tone: "muted" as const } : {}),
          },
        ]}
      />

      {countersFailed ? (
        <Panel title="Traffic">
          <CountersUnavailable />
        </Panel>
      ) : (
        <Row>
          <Panel
            title="Reads per day, past 7 days"
            aside={`${format.count(readTotals.count)} reads · ${format.bytes(
              readTotals.bytes,
            )} served`}
          >
            <Chart points={series.points(readRows)} label={format.day} />
          </Panel>

          <Panel
            title="Pushed this week"
            aside={`${format.count(writeTotals.count)} writes · ${format.bytes(
              writeTotals.bytes,
            )}`}
          >
            {writeRows.length === 0 ? (
              <Prose>Nothing was pushed this week.</Prose>
            ) : (
              <Bars
                rows={writeRows.slice(0, 6).map((row) => ({
                  key: row.dimension,
                  name: row.dimension === "" ? "unattributed" : row.dimension,
                  value: row.count,
                  figure: format.count(row.count),
                  aside: format.bytes(row.bytes),
                }))}
              />
            )}
          </Panel>
        </Row>
      )}
    </>
  );
};
