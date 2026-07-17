//! Shopify official stores (Minisforum, GMKtec…): the public
//! `/products.json` catalog — structured JSON, no anti-bot, no fragile
//! selectors. One listing per available variant. Ported from
//! ent/veille-prix.

use chrono::Utc;
use ferret_domain::RawListing;
use url::Url;

use crate::config::ShopifyConfig;
use crate::politeness::ScrapeClient;
use crate::scrape::DealSource;

use tower::{Service, ServiceExt};

/// 250 products per page; a store bigger than this cap is not a deal board.
const MAX_PAGES: u32 = 10;

pub fn catalog_url(base: &str, page: u32) -> String {
    format!("{}/products.json?limit=250&page={page}", base.trim_end_matches('/'))
}

/// Parse one catalog page into raw listings (available variants only).
/// Also returns how many products the page carried — pagination must stop
/// on an empty *products* page, not on a page whose variants all happen to
/// be unavailable.
pub fn parse_catalog(
    json: &str,
    config: &ShopifyConfig,
) -> anyhow::Result<(Vec<RawListing>, usize)> {
    let data: serde_json::Value = serde_json::from_str(json)?;
    let Some(products) = data["products"].as_array() else {
        anyhow::bail!("no products array (not a Shopify catalog?)");
    };
    let base = config.url.trim_end_matches('/');
    let now = Utc::now();
    let mut listings = Vec::new();
    for product in products {
        let (Some(title), Some(handle)) = (product["title"].as_str(), product["handle"].as_str())
        else {
            continue;
        };
        for variant in product["variants"].as_array().into_iter().flatten() {
            if !variant["available"].as_bool().unwrap_or(false) {
                continue;
            }
            let Some(price) = variant["price"].as_str().and_then(|p| p.parse::<f64>().ok())
            else {
                continue;
            };
            let variant_title = variant["title"].as_str().unwrap_or("");
            let full_title = if variant_title.is_empty() || variant_title == "Default Title" {
                title.to_string()
            } else {
                format!("{title} {variant_title}")
            };
            // each variant is its own deal — without ?variant= they would
            // all share one canonical URL and dedupe into a single deal
            let url = match variant["id"].as_i64() {
                Some(id) => format!("{base}/products/{handle}?variant={id}"),
                None => format!("{base}/products/{handle}"),
            };
            listings.push(RawListing {
                source_id: config.id.clone(),
                title: full_title,
                price_text: format!("{price:.2} {}", config.currency),
                url,
                scraped_at: now,
            });
        }
    }
    Ok((listings, products.len()))
}

pub struct ShopifySource {
    config: ShopifyConfig,
    client: ScrapeClient,
}

impl ShopifySource {
    pub fn new(config: ShopifyConfig, client: ScrapeClient) -> Self {
        Self { config, client }
    }
}

#[async_trait::async_trait]
impl DealSource for ShopifySource {
    fn id(&self) -> &str {
        &self.config.id
    }

    async fn fetch(&self) -> anyhow::Result<Vec<RawListing>> {
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = Url::parse(&catalog_url(&self.config.url, page))?;
            let request = reqwest::Request::new(reqwest::Method::GET, url);
            let mut client = self.client.clone();
            let body = client
                .ready()
                .await?
                .call(request)
                .await?
                .error_for_status()?
                .text()
                .await?;
            let (listings, product_count) = parse_catalog(&body, &self.config)?;
            all.extend(listings);
            if product_count == 0 {
                break;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ShopifyConfig {
        ShopifyConfig {
            id: "test-store".into(),
            url: "https://store.example.com/".into(),
            currency: "EUR".into(),
            interval_minutes: 360,
            delay_ms: 0,
        }
    }

    #[test]
    fn parses_catalog_fixture() {
        let json = include_str!("../../tests/fixtures/shopify_products.json");
        let (listings, product_count) = parse_catalog(json, &config()).unwrap();
        assert_eq!(product_count, 2);

        // 4TB available ✓; 8TB unavailable ✗; Default Title ✓ (bare title);
        // "not-a-price" variant ✗
        assert_eq!(listings.len(), 2);

        assert_eq!(listings[0].title, "IronWolf NAS HDD 4TB");
        assert_eq!(listings[0].price_text, "119.90 EUR");
        assert_eq!(
            listings[0].url,
            "https://store.example.com/products/ironwolf-nas-hdd?variant=91001",
            "variant id keeps each variant a distinct deal"
        );

        assert_eq!(listings[1].title, "MiniPC AI-X1", "Default Title is dropped");
        assert_eq!(
            ferret_domain::normalize::parse_price(&listings[1].price_text),
            Some((89_900, "EUR".into()))
        );
    }

    #[test]
    fn non_catalog_body_is_an_error() {
        assert!(parse_catalog("{\"error\": \"nope\"}", &config()).is_err());
    }

    #[test]
    fn catalog_url_paginates() {
        assert_eq!(
            catalog_url("https://store.example.com/", 3),
            "https://store.example.com/products.json?limit=250&page=3"
        );
    }
}
