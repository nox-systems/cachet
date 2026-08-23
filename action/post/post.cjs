// The post step: push what the job added. Runs on job success only
// (post-if in action.yml), reads the CACHET_* variables the composite
// exported, and never fails the job: the binary's own contract is to log
// and exit zero, and this wrapper adds nothing to it.
const { spawnSync } = require("node:child_process");

const bin = process.env["CACHET_BIN"];
if (bin === undefined || bin.length === 0) {
  process.stderr.write(
    "cachet: CACHET_BIN is unset; the download step ran dry on the network, so nothing will be pushed\n",
  );
} else {
  const answer = spawnSync(bin, ["push"], { stdio: "inherit" });
  if (answer.error !== undefined) {
    process.stderr.write(
      `cachet: the push failed to launch: ${answer.error.message}\n`,
    );
  } else if (answer.status !== 0) {
    process.stderr.write(`cachet: the push exited ${answer.status}\n`);
  }
}
