import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import {
  color,
  dims,
  font,
  leading,
  space,
  text,
  tracking,
  weight,
} from "../../styles/tokens.stylex.ts";

// The console's vocabulary of surfaces. Information sits on the ground
// with hairlines dividing it rather than inside stacked cards: the
// screens are dense, and a card around every group would spend the
// reader's attention on the boxes.

const styles = stylex.create({
  panel: {
    backgroundColor: color.ink2,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.line,
    padding: space.s6,
    display: "flex",
    flexDirection: "column",
    gap: space.s4,
  },
  panelHead: {
    display: "flex",
    alignItems: "baseline",
    justifyContent: "space-between",
    gap: space.s4,
  },
  panelTitle: {
    fontFamily: font.ui,
    fontSize: text.body,
    lineHeight: leading.body,
    fontWeight: weight.bold,
    letterSpacing: tracking.tight,
    margin: 0,
  },
  panelAside: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.muted,
  },
  label: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    letterSpacing: tracking.label,
    textTransform: "uppercase",
    color: color.muted,
    display: "block",
  },
  // The hero number. One per screen, and its scale is the whole reason
  // a reader knows which number the screen is about.
  hero: {
    fontFamily: font.ui,
    fontSize: text.h1,
    lineHeight: leading.h1,
    letterSpacing: tracking.tight,
    fontWeight: weight.bold,
    color: color.text,
    margin: 0,
  },
  intro: { display: "flex", flexDirection: "column", gap: space.s4 },
  // The overview's third block: a wide chart beside a narrower list, in
  // the proportion the design draws them (800 to 488).
  row: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 800fr) minmax(0, 488fr)",
    gap: space.s6,
    // why: stretch, which is the grid default and was overridden to
    // start. Two panels side by side at different heights read as two
    // unrelated things that happen to be adjacent; the design draws them
    // the same height because they are one row of one screen.
    alignItems: "stretch",
    "@media (max-width: 1100px)": { gridTemplateColumns: "minmax(0, 1fr)" },
  },
  prose: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.muted,
    margin: 0,
    // why: 88ch. A comfortable measure is about 65 characters in a
    // proportional face; in a monospaced one every character is an em
    // wide, so the same count is a much narrower column than it looks.
    maxWidth: "88ch",
  },
  // A block of figures divided by hairlines. One grid rather than a row
  // per four, because two grids stacked with a gap between them leave the
  // vertical rules stopping short of the block's bottom edge and the
  // lattice reads as broken.
  tiles: {
    display: "grid",
    gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
    borderTopWidth: "1px",
    borderTopStyle: "solid",
    borderTopColor: color.line,
  },
  tile: {
    paddingBlock: space.s4,
    paddingInline: space.s6,
    borderLeftWidth: "1px",
    borderLeftStyle: "solid",
    borderLeftColor: color.line,
    display: "flex",
    flexDirection: "column",
    gap: space.s2,
  },
  // The first column carries no rule and no inset: the block's left edge
  // is the panel's, not a divider.
  tileFirst: { borderLeftWidth: 0, paddingInline: 0 },
  // Every row after the first draws its own top rule, so the lattice is
  // continuous however many figures a screen has.
  tileWrapped: {
    borderTopWidth: "1px",
    borderTopStyle: "solid",
    borderTopColor: color.line,
  },
  tileValue: {
    fontFamily: font.ui,
    fontSize: text.h3,
    lineHeight: leading.h3,
    letterSpacing: tracking.tight,
    color: color.text,
  },
  signal: { color: color.signal },
  muted: { color: color.muted },
  dim: { color: color.lineStrong },
});

export const Panel = ({
  title,
  aside,
  children,
}: {
  title?: string;
  aside?: ReactNode;
  children: ReactNode;
}) => (
  <section {...stylex.props(styles.panel)}>
    {title === undefined ? null : (
      <header {...stylex.props(styles.panelHead)}>
        <h2 {...stylex.props(styles.panelTitle)}>{title}</h2>
        {aside === undefined ? null : (
          <span {...stylex.props(styles.panelAside)}>{aside}</span>
        )}
      </header>
    )}
    {children}
  </section>
);

export const Label = ({ children }: { children: ReactNode }) => (
  <span {...stylex.props(styles.label)}>{children}</span>
);

export const Hero = ({ children }: { children: ReactNode }) => (
  <p {...stylex.props(styles.hero)}>{children}</p>
);

export const Prose = ({ children }: { children: ReactNode }) => (
  <p {...stylex.props(styles.prose)}>{children}</p>
);

/// A screen's opening: a label, and the thing it labels, with air between
/// them. Without the gap the two sit on adjacent baselines and the label
/// reads as the paragraph's first line.
export const Intro = ({ children }: { children: ReactNode }) => (
  <div {...stylex.props(styles.intro)}>{children}</div>
);

/// Two panels side by side, collapsing to one column when the window is
/// too narrow to give the chart room to be a chart.
export const Row = ({ children }: { children: ReactNode }) => (
  <div {...stylex.props(styles.row)}>{children}</div>
);

export type Tile = {
  readonly label: string;
  readonly value: string;
  readonly tone?: "signal" | "muted";
};

/** How many figures sit on one row before the block wraps. */
const COLUMNS = 4;

export const Tiles = ({ tiles }: { tiles: readonly Tile[] }) => (
  <div {...stylex.props(styles.tiles)}>
    {tiles.map((tile, index) => (
      <div
        key={tile.label}
        {...stylex.props(
          styles.tile,
          index % COLUMNS === 0 && styles.tileFirst,
          index >= COLUMNS && styles.tileWrapped,
        )}
      >
        <Label>{tile.label}</Label>
        <span
          {...stylex.props(
            styles.tileValue,
            tile.tone === "signal" && styles.signal,
            tile.tone === "muted" && styles.muted,
          )}
        >
          {tile.value}
        </span>
      </div>
    ))}
  </div>
);

export { styles as primitiveStyles, dims };
