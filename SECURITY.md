# Security policy

Report vulnerabilities through GitHub's private vulnerability reporting for
this repository. Do not open public issues for suspected vulnerabilities.

## Scope

The security model covers the hosted components: the cachet Worker, its
bindings (R2, KV, Workers secrets), the cachet CLI, and the GitHub Action.
A self-hosted deployment's Cloudflare account, API tokens, and GitHub OAuth
App belong to the operator and are out of scope for reports about cachet
itself.

The threat model in docs/security/threat-model.md describes the attacker
scenarios cachet defends against and the mechanisms and tests that enforce
each defense. A report that breaks one of those mechanisms with a
reproduction is always in scope.

## Supported versions

cachet is pre-1.0 and the main branch is the only supported version. Fixes
land on main and ship in the next tagged release.

## Expectations

Reports get an acknowledgement within a week. Fixes ship before disclosure,
and disclosure happens with credit, unless you ask otherwise.
