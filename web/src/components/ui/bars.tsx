import * as stylex from "@stylexjs/stylex";

import {
  color,
  font,
  leading,
  space,
  text,
  weight,
} from "../../styles/tokens.stylex.ts";

// A ranked list with a bar behind each row: the shape both "reads by
// outcome" and "pushed this week" take. A table would give the same
// numbers and none of the proportion, and the proportion is the point.

const styles = stylex.create({
  list: { display: "flex", flexDirection: "column" },
  row: {
    display: "grid",
    // Fixed slots rather than gap alone, so the bars start on one lane
    // and the numbers end on one lane however long a name is. The name
    // column is a ceiling rather than a width: in a narrow panel it
    // gives way so the bar keeps enough room to be read as a length.
    gridTemplateColumns: "minmax(8ch, 22ch) minmax(0, 1fr) 8ch 8ch",
    alignItems: "center",
    gap: space.s3,
    paddingBlock: space.s3,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
  },
  name: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.text,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  track: {
    display: "block",
    height: "8px",
    backgroundColor: color.ink3,
    position: "relative",
  },
  fill: {
    display: "block",
    height: "100%",
    backgroundColor: color.text,
    transitionProperty: "width",
    transitionDuration: "420ms",
    transitionTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
  },
  fillSignal: { backgroundColor: color.signal },
  figure: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    fontWeight: weight.bold,
    color: color.text,
    textAlign: "right",
  },
  figureSignal: { color: color.signal },
  aside: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.muted,
    textAlign: "right",
  },
});

export type BarRow = {
  readonly key: string;
  readonly name: string;
  readonly value: number;
  readonly figure: string;
  readonly aside: string;
  readonly tone?: "signal";
};

export const Bars = ({ rows }: { rows: readonly BarRow[] }) => {
  // The widest row sets the scale. Sharing one denominator is what makes
  // two bars comparable at a glance, which is the only reason they are
  // bars.
  const peak = rows.reduce((most, row) => Math.max(most, row.value), 0);
  return (
    <div {...stylex.props(styles.list)}>
      {rows.map((row) => (
        <div key={row.key} {...stylex.props(styles.row)}>
          <span {...stylex.props(styles.name)} title={row.name}>
            {row.name}
          </span>
          <span {...stylex.props(styles.track)}>
            <span
              {...stylex.props(
                styles.fill,
                row.tone === "signal" && styles.fillSignal,
              )}
              style={{
                width: peak <= 0 ? "0%" : `${(row.value / peak) * 100}%`,
              }}
            />
          </span>
          <span
            {...stylex.props(
              styles.figure,
              row.tone === "signal" && styles.figureSignal,
            )}
          >
            {row.figure}
          </span>
          <span {...stylex.props(styles.aside)}>{row.aside}</span>
        </div>
      ))}
    </div>
  );
};
