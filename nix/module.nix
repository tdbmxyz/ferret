# NixOS module for the ferret deal tracker. Imported from the flake as
# `nixosModules.ferret`; `self` provides the default packages.
self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.ferret;
  settingsFormat = pkgs.formats.toml {};
  configFile = settingsFormat.generate "ferret.toml" cfg.settings;
in {
  options.services.ferret = {
    enable = lib.mkEnableOption "ferret, the self-hosted deal tracker";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.ferret-server;
      defaultText = lib.literalExpression "ferret.packages.\${system}.ferret-server";
      description = "ferret-server package to run.";
    };

    webPackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.ferret-web;
      defaultText = lib.literalExpression "ferret.packages.\${system}.ferret-web";
      description = "Built web frontend served by the server (null to disable).";
    };

    address = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address to bind to.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 4800;
      description = "Port to listen on.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the ferret port in the firewall.";
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = {};
      example = lib.literalExpression ''
        {
          scrape.renotify_drop_pct = 5.0;
          leboncoin = {
            enabled = true;
            queries = ["rtx 3080" "seagate ironwolf 4to"];
          };
          families = [
            {
              name = "nvidia-rtx";
              models = ["3070" "3080" "3090" "4080" "4090"];
            }
          ];
          notifications = {
            ntfy_url = "https://notify.zeus.balem.fr";
            topic = "deals-zeus";
            token_file = "/run/agenix/ferret-ntfy-token";
          };
          llm = {
            enabled = true;
            base_url = "http://127.0.0.1:8080/v1";
          };
        }
      '';
      description = ''
        ferret configuration, serialized to ferret.toml. See
        crates/ferret-server/ferret.example.toml for the available keys.
        Secrets stay out of the store: point token_file/api_key_file at
        agenix-managed paths and make them readable by the ferret user.
        listen, db_path and static_dir default to sane values below.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    services.ferret.settings = {
      listen = lib.mkDefault "${cfg.address}:${toString cfg.port}";
      db_path = lib.mkDefault "/var/lib/ferret/ferret.db";
      static_dir = lib.mkIf (cfg.webPackage != null) (lib.mkDefault cfg.webPackage);
    };

    systemd.services.ferret = {
      description = "ferret deal tracker";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      # The Leboncoin source falls back to curl when DataDome
      # fingerprint-blocks the plain HTTP client.
      path = [pkgs.curl];

      environment.FERRET_CONFIG = configFile;

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        User = "ferret";
        Group = "ferret";
        StateDirectory = "ferret";
        WorkingDirectory = "/var/lib/ferret";
        Restart = "on-failure";
        RestartSec = 5;

        # A privileged port (e.g. 80) needs the bind capability; the
        # service user is not root.
        AmbientCapabilities = lib.mkIf (cfg.port < 1024) ["CAP_NET_BIND_SERVICE"];
        CapabilityBoundingSet = lib.mkIf (cfg.port < 1024) ["CAP_NET_BIND_SERVICE"];

        # Hardening (the service only needs its state dir and the network).
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
      };
    };

    users.users.ferret = {
      isSystemUser = true;
      group = "ferret";
    };
    users.groups.ferret = {};

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [cfg.port];
  };
}
