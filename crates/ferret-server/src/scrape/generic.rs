//! The generic declarative scraper: one engine interpreting per-source
//! config (URL template + CSS selectors). Parsing is a pure function over
//! the fetched HTML — fixture-testable without any network.

use chrono::Utc;
use ferret_domain::RawListing;
use scraper::{Html, Selector};
use url::Url;

use crate::config::SourceConfig;
use crate::politeness::ScrapeClient;
use crate::scrape::DealSource;

use tower::{Service, ServiceExt};

/// Build the URL for one fetch: `{page}` and `{query}` substituted when
/// present (`{query}` is url-encoded).
pub fn page_url(config: &SourceConfig, query: Option<&str>, page: u32) -> String {
    let url = config.url.replace("{page}", &page.to_string());
    match query {
        Some(q) => {
            let encoded: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
            url.replace("{query}", &encoded)
        }
        None => url,
    }
}

/// Parse one fetched page into raw listings. Listings missing a title,
/// price, or resolvable link are skipped (logged), never fatal.
pub fn parse_listings(html: &str, config: &SourceConfig, base: &Url) -> Vec<RawListing> {
    let Ok(item_sel) = Selector::parse(&config.item_selector) else {
        tracing::error!(source = config.id, selector = config.item_selector, "bad item selector");
        return Vec::new();
    };
    let Ok(title_sel) = Selector::parse(&config.title_selector) else {
        tracing::error!(source = config.id, "bad title selector");
        return Vec::new();
    };
    let Ok(price_sel) = Selector::parse(&config.price_selector) else {
        tracing::error!(source = config.id, "bad price selector");
        return Vec::new();
    };
    let link_sel = config
        .link_selector
        .as_deref()
        .and_then(|s| Selector::parse(s).ok());

    let doc = Html::parse_document(html);
    let now = Utc::now();
    let mut listings = Vec::new();
    for item in doc.select(&item_sel) {
        let title = item
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>());
        let price = item
            .select(&price_sel)
            .next()
            .map(|el| el.text().collect::<String>());
        // link: explicit selector, else the item element itself must carry href
        let href = match &link_sel {
            Some(sel) => item.select(sel).next().and_then(|el| el.value().attr("href")),
            None => item.value().attr("href"),
        };
        let (Some(title), Some(price), Some(href)) = (title, price, href) else {
            tracing::debug!(source = config.id, "skipping incomplete listing");
            continue;
        };
        let Ok(url) = base.join(href) else {
            tracing::debug!(source = config.id, href, "skipping unresolvable link");
            continue;
        };
        listings.push(RawListing {
            source_id: config.id.clone(),
            title: title.trim().to_string(),
            price_text: price.trim().to_string(),
            url: url.to_string(),
            scraped_at: now,
        });
    }
    listings
}

/// A declarative source: config + a polite HTTP client.
pub struct GenericSource {
    config: SourceConfig,
    client: ScrapeClient,
}

impl GenericSource {
    pub fn new(config: SourceConfig, client: ScrapeClient) -> Self {
        Self { config, client }
    }
}

#[async_trait::async_trait]
impl DealSource for GenericSource {
    fn id(&self) -> &str {
        &self.config.id
    }

    async fn fetch(&self) -> anyhow::Result<Vec<RawListing>> {
        // fixed-page source = one pass with no query
        let queries: Vec<Option<&str>> = if self.config.queries.is_empty() {
            vec![None]
        } else {
            self.config.queries.iter().map(|q| Some(q.as_str())).collect()
        };
        let mut all = Vec::new();
        // the same listing can surface for several queries — dedupe by URL
        let mut seen = std::collections::HashSet::new();
        for query in queries {
            for page in 1..=self.config.max_pages {
                let url = page_url(&self.config, query, page);
                let base = Url::parse(&url)?;
                let request = reqwest::Request::new(reqwest::Method::GET, base.clone());
                let mut client = self.client.clone();
                let response = client
                    .ready()
                    .await?
                    .call(request)
                    .await?
                    .error_for_status()?;
                let html = response.text().await?;
                let listings = parse_listings(&html, &self.config, &base);
                let empty = listings.is_empty();
                all.extend(listings.into_iter().filter(|l| seen.insert(l.url.clone())));
                // stop paginating once a page yields nothing
                if empty {
                    break;
                }
                // URL without {page} can't paginate — one fetch only
                if !self.config.url.contains("{page}") {
                    break;
                }
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceConfig;

    fn source_config() -> SourceConfig {
        SourceConfig {
            id: "example-board".into(),
            url: "https://deals.example.com/hardware?page={page}".into(),
            queries: Vec::new(),
            item_selector: "div.listing".into(),
            title_selector: "h2.title".into(),
            price_selector: "span.price".into(),
            link_selector: Some("a.listing-link".into()),
            interval_minutes: 30,
            delay_ms: 0,
            max_concurrency: 1,
            max_pages: 1,
        }
    }

    #[test]
    fn parses_fixture_listings() {
        let html = include_str!("../../tests/fixtures/example_board.html");
        let base = url::Url::parse("https://deals.example.com/hardware").unwrap();
        let listings = parse_listings(html, &source_config(), &base);

        assert_eq!(listings.len(), 2, "broken third listing is skipped");

        assert_eq!(listings[0].title, "Seagate IronWolf 4TB NAS — neuf");
        assert_eq!(listings[0].price_text, "89,99 €");
        // relative href resolved against the page URL
        assert_eq!(
            listings[0].url,
            "https://deals.example.com/item/1?utm_source=feed"
        );
        assert_eq!(listings[0].source_id, "example-board");

        assert_eq!(listings[1].url, "https://deals.example.com/item/2");
    }

    #[test]
    fn page_url_substitution() {
        assert_eq!(
            page_url(&source_config(), None, 2),
            "https://deals.example.com/hardware?page=2"
        );
        let mut cfg = source_config();
        cfg.url = "https://ex.com/deals".into(); // no {page} placeholder
        assert_eq!(page_url(&cfg, None, 2), "https://ex.com/deals");
    }

    #[test]
    fn query_url_substitution() {
        let mut cfg = source_config();
        cfg.url = "https://market.example.com/search?q={query}&page={page}".into();
        assert_eq!(
            page_url(&cfg, Some("rtx 3080"), 1),
            "https://market.example.com/search?q=rtx+3080&page=1"
        );
    }
}
