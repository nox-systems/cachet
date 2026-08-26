import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Bars } from "../src/components/ui/bars.tsx";
import { Chart } from "../src/components/ui/chart.tsx";
import { Tiles } from "../src/components/ui/primitives.tsx";
import {
  CountersUnavailable,
  NoRunsYet,
  NotAnAdmin,
} from "../src/components/ui/states.tsx";
import * as format from "../src/lib/format.ts";

afterEach(cleanup);

describe("Tiles", () => {
  it("shows every figure it is handed", () => {
    render(
      <Tiles
        tiles={[
          { label: "Freed by that run", value: "2.1 GB" },
          { label: "Active leases", value: "37" },
        ]}
      />,
    );
    expect(screen.getByText("Freed by that run")).toBeDefined();
    expect(screen.getByText("2.1 GB")).toBeDefined();
    expect(screen.getByText("37")).toBeDefined();
  });
});

describe("Bars", () => {
  const rows = [
    {
      key: "a",
      name: "loopholelabs/architect",
      value: 812,
      figure: "812",
      aside: "9.4 GB",
    },
    {
      key: "b",
      name: "nox/cachet",
      value: 208,
      figure: "208",
      aside: "1.1 GB",
    },
  ];

  it("names every row and its two figures", () => {
    render(<Bars rows={rows} />);
    expect(screen.getByText("loopholelabs/architect")).toBeDefined();
    expect(screen.getByText("812")).toBeDefined();
    expect(screen.getByText("9.4 GB")).toBeDefined();
  });

  it("scales every bar against the widest row", () => {
    // Sharing one denominator is the only reason these are bars: two
    // rows scaled independently would both be full width and say
    // nothing about proportion.
    const { container } = render(<Bars rows={rows} />);
    const widths = [...container.querySelectorAll("span[style]")].map(
      (node) => (node as HTMLElement).style.width,
    );
    expect(widths[0]).toBe("100%");
    expect(widths[1]).toBe(`${(208 / 812) * 100}%`);
  });

  it("draws nothing rather than dividing by zero", () => {
    const { container } = render(
      <Bars
        rows={[{ key: "z", name: "none", value: 0, figure: "0", aside: "0 B" }]}
      />,
    );
    const fill = container.querySelector("span[style]") as HTMLElement;
    expect(fill.style.width).toBe("0%");
  });
});

describe("Chart", () => {
  const points = [
    { atMs: Date.UTC(2026, 7, 19), count: 1_508, bytes: 0 },
    { atMs: Date.UTC(2026, 7, 20), count: 2_133, bytes: 0 },
  ];

  it("draws a line and marks the live end", () => {
    const { container } = render(<Chart points={points} label={format.day} />);
    expect(container.querySelector("polyline")).not.toBeNull();
    expect(container.querySelector("circle")).not.toBeNull();
    expect(screen.getByText("19 Aug")).toBeDefined();
  });

  it("says an empty window is empty rather than drawing a flat line", () => {
    // A chart with no data and a line at zero reads as "no traffic
    // happened", which is true, but the axis has no scale to say it at.
    const { container } = render(<Chart points={[]} label={format.day} />);
    expect(container.querySelector("polyline")).toBeNull();
    expect(container.querySelector("circle")).toBeNull();
    expect(
      screen.getByText(/Nothing counted in this window yet/),
    ).toBeDefined();
  });
});

describe("the states the mockups do not draw", () => {
  it("tells a deployment that counts without reporting what to set", () => {
    render(<CountersUnavailable />);
    expect(screen.getByText(/counts, and cannot report/)).toBeDefined();
    expect(screen.getByText("CACHET_DEPLOY_STATS_TOKEN")).toBeDefined();
    expect(screen.getByText("CLOUDFLARE_ACCOUNT_ID")).toBeDefined();
  });

  it("says a young deployment is young rather than broken", () => {
    render(<NoRunsYet nextAt="05:00 UTC" />);
    expect(screen.getByText(/No collection has finished yet/)).toBeDefined();
    expect(screen.getByText(/05:00 UTC/)).toBeDefined();
    expect(screen.getByText(/Nothing is wrong/)).toBeDefined();
  });

  it("names the reader and the list that gates them", () => {
    render(<NotAnAdmin login="lane-member" />);
    expect(screen.getByText("lane-member")).toBeDefined();
    expect(screen.getByText("CACHET_ADMINS")).toBeDefined();
  });
});
