#!/usr/bin/env python3
"""Stealth page fetcher for anti-bot sources (eBay…): prints the rendered
HTML of a URL on stdout using Scrapling's stealth browser.

Reference implementation for ferret's `fetch_command` hook — ferret stays a
lean Rust service and shells out here only for sources that fingerprint-block
plain HTTP clients.

Setup (same as ent/veille-prix, see its README "Sources protégées"):
    python3 -m venv .venv && .venv/bin/pip install 'scrapling[fetchers]'
    .venv/bin/python -m playwright install chromium

ferret.toml:
    [ebay]
    enabled = true
    queries = ["rtx 3080"]
    fetch_command = ["/path/to/.venv/bin/python", "/path/to/scripts/stealth-fetch.py", "{url}"]
"""

import sys

from scrapling.fetchers import StealthyFetcher


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: stealth-fetch.py <url>", file=sys.stderr)
        return 2
    page = StealthyFetcher.fetch(sys.argv[1], headless=True, network_idle=True,
                                 timeout=90_000)
    if page.status != 200:
        print(f"HTTP {page.status}", file=sys.stderr)
        return 1
    sys.stdout.write(page.html_content)
    return 0


if __name__ == "__main__":
    sys.exit(main())
