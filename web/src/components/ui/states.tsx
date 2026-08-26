import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import {
  color,
  font,
  leading,
  space,
  text,
  tracking,
  weight,
} from "../../styles/tokens.stylex.ts";

// The states the mockups do not draw, and every one of them is reachable
// on a real deployment: a young one with no collection yet, one counting
// without a token to report with, an org member who is not an admin, and
// the moment before the first answer arrives.

const styles = stylex.create({
  note: {
    display: "flex",
    flexDirection: "column",
    gap: space.s2,
    paddingBlock: space.s8,
    maxWidth: "88ch",
  },
  title: {
    fontFamily: font.ui,
    fontSize: text.body,
    lineHeight: leading.body,
    fontWeight: weight.bold,
    letterSpacing: tracking.tight,
    color: color.text,
    margin: 0,
  },
  body: {
    fontFamily: font.ui,
    fontSize: text.spec,
    lineHeight: leading.body,
    color: color.muted,
    margin: 0,
  },
  code: { color: color.text },
  // Skeletons rather than spinners: the shape of the answer is known
  // before the answer is, and a box that becomes the number reads as the
  // page filling in where a spinner reads as the page being stuck.
  bar: {
    backgroundColor: color.ink3,
    animationName: stylex.keyframes({
      "0%": { opacity: 0.45 },
      "50%": { opacity: 0.9 },
      "100%": { opacity: 0.45 },
    }),
    animationDuration: "1.6s",
    animationIterationCount: "infinite",
    animationTimingFunction: "ease-in-out",
  },
  stack: { display: "flex", flexDirection: "column", gap: space.s3 },
});

export const Note = ({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) => (
  <div {...stylex.props(styles.note)}>
    <p {...stylex.props(styles.title)}>{title}</p>
    <p {...stylex.props(styles.body)}>{children}</p>
  </div>
);

export const Mono = ({ children }: { children: ReactNode }) => (
  <span {...stylex.props(styles.code)}>{children}</span>
);

export const Skeleton = ({
  height = 18,
  width = "100%",
}: {
  height?: number;
  width?: string;
}) => (
  <span
    {...stylex.props(styles.bar)}
    style={{ height: `${height}px`, width }}
    aria-hidden="true"
  />
);

/** A page's worth of skeleton: a hero, a row of figures, and a panel. */
export const LoadingScreen = () => (
  <div {...stylex.props(styles.stack)} aria-busy="true">
    <Skeleton height={14} width="18ch" />
    <Skeleton height={72} width="9ch" />
    <Skeleton height={48} width="100%" />
    <Skeleton height={240} width="100%" />
  </div>
);

/** The counters could not be read.
 *
 * The old copy named one cause, a missing token, and was wrong the first
 * time a deployment with a token hit it. The route answers one status for
 * every reason it cannot read, so the screen says that and points at the
 * log line that does know which. */
export const CountersUnavailable = () => (
  <Note title="Counters unavailable">
    Nothing is lost: reads, writes, and probes are still being counted. Check
    the worker log for <Mono>api.stats_query_failed</Mono>.
  </Note>
);

/** No collection has finished. A young deployment rather than a broken
 *  one, and the difference is worth a sentence. */
export const NoRunsYet = ({ nextAt }: { nextAt?: string }) => (
  <Note title="No collection has finished yet">
    The collector runs on a cron and reports what it did afterwards, so these
    numbers appear after the first run
    {nextAt === undefined ? "" : `, which is due at ${nextAt}`}. Nothing is
    wrong with a deployment that has not collected yet.
  </Note>
);

/** Signed in, and not on the admins list. */
export const NotAnAdmin = ({ login }: { login: string }) => (
  <Note title="This screen is for admins">
    You are signed in as <Mono>{login}</Mono>, who is a member of an
    organisation this deployment serves and is not on its admins list. The
    access screen shows what your credential can reach; the collection and
    traffic screens read reports that <Mono>CACHET_ADMINS</Mono> gates.
  </Note>
);

/** Something refused, and it was not one of the states above. */
export const Failed = ({ message }: { message: string }) => (
  <Note title="The deployment refused that">{message}</Note>
);
