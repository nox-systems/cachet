# cachet

cachet is a self-hostable nix binary cache that runs on Cloudflare Workers
and R2. One deployment serves one GitHub org: CI pushes signed-by-the-server
paths over an OIDC-authenticated write path, laptops authenticate with
GitHub device flow, and garbage collection runs armed from the first day on
a Cloudflare cron. The repo holds the worker, the cachet CLI, a GitHub
Action for CI pushes, and the alchemy program that deploys it all into your
own Cloudflare account.

The easiest way to run your own: clone this repo, run `just bootstrap`
inside `nix develop`, and follow the printed checklists (the runbook is
docs/DEPLOY.md).

## Status

The rewrite is in progress and v1 is not yet usable. The phases and what
v1 will and will not do are tracked in the issues for this repository.

## Governing documents

| Document | Role |
| --- | --- |
| `CLAUDE.md` | The constitution: invariants, lanes, and the doc manifest. |
| `PROSE.md` | The prose law every repo document obeys. |
| `SECURITY.md` | The vulnerability disclosure policy. |

## License

Apache-2.0. See LICENSE.
