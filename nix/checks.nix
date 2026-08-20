# The gate set: one checks.<system> attribute per sandbox-pure gate, one CI
# job per gate. CLAUDE.md §0 binds this list: a gate is named anywhere in
# the repo only when it exists here or as a just verb. The impure gates
# (deny fetches the advisory database; kani downloads its own toolchain) are
# just verbs, not derivations, because a sandboxed build cannot reach the
# network.
{ ... }:
{
  perSystem =
    {
      pkgs,
      craneLib,
      cargoArtifacts,
      commonArgs,
      ...
    }:
    {
      checks = {
        # why: crane appends its own --release --locked; pass neither again.
        # cachet-worker is wasm32-only and excluded from the host closure.
        clippy = craneLib.cargoClippy (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --exclude cachet-worker --all-targets --all-features -- --deny warnings";
          }
        );

        unit = craneLib.cargoNextest (
          commonArgs
          // {
            inherit cargoArtifacts;
            # why: nextest fails an empty suite, and a red lane beats a
            # seeded silence.
            cargoNextestExtraArgs = "--workspace --exclude cachet-worker --profile ci";
          }
        );

        # why: the unit gate would pass these cases hidden in the full
        # suite; the lane runs them isolated so a property failure is its
        # own signal.
        property = craneLib.cargoTest (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--workspace --exclude cachet-worker --test property";
          }
        );

        # why: snapshots commit with their code; drift is a failed test,
        # never a silent refresh in CI.
        golden = craneLib.cargoTest (
          commonArgs
          // {
            inherit cargoArtifacts;
            INSTA_UPDATE = "no";
            cargoTestExtraArgs = "--workspace --exclude cachet-worker --test golden";
          }
        );

        # why: the non-language gates run committed scripts over committed
        # inputs inside the sandbox; the gate is the script's exit code, and
        # no git state can leak in.
        doc-manifest =
          pkgs.runCommand "doc-manifest"
            {
              nativeBuildInputs = [
                pkgs.bash
                pkgs.gawk
                pkgs.findutils
                pkgs.coreutils
                pkgs.gnugrep
                pkgs.gnused
              ];
            }
            ''
              cp ${../CLAUDE.md} CLAUDE.md
              cp ${../README.md} README.md
              cp ${../PROSE.md} PROSE.md
              cp ${../SECURITY.md} SECURITY.md
              cp -r ${../docs} docs
              cp ${../scripts/check-doc-manifest.sh} check-doc-manifest.sh
              bash ./check-doc-manifest.sh .
              touch $out
            '';

        lane-manifest =
          pkgs.runCommand "lane-manifest"
            {
              nativeBuildInputs = [
                pkgs.bash
                pkgs.gawk
                pkgs.findutils
                pkgs.coreutils
                pkgs.gnugrep
                pkgs.gnused
              ];
            }
            ''
              cp -r ${../docs} docs
              mkdir -p .github/workflows
              cp ${../.github/workflows/ci.yml} .github/workflows/ci.yml
              cp ${../scripts/check-lane-manifest.sh} check-lane-manifest.sh
              bash ./check-lane-manifest.sh .
              touch $out
            '';

        actionlint =
          pkgs.runCommand "actionlint"
            {
              nativeBuildInputs = [ pkgs.actionlint ];
            }
            ''
              actionlint ${../.github/workflows}/ci.yml
              touch $out
            '';

        scripts =
          pkgs.runCommand "scripts"
            {
              nativeBuildInputs = [ pkgs.shellcheck ];
            }
            ''
              shellcheck ${../scripts}/*.sh
              touch $out
            '';
      };
    };
}
