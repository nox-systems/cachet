# cachet

cachet is a self-hostable nix binary cache that runs on Cloudflare Workers
and R2. One deployment serves the GitHub orgs you configure: CI pushes over an
OIDC-authenticated write path and the server signs each narinfo after
verifying its NAR byte-for-byte, laptops authenticate with GitHub device
flow, and garbage collection runs armed from the first day on a Cloudflare
cron. The repo holds the worker, the cachet CLI, a GitHub Action for CI
pushes, and the alchemy program that deploys it all into your own
Cloudflare account.

The easiest way to run your own: clone this repo, run `just bootstrap`
inside `nix develop`, and follow the printed checklists (the runbook is
[docs/DEPLOY.md](docs/DEPLOY.md).

## For laptop users

Install the CLI and wire the cache:

```
curl --proto '=https' --tlsv1.2 -sSf \
  'https://github.com/nox-systems/cachet/releases/latest/download/cachet-cli-installer.sh' | sh
cachet login --cache-url https://<the deployment's host>
cachet setup
cachet doctor
```

`setup` edits the daemon's netrc and `/etc/nix/nix.custom.conf` and
restarts the daemon. `doctor` probes the wiring and prints what holds.

## For CI

In a workflow of any repo in the served org:

```yaml
permissions:
  contents: read
  id-token: write

steps:
  - uses: actions/checkout@v7
  - uses: nox-systems/cachet/action@v0
    with:
      cache-url: https://<the deployment's host>
      roots: |
        .#my-package
```

The action installs nix trusting the cache, snapshots the store before
your build, and pushes what the build added on success. Signing happens
server-side; the job needs no secrets beyond its OIDC token.

## Repository layout

- `crates/`: the workspace. `cachet-core` (pure domain), `cachet-crypto`,
  `cachet-api` (the HTTP surface and the generated OpenAPI document),
  `cachet-worker` (the wasm32 deployable), `cachet-push`, `cachet-cli`.
- `action/`: the composite GitHub Action consumers wire into CI.
- `infra/`: the alchemy stack that provisions a deployment.
- `docs/`: DEPLOY.md (the runbook), security/threat-model.md,
  testing/ (the lanes), adr/ (decision records),
  openapi.yaml (generated, drift-gated).
- `workerd/`: the truth lane's driver and fixtures.

## Governing documents

| Document | Role |
| --- | --- |
| [CLAUDE.md](CLAUDE.md) | The constitution: invariants, lanes, and the doc manifest. |
| [PROSE.md](PROSE.md) | The prose law every repo document obeys. |
| [SECURITY.md](SECURITY.md) | The vulnerability disclosure policy. |

## License

Apache-2.0. See [LICENSE](LICENSE).
