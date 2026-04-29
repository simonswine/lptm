{
  description = "lptm — Logs, Profiles, Traces, Metrics for the terminal";

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

        rustTools = with pkgs; [
          cargo
          rustc
          clippy
          rustfmt
          pkg-config
          protobuf_33
        ] ++ lib.optionals stdenv.isDarwin [ libiconv ]
          ++ lib.optionals stdenv.isLinux [ openssl ];

        check = pkgs.writeShellApplication {
          name = "check";
          runtimeInputs = rustTools;
          text = ''
            cargo fmt --all -- --check
            cargo clippy --workspace -- -D warnings
            cargo test --workspace
          '';
        };
      in
      {
        apps.check = {
          type = "app";
          program = "${check}/bin/check";
        };

        devShells.default = pkgs.mkShell {
          packages = rustTools ++ [ check ];
          RUST_BACKTRACE = "1";
        };
      }
    );
}
