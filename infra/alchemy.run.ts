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

    const bucket = yield* Cloudflare.R2.Bucket("Bucket", {
      name: `cachet-${stage}`,
    });
    const kv = yield* Cloudflare.KV.Namespace("KV", {
      title: `cachet-${stage}`,
    });

    const worker = yield* Cloudflare.Worker("Worker", {
      name: `cachet-${stage}`,
      main: "../crates/cachet-worker/build/index.js",
      bundle: false,
      compatibility: { date: "2026-05-01" },
      env: {
        CACHE_BUCKET: bucket,
        CACHET_KV: kv,
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
        GITHUB_OAUTH_CLIENT_SECRET: Config.redacted(
          "GITHUB_OAUTH_CLIENT_SECRET",
        ),
      },
      // The collector fires daily; GC_ARMED stays unset (armed by default).
      crons: ["0 5 * * *"],
      ...(cfg.domain === undefined ? {} : { domain: { name: cfg.domain } }),
    });

    return {
      workerUrl: worker.url,
      domain: cfg.domain ?? null,
    };
  }),
);
