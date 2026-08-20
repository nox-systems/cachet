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
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const repoRoot = path.resolve(import.meta.dirname, "..");
const configPath = path.join(repoRoot, "workerd", "wrangler.toml");
const fixturesDir = path.resolve(
  process.argv[2] ?? path.join(repoRoot, "fixtures", "nix-signed"),
);

const NARINFO_KEY = "qvqa04f0m85m0a6xxnan5vxnwg2jkgl9.narinfo";
const NAR_FILE = "11lx23nn3dpc8mqp0ncnm6wqcxs6pfw32bp8n9c1fkafyzjvn16y.nar.zst";
const NAR_KEY = `nar/${NAR_FILE}`;
const OTHER_NARINFO = "33333333333333333333333333333333.narinfo";

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push(["ok", name]);
    process.stdout.write(`ok ${name}\n`);
  } catch (failure) {
    results.push(["FAIL", `${name}: ${failure.message}`]);
    process.stdout.write(`FAIL ${name}: ${failure.message}\n`);
  }
}

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 200));

// One scenario per persistence directory: fresh R2, fresh edge cache.
async function scenario(name, seed, assertions) {
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

  const port = await freePort();
  const proc = spawn(
    "wrangler",
    [
      "dev",
      "--local",
      "--port",
      String(port),
      "--persist-to",
      persist,
      "--config",
      configPath,
    ],
    { detached: true, stdio: ["ignore", "pipe", "pipe"] },
  );
  let captured = "";
  proc.stdout.on("data", (chunk) => (captured += chunk));
  proc.stderr.on("data", (chunk) => (captured += chunk));
  const base = `http://127.0.0.1:${port}`;

  try {
    const deadline = Date.now() + 60_000;
    let up = false;
    while (Date.now() < deadline) {
      try {
        const probe = await fetch(`${base}/nix-cache-info`);
        if (probe.status === 200) {
          up = true;
          break;
        }
      } catch {
        // Not listening yet.
      }
      if (captured.includes("ERROR")) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    if (!up) {
      throw new Error(`workerd never came up:\n${captured.slice(-2000)}`);
    }
    const events = () => captured;
    const clearEvents = () => (captured = "");
    await assertions({ base, events, clearEvents });
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
      const res = await fetch(`${base}/${NARINFO_KEY}`);
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
      await settle();
    });

    await check("the second get comes from the edge", async () => {
      clearEvents();
      const res = await fetch(`${base}/${NARINFO_KEY}`);
      assert.equal(res.status, 200);
      await settle();
      assert.match(events(), /"event":"read\.edge_hit"/);
      assert.doesNotMatch(events(), /"event":"read\.bucket_hit"/);
    });

    await check("a missing narinfo is a cacheable 404", async () => {
      clearEvents();
      const res = await fetch(`${base}/${OTHER_NARINFO}`);
      assert.equal(res.status, 404);
      assert.equal(
        res.headers.get("content-type"),
        "text/plain; charset=utf-8",
      );
      assert.equal(res.headers.get("cache-control"), "public, max-age=30");
      assert.equal(await res.text(), "not found\n");
      await settle();
    });

    await check(
      "the missing narinfo answers from the edge the second time",
      async () => {
        clearEvents();
        const res = await fetch(`${base}/${OTHER_NARINFO}`);
        assert.equal(res.status, 404);
        await settle();
        assert.match(events(), /"event":"read\.edge_hit"/);
        assert.doesNotMatch(events(), /"event":"read\.miss"/);
      },
    );

    await check("HEAD answers from bucket metadata alone", async () => {
      const res = await fetch(`${base}/${NARINFO_KEY}`, { method: "HEAD" });
      assert.equal(res.status, 200);
      assert.equal(res.headers.get("content-type"), "text/x-nix-narinfo");
      assert.equal(await res.text(), "");
    });

    await check("HEAD on a miss is an empty 404", async () => {
      const res = await fetch(`${base}/${OTHER_NARINFO}`, { method: "HEAD" });
      assert.equal(res.status, 404);
      assert.equal(await res.text(), "");
    });

    await check("a NAR streams with its wire headers", async () => {
      const res = await fetch(`${base}/${NAR_KEY}`);
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
      const first = await fetch(`${base}/${NARINFO_KEY}`);
      assert.equal(first.status, 200);
      await settle();
      clearEvents();
      const second = await fetch(`${base}/${NARINFO_KEY}`);
      assert.equal(second.status, 200);
      await settle();
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
      const first = await fetch(`${base}/${OTHER_NARINFO}`);
      assert.equal(first.status, 404);
      await settle();
      clearEvents();
      const second = await fetch(`${base}/${OTHER_NARINFO}`);
      assert.equal(second.status, 404);
      await settle();
      assert.match(events(), /"event":"read\.edge_hit"/);
      assert.doesNotMatch(events(), /"event":"read\.miss"/);
    });
  },
);

const failed = results.filter(([status]) => status !== "ok").length;
if (failed > 0) {
  process.stdout.write(`${failed} workerd assertion(s) failed\n`);
  process.exit(1);
}
process.stdout.write("workerd lane green\n");
