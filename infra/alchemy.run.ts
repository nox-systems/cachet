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
        CACHET_ORGS: cfg.orgs,
        CACHET_AUDIENCE: cfg.audience,
        CACHET_DEFAULT_BRANCH_REF: cfg.defaultBranchRef,
        CACHET_HOST: cfg.host,
        CACHET_OAUTH_CLIENT_ID: cfg.oauthClientId,
        CACHET_ADMINS: cfg.admins,
        ...(cfg.uiOrigin === undefined
          ? {}
          : { CACHET_UI_ORIGIN: cfg.uiOrigin }),
        ...(cfg.gcGraceMs === undefined
          ? {}
          : { CACHET_GC_GRACE_MS: cfg.gcGraceMs }),
        CACHET_SIGNING_KEY: Config.redacted("CACHET_SIGNING_KEY"),
        CACHET_OAUTH_CLIENT_SECRET: Config.redacted(
          "CACHET_OAUTH_CLIENT_SECRET",
        ),
      },
      // The collector fires daily; GC_ARMED stays unset (armed by default).
      crons: ["0 5 * * *"],
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
      // R2 and KV are what almost every request waits on, so the isolate
      // belongs near them rather than near the client. Streaming a NAR
      // body is unaffected either way, and the cold read path's several
      // sequential round trips are not.
      placement: { mode: "smart" },
      // Only the custom domain serves: no workers.dev URL, no per-version
      // preview URLs. A cache answers on one name, and that name is the
      // signing key's identity.
      workersDev: false,
      domain: { name: cfg.domain },
    });

    return {
      workerUrl: worker.url,
      domain: cfg.domain ?? null,
    };
  }),
);
