# One toolchain resolution, one dependency closure: every check shares
# compiled dependencies through _module.args, and no module re-derives them.
# cachet-worker builds for wasm32 only, so the host closures exclude it; the
# wasm build is a dedicated verb (justfile) rather than a host check.
{ inputs, lib, ... }:
{
  perSystem =
    { system, ... }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ (import inputs.rust-overlay) ];
      };
      toolchain = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain toolchain;
      src = lib.cleanSourceWith {
        src = ../.;
        filter =
          path: type:
          let
            rel = lib.removePrefix (toString ../. + "/") (toString path);
          in
          (craneLib.filterCargoSources path type)
          # why: clippy reads the disallowed lists at lint time.
          || (rel == "clippy.toml")
          # why: nextest reads the ci profile at run time.
          || (rel == ".config/nextest.toml")
          # why: the committed OpenAPI document is served byte-for-byte and
          # compared by the golden lane.
          || (rel == "docs/openapi.yaml")
          # why: golden vectors against real nix outputs live outside cargo's
          # file list; root-level fixtures/ and per-crate fixtures/ both
          # count.
          || (lib.hasPrefix "fixtures/" rel)
          || (lib.hasInfix "/fixtures/" rel)
          # why: insta snapshots are the golden lane's expected bytes;
          # cargo's source filter does not know them.
          || (lib.hasSuffix ".snap" rel);
      };
      commonArgs = {
        inherit src;
        pname = "cachet";
        version = "0.0.1";
        strictDeps = true;
      };
      # why: deny.toml stays out of the crane source on purpose: the deny
      # gate fetches the advisory database and is impure by design, so it
      # runs in the dev shell, never as a flake check.
      cargoArtifacts = craneLib.buildDepsOnly (
        commonArgs
        // {
          # why: cachet-worker is wasm32-only; the host closure covers the
          # five crates that build natively.
          cargoExtraArgs = "--workspace --exclude cachet-worker --all-features --locked";
        }
      );
    in
    {
      _module.args = {
        inherit
          pkgs
          toolchain
          craneLib
          src
          commonArgs
          cargoArtifacts
          ;
      };
    };
}
