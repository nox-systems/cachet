# The development shell: everything a contributor needs, pinned. A missing
# tool is a flake issue to file, never a host install.
{ ... }:
{
  perSystem =
    {
      pkgs,
      toolchain,
      self',
      ...
    }:
    {
      devShells.default = pkgs.mkShell {
        packages = [
          toolchain
          pkgs.cargo-nextest
          pkgs.cargo-deny
          pkgs.just
          pkgs.nixfmt
          pkgs.actionlint
          pkgs.shellcheck
          pkgs.shfmt
          pkgs.jq
          pkgs.git

          # why: the Cloudflare toolchain. wrangler is the local dev runner
          # and the rollback deploy path; alchemy does real deploys; workerd
          # is the truth lane's runtime. wrangler embeds miniflare, and the
          # workerd lane pins its own npm miniflare when it lands, because
          # nixpkgs has no miniflare attribute.
          pkgs.bun
          pkgs.nodejs_22
          pkgs.wrangler

          # why: the wasm build chain. worker-build produces the deployable
          # bundle; wasm-tools powers the wasm-hygiene gate's scans.
          pkgs.worker-build
          pkgs.wasm-tools

          # why: the CLI release pipeline publishes with cargo-dist.
          pkgs.cargo-dist

          # why: the session-capture CLI; .claude/settings.json wires its
          # hooks and guards on it being on PATH.
          self'.packages.entire
        ];
      };
    };
}
