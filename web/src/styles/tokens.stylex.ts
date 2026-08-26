import * as stylex from "@stylexjs/stylex";

// The console's design tokens, carried over from the Paper file. This is
// the only module that exports vars: a value that belongs to the scale
// lives here, and a component that wants one imports it rather than
// writing a literal.

export const color = stylex.defineVars({
  // The ground and the hairlines it is divided by. Two inks rather than
  // one because a panel has to read as a panel without a border heavy
  // enough to draw the eye.
  ink: "#0A0A0A",
  ink2: "#0F0F0F",
  ink3: "#161616",
  line: "#262626",
  lineStrong: "#404040",
  // Text. muted carries labels and secondary numbers; anything a reader
  // has to actually read is text.
  text: "#E5E5E5",
  muted: "#A3A3A3",
  paper: "#FFFFFF",
  // The one intense colour. It marks a refusal, a tripped gate, and the
  // live end of a series, and it is used nowhere else.
  signal: "#E4002B",
  amber: "#FFB000",
});

export const font = stylex.defineConsts({
  // Licensed faces first, shipped faces behind them. A deployment that
  // points CACHET_DEPLOY_FONT_CSS at a stylesheet serving the first name
  // gets it; every other deployment renders the second (ADR 0014).
  ui: '"Berkeley Mono", "Geist Mono", ui-monospace, SFMono-Regular, monospace',
  display:
    '"Neue Haas Grotesk Display Pro", "Geist", system-ui, -apple-system, sans-serif',
  mono: '"Berkeley Mono", "Geist Mono", ui-monospace, SFMono-Regular, monospace',
});

export const text = stylex.defineConsts({
  label: "11px",
  spec: "13px",
  body: "16px",
  path: "20px",
  h3: "32px",
  h2: "48px",
  h1: "72px",
});

export const leading = stylex.defineConsts({
  label: "14px",
  spec: "18px",
  body: "24px",
  h3: "38px",
  h2: "52px",
  h1: "72px",
});

export const weight = stylex.defineConsts({
  light: "300",
  regular: "400",
  bold: "700",
});

export const tracking = stylex.defineConsts({
  tight: "-0.02em",
  normal: "0em",
  label: "0.08em",
});

export const space = stylex.defineConsts({
  s1: "4px",
  s2: "8px",
  s3: "12px",
  s4: "16px",
  s6: "24px",
  s8: "32px",
  s12: "48px",
  s16: "64px",
  s24: "96px",
});

export const dims = stylex.defineConsts({
  rail: "64px",
  hud: "28px",
  header: "64px",
  statusBar: "36px",
  page: "1440px",
});
