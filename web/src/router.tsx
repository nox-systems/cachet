import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
} from "@tanstack/react-router";

import type { Subject, Window } from "./api/schema.ts";
import { Access } from "./screens/access.tsx";
import { Collection } from "./screens/collection.tsx";
import { Laptops } from "./screens/laptops.tsx";
import { Overview } from "./screens/overview.tsx";
import { Traffic } from "./screens/traffic.tsx";
import { Gate } from "./gate.tsx";

// Five screens, declared rather than generated. A file-based tree would
// bring a codegen step and a committed artifact to keep in step with the
// files, which is a drift surface this many routes does not earn.

const rootRoute = createRootRoute({
  component: () => (
    <Gate>
      <Outlet />
    </Gate>
  ),
});

const routes = [
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: Overview,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/collection",
    component: Collection,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/access",
    component: Access,
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/traffic",
    component: Traffic,
    // why: the traffic view's two choices live in the URL, so a filtered
    // view is a link. They are validated here against the same closed
    // sets the worker accepts, so a hand-edited URL lands on a real view
    // rather than on a question the deployment refuses.
    validateSearch: (search: Record<string, unknown>) => ({
      subject: readSubject(search["subject"]),
      window: readWindow(search["window"]),
    }),
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/laptops",
    component: Laptops,
  }),
];

const readSubject = (value: unknown): Subject =>
  value === "writes" || value === "probes" ? value : "reads";

const readWindow = (value: unknown): Window =>
  value === "day" || value === "month" ? value : "week";

export const router = createRouter({
  routeTree: rootRoute.addChildren(routes),
  // The worker serves the console under /console (ADR 0014), so the
  // router's idea of the root has to match the one the browser is on.
  basepath: "/console",
  defaultPreload: "intent",
  scrollRestoration: true,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

export { readSubject, readWindow };
