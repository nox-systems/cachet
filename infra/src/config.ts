// The deploy-time configuration contract. Every value the stack needs is
// read from the environment exactly once, here, and every missing value is
// named in one refusal: a deploy that cannot configure itself fails before
// touching the account, never after half-provisioning it.

/** Everything a stage deploys with. */
export interface StageConfig {
  /** The deployment's name as invoked: the alchemy stage and the
   *  resource names (`cachet-<name>`, or the name as-is when it
   *  already starts with the prefix). */
  stage: string;
  /** The deployment's host name: the signing-key name prefix and, in
   *  production, the custom domain. */
  host: string;
  /** Comma-joined GitHub org slugs the deployment serves. */
  orgs: string;
  /** Comma-joined GitHub logins with admin rights. */
  admins: string;
  /** The GitHub OAuth App's client id. */
  oauthClientId: string;
  /** The OIDC audience (default `cachet`). */
  audience: string;
  /** The ref permitted to renew leases. */
  defaultBranchRef: string;
  /** The custom domain to attach; defaults to the host. */
  domain: string;
  /** The browser OAuth flow's redirect target, when a UI exists. */
  uiOrigin: string | undefined;
  /** The GC grace override; the worker's default (14 days) applies unset. */
  gcGraceMs: string | undefined;
  // The Cloudflare API token the counter route reads analytics with,
  // scoped to reading them and nothing else. Absent means the deployment
  // counts but does not report.
  statsToken: string | undefined;
  /** The Cloudflare account the counter route queries under. Read from
   *  the deploy's own CLOUDFLARE_ACCOUNT_ID, because the worker needs
   *  the same account alchemy authenticates against. */
  accountId: string | undefined;
}

const REQUIRED = ["HOST", "ORGS", "OAUTH_CLIENT_ID", "ADMINS"] as const;
/** The worker's secrets, read by the stack through Config.redacted. */
const SECRETS = ["CACHET_SIGNING_KEY", "CACHET_OAUTH_CLIENT_SECRET"] as const;

/** One environment variable under the deploy prefix, empty reads as absent. */
function value(name: string): string | undefined {
  const found = process.env[`CACHET_DEPLOY_${name}`]?.trim();
  return found === "" || found === undefined ? undefined : found;
}

/** Normalize a comma list: trim every entry, drop empties, re-join. */
function commaList(raw: string): string {
  const items = raw
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item !== "");
  return items.join(",");
}

/**
 * Resolve the stage's configuration from `CACHET_DEPLOY_*` variables and the
 * two deploy-time secrets. Throws naming every missing variable at once: the
 * operator fixes the set, not a drip-feed of one failure per run.
 */
export function loadStageConfig(stage: string): StageConfig {
  if (!/^[a-z][a-z0-9-]{1,31}$/.test(stage)) {
    // why: the name becomes the R2 bucket, KV namespace, and worker name
    // (prefixed with cachet- when absent), so it must fit the strictest
    // of those grammars (bucket names).
    throw new Error(
      `deployment name "${stage}" must match ^[a-z][a-z0-9-]{1,31}$`,
    );
  }
  const missing = REQUIRED.filter((name) => value(name) === undefined).map(
    (name) => `CACHET_DEPLOY_${name}`,
  );
  for (const secret of SECRETS) {
    if ((process.env[secret]?.trim() ?? "") === "") {
      missing.push(secret);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `the ${stage} deployment is missing: ${missing.join(", ")}. ` +
        `Set them in infra/.env.${stage} or in the CI environment's variables and secrets (docs/DEPLOY.md).`,
    );
  }

  const host = value("HOST") as string;
  if (!/^[a-z0-9][a-z0-9.-]*[a-z0-9]$/.test(host) || host.includes("..")) {
    throw new Error(
      `CACHET_DEPLOY_HOST=${JSON.stringify(host)} is not a plausible hostname`,
    );
  }

  // why: workers.dev and preview URLs are disabled on every stage, so the
  // worker needs a custom domain; the host is the natural one because it
  // already names the deployment and the signing key.
  const domain = value("DOMAIN") ?? host;

  // why: reporting takes both halves. The token authorises the query and
  // the account id says which account to run it against, and a worker
  // holding one without the other answers every counter request with a
  // 503 that reads like an outage. Refusing here keeps that from being a
  // thing an operator discovers from a chart that never loads.
  const accountId = process.env.CLOUDFLARE_ACCOUNT_ID?.trim() || undefined;
  const statsToken = value("STATS_TOKEN");
  if (statsToken !== undefined && accountId === undefined) {
    throw new Error(
      `the ${stage} deployment sets CACHET_DEPLOY_STATS_TOKEN but no CLOUDFLARE_ACCOUNT_ID. ` +
        `The counter route needs both: the token authorises the query, the account id says where to run it (docs/DEPLOY.md).`,
    );
  }
  return {
    stage,
    host,
    orgs: commaList(value("ORGS") as string),
    admins: commaList(value("ADMINS") as string),
    oauthClientId: value("OAUTH_CLIENT_ID") as string,
    audience: value("AUDIENCE") ?? "cachet",
    defaultBranchRef: value("DEFAULT_BRANCH_REF") ?? "refs/heads/main",
    domain,
    uiOrigin: value("UI_ORIGIN"),
    gcGraceMs: value("GC_GRACE_MS"),
    statsToken,
    accountId,
  };
}
