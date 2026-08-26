import { useQuery } from "@tanstack/react-query";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import * as api from "../api/client.ts";
import {
  Intro,
  Label,
  Panel,
  Prose,
  Tiles,
} from "../components/ui/primitives.tsx";
import { Failed, LoadingScreen, NoRunsYet } from "../components/ui/states.tsx";
import * as format from "../lib/format.ts";
import {
  color,
  font,
  leading,
  space,
  text,
  weight,
} from "../styles/tokens.stylex.ts";

// What the collector has been doing. The table is the last few runs and
// the panel below is whichever one is selected, because the question
// "did last night's run work" and the question "what exactly did it
// delete" are asked in that order.

const styles = stylex.create({
  table: { display: "flex", flexDirection: "column" },
  head: {
    display: "grid",
    gridTemplateColumns: "16ch 12ch 10ch 9ch 9ch 1fr",
    gap: space.s4,
    paddingBottom: space.s2,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
  },
  row: {
    display: "grid",
    gridTemplateColumns: "16ch 12ch 10ch 9ch 9ch 1fr",
    gap: space.s4,
    alignItems: "center",
    paddingBlock: space.s3,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
    backgroundColor: "transparent",
    borderInlineWidth: 0,
    borderTopWidth: 0,
    textAlign: "left",
    cursor: "pointer",
    color: "inherit",
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    transitionProperty: "background-color",
    transitionDuration: "140ms",
    ":hover": { backgroundColor: color.ink3 },
  },
  selected: { backgroundColor: color.ink3 },
  stamp: { color: color.text, fontWeight: weight.bold },
  cell: { color: color.text },
  result: { color: color.muted },
  tripped: { color: color.signal },
  key: {
    fontFamily: font.mono,
    fontSize: text.label,
    color: color.muted,
  },
});

export const Collection = () => {
  const [selected, setSelected] = useState<string | undefined>(undefined);
  const runs = useQuery({
    queryKey: ["gc-runs"],
    queryFn: api.listGcRuns,
    retry: false,
  });

  // Newest first: the run a person came here to look at is the last one.
  const runIds = [...(runs.data?.runs ?? [])].reverse().slice(0, 8);
  const showing = selected ?? runIds[0];

  const report = useQuery({
    queryKey: ["gc-run", showing],
    queryFn: () => api.getGcRun(showing as string),
    enabled: showing !== undefined,
    retry: false,
  });

  if (runs.isPending) return <LoadingScreen />;
  if (runs.error !== null) return <Failed message={String(runs.error)} />;
  if (runIds.length === 0) return <NoRunsYet />;

  const detail = report.data;

  return (
    <>
      <Intro>
        <Label>Collection</Label>
        <Prose>
          The collector runs on a cron and reports afterwards. Paths stay for
          their grace window after the last lease that named them ends, and a
          run stops itself before deleting more than a quarter of the cache.
        </Prose>
      </Intro>

      <Panel
        title="Recent runs"
        aside={`${runIds.length} of ${runs.data?.runs.length ?? 0} kept reports`}
      >
        <div {...stylex.props(styles.table)}>
          <div {...stylex.props(styles.head)}>
            <Label>Finished</Label>
            <Label>Duration</Label>
            <Label>Paths</Label>
            <Label>Deleted</Label>
            <Label>Freed</Label>
            <Label>Result</Label>
          </div>
          {runIds.map((runId) => (
            <RunRow
              key={runId}
              runId={runId}
              selected={runId === showing}
              onSelect={() => setSelected(runId)}
            />
          ))}
        </div>
      </Panel>

      {detail === undefined ? null : (
        <Panel
          title={`Run on ${format.stamp(detail.finishedAtMs)} UTC`}
          aside={
            <span {...stylex.props(styles.key)}>
              gc-reports/{detail.runId}.json
            </span>
          }
        >
          <Tiles
            tiles={[
              {
                label: "Inventory",
                value: `${format.count(detail.inventoryPaths)} paths`,
              },
              {
                label: "Active leases",
                value: format.count(detail.activeLeases),
              },
              { label: "Marked live", value: format.count(detail.markedPaths) },
              {
                label: "Candidates",
                value: format.count(
                  Math.max(0, detail.inventoryPaths - detail.markedPaths),
                ),
              },
            ]}
          />
          <Tiles
            tiles={[
              {
                label: "Narinfos deleted",
                value: format.count(detail.narinfosDeleted),
              },
              {
                label: "NARs deleted",
                value: format.count(detail.narsDeleted),
              },
              {
                label: "Uploads aborted",
                value: format.count(detail.uploadsAborted),
              },
              { label: "Freed", value: format.bytes(detail.bytesFreed) },
            ]}
          />
          {detail.gate === undefined || detail.gate === null ? null : (
            <Prose>
              This collection stopped at its <strong>{detail.gate}</strong> gate
              and deleted nothing. A tripped gate is the collector refusing to
              proceed on evidence it did not trust, which is the behavior that
              keeps a bug from emptying the cache.
            </Prose>
          )}
        </Panel>
      )}
    </>
  );
};

/** One row, which reads its own report so the table shows real figures
 *  rather than a run id. The list route answers ids alone. */
const RunRow = ({
  runId,
  selected,
  onSelect,
}: {
  runId: string;
  selected: boolean;
  onSelect: () => void;
}) => {
  const report = useQuery({
    queryKey: ["gc-run", runId],
    queryFn: () => api.getGcRun(runId),
    retry: false,
  });
  const detail = report.data;

  return (
    <button
      type="button"
      onClick={onSelect}
      {...stylex.props(styles.row, selected && styles.selected)}
    >
      <span {...stylex.props(styles.stamp)}>
        {detail === undefined ? "…" : format.stamp(detail.finishedAtMs)}
      </span>
      <span {...stylex.props(styles.cell)}>
        {detail === undefined
          ? "—"
          : format.duration(detail.finishedAtMs - detail.startedAtMs)}
      </span>
      <span {...stylex.props(styles.cell)}>
        {detail === undefined ? "—" : format.count(detail.inventoryPaths)}
      </span>
      <span {...stylex.props(styles.cell)}>
        {detail === undefined ? "—" : format.count(detail.narinfosDeleted)}
      </span>
      <span {...stylex.props(styles.cell)}>
        {detail === undefined ? "—" : format.bytes(detail.bytesFreed)}
      </span>
      <span
        {...stylex.props(styles.result, detail?.gate != null && styles.tripped)}
      >
        {detail === undefined
          ? "reading"
          : detail.gate == null
            ? "Completed"
            : `Stopped, ${detail.gate.replaceAll("_", " ")}`}
      </span>
    </button>
  );
};
