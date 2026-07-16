# ferret

Self-hosted deal tracker: scrapes retailer/marketplace listings, extracts
structured product attributes, filters scam/SEO-stuffed listings, and
notifies (ntfy) on genuine matches for saved product-type watches
("4TB HDD", "RTX 3080", …).

Design: [docs/superpowers/specs/2026-07-16-ferret-design.md](docs/superpowers/specs/2026-07-16-ferret-design.md).

## Layout

| Crate | Role |
|---|---|
| `ferret-domain` | Pure types + logic: extraction, family/stuffing scoring, outlier detection, watch matching |
| `ferret-server` | axum server: scheduler, sources, ETL pipeline, SQLite storage, ntfy notifier, REST API |
| `ferret-client` | Typed HTTP client (native + wasm) |
| `ferret-ui` | Shared Leptos components (Deals / Watches views) |
| `ferret-web` | Trunk-built web frontend |
| `ferret-desktop` | Tauri shell (Android primary, desktop secondary) |

## Development

```bash
nix develop                       # toolchain, trunk, cargo-tauri…
cargo test --workspace
cargo run -p ferret-server        # reads ferret.toml / $FERRET_CONFIG
cd crates/ferret-web && trunk serve   # dev frontend on :8081, proxies /api to :4800
```

Configuration reference: `crates/ferret-server/ferret.example.toml`
(declarative `[[sources]]`, the `[leboncoin]` plugin, `[[families]]` tables,
`[llm]` refinement, `[notifications]` via ntfy).

## Pipeline

```
scheduler (per source) → fetch → normalize → extract attributes
  → family/stuffing score → price-outlier check → dedupe/upsert
  → [LLM refinement, ambiguous listings only, fail-open]
  → watch matching → ntfy notify (+ re-notify on price drops)
  → gone/revive lifecycle
```

Heuristic flags (`possible-stuffing`, `price-outlier`) and the LLM verdict
are signals shown to the user, never hard filters.

## Deployment (NixOS)

```nix
# flake input
inputs.ferret.url = "github:tdbmxyz/ferret";

# host config
imports = [inputs.ferret.nixosModules.ferret];
services.ferret = {
  enable = true;
  settings = {
    leboncoin = { enabled = true; queries = ["rtx 3080"]; };
    families = [{ name = "nvidia-rtx"; models = ["3070" "3080" "3090"]; }];
    notifications = {
      ntfy_url = "https://notify.zeus.balem.fr";
      topic = "deals-zeus";
      token_file = "/run/agenix/ferret-ntfy-token";
    };
    llm = { enabled = true; base_url = "http://127.0.0.1:8080/v1"; };
  };
};
```

The module serves the built web frontend from the server and stores state
under `/var/lib/ferret`.
