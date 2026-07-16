# ferret NixOS Module — Implementation Plan

**Goal:** Deploy ferret on zeus like chaos: flake packages (`ferret-server` rust build,
`ferret-web` trunk dist) + `nixosModules.ferret` (`services.ferret`).

**Module shape (mirrors chaos's `nix/module.nix`, minus what ferret doesn't have):**
- Options: `enable`, `package` (default `ferret-server`), `webPackage` (nullable, default
  `ferret-web`, becomes `settings.static_dir`), `address`/`port` (default 4800),
  `openFirewall`, `settings` (free-form TOML → `ferret.toml` via `pkgs.formats.toml`).
- Defaults injected into settings: `listen`, `db_path = /var/lib/ferret/ferret.db`,
  `static_dir` from webPackage.
- Secrets stay host-side: `settings.notifications.token_file` / `settings.llm.api_key_file`
  point at agenix paths (`/run/agenix/...`); the module only needs
  `SupplementaryGroups`-free read access — host config manages secret ownership.
- systemd service: static `ferret` user/group, `StateDirectory=ferret`,
  `WorkingDirectory=/var/lib/ferret`, `FERRET_CONFIG` env, `Restart=on-failure`,
  chaos hardening block, `path = [ pkgs.curl ]` (Leboncoin DataDome fallback shells out
  to curl), network-online ordering.

**Flake additions:** `version` from workspace Cargo.toml; `packages.ferret-server`
(`buildRustPackage`, `-p ferret-server` only); `packages.ferret-web` (trunk offline build,
chaos recipe); `packages.default = ferret-server`; `nixosModules.ferret = import
./nix/module.nix self`. Desktop/Android packaging deferred (local builds via devshell).

**Verification:** `nix build .#ferret-server` and `.#ferret-web`; `nix flake check`-level
eval of the module via a `nixosConfigurations` dry eval or `nix eval` of the module options;
run the built server binary against the built web dist.

**Deploy on zeus (documented, done host-side):** import module, `services.ferret.enable =
true;` + settings with sources/families/[leboncoin]/[llm]/[notifications] and agenix files.
