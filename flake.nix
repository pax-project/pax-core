{
  description = "pax: discover, declare and reproducibly acquire academic papers";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    naersk.url = "github:nix-community/naersk";
  };

  outputs =
    {
      self,
      nixpkgs,
      naersk,
    }:
    let
      pkgs = nixpkgs.legacyPackages."x86_64-linux";
      app_deps = [ pkgs.openssl ];
      naerskLib = pkgs.callPackage naersk { };

      # `nix run` and `lib.getExe` find the `pax` binary through mainProgram;
      # the package is named after the crate (pax-core-papers).
      pax = (naerskLib.buildPackage {
        src = ./.;
        buildInputs = app_deps;
        nativeBuildInputs = [ pkgs.pkg-config ];
      }).overrideAttrs (old: {
        meta = (old.meta or { }) // {
          description = "Discover, declare and reproducibly acquire academic papers";
          mainProgram = "pax";
        };
      });
    in
    {

      packages."x86_64-linux" = {
        default = pax;
        inherit pax;
      };

      # For a consumer that prefers `pkgs.pax` over `inputs.pax-core.packages.<system>.pax`.
      overlays.default = final: prev: {
        pax = self.packages.${prev.stdenv.hostPlatform.system}.pax;
      };

      homeModules.default = import ./home-manager-module.nix self;

      devShells."x86_64-linux".default = pkgs.mkShell {
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs =
          with pkgs;
          [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            # Quality-gate tooling (see justfile): `just` unifies the
            # calling interface across local dev and CI.
            just
          ]
          ++ app_deps;
        env.RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
      };

      env = {
        RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
        OPENSSL_DIR = "${pkgs.openssl.dev}";
      };

    };
}
