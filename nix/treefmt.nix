# The formatter gateway: one command formats every language it covers.
# Markdown is excluded globally: prose is hand-wrapped, and the CLAUDE.md
# import manifest parses by line.
{ ... }:
{
  perSystem =
    { config, toolchain, ... }:
    {
      # why: the auto check would be named treefmt; every gate carries
      # exactly one name across the flake output, the just verb, and the CI
      # job, so this module defines checks.fmt itself.
      treefmt = {
        flakeCheck = false;
        projectRootFile = "flake.nix";
        programs.nixfmt.enable = true;
        # why: the pinned toolchain's own rustfmt, so nix fmt never fights
        # cargo fmt.
        programs.rustfmt = {
          enable = true;
          package = toolchain;
        };
        # yaml and json; prettier has no TOML parser, TOML is hand-formatted.
        programs.prettier.enable = true;
        programs.shfmt.enable = true;
        settings.global.excludes = [
          "*.md"
          "*.lock"
          ".envrc"
          "result"
          "result-*"
          ".direnv/**"
          "target/**"
          "**/node_modules/**"
          "crates/cachet-worker/build/**"
          "dist/**"
          ".alchemy/**"
          "wrangler.local.jsonc"
          # why: the generator owns these bytes; a formatter fighting the
          # generator would make the drift gate unwinnable.
          "docs/openapi.yaml"
        ];
      };
      checks.fmt = config.treefmt.build.check ../.;
    };
}
