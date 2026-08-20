{
  description = "cachet: a self-hostable nix binary cache on Cloudflare Workers";

  # why: plain github: refs keep the flake registry out of the dependency
  # path. The channel is nixos-unstable because only dev and build tools come
  # from it; the compiler comes from rust-toolchain.toml through rust-overlay.
  # flake.lock pins every revision, and dependabot proposes updates.
  # why: no nixConfig block. extra-substituters is a restricted client
  # setting that a non-trusted user ignores, so cache trust is wired in CI,
  # not committed here.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";

    # why: crane builds the workspace as a proper derivation, compiling the
    # dependency closure once and substituting it into every check.
    crane.url = "github:ipetkov/crane";

    # why: rust-overlay resolves the exact toolchain pinned in
    # rust-toolchain.toml, the single source of truth for the compiler.
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      # The two CI and dev-machine architectures in both operating systems.
      # The production target (wasm32-unknown-unknown) is a Rust target, not
      # a nix system.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      imports = [
        inputs.treefmt-nix.flakeModule
        ./nix/rust-workspace.nix
        ./nix/treefmt.nix
        ./nix/devshell.nix
        ./nix/checks.nix
        ./nix/entire.nix
      ];
    };
}
