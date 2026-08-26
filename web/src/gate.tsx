import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";

import * as api from "./api/client.ts";
import { Shell } from "./components/shell.tsx";
import { NotAnAdmin } from "./components/ui/states.tsx";
import { SignIn } from "./screens/sign-in.tsx";

// The one place that decides whether there is a console to show. Three
// answers: not signed in, signed in without admin rights, or the shell.
// Everything below it can assume a credential resolved.

const ADMIN_ONLY = new Set(["/", "/collection", "/traffic", "/laptops"]);

export const Gate = ({ children }: { children: ReactNode }) => {
  const queryClient = useQueryClient();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const nowMs = useClock();

  const config = useQuery({
    queryKey: ["config"],
    queryFn: api.getConfig,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  const who = useQuery({
    queryKey: ["whoami"],
    queryFn: api.getWhoAmI,
    // why: a 401 is the ordinary state of a first visit, not a failure
    // to retry. Retrying it would delay the sign-in screen behind three
    // round trips that were always going to be refused.
    retry: false,
  });

  // The licensed faces, when the deployment names a stylesheet serving
  // them. Unset is the default and the shipped faces render.
  useFontStylesheet(config.data?.fontCss);

  const health = useQuery({
    queryKey: ["health"],
    queryFn: api.getHealth,
    enabled: who.data?.admin === true,
    retry: false,
    refetchInterval: 60_000,
  });

  if (who.isPending) return null;

  if (who.error instanceof api.ApiError && who.error.unauthenticated) {
    return <SignIn {...(config.data ? { config: config.data } : {})} />;
  }
  if (who.error !== null) {
    return (
      <SignIn
        {...(config.data ? { config: config.data } : {})}
        refused={String(who.error)}
      />
    );
  }

  const signOut = () => {
    void api.signOut().then(() => {
      queryClient.clear();
      window.location.assign("/console");
    });
  };

  return (
    <Shell
      {...(config.data ? { config: config.data } : {})}
      {...(who.data ? { who: who.data } : {})}
      {...(health.data ? { health: health.data } : {})}
      nowMs={nowMs}
      onSignOut={signOut}
    >
      {who.data?.admin === false && ADMIN_ONLY.has(pathname) ? (
        <NotAnAdmin login={who.data.login} />
      ) : (
        children
      )}
    </Shell>
  );
};

/** The deployment's clock, ticking locally from the last answer's offset.
 *
 * Every response carries a Date header, so the console never needs a
 * route to ask the time; what it shows is the deployment's second rather
 * than the laptop's, which is what a UTC clock beside a deployment name
 * means. */
const useClock = (): number => {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const tick = window.setInterval(() => setNowMs(Date.now()), 1_000);
    return () => window.clearInterval(tick);
  }, []);
  return nowMs;
};

/** Load one stylesheet, once, when the deployment names one. */
const useFontStylesheet = (href: string | undefined) => {
  useEffect(() => {
    if (href === undefined || href === "") return;
    if (document.querySelector(`link[data-cachet-fonts]`) !== null) return;
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = href;
    link.dataset["cachetFonts"] = "true";
    document.head.append(link);
  }, [href]);
};
