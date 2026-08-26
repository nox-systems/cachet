import * as stylex from "@stylexjs/stylex";
import { Tooltip } from "@base-ui/react/tooltip";
import { Link, useRouterState } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";

import type { Health, PublicConfig, WhoAmI } from "../api/schema.ts";
import { useEdge } from "../lib/edge.ts";
import * as format from "../lib/format.ts";
import {
  color,
  dims,
  font,
  leading,
  space,
  text,
  tracking,
  weight,
} from "../styles/tokens.stylex.ts";
import {
  AccessIcon,
  CollectionIcon,
  LaptopIcon,
  Mark,
  NoxMark,
  OverviewIcon,
  StatusIcon,
  SignOutIcon,
  TrafficIcon,
} from "./icons.tsx";

// The frame every screen sits in: a status strip across the top, a rail
// of marks down the left, a breadcrumb, and a footer naming who is
// signed in. It is the same on all five screens, so it is written once
// and takes only what it shows.

const styles = stylex.create({
  // why: the frame is fixed and the content scrolls inside it. The hud,
  // the rail, the breadcrumb, and the footer are how someone knows which
  // deployment and which screen they are on, and scrolling them away
  // takes that off the glass exactly when a long table needs it most.
  page: {
    height: "100vh",
    overflow: "hidden",
    display: "flex",
    flexDirection: "column",
    backgroundColor: color.ink,
  },
  // why: a grid, not space-between. With three flex children the middle
  // one lands wherever the outer two leave it, so the clock was centred
  // only by coincidence and drifted the moment the right-hand group was
  // empty, which it is for anyone who is not an admin. Equal side tracks
  // put it on the strip's centre line whatever sits beside it.
  hud: {
    height: dims.hud,
    flexShrink: 0,
    display: "grid",
    gridTemplateColumns: "1fr auto 1fr",
    alignItems: "center",
    gap: space.s4,
    paddingInline: space.s6,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    letterSpacing: tracking.label,
    textTransform: "uppercase",
    color: color.muted,
  },
  hudLeft: {
    display: "flex",
    alignItems: "center",
    gap: space.s6,
    minWidth: 0,
    overflow: "hidden",
    whiteSpace: "nowrap",
  },
  hudClock: { justifySelf: "center", whiteSpace: "nowrap" },
  hudRight: {
    justifySelf: "end",
    display: "flex",
    alignItems: "center",
    gap: space.s6,
    whiteSpace: "nowrap",
  },
  status: { display: "inline-flex", alignItems: "center", gap: space.s2 },
  dot: { width: "6px", height: "6px", borderRadius: "50%" },
  healthy: { backgroundColor: color.signal },
  degraded: { backgroundColor: color.amber },
  unknown: { backgroundColor: color.lineStrong },
  body: { flex: 1, display: "flex", minHeight: 0 },
  railScroll: { overflow: "hidden" },
  rail: {
    width: dims.rail,
    flexShrink: 0,
    borderRightWidth: "1px",
    borderRightStyle: "solid",
    borderRightColor: color.line,
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    paddingBlock: space.s4,
    gap: space.s2,
  },
  mark: {
    color: color.text,
    display: "grid",
    placeItems: "center",
    width: "48px",
    height: "36px",
    marginBottom: space.s3,
    textDecoration: "none",
  },
  // Both marks occupy the one cell, so the swap is a cross-fade in place
  // rather than a reflow. The capacitor is what cachet is; the wordmark
  // is whose it is, and hovering asks the second question.
  markLayer: {
    gridArea: "1 / 1",
    display: "flex",
    transitionProperty: "opacity, transform",
    transitionDuration: "260ms",
    transitionTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
  },
  markShown: { opacity: 1, transform: "scale(1)" },
  markHidden: { opacity: 0, transform: "scale(0.86)" },
  tooltip: {
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    letterSpacing: tracking.label,
    textTransform: "uppercase",
    color: color.text,
    backgroundColor: color.ink,
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: color.lineStrong,
    paddingBlock: space.s2,
    paddingInline: space.s3,
    marginLeft: space.s2,
  },
  // Fixed 36px slots so every mark sits on one vertical lane, including
  // the one at the bottom.
  railLink: {
    width: "36px",
    height: "36px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    color: { default: color.muted, ":hover": color.text },
    borderWidth: "1px",
    borderStyle: "solid",
    borderColor: "transparent",
    textDecoration: "none",
    transitionProperty: "color, border-color",
    transitionDuration: "160ms",
  },
  railActive: {
    color: color.text,
    borderColor: color.line,
    backgroundColor: color.ink2,
  },
  railSpacer: { flex: 1 },
  railStatus: {
    width: "36px",
    height: "36px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  },
  railStatusHealthy: { color: color.muted },
  railStatusDegraded: { color: color.amber },
  railStatusUnknown: { color: color.lineStrong },
  main: {
    flex: 1,
    minWidth: 0,
    minHeight: 0,
    display: "flex",
    flexDirection: "column",
  },
  header: {
    height: dims.header,
    flexShrink: 0,
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: space.s4,
    paddingInline: space.s8,
    borderBottomWidth: "1px",
    borderBottomStyle: "solid",
    borderBottomColor: color.line,
  },
  crumbs: {
    display: "flex",
    alignItems: "baseline",
    gap: space.s2,
    fontFamily: font.ui,
    fontSize: text.path,
    lineHeight: leading.body,
    letterSpacing: tracking.tight,
  },
  crumbHome: { color: color.text, fontWeight: weight.bold },
  crumbSlash: { color: color.lineStrong },
  crumbLeaf: { color: color.muted },
  content: {
    flex: 1,
    minWidth: 0,
    minHeight: 0,
    overflowY: "auto",
    overscrollBehavior: "contain",
    paddingInline: space.s8,
    paddingBlock: space.s8,
    display: "flex",
    flexDirection: "column",
    gap: space.s8,
  },
  statusBar: {
    height: dims.statusBar,
    flexShrink: 0,
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    paddingInline: space.s8,
    borderTopWidth: "1px",
    borderTopStyle: "solid",
    borderTopColor: color.line,
    fontFamily: font.ui,
    fontSize: text.label,
    lineHeight: leading.label,
    color: color.muted,
  },
  footerMark: { display: "flex", color: color.muted },
  signOut: {
    marginTop: space.s2,
    backgroundColor: "transparent",
    padding: 0,
    cursor: "pointer",
  },
});

const SCREENS = [
  { to: "/", label: "overview", Icon: OverviewIcon },
  { to: "/collection", label: "garbage collection", Icon: CollectionIcon },
  { to: "/access", label: "access", Icon: AccessIcon },
  { to: "/traffic", label: "traffic", Icon: TrafficIcon },
  { to: "/developers", label: "developers", Icon: LaptopIcon },
] as const;

const statusTone = {
  healthy: styles.healthy,
  degraded: styles.degraded,
  unknown: styles.unknown,
} as const;

const railStatusTone = {
  healthy: styles.railStatusHealthy,
  degraded: styles.railStatusDegraded,
  unknown: styles.railStatusUnknown,
} as const;

/// The rail's mark, which answers a second question on hover.
const RailMark = () => {
  const [over, setOver] = useState(false);
  return (
    <Link
      to="/"
      aria-label="cachet, by nox"
      onMouseEnter={() => setOver(true)}
      onMouseLeave={() => setOver(false)}
      onFocus={() => setOver(true)}
      onBlur={() => setOver(false)}
      {...stylex.props(styles.mark)}
    >
      <span
        {...stylex.props(
          styles.markLayer,
          over ? styles.markHidden : styles.markShown,
        )}
      >
        <Mark />
      </span>
      <span
        {...stylex.props(
          styles.markLayer,
          over ? styles.markShown : styles.markHidden,
        )}
      >
        <NoxMark />
      </span>
    </Link>
  );
};

export const Shell = ({
  config,
  who,
  health,
  nowMs,
  onSignOut,
  children,
}: {
  config?: PublicConfig;
  who?: WhoAmI;
  health?: Health;
  nowMs: number;
  onSignOut: () => void;
  children: ReactNode;
}) => {
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const edge = useEdge();
  const here = SCREENS.find((screen) => screen.to === pathname) ?? SCREENS[0];
  const countdown =
    health?.nextCollectionAtMs === undefined
      ? undefined
      : format.duration(Math.max(0, health.nextCollectionAtMs - nowMs));

  return (
    <div {...stylex.props(styles.page)}>
      <div {...stylex.props(styles.hud)}>
        <span {...stylex.props(styles.hudLeft)}>
          <span>
            {[
              "cachet",
              config?.deployment,
              config === undefined ? undefined : `v${config.version}`,
              config?.buildSha === undefined
                ? undefined
                : `commit ${config.buildSha}`,
            ]
              .filter((part) => part !== undefined && part !== "")
              .join(" · ")}
          </span>
          {/* The colo answering this reader, and how far away it is.
              Placement is unpinned, so this is the same edge that answers
              their substitutions. */}
          {edge.colo === undefined ? null : (
            <Tooltip.Root>
              <Tooltip.Trigger
                render={
                  <span>
                    edge · {edge.colo}
                    {edge.rttMs === undefined ? "" : ` · ${edge.rttMs} ms`}
                  </span>
                }
              />
              <Tooltip.Portal>
                <Tooltip.Positioner side="bottom" sideOffset={6}>
                  <Tooltip.Popup {...stylex.props(styles.tooltip)}>
                    your round trip to this cache
                  </Tooltip.Popup>
                </Tooltip.Positioner>
              </Tooltip.Portal>
            </Tooltip.Root>
          )}
        </span>
        <span {...stylex.props(styles.hudClock)}>
          UTC {format.clock(nowMs)}
        </span>
        <span {...stylex.props(styles.hudRight)}>
          {countdown === undefined ? null : (
            <span>Next collection in {countdown}</span>
          )}
          {health === undefined ? null : (
            <span {...stylex.props(styles.status)}>
              <span
                {...stylex.props(styles.dot, statusTone[health.status])}
                aria-hidden="true"
              />
              {health.status}
            </span>
          )}
        </span>
      </div>

      <div {...stylex.props(styles.body)}>
        <nav
          {...stylex.props(styles.rail, styles.railScroll)}
          aria-label="Screens"
        >
          <RailMark />
          {SCREENS.map((screen) => (
            <Tooltip.Root key={screen.to}>
              <Tooltip.Trigger
                render={
                  <Link
                    to={screen.to}
                    aria-label={screen.label}
                    {...stylex.props(
                      styles.railLink,
                      pathname === screen.to && styles.railActive,
                    )}
                  >
                    <screen.Icon />
                  </Link>
                }
              />
              <Tooltip.Portal>
                <Tooltip.Positioner side="right" sideOffset={4}>
                  <Tooltip.Popup {...stylex.props(styles.tooltip)}>
                    {screen.label}
                  </Tooltip.Popup>
                </Tooltip.Positioner>
              </Tooltip.Portal>
            </Tooltip.Root>
          ))}
          <span {...stylex.props(styles.railSpacer)} />

          {health === undefined ? null : (
            <Tooltip.Root>
              <Tooltip.Trigger
                render={
                  <span
                    aria-label={`Collection is ${health.status}`}
                    {...stylex.props(
                      styles.railStatus,
                      railStatusTone[health.status],
                    )}
                  >
                    <StatusIcon />
                  </span>
                }
              />
              <Tooltip.Portal>
                <Tooltip.Positioner side="right" sideOffset={4}>
                  <Tooltip.Popup {...stylex.props(styles.tooltip)}>
                    collection is {health.status}
                  </Tooltip.Popup>
                </Tooltip.Positioner>
              </Tooltip.Portal>
            </Tooltip.Root>
          )}

          {/* Signing out belongs beside the identity it ends, not in the
              corner opposite it. */}
          {who === undefined ? null : (
            <Tooltip.Root>
              <Tooltip.Trigger
                render={
                  <button
                    type="button"
                    onClick={onSignOut}
                    aria-label="Sign out"
                    {...stylex.props(styles.railLink, styles.signOut)}
                  >
                    <SignOutIcon />
                  </button>
                }
              />
              <Tooltip.Portal>
                <Tooltip.Positioner side="right" sideOffset={4}>
                  <Tooltip.Popup {...stylex.props(styles.tooltip)}>
                    sign out
                  </Tooltip.Popup>
                </Tooltip.Positioner>
              </Tooltip.Portal>
            </Tooltip.Root>
          )}
        </nav>

        <div {...stylex.props(styles.main)}>
          <header {...stylex.props(styles.header)}>
            <span {...stylex.props(styles.crumbs)}>
              <span {...stylex.props(styles.crumbHome)}>cachet</span>
              <span {...stylex.props(styles.crumbSlash)}>/</span>
              <span {...stylex.props(styles.crumbLeaf)}>{here.label}</span>
            </span>
          </header>

          <main {...stylex.props(styles.content)}>{children}</main>

          <footer {...stylex.props(styles.statusBar)}>
            <span>
              {who === undefined
                ? "Not signed in"
                : `Signed in as ${who.login}${who.admin ? " (admin)" : ""}`}
              {who?.expiresAtMs === undefined
                ? ""
                : ` · expires ${format.date(who.expiresAtMs)}`}
              {config === undefined ? "" : ` · ${config.host}`}
            </span>
            <span {...stylex.props(styles.footerMark)} aria-label="nox">
              <NoxMark />
            </span>
          </footer>
        </div>
      </div>
    </div>
  );
};
