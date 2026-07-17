# ferret task runner. `just --list` for the menu.

# Run the full test suite
test:
    cargo test --workspace

# Lint everything, warnings are errors
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the server (reads ferret.toml / $FERRET_CONFIG)
serve:
    cargo run -p ferret-server

# Dev frontend on :8081 with /api proxied to a local server on :4800
web:
    cd crates/ferret-web && trunk serve

# Build the debug Android APK (aarch64). Enters the .#android dev shell
# itself, so it works from the plain default shell too.
apk:
    nix develop .#android --command sh -c 'cd crates/ferret-desktop && cargo tauri android build --apk --target aarch64 --debug'
    @echo "APK: crates/ferret-desktop/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk"

# Regenerate every app icon from the master SVG artwork
icons src="crates/ferret-web/assets/logo.svg":
    nix shell nixpkgs#librsvg -c rsvg-convert --width 1024 --height 1024 -o /tmp/ferret-icon-1024.png {{src}}
    cd crates/ferret-desktop && cargo tauri icon /tmp/ferret-icon-1024.png
    nix shell nixpkgs#librsvg -c rsvg-convert --width 32 --height 32 -o crates/ferret-web/assets/favicon-32.png {{src}}
