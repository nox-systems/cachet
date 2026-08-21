// The main step: snapshot the store before the consumer's own steps run.
// why: this wrapper exists because only a JavaScript action can declare a
// post step; every decision lives in the binary. Failures are log lines,
// never a red job: the post step treats a missing snapshot as an empty
// before-set, which the push's own candidate bound then keeps honest.
const { spawnSync } = require("node:child_process");

const bin = process.env["CACHET_BIN"];
if (bin === undefined || bin.length === 0) {
  process.stderr.write(
    "cachet: CACHET_BIN is unset; the download step did not run, so no snapshot was taken\n",
  );
} else {
  const answer = spawnSync(bin, ["push", "--snapshot-only"], {
    stdio: "inherit",
  });
  if (answer.error !== undefined) {
    process.stderr.write(
      `cachet: the snapshot step failed to launch: ${answer.error.message}\n`,
    );
  } else if (answer.status !== 0) {
    process.stderr.write(`cachet: the snapshot step exited ${answer.status}\n`);
  }
}
