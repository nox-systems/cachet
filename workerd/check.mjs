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

// The lane's plain read credential for object GET/HEADs: every object
// read answers 401 without one.
const READ_AUTH = () => ({ authorization: `Bearer ${GOOD_LAPTOP_TOKEN}` });
const stubHits = { user: 0, memberships: 0, exchange: 0 };
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
    return json(200, {
      count: 1,
      value: mint({ aud: audience ?? "cachet-lane" }),
    });
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
    return memberHit
      ? json(200, { state: "active" })
      : json(404, { message: "Not Found" });
  }
  json(404, { message: "Not Found" });
});
await new Promise((resolve) => stubServer.listen(0, "127.0.0.1", resolve));
const jwksUrl = `http://127.0.0.1:${stubServer.address().port}/jwks.json`;
const githubApiUrl = `http://127.0.0.1:${stubServer.address().port}`;

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
  "a corrupt generation document bypasses the edge",
  async () => {
    const narinfo = await readFile(path.join(fixturesDir, NARINFO_KEY));
    return [
      [NARINFO_KEY, narinfo],
      ["meta/generation", "this was never json"],
    ];
  },
  async ({ base, events, clearEvents }) => {
    await check("the service degrades to bucket reads", async () => {
      const first = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(first.status, 200);
      clearEvents();
      const second = await fetch(`${base}/${NARINFO_KEY}`, {
        headers: READ_AUTH(),
      });
      assert.equal(second.status, 200);
      await untilEvent(events, '"event":"generation.document_corrupt"');
      await untilEvent(events, '"event":"read.bucket_hit"');
      assert.match(events(), /"event":"generation\.document_corrupt"/);
      assert.match(events(), /"event":"read\.bucket_hit"/);
      assert.doesNotMatch(events(), /"event":"read\.edge_hit"/);
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
      const auth = (token) => ({ authorization: `Bearer ${token}` });

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
        },
      );
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
      const objectKey = `nar/${"9".repeat(52)}.nar.zst`;
      const partBytes = Buffer.from("not the fixture nar's bytes".repeat(4)); // 108 bytes

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
        const read = await fetch(`${base}/${NAR_KEY}`, {
          headers: { cookie: `cachet_session=${sessionId}` },
        });
        assert.equal(read.status, 200, "the session opens reads");

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
        await writeFile(
          payload,
          `cachet workerd lane payload ${process.pid}\n`,
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
        const pushed = await runCli(["push"], {
          ...pushEnv,
          CACHET_ROOTS: storePath,
        });
        assert.equal(pushed.status, 0, pushed.stderr);
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
          const pushed = await runCli(["push"], {
            ...pushEnv,
            CACHET_ROOTS: bigPath,
          });
          assert.equal(pushed.status, 0, pushed.stderr);
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
