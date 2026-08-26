# ADR 0014: The deployment serves its own console

- **Status:** Accepted
- **Date:** 2026-08-26
- **Context doc:** [../../CLAUDE.md](../../CLAUDE.md) §1;
  [0002-three-authentication-flows.md](0002-three-authentication-flows.md)

## Context

The console is a browser application that reads a deployment's counters,
collection reports, and access configuration. Where it is served from
decides three other things at once.

The credential settles the first. ADR 0002 gave browsers a session
cookie, and that cookie is `HttpOnly; Secure; SameSite=Lax; Path=/`.
SameSite=Lax means a cross-site request never carries it, and the worker
emits no `Access-Control-*` headers and answers `OPTIONS` with a 404. A
console on another origin would need a CORS layer, a second credential,
or both. Served from the worker's own host it needs none of that, and
`require_admin` already accepts a session identity, so every admin route
works the day the files land.

The second is the nix key space, and it is the one that can break the
deployment's real job. A binary cache is read by asking about paths, and
most of those paths are not in any given cache: nix reads a 404 as "ask
the next substituter" and moves on. Cloudflare's asset layer has a
`not_found_handling` mode that answers an unmatched request with an
application shell, which is the right behavior for an application and a
catastrophe here, because it turns every cache miss into `200 text/html`
and every substituter into a client parsing HTML as a narinfo.

The third is what the deployment costs to run. Serving the console from
its own worker, or from Pages, means a second deployable, a second
domain, and a second thing to keep in step with the first.

## Decision

1. The worker's own deployment carries the console as static assets,
   mounted under `/console`. `GET /` answers 302 to `/console`, because
   nix asks for `/nix-cache-info` and for paths and never for the root.
2. Both of the asset layer's handling modes are off. `notFoundHandling`
   is `none`, so a request matching no file falls through to the worker
   and is routed as it always was. `htmlHandling` is `none`, so the layer
   performs no trailing-slash redirects and `/console` reaches the
   router.
3. The router owns the console's own routes. A path under `/console`
   whose last segment has no extension is a route inside the console's
   router, and the worker serves the shell for it through the `ASSETS`
   binding. Deciding by extension rather than by a list of screens means
   adding a screen changes the console alone.
4. A worker deployed without the binding answers 404 for `/console` and
   serves the protocol exactly as before, which is the same degradation
   the counter binding already makes.
5. The OAuth callback lands on `https://{host}/console` by default.
   `CACHET_UI_ORIGIN` stays as an override for a UI hosted elsewhere, and
   set to the empty string it means the deployment has no UI and the
   callback answers 204.

## Consequences

The console is one more directory in the same deploy: `just deploy`
builds it, alchemy uploads it beside the wasm bundle, and there is no
second domain, no CORS layer, and no second credential. Signing in works
with no configuration at all.

Asset requests are answered by Cloudflare without invoking the worker, so
the console costs nothing per file served. The worker is invoked only for
the console's own routes, which is one request per page load.

The workerd lane cannot bind assets and test the collector in the same
process: under miniflare, binding assets puts Cloudflare's asset router
in front of the worker, and that router has no scheduled handler, so the
collector's dev endpoint answers "exception". The console scenario boots
against a generated copy of the lane config with the asset block appended
and every other scenario runs without it. Production invokes the cron
against the worker itself, so this is a limitation of the local runner.

## Alternatives considered

**`run_worker_first: true`, with the Rust router deciding every path.**
This was the first decision, and it was wrong for a reason worth
recording: it puts the worker in front of `/cdn-cgi/*`, which the
platform and the local runner reserve for themselves. It broke the
collector's dev endpoint immediately, and the set of platform paths it
would break in production is not something this repository can
enumerate. Turning both handling modes off achieves the same protection
by never letting the layer decide anything.

**`notFoundHandling: "single-page-application"` with a `runWorkerFirst`
glob list naming the protocol paths.** Rejected: a protocol path added
later without updating the glob list would silently begin serving the
console's shell to nix clients, which is precisely the drift class
CLAUDE.md §0 exists to prevent.

**A separate console worker on `console.<host>`, binding this one.**
Rejected: it needs the CORS layer and the cross-site cookie problem that
serving from one origin avoids, and it doubles the deployment for an
admin surface with a handful of readers.

**Serving the console from the root rather than `/console`.** Rejected as
close to free but not free: the root is unclaimed today, and a console
there would mean the deployment's protocol surface and its application
surface share a namespace forever. A prefix keeps the two apart and
costs one redirect.
