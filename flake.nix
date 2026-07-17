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
      # Android SDK/NDK for the mobile shell
      config.allowUnfree = true;
      config.android_sdk.accept_license = true;
    };
    inherit (pkgs) lib;

    androidNdkVersion = "27.0.12077973";
    androidComposition = pkgs.androidenv.composeAndroidPackages {
      # what the tauri-generated gradle project compiles against
      platformVersions = ["34" "36"];
      buildToolsVersions = ["34.0.0" "35.0.0"];
      includeNDK = true;
      ndkVersion = androidNdkVersion;
    };

    version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

    # Toolchain pinned by rust-toolchain.toml (stable + wasm32/Android targets).
    rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    rustPlatform = pkgs.makeRustPlatform {
      cargo = rustToolchain;
      rustc = rustToolchain;
    };

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

    ferret-server = rustPlatform.buildRustPackage {
      pname = "ferret-server";
      inherit version;
      src = self;

      cargoLock.lockFile = ./Cargo.lock;

      # Only the backend: the desktop crate would drag the webkit stack in.
      cargoBuildFlags = ["-p" "ferret-server"];
      cargoTestFlags = ["-p" "ferret-server"];

      meta = {
        description = "ferret backend: scraper, ETL pipeline, deals API";
        mainProgram = "ferret-server";
      };
    };

    ferret-web = pkgs.stdenv.mkDerivation {
      pname = "ferret-web";
      inherit version;
      src = self;

      cargoDeps = pkgs.rustPlatform.importCargoLock {lockFile = ./Cargo.lock;};

      nativeBuildInputs = [
        rustToolchain
        pkgs.trunk
        pkgs.binaryen
        wasm-bindgen-cli
        pkgs.rustPlatform.cargoSetupHook
      ];

      buildPhase = ''
        runHook preBuild
        export HOME=$TMPDIR
        cd crates/ferret-web
        trunk build --release --offline true --dist dist
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        cp -r dist $out
        runHook postInstall
      '';

      meta.description = "ferret web frontend (static trunk dist)";
    };
  in {
    packages.${system} = {
      inherit ferret-server ferret-web;
      default = ferret-server;
    };

    nixosModules.ferret = import ./nix/module.nix self;
    devShells.${system} = {
      default = pkgs.mkShell {
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

      # Android build of the shell: `nix develop .#android`, then
      # `cargo tauri android build --apk --target aarch64` in
      # crates/ferret-desktop.
      android = pkgs.mkShell {
      name = "ferret-android";

      packages = with pkgs;
        [
          rustToolchain
          trunk
          binaryen
          just
          cargo-tauri
          jdk17
          androidComposition.androidsdk
        ]
        ++ lib.optional hasCargoLock wasm-bindgen-cli;

      env = rec {
        JAVA_HOME = pkgs.jdk17.home;
        ANDROID_HOME = "${androidComposition.androidsdk}/libexec/android-sdk";
        NDK_HOME = "${ANDROID_HOME}/ndk/${androidNdkVersion}";
      };

      # The tauri CLI insists on `rustup target add`; the rust-overlay
      # toolchain already ships every Android target, so a no-op is honest.
      shellHook = ''
        shim_dir=$(mktemp -d)
        printf '#!/bin/sh\nexit 0\n' > "$shim_dir/rustup"
        chmod +x "$shim_dir/rustup"
        export PATH="$shim_dir:$PATH"
      '';
      };
    };
  };
}
