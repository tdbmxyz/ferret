# Ready-to-paste services.ferret block for /etc/nixos — every source
# ferret ships, pre-filled so a fresh install scrapes real listings on
# the first tick. Adjust queries/stores to taste.
#
# Prerequisite in the flake inputs:   ferret.url = "github:tdbmxyz/ferret";
# and in imports:                     ferret.nixosModules.ferret
{
  services.ferret = {
    enable = true;
    openFirewall = true; # LAN/tailnet only — ferret has no auth

    settings = {
      scrape = {
        outlier_ratio = 0.5; # price < 50% of rolling median → price-outlier flag
        stuffing_threshold = 0.25;
        failure_alert_after = 5;
        renotify_drop_pct = 5.0; # re-notify when a match drops ≥ 5%
      };

      # ---- Leboncoin (validated live: __NEXT_DATA__ parsing + curl
      # fallback for DataDome). Watch queries join these baselines
      # automatically — the list below just guarantees traffic before the
      # first watch exists.
      leboncoin = {
        enabled = true;
        queries = ["rtx 3080" "disque dur 4to" "ssd nvme 2to"];
        pages_per_query = 2; # 35 ads per page
        delay_ms = 3000;
        interval_minutes = 30;
      };

      # ---- Shopify official stores: public /products.json, no anti-bot.
      # One listing per available variant — the "new price" reference next
      # to the second-hand offers. minisforum-eu validated live (~149
      # variants). Add any Shopify store the same way.
      shopify = [
        {
          id = "minisforum-eu";
          url = "https://store.minisforum.de";
          currency = "EUR";
          interval_minutes = 360; # catalogs move slowly
          delay_ms = 1000;
        }
      ];

      # ---- eBay.fr — fingerprint-blocks plain HTTP AND curl (verified):
      # it needs `fetch_command` pointing at a stealth-browser wrapper.
      # Copy scripts/stealth-fetch.py to zeus, then:
      #   python -m venv /var/lib/ferret/venv
      #   /var/lib/ferret/venv/bin/pip install scrapling && scrapling install
      # and set enabled = true. Keep delay_ms large: ~10 fast requests get
      # the IP blocked for several minutes.
      ebay = {
        enabled = false;
        queries = ["rtx 3080"];
        delay_ms = 30000;
        interval_minutes = 60;
        # fetch_command = ["/var/lib/ferret/venv/bin/python" "/var/lib/ferret/stealth-fetch.py" "{url}"];
      };

      # ---- generic CSS-selector sources: any static-HTML listing page.
      # Template, off by default — fill selectors for the site you add.
      # sources = [
      #   {
      #     id = "example-board";
      #     url = "https://deals.example.com/hardware?page={page}";
      #     item_selector = "div.listing";
      #     title_selector = "h2.title";
      #     price_selector = "span.price";
      #     link_selector = "a.listing-link";
      #     interval_minutes = 30;
      #     delay_ms = 2000;
      #     max_pages = 3;
      #   }
      # ];

      # ---- product family tables: sibling-model lists driving stuffing
      # detection, per-model price history, and (as categories) the model
      # dropdowns in guided creation.
      families = [
        {
          name = "nvidia-rtx";
          models = ["2060" "2070" "2080" "3060" "3070" "3080" "3090" "4060" "4070" "4080" "4090" "5080" "5090"];
        }
        {
          name = "ddr4-kit";
          models = ["8GB" "16GB" "32GB" "64GB"];
        }
      ];

      # ---- LLM (TOML base — the ⚙ panel's DB override supersedes this,
      # so if you already configured it in the UI this is just a fallback).
      llm = {
        enabled = true;
        base_url = "https://ai.zeus.balem.fr/v1";
        model = "Qwen3.6-27B-Q4_0";
        timeout_secs = 60; # interpret/revise stretch to ≥300s on their own
      };

      # ---- ntfy push notifications
      notifications = {
        ntfy_url = "https://notify.zeus.balem.fr"; # your ntfy instance
        topic = "ferret";
        # token_file = "/run/agenix/ferret-ntfy-token";
      };
    };
  };
}
