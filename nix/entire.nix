# The Entire CLI (https://github.com/entireio/cli), the session-capture
# tooling. Upstream ships no flake, so the package wraps its pinned release
# tarballs: one per supported system, hash-locked against the release's own
# checksums.txt. A version bump is a deliberate edit here: the version
# string plus the four hashes, taken from the new release's checksums.
{ ... }:
{
  perSystem =
    { pkgs, lib, ... }:
    {
      packages.entire =
        let
          version = "0.10.2";
          sources = {
            "aarch64-darwin" = {
              target = "darwin_arm64";
              hash = "sha256-Ql9Z0vov8aP8waGRZrHXY+LIVCk8uxVpqrz4SFMsokg=";
            };
            "x86_64-darwin" = {
              target = "darwin_amd64";
              hash = "sha256-z16+fq+xbQULN1oRKR5Bd9eB4asksinJxvDkAHtDlPQ=";
            };
            "aarch64-linux" = {
              target = "linux_arm64";
              hash = "sha256-QLrcu8ztZunf4wmHANbh3fhH0T+IMf3Q4eth1XO6bUo=";
            };
            "x86_64-linux" = {
              target = "linux_amd64";
              hash = "sha256-Sg4ed8ueqeEAzbhn8H70R07Gc1900B/vy0ABwHh0RP4=";
            };
          };
          inherit (sources.${pkgs.stdenv.hostPlatform.system}) target hash;
        in
        pkgs.stdenvNoCC.mkDerivation {
          pname = "entire";
          inherit version;

          src = pkgs.fetchurl {
            url = "https://github.com/entireio/cli/releases/download/v${version}/entire_${target}.tar.gz";
            inherit hash;
          };

          # why: the tarball unpacks flat; a fixed source root keeps the
          # layout assumption explicit.
          unpackPhase = ''
            mkdir source
            tar -xzf "$src" -C source
          '';
          sourceRoot = "source";

          installPhase = ''
            runHook preInstall
            install -Dm755 entire "$out/bin/entire"
            runHook postInstall
          '';

          doInstallCheck = true;
          installCheckPhase = ''
            "$out/bin/entire" --help >/dev/null
          '';

          meta = {
            description = "The Entire CLI: agent-session checkpoints alongside commits";
            homepage = "https://github.com/entireio/cli";
            license = lib.licenses.mit;
            mainProgram = "entire";
            platforms = lib.attrNames sources;
          };
        };
    };
}
