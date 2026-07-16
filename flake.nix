{
  description = "ferret - self-hosted deal tracker";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      overlays = [rust-overlay.overlays.default];
    };
    inherit (pkgs) lib;

    # Toolchain pinned by rust-toolchain.toml (stable + wasm32/Android targets).
    rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

    # trunk invokes wasm-bindgen; its CLI version must match the wasm-bindgen
    # crate version in Cargo.lock exactly (wasm-bindgen is not semver-stable).
    # Same pattern (and hashes) as chaos.
    hasCargoLock = builtins.pathExists ./Cargo.lock;

    wasm-bindgen-cli = let
      cargoLock = builtins.fromTOML (builtins.readFile ./Cargo.lock);
      wasmBindgen =
        lib.findFirst
        (p: p.name == "wasm-bindgen")
        (throw "wasm-bindgen not found in Cargo.lock")
        cargoLock.package;
    in
      pkgs.buildWasmBindgenCli rec {
        src = pkgs.fetchCrate {
          pname = "wasm-bindgen-cli";
          version = wasmBindgen.version;
          hash = "sha256-H6Is3fiZVxZCfOMWK5dWMSrtn50VGv0sfdnsT+cTtyk=";
        };

        cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
          inherit src;
          inherit (src) pname version;
          hash = "sha256-VucqkXbCi4qtQzY/HrXiDnbSURsagPsdNVMn1Tw3UiY=";
        };
      };

    # Native libraries required by Tauri v2 (webview + GTK stack).
    tauriLibs = with pkgs; [
      webkitgtk_4_1
      gtk3
      libsoup_3
      openssl
      glib
      cairo
      pango
      gdk-pixbuf
    ];
  in {
    devShells.${system}.default = pkgs.mkShell {
      name = "ferret";

      nativeBuildInputs = with pkgs; [
        pkg-config
        gobject-introspection
      ];

      buildInputs = tauriLibs;

      packages = with pkgs;
        [
          rustToolchain
          cargo-nextest
          just
          trunk
          binaryen # wasm-opt, used by trunk release builds
          cargo-tauri
        ]
        ++ lib.optional hasCargoLock wasm-bindgen-cli;

      # Some webkit/nvidia combinations render a blank Tauri window without it.
      env.WEBKIT_DISABLE_DMABUF_RENDERER = "1";
    };
  };
}
