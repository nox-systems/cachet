// The workerd lane's driver (docs/testing/workerd.md). Each scenario:
// seed workerd's local R2 through the wrangler CLI, boot the built worker
// under `wrangler dev --local` on its own port and persistence directory,
// and assert over real HTTP. Caching behavior is observable through the
// worker's own event log (read.edge_hit, read.bucket_hit, read.miss,
// generation.document_corrupt), which wrangler streams on stdout.
//
// No npm dependencies: node 22's fetch, child_process, and stdlib cover
// everything. Usage: node workerd/check.mjs <fixtures dir>

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";

const repoRoot = path.resolve(import.meta.dirname, "..");
const configPath = path.join(repoRoot, "workerd", "wrangler.toml");
const fixturesDir = path.resolve(
  process.argv[2] ?? path.join(repoRoot, "fixtures", "nix-signed"),
);
const laneFixturesDir = path.join(repoRoot, "workerd", "fixtures");

const NARINFO_KEY = "qvqa04f0m85m0a6xxnan5vxnwg2jkgl9.narinfo";
const NAR_FILE = "11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y.nar.zst";
const NAR_KEY = `nar/${NAR_FILE}`;
const OTHER_NARINFO = "33333333333333333333333333333333.narinfo";

// The lane's OIDC stand-in: one RSA pair per run, a stub JWKS server on
// loopback, and tokens minted fresh (they can never rot the way committed
// token fixtures would).
const jwksKeys = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwksJwk = {
  kid: "lane-1",
  ...jwksKeys.publicKey.export({ format: "jwk" }),
};
// The same server doubles as the GitHub API's stand-in for the verdict
// path: it recognizes exactly one good laptop token, and it counts its
// hits so the KV-verdict caching is observable.
const GOOD_LAPTOP_TOKEN = "lane-laptop-token";

// Whether the stub org still claims its members. A scenario flips this
// to stage a departure; everything else leaves it alone.
const laneMembership = { active: true };

// The lane's plain read credential for object GET/HEADs: every object
// read answers 401 without one.
const READ_AUTH = () => ({ authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` });
const stubHits = { user: 0, memberships: 0, exchange: 0, oidcMint: 0 };
// Cloudflare's SQL API, as far as the counter route can tell. The worker
// reaches it through CACHET_STATS_API_URL, the same way it reaches the
// JWKS and the GitHub API through theirs, so the answer path runs for
// real: the statement it composed arrives here as text, and what this
// answers is what the route deserializes and shapes.
const LANE_STATS_TOKEN = "lane-stats-token";
const statsStub = { sql: [], authorization: null, rows: [], status: 200 };
const LANE_OAUTH_CODE = "lane-code";
const LANE_OUTSIDER_CODE = "lane-code-outsider";
const OUTSIDER_TOKEN = "lane-outsider-token";
const MEMBER_TOKEN = "lane-member-token";
const LANE_OAUTH_SECRET = "lane-oauth-secret";
const stubServer = http.createServer((req, res) => {
  const json = (status, body) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(body));
  };
  if (req.url === "/jwks.json") {
    return json(200, { keys: [jwksJwk] });
  }
  // The CLI's push mints tokens the Actions way: one GET per audience,
  // refused without the request token's header, as GitHub's endpoint does.
  if (req.url.startsWith("/oidc-token")) {
    if (!req.headers.authorization) {
      return json(401, { message: "no authorization header" });
    }
    const audience = new URL(req.url, "http://stub").searchParams.get(
      "audience",
    );
    stubHits.oidcMint += 1;
    return json(200, {
      count: 1,
      value: mint({ aud: audience ?? "cachet-lane" }),
    });
  }
  if (req.url.endsWith("/analytics_engine/sql") && req.method === "POST") {
    let sql = "";
    req.on("data", (chunk) => (sql += chunk));
    req.on("end", () => {
      statsStub.sql.push(sql);
      statsStub.authorization = req.headers.authorization ?? null;
      if (statsStub.status !== 200) {
        return json(statsStub.status, { errors: ["lane refusal"] });
      }
      json(200, { data: statsStub.rows });
    });
    return;
  }
  if (req.url === "/login/oauth/access_token" && req.method === "POST") {
    stubHits.exchange += 1;
    let form = "";
    req.on("data", (chunk) => (form += chunk));
    req.on("end", () => {
      const params = new URLSearchParams(form);
      const secretsOk =
        params.get("client_id") === "lane-oauth-client" &&
        params.get("client_secret") === LANE_OAUTH_SECRET;
      const token =
        params.get("code") === LANE_OAUTH_CODE
          ? GOOD_LAPTOP_TOKEN
          : params.get("code") === LANE_OUTSIDER_CODE
            ? OUTSIDER_TOKEN
            : null;
      json(
        200,
        secretsOk && token
          ? {
              access_token: token,
              token_type: "bearer",
              scope: "read:org read:user",
            }
          : { error: "bad_verification_code" },
      );
    });
    return;
  }
  const bearer = (req.headers.authorization ?? "").replace("Bearer ", "");
  if (req.url === "/user") {
    stubHits.user += 1;
    if (bearer === GOOD_LAPTOP_TOKEN) {
      return json(200, { login: "lane-dev" });
    }
    if (bearer === OUTSIDER_TOKEN) {
      return json(200, { login: "lane-outsider" });
    }
    if (bearer === MEMBER_TOKEN) {
      return json(200, { login: "lane-member" });
    }
    return json(401, { message: "bad credentials" });
  }
  if (req.url.startsWith("/orgs/")) {
    stubHits.memberships += 1;
    const memberHit =
      (bearer === GOOD_LAPTOP_TOKEN &&
        req.url === "/orgs/lane-org/memberships/lane-dev") ||
      (bearer === MEMBER_TOKEN &&
        req.url === "/orgs/lane-org/memberships/lane-member");
    // A scenario can make the org forget someone, which is what a
    // departure looks like from here.
    return memberHit && laneMembership.active
      ? json(200, { state: "active" })
      : json(404, { message: "Not Found" });
  }
  json(404, { message: "Not Found" });
});
await new Promise((resolve) => stubServer.listen(0, "127.0.0.1", resolve));
const jwksUrl = `http://127.0.0.1:${stubServer.address().port}/jwks.json`;
const githubApiUrl = `http://127.0.0.1:${stubServer.address().port}`;
const statsApiUrl = `http://127.0.0.1:${stubServer.address().port}`;

const b64url = (data) =>
  Buffer.from(data)
    .toString("base64")
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");

// Mint a lane OIDC token: the claims the policy requires, overridable key
// by key for the rejection cases.
function mint(overrides = {}) {
  const { alg, ...claimOverrides } = overrides;
  const header = b64url(
    JSON.stringify({ alg: alg ?? "RS256", typ: "JWT", kid: "lane-1" }),
  );
  const nowSec = Math.floor(Date.now() / 1000);
  const claims = {
    iss: "https://token.actions.githubusercontent.com",
    aud: "cachet-lane",
    exp: nowSec + 600,
    iat: nowSec - 10,
    repository: "lane-org/lane-repo",
    repository_owner: "lane-org",
    ref: "refs/heads/main",
    run_id: "41",
    sha: "abc",
    ...claimOverrides,
  };
  const payload = b64url(JSON.stringify(claims));
  const signature = crypto.sign(
    "RSA-SHA256",
    Buffer.from(`${header}.${payload}`),
    jwksKeys.privateKey,
  );
  return `${header}.${payload}.${b64url(signature)}`;
}

// The lane's signing key enters as .dev.vars, the same way a deployment's
// would; it is committed material in workerd/fixtures, and the file is
// deleted when the lane ends.
const devVarsPath = path.join(repoRoot, "workerd", ".dev.vars");
const laneSigningSecret = (
  await readFile(path.join(laneFixturesDir, "signing-key.secret"), "utf8")
).trimEnd();

// The lane's public half, parsed from its committed fixture so the name
// the worker stamps with is the name the checks verify against. A lane
// asserting only the Sig line's name armors the text while the bytes are
// free to rot: the signature must verify, the way a nix client verifies.
const [laneSigningName, laneSigningPublicB64] = (
  await readFile(path.join(laneFixturesDir, "signing-key.public"), "utf8")
)
  .trimEnd()
  .split(":", 2);
const lanePublicKey = crypto.createPublicKey({
  format: "jwk",
  key: {
    kty: "OKP",
    crv: "Ed25519",
    x: Buffer.from(laneSigningPublicB64, "base64").toString("base64url"),
  },
});

// nix's client-side contract, re-derived inside the lane: the fingerprint
// is `1;<storePath>;<narHash>;<narSize>;` followed by the references
// sorted, deduplicated, and joined with commas; the signature covers
// those bytes directly. Any Sig line naming the lane's key must verify
// against it, exactly as a real nix client's check would require.
function laneSignatureVerifies(body) {
  const fields = new Map();
  const sigs = [];
  for (const line of body.split("\n")) {
    const splitAt = line.indexOf(": ");
    if (splitAt === -1) continue;
    const value = line.slice(splitAt + 2);
    if (line.slice(0, splitAt) === "Sig") sigs.push(value);
    else fields.set(line.slice(0, splitAt), value);
  }
  const references = (fields.get("References") ?? "").trim();
  // Fingerprint references are full store paths, never the document's
  // basenames (ValidPathInfo::fingerprint prints through printStorePath).
  const canon = references
    ? [...new Set(references.split(/\s+/))]
        .sort()
        .map((ref) => `/nix/store/${ref}`)
        .join(",")
    : "";
  const fingerprint = `1;${fields.get("StorePath")};${fields.get("NarHash")};${fields.get("NarSize")};${canon}`;
  for (const sig of sigs) {
    const [name, b64] = sig.split(":", 2);
    if (name !== laneSigningName) continue;
    return crypto.verify(
      null,
      Buffer.from(fingerprint, "ascii"),
      lanePublicKey,
      Buffer.from(b64, "base64"),
    );
  }
  return false;
}

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push(["ok", name]);
    process.stdout.write(`ok ${name}\n`);
  } catch (failure) {
    // assert.equal's message drops the values when the stack is terse;
    // print them so a wire disagreement reads as a wire disagreement.
    const detail =
      failure.actual !== undefined
        ? ` (actual ${JSON.stringify(failure.actual)}, expected ${JSON.stringify(failure.expected)})`
        : "";
    results.push(["FAIL", `${name}: ${failure.message}${detail}`]);
    process.stdout.write(`FAIL ${name}: ${failure.message}${detail}\n`);
  }
}

// Event-log observations: the worker emits an event before it answers,
// but the log's pipe is asynchronous. Wait FOR the marker, never a fixed
// nap: one FIFO stream delivers everything the marker chronologically
// follows, which is what licenses the negative matches after it. A
// marker that never arrives is the test's own failure, on the clock of
// the system under test instead of a guessed interval.
// Wait for one marker to appear, or fail with the stream's tail.
//
// This is the lane's only synchronization primitive, and every assertion
// about what the worker did is built on it the same way: wait for an
// event the request under test is guaranteed to emit, and only then
// assert what must NOT have happened. Reading the stream without that
// wait samples it, because the worker answers the client before wrangler
// flushes the line, and a sample is not an assertion.
async function untilEvent(events, marker) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (events().includes(marker)) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `event never appeared: ${marker}\n--- stream tail ---\n${events().slice(-800)}`,
  );
}

// Boot one wrangler dev. The port is never chosen: wrangler binds an
// OS-assigned port (--port 0) and names it in its ready banner, so the
// race a probe-and-release dance invites cannot exist. The boot's answer
// is the banner itself; its absence by the deadline is the test's own
// signal, never retried.
async function bootWorkerd(persist, vars) {
  const proc = spawn(
    "wrangler",
    [
      "dev",
      "--local",
      "--port",
      "0",
      "--persist-to",
      persist,
      "--config",
      configPath,
      "--var",
      `CACHET_JWKS_URL:${jwksUrl}`,
      "--var",
      `CACHET_GITHUB_API_URL:${githubApiUrl}`,
      "--var",
      `CACHET_GITHUB_WEB_URL:${githubApiUrl}`,
      ...Object.entries(vars).flatMap(([name, value]) => [
        "--var",
        `${name}:${value}`,
      ]),
    ],
    { detached: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  let captured = "";
  proc.stdout.on("data", (chunk) => (captured += chunk));
  proc.stderr.on("data", (chunk) => (captured += chunk));
  const deadline = Date.now() + 60_000;
  let base = "";
  while (Date.now() < deadline) {
    const banner = captured.match(/Ready on (http:\/\/[^\s]+)/);
    if (banner) {
      base = banner[1];
      break;
    }
    if (captured.includes("ERROR")) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  if (!base) {
    try {
      process.kill(-proc.pid, "SIGKILL");
    } catch {
      proc.kill("SIGKILL");
    }
    throw new Error(`workerd never came up:\n${captured.slice(-2000)}`);
  }
  // The stream is consumed monotonically: clearing advances an offset
  // rather than swapping the string, so a late-flushed line from an
  // earlier assertion can never bleed into the next one's window.
  let epoch = 0;
  const events = () => captured.slice(epoch);
  const clearEvents = () => (epoch = captured.length);
  const fullEvents = () => captured;
  return { proc, base, events, clearEvents, fullEvents };
}

// One scenario per persistence directory: fresh R2, fresh edge cache.
async function scenario(name, seed, assertions, vars = {}) {
  const persist = await mkdtemp(path.join(os.tmpdir(), "cachet-lane-"));
  for (const [key, content] of await seed()) {
    const seedFile = path.join(persist, "seed-input");
    await writeFile(seedFile, content);
    const put = spawnSync(
      "wrangler",
      [
        "r2",
        "object",
        "put",
        `cachet-lane/${key}`,
        "--file",
        seedFile,
        "--local",
        "--persist-to",
        persist,
        "--config",
        configPath,
      ],
      { encoding: "utf8" },
    );
    if (put.status !== 0) {
      throw new Error(`seeding ${key} failed: ${put.stderr}${put.stdout}`);
    }
  }

  const { proc, base, events, clearEvents, fullEvents } = await bootWorkerd(
    persist,
    vars,
  );

  try {
    const failuresBefore = results.filter(([state]) => state === "FAIL").length;
    await assertions({ base, events, clearEvents, persist });
    // why: a wire answer alone (a 503) never says WHICH 503; the worker's
    // event stream does. Spill it attached to the scenario that failed,
    // or the CI log reads as noise. Diagnostics read the whole stream;
    // the epoch windows belong to assertions, not to failure evidence.
    const failuresAfter = results.filter(([state]) => state === "FAIL").length;
    if (failuresAfter > failuresBefore) {
      const traffic = fullEvents()
        .split("\n")
        .filter(
          (line) => line.includes('"event"') || line.includes("wrangler:info]"),
        )
        .slice(-60)
        .join("\n");
      process.stdout.write(
        `--- worker log tail for "${name}" ---\n${traffic}\n--- end worker log ---\n`,
      );
    }
  } finally {
    try {
      process.kill(-proc.pid, "SIGKILL");
    } catch {
      proc.kill("SIGKILL");
    }
  }
}

await scenario(
  "the read path serves and caches",
  async () => [
    [NARINFO_KEY, await readFile(path.join(fixturesDir, NARINFO_KEY))],
    [NAR_KEY, await readFile(path.join(fixturesDir, NAR_FILE))],
  ],
  async ({ base, events, clearEvents }) => {
    await check("GET /nix-cache-info answers the handshake", async () => {
      const res = await fetch(`${base}/nix-cache-info`);
      assert.equal(res.status, 200);
      assert.equal(res.headers.get("content-type"), "text/x-nix-cache-info");
      assert.equal(res.headers.get("cache-control"), "public, max-age=300");
      assert.equal(
        await res.text(),
        "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n",
      );
    });

    await check("a seeded narinfo serves with the wire headers", async () => {
      const res = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      const seeded = await readFile(
        path.join(fixturesDir, NARINFO_KEY),
        "utf8",
      );
      assert.equal(res.status, 200);
      assert.equal(res.headers.get("content-type"), "text/x-nix-narinfo");
      assert.equal(
        res.headers.get("cache-control"),
        "public, max-age=2592000, immutable",
      );
      const body = await res.text();
      assert.equal(body, seeded);
      assert.equal(
        Number(res.headers.get("content-length")),
        Buffer.byteLength(seeded),
      );
      // The answer implies the emission; closing the check's own window
      // is what keeps its events out of the next check's stream.
      await untilEvent(events, '"event":"read.bucket_hit"');
    });

    await check("the second get comes from the edge", async () => {
      clearEvents();
      const res = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(res.status, 200);
      await untilEvent(events, '"event":"read.edge_hit"');
      assert.match(events(), /"event":"read\.edge_hit"/);
      assert.doesNotMatch(events(), /"event":"read\.bucket_hit"/);
    });

    await check("a missing narinfo is a cacheable 404", async () => {
      clearEvents();
      const res = await fetch(`${base}/${OTHER_NARINFO}`, {
        headers: READ_AUTH(),
      });
      assert.equal(res.status, 404);
      assert.equal(
        res.headers.get("content-type"),
        "text/plain; charset=utf-8",
      );
      assert.equal(res.headers.get("cache-control"), "public, max-age=30");
      assert.equal(await res.text(), "not found\n");
      await untilEvent(events, '"event":"read.miss"');
    });

    await check(
      "the missing narinfo answers from the edge the second time",
      async () => {
        clearEvents();
        const res = await fetch(`${base}/${OTHER_NARINFO}`, {
          headers: READ_AUTH(),
        });
        assert.equal(res.status, 404);
        await untilEvent(events, '"event":"read.edge_hit"');
        assert.match(events(), /"event":"read\.edge_hit"/);
        assert.doesNotMatch(events(), /"event":"read\.miss"/);
      },
    );

    await check("HEAD answers from bucket metadata alone", async () => {
      const res = await fetch(`${base}/${NARINFO_KEY}`, {
        method: "HEAD",
        headers: READ_AUTH(),
      });
      assert.equal(res.status, 200);
      assert.equal(res.headers.get("content-type"), "text/x-nix-narinfo");
      assert.equal(await res.text(), "");
    });

    await check("HEAD on a miss is an empty 404", async () => {
      const res = await fetch(`${base}/${OTHER_NARINFO}`, {
        method: "HEAD",
        headers: READ_AUTH(),
      });
      assert.equal(res.status, 404);
      assert.equal(await res.text(), "");
    });

    await check("a NAR read without a credential is 401", async () => {
      const res = await fetch(`${base}/${NAR_KEY}`);
      assert.equal(res.status, 401);
      assert.equal((await res.json()).code, "unauthorized");
    });

    await check("a NAR streams with its wire headers", async () => {
      const res = await fetch(`${base}/${NAR_KEY}`, {
        headers: { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` },
      });
      const seeded = await readFile(path.join(fixturesDir, NAR_FILE));
      assert.equal(res.status, 200);
      assert.equal(res.headers.get("content-type"), "application/x-nix-nar");
      assert.equal(
        res.headers.get("cache-control"),
        "public, max-age=2592000, immutable",
      );
      assert.deepEqual(Buffer.from(await res.arrayBuffer()), seeded);
    });

    await check("a shapeless path is a problem 404", async () => {
      const res = await fetch(`${base}/hello`);
      assert.equal(res.status, 404);
      assert.equal(res.headers.get("content-type"), "application/problem+json");
      const body = await res.json();
      assert.equal(body.code, "not_found");
      assert.equal(body.status, 404);
    });

    await check(
      "a grammar-broken narinfo path is the locked problem body",
      async () => {
        const res = await fetch(`${base}/notahash.narinfo`);
        assert.equal(res.status, 400);
        assert.equal(
          res.headers.get("content-type"),
          "application/problem+json",
        );
        assert.equal(
          await res.text(),
          '{"type":"about:blank","status":400,"title":"key grammar rejected","code":"malformed_key"}\n',
        );
      },
    );

    await check("POST is not a read", async () => {
      const res = await fetch(`${base}/${NARINFO_KEY}`, { method: "POST" });
      assert.equal(res.status, 404);
    });
  },
);

await scenario(
  "a corrupt generation document bypasses the negative edge only",
  async () => {
    const narinfo = await readFile(path.join(fixturesDir, NARINFO_KEY));
    return [
      [NARINFO_KEY, narinfo],
      ["meta/generation", "this was never json"],
    ];
  },
  async ({ base, events, clearEvents }) => {
    // A stored object's edge entry carries no generation, because both
    // kinds of object are addressed by a hash of their own content: a
    // generation nobody can read says nothing about whether those bytes
    // are still the bytes. A cached absence is the entry a corrupt
    // generation must not be trusted with, since it is the one a write
    // can make wrong, so that is the half that degrades.
    await check("a stored object still answers from the edge", async () => {
      const first = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(first.status, 200);
      await untilEvent(events, '"event":"generation.document_corrupt"');
      clearEvents();
      const second = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(second.status, 200);
      await untilEvent(events, '"event":"read.edge_hit"');
      assert.match(events(), /"event":"read\.edge_hit"/);
    });

    await check(
      "an absence is re-read from the bucket every time",
      async () => {
        // The mirror of the generation-zero scenario below, which proves
        // a miss IS cached when the generation reads. Each request is
        // waited for on its own miss before anything is claimed about
        // it, so the second request's "no edge hit" is a fact about a
        // request whose log line has already arrived.
        clearEvents();
        const first = await fetch(`${base}/${OTHER_NARINFO}`, {
          headers: READ_AUTH(),
        });
        assert.equal(first.status, 404);
        await untilEvent(events, '"event":"read.miss"');

        clearEvents();
        const second = await fetch(`${base}/${OTHER_NARINFO}`, {
          headers: READ_AUTH(),
        });
        assert.equal(second.status, 404);
        // With no readable generation there is no key to cache a miss
        // under, so the second request cannot be answered by the first.
        await untilEvent(events, '"event":"read.miss"');
        assert.doesNotMatch(events(), /"event":"read\.edge_hit"/);
      },
    );
  },
);

await scenario(
  "a login trades a GitHub identity for the deployment's own credential",
  async () => {
    const narinfo = await readFile(path.join(fixturesDir, NARINFO_KEY));
    return [[NARINFO_KEY, narinfo]];
  },
  async ({ base, persist }) => {
    let issued = null;

    await check("a GitHub identity exchanges for a read token", async () => {
      const res = await fetch(`${base}/api/login/exchange`, {
        method: "POST",
        headers: READ_AUTH(),
      });
      const text = await res.text();
      assert.equal(res.status, 200, text);
      assert.equal(res.headers.get("cache-control"), "no-store");
      issued = JSON.parse(text);
      assert.match(
        issued.token,
        /^cachet_[A-Za-z0-9_-]{43}$/,
        "the grammar the read path tells credential shapes apart by",
      );
      assert.equal(issued.login, "lane-dev");
      assert.ok(issued.expiresAtMs > Date.now(), "it outlives its issue");
    });

    await check(
      "the issued token reads, and the GitHub one still does",
      async () => {
        // Both work during a migration: a laptop that has not run the new
        // login yet is not locked out by one that has.
        for (const headers of [
          { authorization: `Bearer ${issued.token}` },
          READ_AUTH(),
        ]) {
          const res = await fetch(`${base}/${NARINFO_KEY}`, { headers });
          assert.equal(res.status, 200, await res.text());
        }
      },
    );

    await check("netrc basic auth carries it too", async () => {
      // The daemon sends it as an HTTP Basic password, which is the
      // whole point of issuing it.
      const basic = Buffer.from(`cachet:${issued.token}`).toString("base64");
      const res = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: { authorization: `Basic ${basic}` },
      });
      assert.equal(res.status, 200, await res.text());
    });

    await check("the deployment stores a digest, never the token", async () => {
      // A reader of the deployment's own state finds nothing they can
      // present. This is the property the threat model rests on.
      const dump = spawnSync(
        "wrangler",
        [
          "kv",
          "key",
          "list",
          "--binding",
          "CACHET_KV",
          "--local",
          "--persist-to",
          persist,
          "--config",
          configPath,
        ],
        { encoding: "utf8" },
      );
      assert.equal(dump.status, 0, dump.stderr);
      assert.ok(
        !dump.stdout.includes(issued.token),
        "the token itself is not a key",
      );
      const keys = JSON.parse(dump.stdout).map((entry) => entry.name);
      const record = keys.find((name) => name.startsWith("readtoken/"));
      assert.ok(record, `no readtoken record among ${keys.join(", ")}`);
      assert.match(record, /^readtoken\/[0-9a-f]{64}$/, "keyed by SHA-256");
    });

    await check("an outsider cannot exchange", async () => {
      const res = await fetch(`${base}/api/login/exchange`, {
        method: "POST",
        headers: { authorization: `Bearer ${OUTSIDER_TOKEN}` },
      });
      const text = await res.text();
      assert.equal(res.status, 403, text);
      assert.equal(JSON.parse(text).code, "forbidden_org");
    });

    await check("an anonymous exchange is refused", async () => {
      const res = await fetch(`${base}/api/login/exchange`, { method: "POST" });
      const text = await res.text();
      assert.equal(res.status, 401, text);
      assert.equal(JSON.parse(text).code, "unauthorized");
    });

    await check("losing membership stops the token reading", async () => {
      // The whole reason the record holds the GitHub credential: the
      // issued token is a pointer, so membership is still GitHub's
      // answer and a departure closes access at the verdict TTL rather
      // than at the credential's expiry.
      const before = stubHits.memberships;
      laneMembership.active = false;
      try {
        // The verdict and the isolate memo both have to lapse for the
        // next read to ask again, so the scenario clears what it can and
        // asserts against a fresh credential rather than waiting out a
        // TTL it does not control.
        const fresh = await fetch(`${base}/api/login/exchange`, {
          method: "POST",
          headers: { authorization: `Bearer ${MEMBER_TOKEN}` },
        });
        assert.equal(fresh.status, 403, await fresh.text());
        assert.ok(
          stubHits.memberships > before,
          "the deployment asked GitHub rather than trusting its own record",
        );
      } finally {
        laneMembership.active = true;
      }
    });

    await check("revoking stops the token reading", async () => {
      const gone = await fetch(`${base}/api/login/revoke`, {
        method: "POST",
        headers: { authorization: `Bearer ${issued.token}` },
      });
      assert.equal(gone.status, 204, await gone.text());
      const after = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: { authorization: `Bearer ${issued.token}` },
      });
      // why: the isolate memo would otherwise keep answering for this
      // token, which would make a logout the holder can watch fail.
      assert.equal(after.status, 401, await after.text());
    });
  },
);

await scenario(
  "a fresh bucket runs generation zero",
  async () => [],
  async ({ base, events, clearEvents }) => {
    await check("miss then edge-cached miss on an empty bucket", async () => {
      const first = await fetch(`${base}/${OTHER_NARINFO}`, {
        headers: READ_AUTH(),
      });
      assert.equal(first.status, 404);
      await untilEvent(events, '"event":"read.miss"');
      clearEvents();
      const second = await fetch(`${base}/${OTHER_NARINFO}`, {
        headers: READ_AUTH(),
      });
      assert.equal(second.status, 404);
      await untilEvent(events, '"event":"read.edge_hit"');
      assert.match(events(), /"event":"read\.edge_hit"/);
      assert.doesNotMatch(events(), /"event":"read\.miss"/);
    });
  },
);

// The write scenarios need the signing key present as a deployment secret;
// the driver writes .dev.vars and removes it when they finish.
await writeFile(
  devVarsPath,
  `CACHET_SIGNING_KEY=${laneSigningSecret}\nCACHET_OAUTH_CLIENT_SECRET=${LANE_OAUTH_SECRET}\n`,
);
try {
  await scenario(
    "writes verify, then sign",
    async () => [],
    async ({ base, events }) => {
      const narinfoFixture = await readFile(
        path.join(fixturesDir, NARINFO_KEY),
        "utf8",
      );
      const narBytes = await readFile(path.join(fixturesDir, NAR_FILE));
      const validToken = mint();
      // Every NAR write declares what its frame decodes to. The worker
      // measures the bytes as they stream into the bucket, and a decoder
      // needs its ceiling before it reads the first one; the narinfo that
      // would carry NarSize has not arrived yet at that point.
      const laneNarSize = narinfoFixture.match(/NarSize: (\d+)/)[1];
      const auth = (token) => ({
        authorization: `Bearer ${token}`,
        "x-cachet-nar-bytes": laneNarSize,
      });

      await check(
        "the public config names orgs, host, and the deployment key",
        async () => {
          const res = await fetch(`${base}/api/public/config`);
          assert.equal(res.status, 200);
          assert.equal(res.headers.get("cache-control"), "no-store");
          const body = await res.json();
          assert.deepEqual(body.orgs, ["lane-org"]);
          assert.equal(body.host, "cachet.lane.invalid");
          assert.equal(typeof body.oauthClientId, "string");
          const lanePublic = (
            await readFile(
              path.join(laneFixturesDir, "signing-key.public"),
              "utf8",
            )
          ).trim();
          assert.equal(body.publicKey, lanePublic);
          // The console header's identity line reads from here, so an
          // org member who is not an admin still gets a header.
          assert.equal(body.deployment, "cachet-lane");
          assert.match(body.version, /^\d+\.\d+\.\d+$/);
          // The lane's build stamps no commit and licenses no fonts, and
          // both are absent rather than null, so a client can tell "not
          // stamped" from "stamped empty".
          assert.equal("fontCss" in body, false, JSON.stringify(body));
        },
      );

      await check("the served OpenAPI is the committed document", async () => {
        const spec = await fetch(`${base}/api/openapi.json`);
        assert.equal(spec.status, 200);
        assert.equal(spec.headers.get("content-type"), "application/yaml");
        const committed = await readFile(
          path.join(repoRoot, "docs", "openapi.yaml"),
          "utf8",
        );
        assert.equal(
          await spec.text(),
          committed,
          "the served document is the committed one",
        );
      });

      await check("a write without a credential is 401", async () => {
        const res = await fetch(`${base}/${NAR_KEY}`, {
          method: "PUT",
          body: narBytes,
        });
        assert.equal(res.status, 401);
        assert.equal((await res.json()).code, "unauthorized");
      });

      await check("a token from another org is 403", async () => {
        const res = await fetch(`${base}/${NAR_KEY}`, {
          method: "PUT",
          headers: auth(mint({ repository_owner: "elsewhere" })),
          body: narBytes,
        });
        assert.equal(res.status, 403);
        assert.equal((await res.json()).code, "forbidden_org");
      });

      await check("alg confusion and staleness are 401", async () => {
        for (const token of [
          mint({ alg: "HS256" }),
          mint({ exp: Math.floor(Date.now() / 1000) - 7200 }),
          mint({ aud: "someone-else" }),
        ]) {
          const res = await fetch(`${base}/${NAR_KEY}`, {
            method: "PUT",
            headers: auth(token),
            body: narBytes,
          });
          assert.equal(res.status, 401);
        }
      });

      await check("an unparseable Authorization header is 400", async () => {
        const res = await fetch(`${base}/${NAR_KEY}`, {
          method: "PUT",
          headers: { authorization: "JustSomeGarbage" },
          body: narBytes,
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_auth");
      });

      await check("an oversized Authorization header is 400", async () => {
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          headers: { authorization: `Bearer ${"x".repeat(9000)}` },
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_auth");
      });

      await check("an unparseable narinfo document is 400", async () => {
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body: "this is not a narinfo at all",
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_narinfo");
      });

      await check("an unparseable roots payload is 400", async () => {
        const res = await fetch(`${base}/roots/lane-org-lane-repo`, {
          method: "POST",
          headers: auth(validToken),
          body: "a lease body is JSON, not this",
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_roots");
      });

      await check("a narinfo over its byte cap is 413", async () => {
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body: Buffer.alloc(70_000, 65),
        });
        assert.equal(res.status, 413);
        assert.equal((await res.json()).code, "body_too_large");
      });

      await check("the fixture NAR stores", async () => {
        const res = await fetch(`${base}/${NAR_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body: narBytes,
        });
        assert.equal(res.status, 204, await res.text());
      });

      await check(
        "a narinfo naming another store path is refused",
        async () => {
          const body = narinfoFixture.replace(
            "/nix/store/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9-",
            "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-",
          );
          const res = await fetch(`${base}/${NARINFO_KEY}`, {
            method: "PUT",
            headers: auth(validToken),
            body,
          });
          assert.equal(res.status, 400);
          assert.equal((await res.json()).code, "store_path_mismatch");
        },
      );

      await check("an unverified compression is refused", async () => {
        const body = narinfoFixture.replaceAll(".nar.zst", ".nar.xz");
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body,
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "unsupported_compression");
      });

      await check("a narinfo whose NAR is absent is 409", async () => {
        const body = narinfoFixture.replaceAll(
          "11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y",
          "w".repeat(52),
        );
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body,
        });
        assert.equal(res.status, 409);
        assert.equal((await res.json()).code, "narinfo_nar_missing");
      });

      await check("a file-hash lie is refused before signing", async () => {
        const body = narinfoFixture.replace(
          "FileHash: sha256:1",
          "FileHash: sha256:2",
        );
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body,
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "file_hash_mismatch");
        // why: problem bodies stay gossip-free; the forensic answer for a
        // disagreement is the operator event's declared/measured pairs.
        const declared_pair = `"fileHashDeclared":"${body.match(/FileHash: (\S+)/)[1]}"`;
        await untilEvent(events, declared_pair);
        const stream = events();
        const declared = body.match(/FileHash: (\S+)/)[1];
        assert.ok(
          stream.includes(`"fileHashDeclared":"${declared}"`),
          stream.slice(-500),
        );
        assert.ok(
          stream.includes(
            `"fileHashMeasured":"sha256:${NAR_FILE.replace(".nar.zst", "")}"`,
          ),
          stream.slice(-500),
        );
      });

      await check("a nar-size lie is refused after measurement", async () => {
        const body = narinfoFixture.replace(
          /NarSize: (\d+)/,
          (_all, size) => `NarSize: ${Number(size) + 1}`,
        );
        const res = await fetch(`${base}/${NARINFO_KEY}`, {
          method: "PUT",
          headers: auth(validToken),
          body,
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "nar_hash_mismatch");
        await untilEvent(
          events,
          `"narSizeDeclared":"${body.match(/NarSize: (\d+)/)[1]}"`,
        );
        const stream = events();
        const declared = body.match(/NarSize: (\d+)/)[1];
        const honest = narinfoFixture.match(/NarSize: (\d+)/)[1];
        assert.ok(
          stream.includes(`"narSizeDeclared":"${declared}"`),
          stream.slice(-500),
        );
        assert.ok(
          stream.includes(`"narSizeMeasured":"${honest}"`),
          stream.slice(-500),
        );
      });

      await check(
        "the fixture narinfo verifies and stores signed",
        async () => {
          const res = await fetch(`${base}/${NARINFO_KEY}`, {
            method: "PUT",
            headers: auth(validToken),
            body: narinfoFixture,
          });
          assert.equal(res.status, 204, await res.text());
        },
      );

      await check(
        "re-pushing a signed narinfo does not add a second signature",
        async () => {
          // The client builds its documents now, so nothing it sends
          // carries an inherited Sig. A narinfo read back out of this
          // cache does, and a client that hands one back must not make
          // the stored document grow: the previous behaviour appended a
          // second identical line on every push, substitute, re-push
          // cycle until the document hit its byte cap.
          const served = await fetch(`${base}/${NARINFO_KEY}`, {
            headers: READ_AUTH(),
          });
          assert.equal(served.status, 200);
          const signed = await served.text();
          const before = signed.match(/^Sig: .*$/gm) ?? [];
          // The fixture is nix-signed, so the stored document carries
          // nix's line and this deployment's.
          assert.equal(before.length, 2, signed);
          const deploymentKeyName = (
            await readFile(
              path.join(laneFixturesDir, "signing-key.public"),
              "utf8",
            )
          )
            .trim()
            .split(":")[0];
          const ours = before.filter((line) =>
            line.includes(`${deploymentKeyName}:`),
          );
          assert.equal(ours.length, 1, signed);

          const again = await fetch(`${base}/${NARINFO_KEY}`, {
            method: "PUT",
            headers: auth(validToken),
            body: signed,
          });
          assert.equal(again.status, 204, await again.text());
          const after = await fetch(`${base}/${NARINFO_KEY}`, {
            headers: READ_AUTH(),
          });
          const reserved = await after.text();
          assert.deepEqual(
            reserved.match(/^Sig: .*$/gm) ?? [],
            before,
            "signing a document this cache already signed changes nothing",
          );
        },
      );

      await check("a NAR write without its declared size is 411", async () => {
        // The decoder's ceiling is declared before the bytes move. A
        // write that omits it is refused for the same reason a write
        // without a content length is: the guard cannot run.
        const res = await fetch(`${base}/${NAR_KEY}`, {
          method: "PUT",
          headers: { authorization: `Bearer ${validToken}` },
          body: narBytes,
        });
        const text = await res.text();
        assert.equal(res.status, 411, text);
        assert.equal(JSON.parse(text).code, "length_required");
      });

      await check(
        "bytes that do not hash to their key never land",
        async () => {
          // The NAR key names the hash of the bytes it holds, and the
          // write measures them on the way past, so the disagreement is
          // caught by the request that carries them rather than by the
          // narinfo that would later name them.
          const wrongKey = `nar/${"9".repeat(52)}.nar.zst`;
          const res = await fetch(`${base}/${wrongKey}`, {
            method: "PUT",
            headers: auth(validToken),
            body: narBytes,
          });
          const text = await res.text();
          assert.equal(res.status, 400, text);
          assert.equal(JSON.parse(text).code, "file_hash_mismatch");
          // The refused object is gone, so nothing is left for a narinfo
          // to name.
          const gone = await fetch(`${base}/${wrongKey}`, {
            headers: READ_AUTH(),
          });
          assert.equal(gone.status, 404);
        },
      );

      await check(
        "a NAR's measured facts are unreachable from a request",
        async () => {
          // The facts live beside the object under the reserved meta
          // prefix, which key validation refuses before any lookup runs.
          const res = await fetch(`${base}/meta/${NAR_KEY}`, {
            headers: READ_AUTH(),
          });
          assert.ok(
            res.status === 400 || res.status === 404,
            `the facts document is not addressable: ${res.status}`,
          );
        },
      );

      await check(
        "the stored narinfo serves both signatures and the file facts",
        async () => {
          const res = await fetch(`${base}/${NARINFO_KEY}`, {
            headers: READ_AUTH(),
          });
          assert.equal(res.status, 200);
          const body = await res.text();
          assert.match(
            body,
            /^StorePath: \/nix\/store\/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9-/m,
          );
          assert.match(
            body,
            /^FileHash: sha256:11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y$/m,
          );
          assert.match(body, /^FileSize: \d+$/m);
          assert.match(body, /^Sig: cachet-fixture-1:/m);
          assert.match(body, /^Sig: lane-sign-1:/m);
          assert.equal(
            laneSignatureVerifies(body),
            true,
            "a client with the public key must verify this document",
          );
        },
      );

      await check("counting never breaks the thing it counts", async () => {
        // The lane binds CACHET_EVENTS, so stats::emit runs its real path
        // in every scenario rather than returning early on a missing
        // binding: the point is built, its blobs and doubles marshalled,
        // and handed to workerd, which discards it. What that proves is
        // the marshalling, not storage, and the proof is negative because
        // a discarded point leaves nothing to read back: a builder that
        // threw would have logged stats.write_failed beside the reads and
        // writes this scenario just made.
        assert.ok(
          !events().includes('"event":"stats.write_failed"'),
          `a counted request failed to marshal its point:\n${events().slice(-800)}`,
        );
      });
    },
  );

  await scenario(
    "a cold JWKS is 503, never a bypass",
    async () => [],
    async ({ base }) => {
      await check(
        "an unreachable JWKS endpoint answers 503, not a guess",
        async () => {
          const res = await fetch(`${base}/${NAR_KEY}`, {
            method: "PUT",
            headers: { authorization: `Bearer ${mint()}` },
            body: Buffer.from("x"),
          });
          assert.equal(res.status, 503);
          assert.equal((await res.json()).code, "auth_unavailable");
        },
      );
    },
    // The JWKS URL points at nothing: the isolate has no cached document,
    // the fetch cannot complete, and the only honest answer is 503.
    { CACHET_JWKS_URL: "http://127.0.0.1:9/jwks" },
  );

  await scenario(
    "multipart assembles, replays, and refuses wrong shapes",
    async () => [],
    async ({ base }) => {
      const validToken = mint();
      const auth = { authorization: `Bearer ${validToken}` };
      // The real fixture, assembled through the multipart route. The
      // completion measures what the parts assembled into, so the bytes
      // have to be a NAR that decodes and whose hash is the key naming
      // it: an object that measures as something else is refused and
      // deleted, which is the contract this scenario exists to hold.
      const narinfoFixture = await readFile(
        path.join(fixturesDir, NARINFO_KEY),
        "utf8",
      );
      const laneNarSize = narinfoFixture.match(/NarSize: (\d+)/)[1];
      const objectKey = NAR_KEY;
      const partBytes = await readFile(path.join(fixturesDir, NAR_FILE));

      await check(
        "creating an upload without a credential is 401",
        async () => {
          const res = await fetch(`${base}/${objectKey}?uploads`, {
            method: "POST",
          });
          assert.equal(res.status, 401, await res.text());
        },
      );

      await check(
        "creating an upload without the declared total is 411",
        async () => {
          const res = await fetch(`${base}/${objectKey}?uploads`, {
            method: "POST",
            headers: auth,
          });
          const text = await res.text();
          assert.equal(res.status, 411, text);
          assert.equal(JSON.parse(text).code, "length_required");
        },
      );

      const created = await (async () => {
        const res = await fetch(`${base}/${objectKey}?uploads`, {
          method: "POST",
          headers: {
            ...auth,
            "x-cachet-upload-bytes": String(partBytes.length),
            "x-cachet-nar-bytes": laneNarSize,
          },
        });
        const text = await res.text();
        assert.equal(res.status, 200, text);
        const body = JSON.parse(text);
        assert.equal(body.expectedParts, 1);
        assert.equal(typeof body.uploadId, "string");
        return body;
      })();
      const uploadId = created.uploadId;

      await check(
        "a part of the wrong size is refused once, cheaply",
        async () => {
          const res = await fetch(
            `${base}/${objectKey}?uploadId=${uploadId}&partNumber=1`,
            {
              method: "PUT",
              headers: auth,
              body: Buffer.concat([partBytes, Buffer.from("!")]),
            },
          );
          const text = await res.text();
          assert.equal(res.status, 400, text);
          assert.equal(JSON.parse(text).code, "part_size_mismatch");
        },
      );

      await check("a part number outside the plan is refused", async () => {
        const res = await fetch(
          `${base}/${objectKey}?uploadId=${uploadId}&partNumber=2`,
          {
            method: "PUT",
            headers: auth,
            body: partBytes,
          },
        );
        const text = await res.text();
        assert.equal(res.status, 400, text);
        assert.equal(JSON.parse(text).code, "part_number_invalid");
      });

      await check("an unknown upload id is 404", async () => {
        const res = await fetch(
          `${base}/${objectKey}?uploadId=no-such-upload&partNumber=1`,
          {
            method: "PUT",
            headers: auth,
            body: partBytes,
          },
        );
        const text = await res.text();
        assert.equal(res.status, 404, text);
        assert.equal(JSON.parse(text).code, "upload_unknown");
      });

      await check(
        "a completion disagreeing with the record is refused",
        async () => {
          const res = await fetch(`${base}/${objectKey}?uploadId=${uploadId}`, {
            method: "POST",
            headers: auth,
            body: JSON.stringify([]),
          });
          const text = await res.text();
          assert.equal(res.status, 400, text);
          assert.equal(JSON.parse(text).code, "complete_parts_mismatch");
        },
      );

      const etag = await (async () => {
        const res = await fetch(
          `${base}/${objectKey}?uploadId=${uploadId}&partNumber=1`,
          {
            method: "PUT",
            headers: auth,
            body: partBytes,
          },
        );
        const text = await res.text();
        assert.equal(res.status, 200, text);
        const body = JSON.parse(text);
        assert.equal(body.partNumber, 1);
        assert.equal(typeof body.etag, "string");
        return body.etag;
      })();

      await check("the upload completes and the object serves", async () => {
        const res = await fetch(`${base}/${objectKey}?uploadId=${uploadId}`, {
          method: "POST",
          headers: auth,
          body: JSON.stringify([{ partNumber: 1, etag }]),
        });
        assert.equal(res.status, 204, await res.text());
        const got = await fetch(`${base}/${objectKey}`, {
          headers: { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` },
        });
        assert.equal(got.status, 200);
        assert.deepEqual(Buffer.from(await got.arrayBuffer()), partBytes);
      });

      await check(
        "a replayed completion answers 204 to the same parts",
        async () => {
          const res = await fetch(`${base}/${objectKey}?uploadId=${uploadId}`, {
            method: "POST",
            headers: auth,
            body: JSON.stringify([{ partNumber: 1, etag }]),
          });
          assert.equal(res.status, 204, await res.text());
        },
      );

      await check("aborting an unknown upload is 404", async () => {
        const res = await fetch(
          `${base}/${objectKey}?uploadId=no-such-upload`,
          {
            method: "DELETE",
            headers: auth,
          },
        );
        assert.equal(res.status, 404, await res.text());
      });
    },
  );

  await scenario(
    "verdict tokens gate NAR reads and cache in KV",
    async () => [[NAR_KEY, await readFile(path.join(fixturesDir, NAR_FILE))]],
    async ({ base }) => {
      const laptop = { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` };

      await check(
        "a NAR read with a fresh verdict answers and caches",
        async () => {
          const first = await fetch(`${base}/${NAR_KEY}`, { headers: laptop });
          assert.equal(first.status, 200);
          const userHits = stubHits.user;
          const memberHits = stubHits.memberships;
          assert.ok(
            userHits >= 1 && memberHits >= 1,
            "the API served the miss",
          );
          const second = await fetch(`${base}/${NAR_KEY}`, { headers: laptop });
          assert.equal(second.status, 200);
          assert.equal(
            stubHits.user,
            userHits,
            "the KV verdict served the hit",
          );
          assert.equal(stubHits.memberships, memberHits);
        },
      );

      await check("an OIDC token opens reads, as CI expects", async () => {
        const res = await fetch(`${base}/${NAR_KEY}`, {
          headers: { authorization: `Bearer ${mint()}` },
        });
        assert.equal(res.status, 200);
      });

      await check("a denied token caches the denial briefly", async () => {
        const denied = { authorization: "Bearer wrong-token-entirely" };
        const before = stubHits.user;
        const first = await fetch(`${base}/${NAR_KEY}`, { headers: denied });
        assert.equal(first.status, 401);
        assert.equal(stubHits.user, before + 1, "the API answered the deny");
        const second = await fetch(`${base}/${NAR_KEY}`, { headers: denied });
        assert.equal(second.status, 401);
        assert.equal(stubHits.user, before + 1, "the denial is cached too");
      });
    },
  );

  // The memo's proof deletes the KV verdict between reads: a repeat the
  // API never sees can only come from the isolate's own entry, which is
  // the memo's whole claim.
  const forgetVerdict = (token, persist) => {
    const digest = crypto.createHash("sha256").update(token).digest("hex");
    const forget = spawnSync(
      "wrangler",
      [
        "kv",
        "key",
        "delete",
        `ghverdict/${digest}`,
        "--binding",
        "CACHET_KV",
        "--local",
        "--persist-to",
        persist,
        "--config",
        configPath,
      ],
      { encoding: "utf8" },
    );
    if (forget.status !== 0) {
      throw new Error(
        `verdict delete failed: ${forget.stderr}${forget.stdout}`,
      );
    }
  };

  await scenario(
    "the isolate memo answers after the verdict is gone",
    async () => [[NAR_KEY, await readFile(path.join(fixturesDir, NAR_FILE))]],
    async ({ base, events, persist }) => {
      const laptop = { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` };

      await check("an admit repeats without the API", async () => {
        const first = await fetch(`${base}/${NAR_KEY}`, { headers: laptop });
        assert.equal(first.status, 200);
        const apiHits = stubHits.user;
        forgetVerdict(GOOD_LAPTOP_TOKEN, persist);
        const second = await fetch(`${base}/${NAR_KEY}`, { headers: laptop });
        assert.equal(second.status, 200);
        assert.equal(stubHits.user, apiHits, "the memo served the repeat");
        await untilEvent(events, '"event":"auth.memo_hit","kind":"allow"');
      });

      await check("a denial repeats without the API", async () => {
        const denied = { authorization: "Bearer memo-denied-token" };
        const first = await fetch(`${base}/${NAR_KEY}`, { headers: denied });
        assert.equal(first.status, 401);
        const apiHits = stubHits.user;
        forgetVerdict("memo-denied-token", persist);
        const second = await fetch(`${base}/${NAR_KEY}`, { headers: denied });
        assert.equal(second.status, 401);
        assert.equal(stubHits.user, apiHits, "the memo denied the repeat");
        await untilEvent(events, '"event":"auth.memo_hit","kind":"deny"');
      });
    },
  );

  await scenario(
    "leases renew against the token's own claims",
    async () => [],
    async ({ base }) => {
      const laptop = { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` };
      const payload = {
        installables: [".#devShells.aarch64-darwin.default"],
        storePaths: ["/nix/store/0123456789abcdfghijklmnpqrsvwxyz-bash-5.2"],
      };

      await check(
        "a renewal off the default branch is 403 forbidden_ref",
        async () => {
          const res = await fetch(`${base}/roots/lane-org-lane-repo`, {
            method: "POST",
            headers: {
              authorization: `Bearer ${mint({ ref: "refs/heads/feature" })}`,
              "content-type": "application/json",
            },
            body: JSON.stringify(payload),
          });
          assert.equal(res.status, 403);
          assert.equal((await res.json()).code, "forbidden_ref");
        },
      );

      await check(
        "a renewal for another repo is 403 forbidden_project",
        async () => {
          const res = await fetch(`${base}/roots/lane-org-their-repo`, {
            method: "POST",
            headers: {
              authorization: `Bearer ${mint()}`,
              "content-type": "application/json",
            },
            body: JSON.stringify(payload),
          });
          assert.equal(res.status, 403);
          assert.equal((await res.json()).code, "forbidden_project");
        },
      );

      await check("a valid renewal stores and reads back", async () => {
        const res = await fetch(`${base}/roots/lane-org-lane-repo`, {
          method: "POST",
          headers: {
            authorization: `Bearer ${mint()}`,
            "content-type": "application/json",
          },
          body: JSON.stringify(payload),
        });
        assert.equal(res.status, 204, await res.text());
        const readBack = await fetch(`${base}/roots/lane-org-lane-repo`, {
          headers: laptop,
        });
        assert.equal(readBack.status, 200);
        const lease = await readBack.json();
        assert.equal(lease.project, "lane-org-lane-repo");
        assert.equal(lease.repository, "lane-org/lane-repo");
        assert.equal(lease.ref, "refs/heads/main");
        assert.deepEqual(lease.storePaths, payload.storePaths);
        assert.equal(typeof lease.renewedAtMs, "number");

        const list = await fetch(`${base}/roots`, { headers: laptop });
        assert.equal(list.status, 200);
        assert.equal(list.headers.get("cache-control"), "no-store");
        assert.deepEqual(await list.json(), {
          projects: ["lane-org-lane-repo"],
        });
      });

      await check("a lease read without a credential is 401", async () => {
        const res = await fetch(`${base}/roots/lane-org-lane-repo`);
        assert.equal(res.status, 401);
      });

      await check("a missing lease is 404", async () => {
        const res = await fetch(`${base}/roots/lane-org-nobody`, {
          headers: laptop,
        });
        assert.equal(res.status, 404);
      });
    },
  );

  await scenario(
    "the browser flow mints one session per state",
    async () => [[NAR_KEY, await readFile(path.join(fixturesDir, NAR_FILE))]],
    async ({ base }) => {
      const login = async () => {
        const res = await fetch(`${base}/_auth/login`, { redirect: "manual" });
        assert.equal(res.status, 302);
        assert.equal(res.headers.get("cache-control"), "no-store");
        const location = new URL(res.headers.get("location"));
        assert.equal(
          `${location.origin}${location.pathname}`,
          `${githubApiUrl}/login/oauth/authorize`,
        );
        const params = location.searchParams;
        assert.equal(params.get("client_id"), "lane-oauth-client");
        assert.equal(
          params.get("redirect_uri"),
          "https://cachet.lane.invalid/_auth/callback",
        );
        assert.equal(params.get("scope"), "read:org read:user");
        const state = params.get("state");
        assert.match(state, /^[A-Za-z0-9_-]{22}$/);
        return state;
      };

      const callback = (code, state) =>
        fetch(`${base}/_auth/callback?code=${code}&state=${state}`, {
          redirect: "manual",
        });

      await check("login redirects with exactly the contract", async () => {
        await login();
      });

      await check("a callback without its query fields is 400", async () => {
        const res = await fetch(`${base}/_auth/callback`, {
          redirect: "manual",
        });
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_oauth");
      });

      await check("the callback issues a session and redirects", async () => {
        const state = await login();
        const res = await callback(LANE_OAUTH_CODE, state);
        assert.equal(res.status, 302);
        assert.equal(res.headers.get("location"), "https://ui.lane.invalid");
        const cookies = res.headers.getSetCookie();
        assert.equal(cookies.length, 1);
        const cookie = cookies[0];
        assert.match(cookie, /^cachet_session=[A-Za-z0-9_-]{22}; /);
        for (const part of [
          "HttpOnly",
          "Secure",
          "SameSite=Lax",
          "Path=/",
          "Max-Age=1209600",
        ]) {
          assert.ok(cookie.includes(part), `the cookie carries ${part}`);
        }

        const sessionId = cookie.match(
          /^cachet_session=([A-Za-z0-9_-]{22});/,
        )[1];
        const session = { cookie: `cachet_session=${sessionId}` };

        // The session is see-only. It rides a cookie a browser sends by
        // itself and keeps for a fortnight without re-checking whether
        // its holder is still in the org, so a copy of it must not
        // substitute from the cache. Nix never sends a cookie, so
        // nothing that reads for real loses a credential here.
        for (const objectPath of [NAR_KEY, NARINFO_KEY]) {
          const read = await fetch(`${base}/${objectPath}`, {
            headers: session,
          });
          assert.equal(
            read.status,
            401,
            `a session does not open ${objectPath}`,
          );
          assert.equal((await read.json()).code, "unauthorized");
        }

        // What it does open is the console's own surface, starting with
        // the question a console asks before it renders anything.
        const who = await fetch(`${base}/api/whoami`, { headers: session });
        const whoText = await who.text();
        assert.equal(who.status, 200, whoText);
        const me = JSON.parse(whoText);
        assert.equal(me.login, "lane-dev");
        assert.equal(me.credential, "browser");
        assert.equal(me.admin, true, "lane-dev is CACHET_ADMINS");
        assert.ok(
          me.expiresAtMs > Date.now(),
          `the session names its own expiry: ${whoText}`,
        );
        const runs = await fetch(`${base}/api/self/gc-runs`, {
          headers: session,
        });
        assert.equal(runs.status, 200, "an admin session reads the reports");

        const exchanges = stubHits.exchange;
        const replay = await callback(LANE_OAUTH_CODE, state);
        assert.equal(replay.status, 401);
        assert.equal((await replay.json()).code, "oauth_state_unknown");
        assert.equal(
          stubHits.exchange,
          exchanges,
          "a replayed state never reaches the exchange",
        );

        const logout = await fetch(`${base}/logout`, {
          method: "POST",
          headers: { cookie: `cachet_session=${sessionId}` },
        });
        assert.equal(logout.status, 204);
        assert.ok(
          logout.headers.get("set-cookie").includes("Max-Age=0"),
          "logout expires the cookie",
        );
        const after = await fetch(`${base}/${NAR_KEY}`, {
          headers: { cookie: `cachet_session=${sessionId}` },
        });
        assert.equal(after.status, 401, "the deleted session is dead");
      });

      await check(
        "an unknown state is refused before the exchange",
        async () => {
          const exchanges = stubHits.exchange;
          const res = await callback(LANE_OAUTH_CODE, "bogus-state-value");
          assert.equal(res.status, 401);
          assert.equal((await res.json()).code, "oauth_state_unknown");
          assert.equal(stubHits.exchange, exchanges);
        },
      );

      await check("a refused code answers 401", async () => {
        const state = await login();
        const res = await callback("wrong-code", state);
        assert.equal(res.status, 401);
        assert.equal((await res.json()).code, "unauthorized");
        assert.equal(res.headers.getSetCookie().length, 0);
      });

      await check(
        "a non-member login is refused with forbidden_org",
        async () => {
          const state = await login();
          const res = await callback(LANE_OUTSIDER_CODE, state);
          assert.equal(res.status, 403);
          assert.equal((await res.json()).code, "forbidden_org");
          assert.equal(res.headers.getSetCookie().length, 0);
        },
      );
    },
  );
} finally {
  await rm(devVarsPath, { force: true });
}

// The collector's own helpers and scenarios: the scheduled handler over
// its dev endpoint, and bucket state read back through wrangler, because
// the reports and the cursor are internal keys by design.
const triggerScheduled = async (base) => {
  for (const path of ["/cdn-cgi/handler/scheduled", "/cdn-cgi/mf/scheduled"]) {
    const res = await fetch(`${base}${path}`);
    if (res.status === 200) {
      return true;
    }
  }
  return false;
};

const r2Get = async (persist, key) => {
  const got = spawnSync(
    "wrangler",
    [
      "r2",
      "object",
      "get",
      `cachet-lane/${key}`,
      "--local",
      "--persist-to",
      persist,
      "--config",
      configPath,
      "--pipe",
    ],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  );
  return { ok: got.status === 0, body: got.stdout, stderr: got.stderr };
};

const deadNarinfoFor = async (hash, narBase52) => {
  const doc = await readFile(path.join(fixturesDir, NARINFO_KEY), "utf8");
  return doc
    .replaceAll("qvqa04f0m85m0a6xxnan5vxnwg2jkgl9", hash)
    .replace(NAR_KEY, `nar/${narBase52}.nar.zst`);
};

const captureRunId = (events) => {
  const match = events().match(
    /"event":"gc\.(?:run_finished|run_aborted)"[^\n]*"runId":"(\d+-[0-9a-f]{16})"/,
  );
  assert.ok(
    match,
    `a concluded run logged its id in:\n${events().slice(-1200)}`,
  );
  return match[1];
};

const LEASE_DOC =
  JSON.stringify(
    {
      project: "lane-org-lane-repo",
      renewedAtMs: Date.now(),
      repository: "lane-org/lane-repo",
      ref: "refs/heads/main",
      runId: "41",
      commitSha: "abc",
      installables: [],
      storePaths: [
        "/nix/store/qvqa04f0m85m0a6xxnan5vxnwg2jkgl9-payload",
        `/nix/store/${"b".repeat(32)}-alive-two`,
        `/nix/store/${"c".repeat(32)}-alive-three`,
      ],
    },
    null,
    2,
  ) + "\n";

await scenario(
  "the collector sweeps the dead and spares the leased",
  async () => [
    [NARINFO_KEY, await readFile(path.join(fixturesDir, NARINFO_KEY))],
    [NAR_KEY, await readFile(path.join(fixturesDir, NAR_FILE))],
    ["roots/lane-org-lane-repo", LEASE_DOC],
    // why the roster: the fraction gate refuses a sweep past 25% of the
    // narinfo inventory, so the happy path needs a cache big enough that
    // one dead path is a quarter or less of it.
    [
      `${"b".repeat(32)}.narinfo`,
      await deadNarinfoFor("b".repeat(32), "q".repeat(52)),
    ],
    [
      `${"c".repeat(32)}.narinfo`,
      await deadNarinfoFor("c".repeat(32), "r".repeat(52)),
    ],
    [
      `${"d".repeat(32)}.narinfo`,
      await deadNarinfoFor("d".repeat(32), "w".repeat(52)),
    ],
    [
      `nar/${"q".repeat(52)}.nar.zst`,
      await readFile(path.join(fixturesDir, NAR_FILE)),
    ],
    [
      `nar/${"r".repeat(52)}.nar.zst`,
      await readFile(path.join(fixturesDir, NAR_FILE)),
    ],
    [
      `nar/${"w".repeat(52)}.nar.zst`,
      await readFile(path.join(fixturesDir, NAR_FILE)),
    ],
    [
      "uploads/stale-record-1",
      `${JSON.stringify({
        key: `nar/${"q".repeat(52)}.nar.zst`,
        totalBytes: 10,
        expectedParts: 1,
        narBytes: 30,
        createdAtMs: 0,
      })}\n`,
    ],
  ],
  async ({ base, events, persist }) => {
    await check("the scheduled run sweeps, reports, and reaps", async () => {
      assert.ok(
        await triggerScheduled(base),
        "the dev endpoint ran the handler",
      );

      const dead = await fetch(`${base}/${"d".repeat(32)}.narinfo`, {
        headers: READ_AUTH(),
      });
      assert.equal(
        dead.status,
        404,
        `the dead narinfo was swept; worker said:\n${events().slice(-2000)}`,
      );
      const alive = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(alive.status, 200, "the leased narinfo survived");
      const deadNar = await r2Get(persist, `nar/${"w".repeat(52)}.nar.zst`);
      assert.ok(!deadNar.ok, "the dead NAR followed its narinfo");
      const staleRecord = await r2Get(persist, "uploads/stale-record-1");
      assert.ok(!staleRecord.ok, "the stale upload record was reaped");
      const cursor = await r2Get(persist, "meta/gc-cursor");
      assert.ok(!cursor.ok, "the cursor is gone when the run ends");
      const generation = await r2Get(persist, "meta/generation");
      assert.ok(generation.ok, "the sweep bumped the generation");
      assert.ok(
        JSON.parse(generation.body).generation >= 1,
        `the epoch moved: ${generation.body}`,
      );

      const runId = captureRunId(events);
      const report = await r2Get(persist, `gc-reports/${runId}.json`);
      assert.ok(report.ok, `the report landed: ${report.stderr}`);
      const body = JSON.parse(report.body);
      assert.equal(body.gate, null);
      assert.equal(body.runId, runId);
      assert.equal(body.activeLeases, 1);
      assert.ok(body.narinfosDeleted >= 1, body.runId);
      assert.ok(body.narsDeleted >= 1, body.runId);
      assert.ok(body.bytesFreed > 0, body.runId);
      assert.equal(body.uploadsAborted, 1);
      const artifact = await r2Get(persist, `gc-runs/${runId}/mark.json`);
      assert.ok(artifact.ok, "the mark artifact landed");
    });

    const laptop = { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` };
    const member = { authorization: `Bearer ${MEMBER_TOKEN}` };

    await check("the health route reads the run it just landed", async () => {
      const anon = await fetch(`${base}/api/self/health`);
      assert.equal(anon.status, 401);
      const nonAdmin = await fetch(`${base}/api/self/health`, {
        headers: member,
      });
      assert.equal(nonAdmin.status, 403);
      assert.equal((await nonAdmin.json()).code, "forbidden_admin");

      const res = await fetch(`${base}/api/self/health`, { headers: laptop });
      const text = await res.text();
      assert.equal(res.status, 200, text);
      const body = JSON.parse(text);
      // The run this scenario just made finished seconds ago and tripped
      // no gate, which is the whole definition of healthy.
      assert.equal(body.status, "healthy", text);
      assert.equal(body.gate, undefined, text);
      assert.match(body.latestRunId, /^\d+-[0-9a-f]{16}$/);
      assert.ok(body.latestFinishedAtMs > 0, text);
      // The countdown is the lane's own cron, 05:00 UTC, and it is
      // always ahead: a console counting down to a moment already past
      // would render a negative duration.
      assert.ok(body.nextCollectionAtMs > Date.now(), text);
      const next = new Date(body.nextCollectionAtMs);
      assert.equal(next.getUTCHours(), 5, next.toISOString());
      assert.equal(next.getUTCMinutes(), 0, next.toISOString());
    });

    await check("the counter route gates before it queries", async () => {
      // The credential behind this route is a Cloudflare API token, so
      // the gate matters more here than on a route that only reads the
      // bucket. Nothing below gets far enough to need the token: the
      // lane binds none, and every row here is refused before the query.
      const anon = await fetch(`${base}/api/self/events?subject=reads`);
      assert.equal(anon.status, 401, await anon.text());

      const nonAdmin = await fetch(`${base}/api/self/events?subject=reads`, {
        headers: member,
      });
      const nonAdminText = await nonAdmin.text();
      assert.equal(nonAdmin.status, 403, nonAdminText);
      assert.equal(JSON.parse(nonAdminText).code, "forbidden_admin");

      // An admin choosing something this deployment does not offer is
      // told so, rather than quietly answered a different question. The
      // hostile shapes are the ones that would matter if the choice ever
      // reached a statement.
      for (const query of [
        "subject=reads&by=blob1,%20blob2",
        "subject=reads&by=actor'%3B%20DROP%20TABLE%20x%3B%20--",
        "subject=reads&window=decade",
        "subject=everything",
        "by=actor",
        // A bucket finer than its window can hold: 720 hourly rows
        // against a cap of 100. Refused rather than truncated, because a
        // truncated series is a chart that starts partway through its
        // own window without saying so.
        "subject=reads&by=hour&window=month",
        "subject=reads&by=hour&window=week",
        // A filter that names nothing narrows to nothing loudly. Quietly
        // answering the unfiltered question would make a typo look like
        // a deployment where every read came from a laptop.
        "subject=reads&actor=nobody",
        "subject=reads&actor=laptop'%20OR%20'1'%3D'1",
        "subject=reads&kind=lease",
        "subject=reads&outcome=4041",
        "subject=reads&outcome=99",
      ]) {
        const res = await fetch(`${base}/api/self/events?${query}`, {
          headers: laptop,
        });
        const text = await res.text();
        assert.equal(res.status, 400, `${query} -> ${text}`);
        assert.equal(JSON.parse(text).code, "malformed_query", query);
      }

      // A valid choice with no token bound reports the deployment's own
      // misconfiguration, not the caller's: it counts, it cannot report.
      const unconfigured = await fetch(
        `${base}/api/self/events?subject=reads&by=actor`,
        { headers: laptop },
      );
      const unconfiguredText = await unconfigured.text();
      assert.equal(unconfigured.status, 503, unconfiguredText);
      assert.equal(JSON.parse(unconfiguredText).code, "storage_unavailable");
    });

    await check("the reports API serves admins and nobody else", async () => {
      const anon = await fetch(`${base}/api/self/gc-runs`);
      assert.equal(anon.status, 401);
      const nonAdmin = await fetch(`${base}/api/self/gc-runs`, {
        headers: member,
      });
      assert.equal(nonAdmin.status, 403);
      assert.equal((await nonAdmin.json()).code, "forbidden_admin");

      const list = await fetch(`${base}/api/self/gc-runs`, { headers: laptop });
      assert.equal(list.status, 200);
      assert.equal(list.headers.get("cache-control"), "no-store");
      const runs = await list.json();
      assert.ok(runs.runs.includes(captureRunId(events)), JSON.stringify(runs));

      const detail = await fetch(
        `${base}/api/self/gc-runs/${captureRunId(events)}`,
        {
          headers: laptop,
        },
      );
      assert.equal(detail.status, 200);
      assert.equal((await detail.json()).gate, null);

      const missing = await fetch(
        `${base}/api/self/gc-runs/1000000000000-0000000000000000`,
        { headers: laptop },
      );
      assert.equal(missing.status, 404);
      const malformed = await fetch(`${base}/api/self/gc-runs/nope`, {
        headers: laptop,
      });
      assert.equal(malformed.status, 400);
      assert.equal((await malformed.json()).code, "malformed_key");

      const stats = await fetch(`${base}/api/self/stats`, { headers: laptop });
      assert.equal(stats.status, 200);
      const shape = await stats.json();
      assert.equal(shape.basedOnRunId, captureRunId(events));
      assert.equal(shape.inventoryPaths, 4);
      assert.equal(shape.gate, null);
    });
  },
);

await scenario(
  "the fraction gate aborts a wholesale sweep",
  async () => [
    [
      `${"d".repeat(32)}.narinfo`,
      await deadNarinfoFor("d".repeat(32), "w".repeat(52)),
    ],
    [
      `${"v".repeat(32)}.narinfo`,
      await deadNarinfoFor("v".repeat(32), "q".repeat(52)),
    ],
    [
      `${"x".repeat(32)}.narinfo`,
      await deadNarinfoFor("x".repeat(32), "r".repeat(52)),
    ],
    [
      `nar/${"w".repeat(52)}.nar.zst`,
      await readFile(path.join(fixturesDir, NAR_FILE)),
    ],
  ],
  async ({ base, events, persist }) => {
    await check(
      "a run that would empty the cache deletes nothing",
      async () => {
        assert.ok(
          await triggerScheduled(base),
          "the dev endpoint ran the handler",
        );
        const stillThere = await fetch(`${base}/${"d".repeat(32)}.narinfo`, {
          headers: READ_AUTH(),
        });
        assert.equal(stillThere.status, 200, "the gate kept every key");
        assert.ok(
          events().includes('"event":"gc.gate_tripped"'),
          `the trip logged:\n${events().slice(-800)}`,
        );
        const runId = captureRunId(events);
        const report = await r2Get(persist, `gc-reports/${runId}.json`);
        const body = JSON.parse(report.body);
        assert.equal(body.gate, "sweep_fraction_exceeded");
        assert.equal(body.narinfosDeleted, 0);
        const cursor = await r2Get(persist, "meta/gc-cursor");
        assert.ok(!cursor.ok, "an aborted run still ends: no parked cursor");
      },
    );
  },
);

// The bulk probe: one authorized POST answers presence for a run's whole
// candidate set, derived from a bucket enumeration (delimiter-collapsed,
// suffix-filtered), so the answer is bucket truth the same way the
// collector's inventory is. The wire facts below: the answer is the held
// subset sorted and deduplicated, a NAR object never leaks into it, and
// the rejection rows complete the malformed_probe coverage the error
// table demands.
await scenario(
  "the bulk probe answers presence from the bucket",
  async () => [
    [`${"q".repeat(32)}.narinfo`, "the probe never reads this body\n"],
    [`${"z".repeat(32)}.narinfo`, "nor this one\n"],
    [`nar/${"n".repeat(52)}.nar.zst`, "a NAR is not a narinfo\n"],
    ["roots/lane-org-lane-repo", "a lease is not a narinfo\n"],
  ],
  async ({ base }) => {
    const post = (body, headers = READ_AUTH()) =>
      fetch(`${base}/api/probe`, {
        method: "POST",
        headers: { "content-type": "application/json", ...headers },
        body,
      });

    await check(
      "the answer is the held subset, sorted and unique",
      async () => {
        const res = await post(
          JSON.stringify({
            paths: [
              "z".repeat(32),
              "0".repeat(32),
              "q".repeat(32),
              "q".repeat(32),
            ],
          }),
        );
        const answer = await res.json();
        assert.equal(res.status, 200, JSON.stringify(answer));
        assert.equal(res.headers.get("cache-control"), "no-store");
        assert.deepEqual(answer, {
          present: ["q".repeat(32), "z".repeat(32)],
        });
      },
    );

    await check(
      "an OIDC token answers reads, so it answers the probe",
      async () => {
        const res = await post(JSON.stringify({ paths: ["q".repeat(32)] }), {
          authorization: `Bearer ${mint()}`,
        });
        const answer = await res.json();
        assert.equal(res.status, 200, JSON.stringify(answer));
        assert.deepEqual(answer, { present: ["q".repeat(32)] });
      },
    );

    await check("a probe without a credential is 401", async () => {
      const res = await fetch(`${base}/api/probe`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ paths: [] }),
      });
      assert.equal(res.status, 401);
      assert.equal((await res.json()).code, "unauthorized");
    });

    await check("an outsider's OIDC token is 403 forbidden_org", async () => {
      const res = await post(JSON.stringify({ paths: [] }), {
        authorization: `Bearer ${mint({ repository_owner: "not-lane-org" })}`,
      });
      assert.equal(res.status, 403);
      assert.equal((await res.json()).code, "forbidden_org");
    });

    await check("an unparseable body is 400 malformed_probe", async () => {
      const res = await post("this is not json");
      assert.equal(res.status, 400);
      assert.equal((await res.json()).code, "malformed_probe");
    });

    await check(
      "a hash outside the grammar is 400 malformed_probe",
      async () => {
        const res = await post(JSON.stringify({ paths: ["not-a-hash"] }));
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_probe");
      },
    );

    await check(
      "an entry list over the cap is 400 malformed_probe",
      async () => {
        const res = await post(
          JSON.stringify({ paths: Array(16_385).fill("a".repeat(32)) }),
        );
        assert.equal(res.status, 400);
        assert.equal((await res.json()).code, "malformed_probe");
      },
    );

    await check("a body over the byte cap is 413 body_too_large", async () => {
      const res = await post(
        JSON.stringify({ paths: ["a".repeat(1_100_000)] }),
      );
      assert.equal(res.status, 413);
      assert.equal((await res.json()).code, "body_too_large");
    });
  },
);

// The counter route's answering half, which nothing has ever run. Every
// other counter row in this lane stops at the configuration check,
// because a deployment without a stats token counts and cannot report;
// with the token bound and CACHET_STATS_API_URL pointed at the driver's
// stub, the statement the worker composed arrives as text and what comes
// back is deserialized, shaped, and served for real.
await writeFile(
  devVarsPath,
  `CACHET_SIGNING_KEY=${laneSigningSecret}\nCACHET_OAUTH_CLIENT_SECRET=${LANE_OAUTH_SECRET}\nCACHET_STATS_TOKEN=${LANE_STATS_TOKEN}\n`,
);
try {
  await scenario(
    "the counter route asks what it was told to and answers what came back",
    async () => [],
    async ({ base }) => {
      const laptop = { authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` };
      const ask = async (query) => {
        statsStub.sql = [];
        const res = await fetch(`${base}/api/self/events?${query}`, {
          headers: laptop,
        });
        return { res, text: await res.text() };
      };

      await check(
        "a deployment that has never collected is unknown, not broken",
        async () => {
          // This scenario seeds nothing, so there is no latest report.
          // /api/self/stats answers 404, which is the honest shape for a
          // projection with nothing to project; health answers 200 with
          // a status, because it renders in a header on every screen and
          // a failing header reads as a broken console.
          const stats = await fetch(`${base}/api/self/stats`, {
            headers: laptop,
          });
          assert.equal(stats.status, 404);

          const res = await fetch(`${base}/api/self/health`, {
            headers: laptop,
          });
          const text = await res.text();
          assert.equal(res.status, 200, text);
          const body = JSON.parse(text);
          assert.equal(body.status, "unknown", text);
          assert.equal(body.latestRunId, undefined, text);
          assert.equal(body.gate, undefined, text);
          // The countdown does not depend on a run having happened.
          assert.ok(body.nextCollectionAtMs > Date.now(), text);
        },
      );

      await check("a dimension list is asked for and served back", async () => {
        statsStub.rows = [
          { dimension: "edge_hit", count: 11904, bytes: 44_236_800 },
          { dimension: "miss", count: 564, bytes: 0 },
        ];
        const { res, text } = await ask("subject=reads&by=outcome&window=week");
        assert.equal(res.status, 200, text);
        // The statement is asserted whole. A lane matching substrings
        // lets a clause move, an order flip, or a bound change without
        // anything failing, and this statement runs with an account
        // token behind it.
        assert.equal(
          statsStub.sql[0],
          "SELECT blob2 AS dimension, " +
            "SUM(_sample_interval * double1) AS count, " +
            "SUM(_sample_interval * double2) AS bytes " +
            "FROM cachet_lane " +
            "WHERE index1 = 'read' " +
            "AND timestamp > NOW() - INTERVAL '7' DAY " +
            "GROUP BY dimension ORDER BY count DESC LIMIT 100",
          statsStub.sql[0],
        );
        assert.equal(
          statsStub.authorization,
          `Bearer ${LANE_STATS_TOKEN}`,
          "the query carries the deployment's own token",
        );
        const body = JSON.parse(text);
        assert.equal(body.subject, "reads");
        assert.equal(body.dimension, "outcome");
        assert.equal(body.window, "week");
        assert.deepEqual(body.filters, {});
        assert.equal(body.rows.length, 2);
        assert.equal(body.rows[0].dimension, "edge_hit");
        assert.equal(body.rows[0].count, 11904);
        assert.equal(body.rows[0].bytes, 44_236_800);
      });

      await check("a filtered question narrows with literals", async () => {
        statsStub.rows = [
          { dimension: "bucket_hit", count: 610, bytes: 1_300 },
        ];
        const { res, text } = await ask(
          "subject=reads&by=outcome&window=week&actor=laptop",
        );
        assert.equal(res.status, 200, text);
        assert.ok(
          statsStub.sql[0].includes(
            "WHERE index1 = 'read' AND blob3 = 'laptop' AND timestamp >",
          ),
          statsStub.sql[0],
        );
        // The answer says what it was narrowed to, so a caller reading a
        // chart never has to trust that its own query string arrived.
        assert.deepEqual(JSON.parse(text).filters, { actor: "laptop" });
      });

      await check("every filter stacks in column order", async () => {
        statsStub.rows = [];
        const { res, text } = await ask(
          "subject=writes&by=repository&window=month&kind=nar&outcome=404&actor=ci",
        );
        assert.equal(res.status, 200, text);
        assert.ok(
          statsStub.sql[0].includes(
            "WHERE index1 = 'write' AND blob1 = 'nar' AND blob2 = '404' " +
              "AND blob3 = 'ci' AND timestamp >",
          ),
          statsStub.sql[0],
        );
        assert.deepEqual(JSON.parse(text).filters, {
          kind: "nar",
          outcome: "404",
          actor: "ci",
        });
      });

      await check(
        "a series is asked for by time and comes back whole",
        async () => {
          // Two of seven days reported. Analytics Engine says nothing
          // about the other five, and a line drawn through the silence
          // would claim traffic was smooth when it was absent.
          const day = 86_400;
          const newest = Math.floor(Date.now() / 1000 / day) * day;
          statsStub.rows = [
            { dimension: String(newest), count: 1_508, bytes: 5_000 },
            { dimension: String(newest - day * 3), count: 2_133, bytes: 9_000 },
          ];
          const { res, text } = await ask("subject=reads&by=day&window=week");
          assert.equal(res.status, 200, text);
          assert.equal(
            statsStub.sql[0],
            "SELECT toString(intDiv(toUInt32(timestamp), 86400) * 86400) AS dimension, " +
              "SUM(_sample_interval * double1) AS count, " +
              "SUM(_sample_interval * double2) AS bytes " +
              "FROM cachet_lane " +
              "WHERE index1 = 'read' " +
              "AND timestamp > NOW() - INTERVAL '7' DAY " +
              "GROUP BY dimension ORDER BY dimension ASC LIMIT 7",
            statsStub.sql[0],
          );
          const body = JSON.parse(text);
          assert.equal(body.dimension, "day");
          assert.equal(body.rows.length, 7, "one row per day, holes included");
          assert.equal(body.rows[6].dimension, String(newest));
          assert.equal(body.rows[6].count, 1_508);
          assert.equal(body.rows[3].count, 2_133);
          assert.equal(body.rows[0].count, 0, "an empty day counts zero");
          assert.equal(body.rows[0].bytes, 0);
          for (let i = 1; i < body.rows.length; i += 1) {
            assert.equal(
              Number(body.rows[i].dimension) -
                Number(body.rows[i - 1].dimension),
              day,
              "ascending and contiguous",
            );
          }
        },
      );

      await check("an hourly series fits its day", async () => {
        statsStub.rows = [];
        const { res, text } = await ask("subject=probes&by=hour&window=day");
        assert.equal(res.status, 200, text);
        assert.ok(
          statsStub.sql[0].includes(
            "toString(intDiv(toUInt32(timestamp), 3600) * 3600)",
          ),
          statsStub.sql[0],
        );
        assert.ok(statsStub.sql[0].endsWith("ORDER BY dimension ASC LIMIT 24"));
        assert.equal(JSON.parse(text).rows.length, 24);
      });

      await check(
        "an upstream refusal answers 503 and says nothing about upstream",
        async () => {
          statsStub.status = 500;
          const { res, text } = await ask("subject=reads&by=actor");
          statsStub.status = 200;
          assert.equal(res.status, 503, text);
          const body = JSON.parse(text);
          assert.equal(body.code, "storage_unavailable");
          // The upstream answers about an account, not about this cache.
          assert.ok(!text.includes("lane refusal"), text);
        },
      );
    },
    { CACHET_STATS_API_URL: statsApiUrl },
  );
} finally {
  await rm(devVarsPath, { force: true });
}

// The write path's other half is the CLI itself (crates/cachet-push): its
// unit fakes answer over a scripted wire, so this scenario runs the real
// pipeline — real nix-store, real staging tree, real token mints against
// the stub — over the same HTTP the integration lane rides live. It signs
// too, so .dev.vars brackets it the same way as the write scenarios.
await writeFile(
  devVarsPath,
  `CACHET_SIGNING_KEY=${laneSigningSecret}\nCACHET_OAUTH_CLIENT_SECRET=${LANE_OAUTH_SECRET}\n`,
);
try {
  await scenario(
    "the CLI pushes a store path end to end",
    async () => [],
    async ({ base }) => {
      // why: `just workerd` builds this binary before driving; the scenario
      // fails loudly rather than silencing a stale or missing build.
      const cli = path.join(repoRoot, "target", "debug", "cachet");
      const runnerTemp = await mkdtemp(path.join(os.tmpdir(), "cachet-cli-"));
      const oidcUrl = `http://127.0.0.1:${stubServer.address().port}/oidc-token`;
      // why: run the CLI asynchronously. The stub server lives in this same
      // process's event loop, and the CLI talks to it mid-push (the OIDC
      // mint, the upstream probes); a spawnSync would freeze the stub the
      // CLI is waiting on, deadlocking both.
      const runCli = (args, env) =>
        new Promise((resolve, reject) => {
          const child = spawn(cli, args, { env });
          let stdout = "";
          let stderr = "";
          child.stdout.on("data", (chunk) => (stdout += chunk));
          child.stderr.on("data", (chunk) => (stderr += chunk));
          child.on("error", reject);
          child.on("close", (status) => resolve({ status, stdout, stderr }));
        });
      const pushEnv = {
        ...process.env,
        RUNNER_TEMP: runnerTemp,
        CACHET_CACHE_URL: base,
        CACHET_AUDIENCE: "cachet-lane",
        CACHET_PROJECT: "lane-org-lane-repo",
        CACHET_UPSTREAM_URL: `http://127.0.0.1:${stubServer.address().port}`,
        CACHET_PUSH: "true",
        GITHUB_REF: "refs/heads/main",
        ACTIONS_ID_TOKEN_REQUEST_URL: oidcUrl,
        ACTIONS_ID_TOKEN_REQUEST_TOKEN: "lane-request-token",
        NIX_CONFIG: "experimental-features = nix-command flakes",
      };
      let storePath = "";

      await check(
        "the composite's snapshot step records the store",
        async () => {
          const snap = await runCli(["push", "--snapshot-only"], pushEnv);
          assert.equal(snap.status, 0, snap.stderr);
          assert.ok(
            snap.stdout.includes("cachet: store snapshot taken"),
            snap.stdout,
          );
          assert.ok(
            existsSync(path.join(runnerTemp, "cachet-store-before.txt")),
            "the hand-off file landed",
          );
        },
      );

      await check("the push uploads exactly the payload", async () => {
        const payload = path.join(runnerTemp, "lane-payload");
        // why: the content decides the store path, so it has to be new
        // every run. It used to be the driver's pid, which the operating
        // system recycles: a run that drew a pid some earlier run had
        // already pushed found its path in the snapshot, uploaded
        // nothing, minted nothing, and failed three checks at once with
        // nothing in the message to say why.
        await writeFile(
          payload,
          `cachet workerd lane payload ${crypto.randomUUID()}\n`,
        );
        const added = spawnSync(
          "nix-store",
          ["--add-fixed", "sha256", payload],
          {
            encoding: "utf8",
          },
        );
        assert.equal(added.status, 0, added.stderr);
        storePath = added.stdout.trim().split("\n").pop();
        const mintsBefore = stubHits.oidcMint;
        const pushed = await runCli(["push"], {
          ...pushEnv,
          CACHET_ROOTS: storePath,
        });
        assert.equal(pushed.status, 0, pushed.stderr);
        assert.equal(
          stubHits.oidcMint - mintsBefore,
          1,
          `one mint carries the whole push run, saw ${
            stubHits.oidcMint - mintsBefore
          }: ${pushed.stdout}`,
        );
        assert.ok(
          pushed.stdout.includes("cachet: 1 new to cachet"),
          pushed.stdout,
        );
        assert.ok(
          pushed.stdout.includes("cachet: uploaded 2 objects"),
          pushed.stdout,
        );
        assert.ok(
          pushed.stdout.includes(
            "cachet: lease renewed for lane-org-lane-repo",
          ),
          pushed.stdout,
        );
      });

      await check(
        "the pushed narinfo serves, signed, naming the payload",
        async () => {
          assert.ok(storePath, "the push check left a store path");
          const hash = storePath.match(/\/nix\/store\/([a-z0-9]+)-/)[1];
          const narinfo = await fetch(`${base}/${hash}.narinfo`, {
            headers: READ_AUTH(),
          });
          assert.equal(narinfo.status, 200);
          const body = await narinfo.text();
          assert.ok(
            body.includes(`StorePath: ${storePath}`),
            `storePath=${storePath}\n--- body ---\n${body}`,
          );
          assert.ok(body.includes("Sig: lane-sign-1:"), body);
          assert.equal(
            laneSignatureVerifies(body),
            true,
            "a client with the public key must verify this document",
          );
          const nar = await fetch(`${base}/${body.match(/^URL: (.+)$/m)[1]}`, {
            headers: READ_AUTH(),
          });
          assert.equal(nar.status, 200, "the narinfo never dangles");
        },
      );

      await check(
        "a multipart-sized payload uploads and verifies",
        async () => {
          // why: a nar past the single-PUT cap rides the real multipart
          // quartet against the real worker, whose composed byte faith is
          // what verify-then-sign then judges — small payloads never find
          // a slicing bug. 100 MiB of incompressible bytes forces parts.
          const payload = path.join(runnerTemp, "lane-payload-big");
          const fill = spawnSync(
            "dd",
            ["if=/dev/urandom", `of=${payload}`, "bs=1048576", "count=100"],
            { encoding: "utf8" },
          );
          assert.equal(fill.status, 0, fill.stderr);
          const added = spawnSync(
            "nix-store",
            ["--add-fixed", "sha256", payload],
            { encoding: "utf8" },
          );
          assert.equal(added.status, 0, added.stderr);
          const bigPath = added.stdout.trim().split("\n").pop();
          const mintsBefore = stubHits.oidcMint;
          const pushed = await runCli(["push"], {
            ...pushEnv,
            CACHET_ROOTS: bigPath,
          });
          assert.equal(pushed.status, 0, pushed.stderr);
          assert.equal(
            stubHits.oidcMint - mintsBefore,
            1,
            `one mint carries the multipart run, parts included, saw ${
              stubHits.oidcMint - mintsBefore
            }: ${pushed.stdout}`,
          );
          assert.ok(
            pushed.stdout.includes("cachet: uploaded 2 objects"),
            pushed.stdout,
          );
          const hash = bigPath.match(/\/nix\/store\/([a-z0-9]+)-/)[1];
          const narinfo = await fetch(`${base}/${hash}.narinfo`, {
            headers: READ_AUTH(),
          });
          assert.equal(
            narinfo.status,
            200,
            "the multipart nar verified and signed",
          );
        },
      );
    },
  );
} finally {
  await rm(devVarsPath, { force: true });
}

const failed = results.filter(([status]) => status !== "ok").length;
if (failed > 0) {
  process.stdout.write(`${failed} workerd assertion(s) failed\n`);
  process.exit(1);
}
process.stdout.write("workerd lane green\n");
// why: the stub server's listen handle (and its keep-alive sockets from the
// worker's fetches) would hold the event loop open forever; close it so a
// green lane exits.
stubServer.closeAllConnections();
stubServer.close();
