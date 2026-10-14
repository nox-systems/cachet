// The post step: revoke the read credential the deployment issued for
// this job, so a copy taken during the job stops reading when the job
// ends rather than six hours after it was issued. The deployment revokes
// every other credential of the job with it, which covers credentials a
// step of the job minted for itself, and a second use of the composite
// in the same job. It runs after the
// push's post step (the composite lists this action first, and post steps
// run in reverse), and it never fails the job: every outcome is one log
// line and a zero exit.
const { spawnSync } = require("node:child_process");

const netrc = process.env["CACHET_ISSUED_NETRC"];
const url = process.env["CACHET_REVOKE_URL"];

function revoke() {
  if (!netrc || !url) {
    // A fork, a job with no OIDC grant, or a deployment that did not issue
    // a credential: there is nothing of this job's to revoke.
    return "cachet: no issued read credential to revoke";
  }
  // why: curl reads the credential straight from the daemon's netrc, the
  // way the probe does, so the credential never enters this process or
  // its environment. That file is 0600 and owned by root, hence sudo; -n
  // because a runner without passwordless sudo must answer at once rather
  // than wait for a password nobody will type.
  const curl = [
    "curl",
    "-sS",
    "-o",
    "/dev/null",
    "-w",
    "%{http_code}",
    "--max-time",
    "20",
    "--netrc-file",
    netrc,
    "-X",
    "POST",
    url,
  ];
  const root = typeof process.getuid === "function" && process.getuid() === 0;
  const [command, ...args] = root ? curl : ["sudo", "-n", ...curl];
  const answer = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 30_000,
  });
  const expires = "It expires on its own six hours after it was issued.";
  if (answer.error !== undefined) {
    return `cachet: the read credential was not revoked (${answer.error.message}). ${expires}`;
  }
  // why name sudo: a runner without passwordless sudo cannot read the
  // root-owned netrc, and "the deployment answered nothing" would send
  // whoever reads the log looking at the deployment.
  if (!root && answer.status === 1 && /^sudo: /m.test(answer.stderr ?? "")) {
    return `cachet: the read credential was not revoked, because sudo -n could not run without a password and only root can read the netrc that holds it. ${expires}`;
  }
  if (answer.stdout === "204") {
    return "cachet: revoked this job's read credential and every other credential of the job";
  }
  if (answer.stdout === "404") {
    return "cachet: the deployment held no such credential, because it was already revoked or has expired";
  }
  return `cachet: the read credential was not revoked (the deployment answered ${answer.stdout || `nothing, exit ${answer.status}`}). ${expires}`;
}

try {
  process.stdout.write(`${revoke()}\n`);
} catch (failure) {
  process.stdout.write(`cachet: the revoke step failed: ${failure}\n`);
}
