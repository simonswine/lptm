{
  description = "exploretui — Grafana Explore for the terminal";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            with pkgs;
            [
              rustup
              pkg-config
            ]
            ++ lib.optionals stdenv.isDarwin [
              libiconv
            ]
            ++ lib.optionals stdenv.isLinux [
              openssl
            ];

          RUST_BACKTRACE = "1";
        };
      }
    );
}
