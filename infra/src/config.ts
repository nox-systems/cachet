// The deploy-time configuration contract. Every value the stack needs is
// read from the environment exactly once, here, and every missing value is
// named in one refusal: a deploy that cannot configure itself fails before
// touching the account, never after half-provisioning it.

/** Everything a stage deploys with. */
export interface StageConfig {
  /** The stage as invoked (`staging`, `production`). */
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
  /** The custom domain to attach; production defaults it to the host. */
  domain: string;
  /** The browser OAuth flow's redirect target, when a UI exists. */
  uiOrigin: string | undefined;
  /** The GC grace override; zeroed for staging, the code default elsewhere. */
  gcGraceMs: string | undefined;
}

const REQUIRED = ["HOST", "ORGS", "OAUTH_CLIENT_ID", "ADMINS"] as const;
/** The worker's secrets, read by the stack through Config.redacted. */
const SECRETS = ["CACHET_SIGNING_KEY", "GITHUB_OAUTH_CLIENT_SECRET"] as const;

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
        `Set them in infra/.env.${stage} or as the CI environment's secrets (docs/DEPLOY.md).`,
    );
  }

  const host = value("HOST") as string;
  if (!/^[a-z0-9][a-z0-9.-]*[a-z0-9]$/.test(host) || host.includes("..")) {
    throw new Error(
      `CACHET_DEPLOY_HOST=${JSON.stringify(host)} is not a plausible hostname`,
    );
  }

  const production = stage === "production";
  const domain = value("DOMAIN") ?? (production ? host : undefined);
  if (domain === undefined) {
    // why: workers.dev and preview URLs are disabled on every stage, so a
    // stage without a custom domain is a worker that answers nowhere.
    throw new Error(
      `the ${stage} deployment has no domain: set CACHET_DEPLOY_DOMAIN ` +
        `(production defaults it to CACHET_DEPLOY_HOST; every other stage must name its own hostname).`,
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
    gcGraceMs: value("GC_GRACE_MS") ?? (production ? undefined : "0"),
  };
}
