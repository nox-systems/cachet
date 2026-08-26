import * as stylex from "@stylexjs/stylex";

import * as series from "../../lib/series.ts";
import {
  color,
  font,
  leading,
  space,
  text,
} from "../../styles/tokens.stylex.ts";
import { count } from "../../lib/format.ts";

// One line over time. Hand-drawn SVG rather than a charting library: the
// shape is an area, a line, two gridlines and a dot, and a library would
// bring a renderer, a theme system, and an axis engine to draw them.

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
  plot: { flex: 1, minWidth: 0 },
  svg: { display: "block", width: "100%", height: `${HEIGHT}px` },
  grid: { stroke: color.line, strokeWidth: 1 },
  area: { fill: color.ink3 },
  line: {
    fill: "none",
    stroke: color.text,
    strokeWidth: 2,
    strokeLinejoin: "round",
  },
  head: { fill: color.signal },
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
  empty: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.muted,
  },
});

export const Chart = ({
  points,
  label,
}: {
  points: readonly series.Point[];
  label: (atMs: number) => string;
}) => {
  const values = points.map((point) => point.count);
  const { top } = series.scale(values);
  const drawn = series.path(values, WIDTH, HEIGHT, top);
  const tickIndexes = series.ticks(points);

  return (
    <div {...stylex.props(styles.frame)}>
      <div {...stylex.props(styles.axis)}>
        <span {...stylex.props(styles.axisLabel)}>{count(top)}</span>
        <span {...stylex.props(styles.axisLabel)}>{count(top / 2)}</span>
        <span {...stylex.props(styles.axisLabel)}>0</span>
      </div>
      <div {...stylex.props(styles.plot)}>
        <svg
          {...stylex.props(styles.svg)}
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          preserveAspectRatio="none"
          role="img"
          aria-label={`${count(series.total(points.map((point) => ({ dimension: "", count: point.count, bytes: point.bytes }))).count)} over ${points.length} buckets`}
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
            // The live end. One dot in the one intense colour, marking
            // the bucket the deployment is in right now.
            <circle
              {...stylex.props(styles.head)}
              cx={drawn.last.x}
              cy={drawn.last.y}
              r="4"
            />
          )}
        </svg>
        <div {...stylex.props(styles.ticks)}>
          {points.length === 0 ? (
            <span {...stylex.props(styles.empty)}>
              Nothing counted in this window yet.
            </span>
          ) : (
            tickIndexes.map((index) => (
              <span key={index} {...stylex.props(styles.tick)}>
                {label(points[index]?.atMs ?? 0)}
              </span>
            ))
          )}
        </div>
      </div>
    </div>
  );
};
