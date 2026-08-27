import * as stylex from "@stylexjs/stylex";

import type { PublicConfig } from "../api/schema.ts";
import { Mark } from "../components/icons.tsx";
import {
  color,
  font,
  leading,
  space,
  text,
  tracking,
  weight,
} from "../styles/tokens.stylex.ts";

// The first screen anyone sees, and the only one that renders without a
// credential. It says which deployment this is before asking for one,
// because a person with two of these open needs to know which they are
// signing into.

const styles = stylex.create({
  page: {
    minHeight: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    padding: space.s8,
    backgroundColor: color.ink,
  },
  card: {
    display: "flex",
    flexDirection: "column",
    gap: space.s6,
    maxWidth: "56ch",
    width: "100%",
  },
  mark: { color: color.text },
  title: {
    fontFamily: font.display,
    fontSize: text.h2,
    lineHeight: leading.h2,
    letterSpacing: tracking.tight,
    fontWeight: weight.bold,
    color: color.text,
    margin: 0,
  },
  host: {
    fontFamily: font.mono,
    fontSize: text.spec,
    lineHeight: leading.spec,
    color: color.muted,
  },
  prose: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.muted,
    margin: 0,
  },
  action: {
    fontFamily: font.ui,
    fontSize: text.body,
    lineHeight: leading.body,
    letterSpacing: tracking.tight,
    color: color.ink,
    backgroundColor: color.text,
    borderWidth: 0,
    paddingBlock: space.s3,
    paddingInline: space.s6,
    cursor: "pointer",
    textDecoration: "none",
    alignSelf: "flex-start",
    transitionProperty: "background-color",
    transitionDuration: "140ms",
    ":hover": { backgroundColor: color.paper },
  },
  refused: { color: color.signal },
});

export const SignIn = ({
  config,
  refused,
}: {
  config?: PublicConfig;
  refused?: string;
}) => (
  <div {...stylex.props(styles.page)}>
    <div {...stylex.props(styles.card)}>
      <span {...stylex.props(styles.mark)}>
        <Mark />
      </span>
      <div>
        <h1 {...stylex.props(styles.title)}>
          {config?.deployment ?? "cachet"}
        </h1>
        <span {...stylex.props(styles.host)}>{config?.host ?? ""}</span>
      </div>
      {refused === undefined ? null : (
        <p {...stylex.props(styles.prose, styles.refused)}>{refused}</p>
      )}
      <a href="/_auth/login" {...stylex.props(styles.action)}>
        Sign in with GitHub
      </a>
    </div>
  </div>
);
