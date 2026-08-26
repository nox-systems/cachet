import * as stylex from "@stylexjs/stylex";
import { useCallback, useRef, useState } from "react";

import { bytes as formatBytes, count } from "../../lib/format.ts";
import * as series from "../../lib/series.ts";
import {
  color,
  font,
  leading,
  space,
  text,
  weight,
} from "../../styles/tokens.stylex.ts";

// One line over time, with the hover layer that makes it readable. Plain
// SVG rather than a charting library: the shape is an area, a line, two
// gridlines and a dot, and the two libraries worth considering are either
// archived (TanStack react-charts) or would bring a second styling system
// with them (shadcn's charts are Recharts plus Tailwind).

const WIDTH = 1168;
const HEIGHT = 200;

const styles = stylex.create({
  frame: { display: "flex", gap: space.s3 },
  axis: {
    width: "84px",
    flexShrink: 0,
    display: "flex",
    flexDirection: "column",
    justifyContent: "space-between",
    alignItems: "flex-end",
    paddingBottom: "1px",
  },
  axisLabel: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    color: color.muted,
  },
  plot: { flex: 1, minWidth: 0, position: "relative" },
  // why: focusable. Everything the pointer reveals has to be reachable
  // from the keyboard, so the plot takes focus and the arrows walk the
  // series; the readout below is a live region so it is spoken as it
  // moves rather than only drawn.
  surface: {
    display: "block",
    width: "100%",
    height: `${HEIGHT}px`,
    outlineWidth: { default: 0, ":focus-visible": "1px" },
    outlineStyle: "solid",
    outlineColor: color.lineStrong,
    cursor: "crosshair",
  },
  grid: { stroke: color.line, strokeWidth: 1 },
  area: { fill: color.ink3 },
  line: {
    fill: "none",
    stroke: color.text,
    strokeWidth: 2,
    strokeLinejoin: "round",
  },
  head: { fill: color.signal },
  crosshair: { stroke: color.lineStrong, strokeWidth: 1 },
  // The point under the pointer, ringed in the surface colour so it reads
  // as lifted off the line rather than as a second dot on it.
  cursorRing: { fill: color.ink2, stroke: color.text, strokeWidth: 2 },
  tip: {
    position: "absolute",
    top: 0,
    pointerEvents: "none",
    backgroundColor: color.ink,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.lineStrong,
    paddingBlock: space.s2,
    paddingInline: space.s3,
    display: "flex",
    flexDirection: "column",
    gap: "2px",
    whiteSpace: "nowrap",
    transform: "translateX(-50%)",
  },
  // Values lead and labels follow: the reader already knows the series
  // and came for the number.
  tipValue: {
    fontFamily: font.ui,
    fontSize: text.body,
    lineHeight: leading.body,
    fontWeight: weight.bold,
    color: color.text,
  },
  tipMeta: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    color: color.muted,
  },
  ticks: {
    display: "flex",
    justifyContent: "space-between",
    paddingTop: space.s2,
  },
  tick: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    color: color.muted,
  },
  tickActive: { color: color.text },
  empty: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.muted,
  },
  reading: {
    position: "absolute",
    width: "1px",
    height: "1px",
    overflow: "hidden",
    clipPath: "inset(50%)",
  },
});

export const Chart = ({
  points,
  label,
  noun = "reads",
}: {
  points: readonly series.Point[];
  label: (atMs: number) => string;
  noun?: string;
}) => {
  const plot = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<number | undefined>(undefined);

  const values = points.map((point) => point.count);
  const { top } = series.scale(values);
  const drawn = series.path(values, WIDTH, HEIGHT, top);
  const tickIndexes = series.ticks(points);
  const step = points.length <= 1 ? 0 : WIDTH / (points.length - 1);

  const track = useCallback(
    (clientX: number) => {
      const box = plot.current?.getBoundingClientRect();
      if (box === undefined || box.width === 0) return;
      setActive(
        series.nearest((clientX - box.left) / box.width, points.length),
      );
    },
    [points.length],
  );

  const move = useCallback(
    (index: number) => {
      if (points.length === 0) return;
      setActive(Math.min(points.length - 1, Math.max(0, index)));
    },
    [points.length],
  );

  const at = active === undefined ? undefined : points[active];
  // The tooltip rides the crosshair and stops short of both edges, so it
  // never hangs off the panel it belongs to.
  const tipPercent =
    active === undefined || points.length <= 1
      ? 50
      : Math.min(94, Math.max(6, (active / (points.length - 1)) * 100));

  return (
    <div {...stylex.props(styles.frame)}>
      <div {...stylex.props(styles.axis)}>
        <span {...stylex.props(styles.axisLabel)}>{count(top)}</span>
        <span {...stylex.props(styles.axisLabel)}>{count(top / 2)}</span>
        <span {...stylex.props(styles.axisLabel)}>0</span>
      </div>
      <div {...stylex.props(styles.plot)} ref={plot}>
        <svg
          {...stylex.props(styles.surface)}
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          tabIndex={points.length === 0 ? -1 : 0}
          aria-label={`${noun} over ${points.length} buckets. Use the arrow keys to read each one.`}
          onPointerMove={(event) => track(event.clientX)}
          onPointerLeave={() => setActive(undefined)}
          onFocus={() => move(points.length - 1)}
          onBlur={() => setActive(undefined)}
          onKeyDown={(event) => {
            const from = active ?? points.length - 1;
            if (event.key === "ArrowLeft") move(from - 1);
            else if (event.key === "ArrowRight") move(from + 1);
            else if (event.key === "Home") move(0);
            else if (event.key === "End") move(points.length - 1);
            else return;
            event.preventDefault();
          }}
        >
          <line
            {...stylex.props(styles.grid)}
            x1="0"
            y1="0.5"
            x2={WIDTH}
            y2="0.5"
          />
          <line
            {...stylex.props(styles.grid)}
            x1="0"
            y1={HEIGHT / 2}
            x2={WIDTH}
            y2={HEIGHT / 2}
          />
          <line
            {...stylex.props(styles.grid)}
            x1="0"
            y1={HEIGHT - 0.5}
            x2={WIDTH}
            y2={HEIGHT - 0.5}
          />
          {drawn.area === "" ? null : (
            <path {...stylex.props(styles.area)} d={drawn.area} />
          )}
          {drawn.line === "" ? null : (
            <polyline {...stylex.props(styles.line)} points={drawn.line} />
          )}
          {points.length === 0 ? null : (
            <circle
              {...stylex.props(styles.head)}
              cx={drawn.last.x}
              cy={drawn.last.y}
              r="4"
            />
          )}
          {active === undefined || at === undefined ? null : (
            <>
              <line
                {...stylex.props(styles.crosshair)}
                x1={active * step}
                y1="0"
                x2={active * step}
                y2={HEIGHT}
              />
              <circle
                {...stylex.props(styles.cursorRing)}
                cx={active * step}
                cy={HEIGHT - Math.min(1, at.count / top) * HEIGHT}
                r="5"
              />
            </>
          )}
        </svg>

        {at === undefined ? null : (
          <div {...stylex.props(styles.tip)} style={{ left: `${tipPercent}%` }}>
            <span {...stylex.props(styles.tipValue)}>
              {count(at.count)} {noun}
            </span>
            <span {...stylex.props(styles.tipMeta)}>
              {label(at.atMs)}
              {at.bytes > 0 ? ` · ${formatBytes(at.bytes)}` : ""}
            </span>
          </div>
        )}

        {/* Spoken as the cursor moves, so the keyboard reader hears what
            the pointer reader sees. */}
        <span {...stylex.props(styles.reading)} aria-live="polite">
          {at === undefined
            ? ""
            : `${label(at.atMs)}: ${count(at.count)} ${noun}`}
        </span>

        <div {...stylex.props(styles.ticks)}>
          {points.length === 0 ? (
            <span {...stylex.props(styles.empty)}>
              Nothing counted in this window yet.
            </span>
          ) : (
            tickIndexes.map((index) => (
              <span
                key={index}
                {...stylex.props(
                  styles.tick,
                  index === active && styles.tickActive,
                )}
              >
                {label(points[index]?.atMs ?? 0)}
              </span>
            ))
          )}
        </div>
      </div>
    </div>
  );
};
