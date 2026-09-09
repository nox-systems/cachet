# ADR 0021: A deployment advertises its previous public keys

- **Status:** Accepted
- **Date:** 2026-09-09
- **Context doc:** [../DEPLOY.md](../DEPLOY.md); extends the rotation
  reasoning in [ADR 0013](0013-edge-keys-outlive-sweeps.md)

## Context

`GET /api/public/config` serves one `publicKey`, computed from the one
signing secret the worker holds. `cachet setup` reads it and appends it
to the daemon's `extra-trusted-public-keys`, and it never removes a key,
so a laptop configured before a rotation and re-run after it trusts both
keys and verifies every path.

A laptop configured for the first time after a rotation learns only the
new key. Every narinfo signed before the rotation carries the old one, so
that laptop reads every path pushed before the rotation as unverifiable
until the path is pushed again. The rotation runbook did not mention this,
and a hosted deployer that rotates keys with one click would hit it on
every rotation.

## Decision

1. A deployment carries an optional var, `CACHET_PREVIOUS_PUBLIC_KEYS`,
   holding the public keys it signed with before, comma-joined in nix's
   `name:base64` form. It is supplied as `CACHET_DEPLOY_PREVIOUS_PUBLIC_KEYS`
   and rostered in cachet-deploy.
2. The public config document gains `previousPublicKeys`, a list in the
   same form, absent when the deployment has never rotated, so an
   unrotated deployment serves the document it always did.
3. `cachet setup` trusts every key the document lists, the current one
   and the previous ones, and `cachet doctor` names how many previous
   keys the deployment advertises.
4. The worker parses every listed key before serving the document and
   refuses the whole document as `auth_unavailable` on a malformed one,
   logging the var's name. `cachet setup` writes what the document lists
   into nix's configuration, and nix refuses a configuration holding a
   malformed key, so a typo in the var would break a laptop's nix rather
   than one cache.

## Consequences

A rotation is three edits and a deploy: generate `<host>-2`, replace the
signing secret, and move the old public key into the var. Both laptops
that re-run `setup` and laptops set up afterwards verify every path.

Retiring a key that must no longer be trusted is removing it from the
var, which stops advertising it to new laptops, plus a hand edit of
`nix.custom.conf` on laptops already configured, because `setup` only
adds. That was true before this change and stays true.

The deployment holds no old secret. The previous keys are public values
in a plain var, so nothing about key custody changes.

## Alternatives considered

**Serve a list in `publicKey`.** Every reader of the field (the CLI, the
console, the doctor, the workerd lane) would change at once, and a
deployment that never rotated would change its document for no reason.
Rejected.

**A second document, `/api/public/keys`.** One more thing to keep in
step with the first, for a value that belongs beside the current key.
Rejected.

**Keep the previous secrets bound and derive their public halves.** The
worker would then hold every key it ever signed with, which widens the
custody surface for a public value. Rejected.
