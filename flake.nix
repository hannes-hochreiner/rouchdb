{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix, ... }:
  let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
    };
    toolchain = with fenix.packages.${system}; combine [
      # stable.cargo
      # stable.rustc
      # stable.rust-analyzer
      # stable.rustfmt
      # stable.clippy
      stable.completeToolchain
      targets.wasm32-unknown-unknown.stable.rust-std
    ];
  in {

    # packages.${system}.hello = nixpkgs.legacyPackages.x86_64-linux.hello;

    # packages.${system}.default = self.packages.x86_64-linux.hello;

    devShells.${system}.default = pkgs.mkShell {
      name = "rust-custom-element";
      
      # Inherit inputs from checks.
      # checks = self.checks.${system};
      shellHook = ''
        # code .
        exec nu
      '';
      # Additional dev-shell environment variables can be set directly
      # MY_CUSTOM_DEVELOPMENT_VAR = "something else";
      # Extra inputs can be added here; cargo and rustc are provided by default.
      buildInputs = with pkgs; [
        toolchain
        bun
        # wasm-bindgen-cli_0_2_105
        wasm-pack
        nushell
        libxml2
        # docker-compose
        trunk
        openssl
        pkg-config
      ];

      OPENSSL_DEV = pkgs.openssl.dev;
      PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
    };

  };
}
