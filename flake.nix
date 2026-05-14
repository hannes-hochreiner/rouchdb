{
  description = "RouchDB development environment";

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
      config.allowUnfree = true;
    };
    toolchain = with fenix.packages.${system}; combine [
      latest.cargo
      latest.rustc
      latest.rust-analyzer
      latest.rustfmt
      latest.clippy
      # latest.completeToolchain
      targets.wasm32-unknown-unknown.latest.rust-std
    ];
  in {
    # packages.${system}.hello = nixpkgs.legacyPackages.x86_64-linux.hello;

    # packages.${system}.default = self.packages.x86_64-linux.hello;

    devShells.${system}.default = pkgs.mkShell {
      name = "rouchdb";
      
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
        bash # default shell for vscode terminal
        mdbook
        (vscode-with-extensions.override {
          vscodeExtensions = with vscode-extensions; [
            rust-lang.rust-analyzer
            anthropic.claude-code
            streetsidesoftware.code-spell-checker
            fill-labs.dependi
            tamasfe.even-better-toml
            bbenoist.nix
            thenuprojectcontributors.vscode-nushell-lang
          # ] ++ pkgs.vscode-utils.extensionsFromVscodeMarketplace [
          #   {
          #     name = "remote-ssh-edit";
          #     publisher = "ms-vscode-remote";
          #     version = "0.47.2";
          #     sha256 = "1hp6gjh4xp2m1xlm1jsdzxw9d8frkiidhph6nvl24d0h8z34w49g";
          #   }
          ];
        })
      ];

      OPENSSL_DEV = pkgs.openssl.dev;
      PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
    };

  };
}
