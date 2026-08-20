# Every recipe is a thin alias over a flake output; the build logic lives in
# the flake, never here. `just check` is the pre-push parity verb: green
# locally means green in CI, because CI runs the same verbs.

sys := `nix eval --impure --raw --expr 'builtins.currentSystem'`

default: check

# the pre-push parity verb: every gate, one to one with the CI jobs
check:
    #!/usr/bin/env bash
    set -euo pipefail
    gates="$(nix eval --json '.#checks.{{ sys }}' --apply 'builtins.attrNames' | jq -r '.[] | ".#checks.{{ sys }}." + .')"
    # shellcheck disable=SC2086
    nix build -L --keep-going $gates
    just deny
    just kani
    just wasm
    just clippy-wasm
    just wasm-hygiene
    just openapi-check

# format the tree
fmt:
    nix fmt

# check formatting
fmt-check:
    nix build -L '.#checks.{{ sys }}.fmt'

# the workspace lint gate
clippy:
    nix build -L '.#checks.{{ sys }}.clippy'

# the unit lane
unit:
    nix build -L '.#checks.{{ sys }}.unit'

# the property lane
property:
    nix build -L '.#checks.{{ sys }}.property'

# the golden lane
golden:
    nix build -L '.#checks.{{ sys }}.golden'

# the kani lane; impure: the pinned verifier and its toolchain install under the user
kani crate="":
    #!/usr/bin/env bash
    set -euo pipefail
    # why: kani-verifier is not packaged in nixpkgs; its pinned install and
    # toolchain download live outside the dev shell, like cargo deny.
    export PATH="$HOME/.cargo/bin:$PATH"
    command -v cargo-kani >/dev/null 2>&1 || cargo install --locked kani-verifier@0.67.0
    cargo kani setup
    if [ -n "{{ crate }}" ]; then
        cargo kani -p "{{ crate }}"
    else
        for c in $(bash scripts/list-kani-crates.sh); do cargo kani -p "$c"; done
    fi

# the CLAUDE.md manifest bijection
doc-manifest:
    nix build -L '.#checks.{{ sys }}.doc-manifest'

# the lane bijection
lane-manifest:
    nix build -L '.#checks.{{ sys }}.lane-manifest'

# the workflow lint
actionlint:
    nix build -L '.#checks.{{ sys }}.actionlint'

# the shell lint
scripts:
    nix build -L '.#checks.{{ sys }}.scripts'

# the impure gate: cargo deny fetches the advisory database
deny:
    cargo deny check

# regenerate docs/openapi.yaml from the route descriptors
openapi:
    cargo run -p cachet-api --features yaml-export --bin openapi > docs/openapi.yaml

# the OpenAPI bijection: descriptors against the committed document
openapi-check:
    bash scripts/check-openapi-drift.sh

# build the deployable worker bundle with worker-build
wasm:
    cd crates/cachet-worker && worker-build --release

# the workerd lane: the built worker under wrangler dev --local, real R2
# and Cache API semantics, asserted over real HTTP
workerd: wasm
    node workerd/check.mjs fixtures/nix-signed

# the shipped-artifact hygiene gate: no banned runtimes, no secret-shaped strings
wasm-hygiene:
    bash scripts/check-wasm-hygiene.sh

# the worker crate's own lint pass, on its production target (the host
# clippy gate excludes it because it is wasm32-only)
clippy-wasm:
    cargo clippy -p cachet-worker --target wasm32-unknown-unknown --all-features -- --deny warnings
