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

# build the deployable worker bundle with worker-build, then stamp the JS
# loader with the wasm's hash: deploy-time drift detection hashes the
# entry, whose text worker-build emits identically on every build — the
# stamp is how wasm-only changes still read as a change (ADR 0009).
wasm:
    # The commit the bundle was built from, stamped into the binary so the
    # console's header names it. A build outside a git tree leaves it
    # empty, and the worker then says nothing rather than naming a commit
    # it was not built from.
    cd crates/cachet-worker && CACHET_BUILD_SHA="$(git rev-parse --short=6 HEAD 2>/dev/null || echo '')" worker-build --profile worker
    printf '// cachet-bundle-sha256: %s\n' "$(openssl dgst -sha256 -r crates/cachet-worker/build/index_bg.wasm | cut -d' ' -f1)" >> crates/cachet-worker/build/index.js

# the workerd lane: the built worker under wrangler dev --local, real R2
# and Cache API semantics, asserted over real HTTP; the CLI binary rides
# the same lane, driven by the end-to-end push scenario
workerd: wasm
    cargo build -p cachet-cli
    node workerd/check.mjs fixtures/nix-signed

# the shipped-artifact hygiene gate: no banned runtimes, no secret-shaped strings
wasm-hygiene:
    bash scripts/check-wasm-hygiene.sh

# the worker crate's own lint pass, on its production target (the host
# clippy gate excludes it because it is wasm32-only)
clippy-wasm:
    cargo clippy -p cachet-worker --target wasm32-unknown-unknown --all-features -- --deny warnings

# deploy one stage end to end: build the bundle, source the stage's env
# file when present, run the alchemy stack. The operator's two secrets and
# the CACHET_DEPLOY_* set come from infra/.env.<stage> or the environment.
deploy stage:
    #!/usr/bin/env bash
    set -euo pipefail
    just wasm
    cd infra
    bun install --frozen-lockfile --silent
    if [ -f ".env.{{ stage }}" ]; then
        set -a
        # shellcheck disable=SC1090
        . ".env.{{ stage }}"
        set +a
    fi
    # why: alchemy asks before it changes anything, and a run with no
    # terminal cannot answer, so every CI deploy stopped at the prompt
    # with "Non-interactive terminal detected". Approving on behalf of a
    # run that has no way to be asked is not a decision being taken away
    # from anyone: it is the only answer available. A terminal still gets
    # the prompt, which is where a human is present to read the plan.
    approve=()
    if [ ! -t 0 ]; then
        approve=(--yes)
    fi
    # why: the output is inspected, not just the exit status. alchemy
    # answers its own refusal with a zero exit, so a deploy that printed
    # its plan and applied none of it reported success: staging went green
    # for days while running whatever it had before, and the integration
    # lane behind it tested that stale worker and passed. A deploy that
    # deploys nothing has to be red.
    log="$(mktemp)"
    trap 'rm -f "${log}"' EXIT
    set +e
    bun run deploy --stage "{{ stage }}" "${approve[@]}" 2>&1 | tee "${log}"
    status=${PIPESTATUS[0]}
    set -e
    if [ "${status}" -ne 0 ]; then
        exit "${status}"
    fi
    if grep -q "Non-interactive terminal detected" "${log}"; then
        echo "cachet: alchemy printed its plan and applied none of it." >&2
        echo "  The deployment is unchanged. Nothing here can answer its prompt," >&2
        echo "  so this run cannot deploy; it must not report that it did." >&2
        exit 1
    fi

# tear one stage down; alchemy confirms before deleting anything
destroy stage:
    #!/usr/bin/env bash
    set -euo pipefail
    cd infra
    if [ -f ".env.{{ stage }}" ]; then
        set -a
        # shellcheck disable=SC1090
        . ".env.{{ stage }}"
        set +a
    fi
    bun run destroy --stage "{{ stage }}"

# the first-run walkthrough: writes infra/.env.production and prints the
# checklists for the GitHub and Cloudflare sides
bootstrap *args:
    bash scripts/bootstrap.sh {{ args }}

# the integration lane: the live round trip against a deployment
integration:
    bash scripts/integration.sh
