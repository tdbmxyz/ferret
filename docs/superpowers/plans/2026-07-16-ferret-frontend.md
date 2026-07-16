# ferret Frontend — Implementation Plan

**Goal:** The four frontend crates from the spec, mirroring chaos's proven layout:
`ferret-client` (typed HTTP client, native+wasm), `ferret-ui` (shared Leptos CSR components),
`ferret-web` (Trunk-built web frontend), `ferret-desktop` (Tauri shell, Android-primary).

**v1 UI scope:** two views behind a header tab switch (no router):
- **Deals** (default): all deals or one watch's matches, newest first — title→listing link,
  price, source, condition/capacity, badges for heuristic flags (`possible-stuffing`,
  `price-outlier`), LLM verdict + reason, dimmed style for `gone` deals; auto-refresh (60 s tick).
- **Watches**: create form (name, family from `/api/families`, model, min capacity,
  min/max price in €), list with active toggle and delete.

**Server-side prerequisites (same commit series):**
- `GET /api/health` → `{"status":"ok"}` (connectivity probe for the shells).
- `static_dir` config + SPA fallback (`ServeDir` + `ServeFile` on index.html), chaos pattern —
  the server serves the trunk dist in production. tower-http `fs` feature.
- Permissive CORS (`CorsLayer::permissive()`): the Tauri webview is cross-origin and the
  trust model is LAN/tailnet single-user — no cookies/auth to protect.

**API base resolution in ferret-web/main.rs** (simplified chaos pattern):
`window.FERRET_API_BASE` (injected by the shell) → `localStorage["ferret-api-base"]` →
page origin (unless a tauri origin) → `http://127.0.0.1:4800`.

**ferret-desktop** mirrors chaos-desktop: lib (`staticlib`,`cdylib`,`rlib` for Android) + thin
bin, `tauri.conf.json` (identifier `xyz.tdbm.ferret`, `frontendDist = ../ferret-web/dist`),
capabilities file, `FERRET_SERVER` env / `~/.config/ferret/server` file → injected
`window.FERRET_API_BASE`, `open_external` command, NVIDIA DMABUF workaround. Icons: chaos
placeholders copied, to re-brand later. `cargo tauri android init` + APK build deferred to
the deployment phase (needs the Android SDK shell).

**Toolchain/flake:** wasm32 + Android targets in rust-toolchain.toml; devShell gains trunk,
binaryen, lock-pinned wasm-bindgen-cli (chaos flake pattern + same hashes, wasm-bindgen
pinned `=0.2.126` in the workspace), tauri system libs (webkitgtk/gtk3), cargo-tauri,
pkg-config. `[llm]`→leptos deps in workspace Cargo.toml.

**Verification:** full native test suite + clippy; `cargo check -p ferret-web --target
wasm32-unknown-unknown`; `trunk build` produces a dist; server serves the dist
(`static_dir`) — browse smoke test via curl; `cargo check -p ferret-desktop` under the
extended devShell.
