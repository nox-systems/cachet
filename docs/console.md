# The console

Every deployment serves a browser console at `https://<host>/console`,
and its root redirects there. It reads what the deployment already knows
about itself: what the collector has been doing, what the counters
recorded, and who can reach the cache. It writes nothing. The only action
on it is signing out.

Sign in with GitHub through the same OAuth App the CLI uses. An admin
sees all five screens; an org member outside `CACHET_ADMINS` sees the
access screen and a line saying which list gates the rest. A browser
session authenticates the console and answers 401 on the paths that serve
cache bytes, so a cookie copied out of a browser cannot substitute
(ADR 0016).

## The screens

The header is the same on all of them. It names the deployment, the
worker's version, and the commit it was built from, all from
`/api/public/config`; it shows the deployment's clock, taken from the
`Date` header every answer already carries; and for an admin it counts
down to the next collection and shows a status, both from
`/api/self/health`.

It also names the Cloudflare colo that answered the reader and the time
to first byte of the console itself. Neither costs a request. The colo
rides the `cf-ray` header Cloudflare puts on every response, read off
answers the console was making anyway, and the timing is the browser's
own record of the navigation that loaded the page, so it is the round
trip the reader actually waited through rather than a probe standing in
for one. Both mean what they say because placement is unpinned
(docs/DEPLOY.md): the worker runs at the colo nearest the client, so the
edge that served this console is the edge that serves that laptop's
substitutions. They speak for the reader and not for CI, whose runners
sit wherever GitHub puts them.

It also names the Cloudflare colo answering the reader and the round trip
to it. Both come from timing a fetch of `/nix-cache-info`, which is the
one protocol path needing no credential: Cloudflare puts the colo in the
last segment of the `cf-ray` header on every response, so the deployment
serves nothing extra for it. The number means what it says because
placement is unpinned (docs/DEPLOY.md), so the worker runs at the colo
nearest the client and the edge answering the console is the same one
answering that laptop's substitutions. It is a median of the last five
probes, because one round trip that hit a cold isolate should not move a
number somebody is using to decide whether their cache is near them. It
speaks for the reader and not for CI, whose runners sit wherever GitHub
puts them.

**Overview** answers how big the cache is and what put it there. The hero
is the inventory the last collection counted, which is the one number
that means "how big" without qualification. Below it: what that run
freed, how many reports are kept, the week's reads and writes, a line of
reads per day, and the repositories that pushed, largest first.

**Garbage collection** answers whether the collector is working. The
table is the last eight runs with their durations, deletions, and
results; a run that tripped a gate says so in the signal colour.
Selecting a run shows its whole report.

**Access** answers who can reach this deployment and how. The
organizations, the public key, the OAuth client id, the workflow snippet
CI needs, and the three commands a laptop runs; every command and key
copies on click. It closes by naming the reader's own session and when it
expires.

**Traffic** answers what the deployment is serving. Reads, writes, or
probes, over a day, a week, or a month, as a line and as a ranked list of
outcomes. Both choices live in the URL, so a view is a link. The day
window draws hourly buckets and the other two draw daily ones, because
those are the pairs `/api/self/events` admits.

**Developers** answers what the cache is doing for people rather than for
CI. The whole screen is one cross-filtered question, reads whose actor is
a laptop token, which is what the `actor` filter exists for.

The screens carry facts and not explanations of them. What a gate is,
what a grace window does, and why laptops cannot write are all true and
all belong in this document rather than beside a number, because a
paragraph explaining the system sits between a reader and the figure they
came for.

## The states the mockups do not draw

Each of these is reachable on a real deployment, and each says what is
true rather than showing an error.

- **Signed out.** `/api/whoami` answers 401, which is the ordinary state
  of a first visit rather than a failure. The console shows a sign-in
  screen naming the deployment and the orgs it serves.
- **An org member who is not an admin.** The access screen renders; the
  other four name `CACHET_ADMINS` as the reason they do not.
- **Loading.** Skeletons in the shape of the answer, never a spinner.
- **A deployment that has never collected.** `/api/self/stats` answers
  404 by design, and the console reads that as "no run yet" rather than
  as a failure. `/api/self/health` answers 200 `unknown` for the same
  situation, because it renders in a header on every screen.
- **A deployment that counts and cannot report.** Without
  `CACHET_DEPLOY_STATS_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` the counter
  route answers 503, and the traffic and laptops screens say which two
  values to set and that nothing counted has been lost.
- **A run that tripped a gate.** Shown in the signal colour on the run
  table and named on the overview when it was the latest run.

## How it is built and served

The console is React with TanStack Router and Query, styled with StyleX
against the tokens in `web/src/styles/tokens.stylex.ts`, and it decodes
every answer with an `effect` schema before a screen sees it: a
deployment newer or older than the console beside it fails at the
boundary with the field named rather than three components later as an
`undefined` on screen. The charts are hand-drawn SVG, because the shape
is an area, a line, two gridlines and a dot, and the libraries worth
considering are either archived (TanStack's react-charts) or arrive with
a second styling system (shadcn's charts are Recharts and Tailwind).

Every line carries a hover layer, which is part of a chart rather than an
addition to one: a crosshair that snaps to the nearest bucket, a readout
where the value leads and the date follows, and the same reading from the
keyboard, where the plot takes focus and the arrows walk the series into
a live region. The bars need no tooltip, because every bar is already
labelled with both its figures; what they gain on hover is the lift that
says the row noticed.

`just web` builds it into `web/dist`, and `just deploy` uploads that
directory as the worker's static assets. The asset layer answers a
request that names one of those files and never invents an answer for one
that does not, which is what keeps a cache miss a cache miss (ADR 0014).

The console ships Geist and Geist Mono, both open-licensed. A deployment
that holds a licence for other faces points `CACHET_DEPLOY_FONT_CSS` at a
stylesheet serving them; the console loads that one stylesheet and the
token stacks name those families first. Unset, which is the default, it
renders what it ships.

Its tests are the console lane (docs/testing/console.md).
