import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./styles/global.css";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { router } from "./router.tsx";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // why: the counters move on the order of minutes and the reports on
      // the order of a day, so a fresh answer per minute is more than the
      // screens need and a refetch on focus is what a person expects when
      // they come back to a tab.
      staleTime: 30_000,
      refetchOnWindowFocus: true,
      retry: false,
    },
  },
});

const root = document.getElementById("root");
if (root === null) throw new Error("the console's root element is missing");

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);
