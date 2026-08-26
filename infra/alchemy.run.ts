// The cachet deployment: one R2 bucket, one KV namespace, the worker with
// its bindings, the collector's cron, and in production the custom domain.
// Stages isolate everything — cachet-staging and cachet-production share no
// resource — and the worker bundle is the worker-build artifact, uploaded
// byte-for-byte (`bundle: false`): rolldown must not rewrite exports the
// wasm module links against.
//
// Run: `just deploy production` (which wraps `alchemy deploy --stage`).

import * as Alchemy from "alchemy";
import * as Cloudflare from "alchemy/Cloudflare";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

import { loadStageConfig } from "./src/config.ts";

// The collector's schedule, named once: the platform gets it as the
// worker's trigger and the worker gets it as configuration, so the
// console's countdown and the cron that fires can never disagree.
const GC_CRON = "0 5 * * *";

export default Alchemy.Stack(
  "cachet",
  {
    providers: Cloudflare.providers(),
    state: Cloudflare.state(),
  },
  Effect.gen(function* () {
    const stage = yield* Alchemy.Stage;
    const cfg = loadStageConfig(stage);

    // why: resources live in the account's flat namespace, so they carry
    // the cachet- prefix — but the prefix is a guarantee, not a ritual:
    // a name that already starts with it stands alone.
    const resourceName = stage.startsWith("cachet-")
      ? stage
      : `cachet-${stage}`;

    const bucket = yield* Cloudflare.R2.Bucket("Bucket", {
      name: resourceName,
    });
    const kv = yield* Cloudflare.KV.Namespace("KV", {
      title: resourceName,
    });
    // Reads, writes, and probes are counted here, one data point each,
    // with the dimensions a question gets grouped by: what, how it went,
    // who asked, and which repository they were pushing for. The worker
    // can only write to it (the platform allows nothing else), so
    // reading is Cloudflare's SQL API and the queries live in
    // docs/DEPLOY.md. A dataset is pure configuration: nothing is
    // created, and nothing is destroyed when a deployment goes away.
    const events = yield* Cloudflare.AnalyticsEngine.Dataset("Events", {
      dataset: resourceName.replaceAll("-", "_"),
    });

    const worker = yield* Cloudflare.Worker("Worker", {
      name: resourceName,
      main: "../crates/cachet-worker/build/index.js",
      bundle: false,
      compatibility: { date: "2026-05-01" },
      env: {
        CACHE_BUCKET: bucket,
        CACHET_KV: kv,
        CACHET_EVENTS: events,
        // The dataset's own name, so the counter route can query the
        // one it writes to without a second place to keep it in step.
        CACHET_STATS_DATASET: resourceName.replaceAll("-", "_"),
        // why: reading a dataset is Cloudflare's SQL API, which takes an
        // account token; a worker cannot read what it writes here. The
        // token is scoped to reading account analytics and nothing else,
        // so a compromised worker gains a view of counters the operator
        // already owns and no power over R2, KV, or the worker itself.
        // Optional: a deployment without it counts normally and simply
        // cannot report.
        ...(cfg.statsToken === undefined
          ? {}
          : { CACHET_STATS_TOKEN: Config.redacted("CACHET_STATS_TOKEN") }),
        // The account the SQL API runs the query under. A worker cannot
        // read the dataset it writes to without naming an account, and
        // this is the same one alchemy is deploying into, so it comes
        // from the deploy's own environment rather than a second copy.
        ...(cfg.accountId === undefined
          ? {}
          : { CLOUDFLARE_ACCOUNT_ID: cfg.accountId }),
        // The console's header names the deployment and counts down to
        // the next collection. The name and the cron live here and
        // nowhere else the worker can see, so they ride in as
        // configuration rather than being guessed at request time.
        CACHET_DEPLOY_NAME: stage,
        CACHET_GC_CRON: GC_CRON,
        ...(cfg.fontCss === undefined ? {} : { CACHET_FONT_CSS: cfg.fontCss }),
        CACHET_ORGS: cfg.orgs,
        CACHET_AUDIENCE: cfg.audience,
        CACHET_DEFAULT_BRANCH_REF: cfg.defaultBranchRef,
        CACHET_HOST: cfg.host,
        CACHET_OAUTH_CLIENT_ID: cfg.oauthClientId,
        CACHET_ADMINS: cfg.admins,
        ...(cfg.gcGraceMs === undefined
          ? {}
          : { CACHET_GC_GRACE_MS: cfg.gcGraceMs }),
        CACHET_SIGNING_KEY: Config.redacted("CACHET_SIGNING_KEY"),
        CACHET_OAUTH_CLIENT_SECRET: Config.redacted(
          "CACHET_OAUTH_CLIENT_SECRET",
        ),
      },
      // The collector fires daily; GC_ARMED stays unset (armed by default).
      crons: [GC_CRON],
      // The worker's own event stream is the only way to tell an edge hit
      // from a bucket read in production, and without this it goes
      // nowhere: a slow read cannot be diagnosed from the outside.
      observability: { enabled: true },
      // A multipart completion reads the assembled NAR back to measure
      // it, which is the one request in the system whose cost scales with
      // the object. The paid-plan default is thirty seconds; the ceiling
      // is five minutes, and a NAR large enough to need multipart deserves
      // the headroom rather than a killed request the client retries.
      limits: { cpuMs: 300_000 },
      // why: no placement pin. Smart Placement moves the isolate next to
      // R2 and KV, which is right for a worker whose every request makes
      // backend round trips and wrong for this one: the hot path is an
      // edge-cache hit, which touches no backend at all, and the Cache
      // API answers at the colo the worker runs in. Pinning it therefore
      // added the client's round trip to the placed colo onto every
      // cached read. Measured from Vancouver against a warm narinfo:
      // 39ms without it, ~150ms with, where cache.nixos.org from the
      // same laptop answers in 31ms.
      // Only the custom domain serves: no workers.dev URL, no per-version
      // preview URLs. A cache answers on one name, and that name is the
      // signing key's identity.
      workersDev: false,
      domain: { name: cfg.domain },
      // The browser console, built by `just web` into web/dist and
      // uploaded beside the wasm bundle. base mirrors Vite's, so the
      // manifest holds the paths the worker asks for.
      //
      // why: both handling modes are "none", which is the whole safety
      // argument. The asset layer answers a request that names a file
      // under /console and never invents an answer for one that does
      // not: an unmatched request falls through to the worker, which
      // routes it as it always has. notFoundHandling other than "none"
      // would eventually answer a cache miss with the console's shell,
      // and a nix client reads "does this cache hold it" from the status,
      // so 200 text/html is a wrong answer to a question about a path.
      // htmlHandling is "none" so /console reaches the router rather than
      // being redirected by the layer (ADR 0014).
      assets: {
        directory: "../web/dist",
        base: "/console",
        htmlHandling: "none",
        notFoundHandling: "none",
      },
    });

    return {
      workerUrl: worker.url,
      domain: cfg.domain ?? null,
    };
  }),
);
