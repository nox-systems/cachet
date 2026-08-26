import * as stylex from "@stylexjs/stylex";
import { Link, useRouterState } from "@tanstack/react-router";
import type { ReactNode } from "react";

import type { Health, PublicConfig, WhoAmI } from "../api/schema.ts";
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
  OverviewIcon,
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
  hudClock: { justifySelf: "center", whiteSpace: "nowrap" },
  hudRight: {
    justifySelf: "end",
    display: "flex",
    alignItems: "center",
    gap: space.s6,
    whiteSpace: "nowrap",
  },
  dot: { fontSize: "9px", marginRight: space.s2 },
  healthy: { color: color.signal },
  degraded: { color: color.amber },
  unknown: { color: color.lineStrong },
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
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: "36px",
    marginBottom: space.s3,
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
  signOut: {
    display: "flex",
    alignItems: "center",
    gap: space.s2,
    backgroundColor: "transparent",
    borderWidth: 0,
    padding: 0,
    cursor: "pointer",
    color: { default: color.muted, ":hover": color.text },
    fontFamily: font.ui,
    fontSize: text.label,
    letterSpacing: tracking.label,
    textTransform: "uppercase",
  },
});

const SCREENS = [
  { to: "/", label: "overview", Icon: OverviewIcon },
  { to: "/collection", label: "garbage collection", Icon: CollectionIcon },
  { to: "/access", label: "access", Icon: AccessIcon },
  { to: "/traffic", label: "traffic", Icon: TrafficIcon },
  { to: "/laptops", label: "laptops", Icon: LaptopIcon },
] as const;

const statusTone = {
  healthy: styles.healthy,
  degraded: styles.degraded,
  unknown: styles.unknown,
} as const;

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
  const here = SCREENS.find((screen) => screen.to === pathname) ?? SCREENS[0];
  const countdown =
    health?.nextCollectionAtMs === undefined
      ? undefined
      : format.duration(Math.max(0, health.nextCollectionAtMs - nowMs));

  return (
    <div {...stylex.props(styles.page)}>
      <div {...stylex.props(styles.hud)}>
        <span>
          {[
            "cachet",
            config?.deployment,
            config === undefined ? undefined : `v${config.version}`,
            config?.buildSha,
          ]
            .filter((part) => part !== undefined && part !== "")
            .join(" · ")}
        </span>
        <span {...stylex.props(styles.hudClock)}>
          UTC {format.clock(nowMs)}
        </span>
        <span {...stylex.props(styles.hudRight)}>
          {countdown === undefined ? null : (
            <span>Next collection in {countdown}</span>
          )}
          {health === undefined ? null : (
            <span>
              <span {...stylex.props(styles.dot, statusTone[health.status])}>
                ●
              </span>
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
          <span {...stylex.props(styles.mark)}>
            <Mark />
          </span>
          {SCREENS.map((screen) => (
            <Link
              key={screen.to}
              to={screen.to}
              aria-label={screen.label}
              title={screen.label}
              {...stylex.props(
                styles.railLink,
                pathname === screen.to && styles.railActive,
              )}
            >
              <screen.Icon />
            </Link>
          ))}
          <span {...stylex.props(styles.railSpacer)} />
        </nav>

        <div {...stylex.props(styles.main)}>
          <header {...stylex.props(styles.header)}>
            <span {...stylex.props(styles.crumbs)}>
              <span {...stylex.props(styles.crumbHome)}>cachet</span>
              <span {...stylex.props(styles.crumbSlash)}>/</span>
              <span {...stylex.props(styles.crumbLeaf)}>{here.label}</span>
            </span>
            <button
              type="button"
              onClick={onSignOut}
              {...stylex.props(styles.signOut)}
            >
              <SignOutIcon />
              Sign out
            </button>
          </header>

          <main {...stylex.props(styles.content)}>{children}</main>

          <footer {...stylex.props(styles.statusBar)}>
            <span>
              {who === undefined
                ? "Not signed in"
                : `Signed in as ${who.login}${who.admin ? " (admin)" : ""}`}
              {config === undefined ? "" : ` · ${config.host}`}
            </span>
            <span>NOX</span>
          </footer>
        </div>
      </div>
    </div>
  );
};
