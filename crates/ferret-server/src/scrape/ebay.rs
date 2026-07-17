//! eBay.fr — hand-written `DealSource` (new + occasion), ported from the
//! parser veille-prix validated against real pages on 2026-07-05.
//!
//! One search-result page per query, 2026 `s-card` layout, parsed with CSS
//! selectors. eBay rate-limits the IP after ~10 rapid requests: the
//! politeness delay defaults to 30 s, and a 403/429 gets one retry after a
//! 120 s pause before counting as a fetch failure.

use chrono::Utc;
use ferret_domain::RawListing;
use scraper::{Html, Selector};
use url::Url;

use crate::config::EbayConfig;
use crate::politeness::ScrapeClient;
use crate::scrape::DealSource;

use tower::{Service, ServiceExt};

pub const SOURCE_ID: &str = "ebay";
const RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// `_sop=15`: sort by price + shipping, lowest first — the deals end.
pub fn search_url(query: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    format!("https://www.ebay.fr/sch/i.html?_nkw={encoded}&_sop=15")
}

/// Parse one result page. A page without any `s-card` scaffold at all is a
/// hard error (blocked or new layout) so backoff/alerting fires; cards that
/// are placeholders ("Shop on eBay"), out-of-zone (non-EUR price) or
/// incomplete are silently skipped.
pub fn parse_search_page(html: &str) -> anyhow::Result<Vec<RawListing>> {
    let card_sel = Selector::parse("li.s-card").expect("valid selector");
    let title_sel = Selector::parse(".s-card__title").expect("valid selector");
    let price_sel = Selector::parse(".s-card__price").expect("valid selector");
    let link_sel = Selector::parse("a[href*='/itm/']").expect("valid selector");

    let doc = Html::parse_document(html);
    let cards: Vec<_> = doc.select(&card_sel).collect();
    anyhow::ensure!(
        !cards.is_empty(),
        "no s-card results on page (blocked or new layout)"
    );

    let now = Utc::now();
    let mut listings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for card in cards {
        let title: String = match card.select(&title_sel).next() {
            Some(el) => el.text().collect::<String>().trim().to_string(),
            None => continue,
        };
        let Some(price) = card
            .select(&price_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
        else {
            continue; // placeholder card ("Shop on eBay") has no price
        };
        if !price.contains("EUR") && !price.contains('€') {
            continue; // out-of-zone card (USD) — skip
        }
        let Some(item_id) = card
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .and_then(extract_item_id)
        else {
            continue;
        };
        if title.len() < 10 || !seen.insert(item_id.clone()) {
            continue;
        }
        listings.push(RawListing {
            source_id: SOURCE_ID.into(),
            title,
            price_text: price,
            url: format!("https://www.ebay.fr/itm/{item_id}"),
            scraped_at: now,
        });
    }
    Ok(listings)
}

fn extract_item_id(href: &str) -> Option<String> {
    let after = href.split("/itm/").nth(1)?;
    let id: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!id.is_empty()).then_some(id)
}

pub struct EbaySource {
    config: EbayConfig,
    client: ScrapeClient,
    /// live watch queries merged in at fetch time (None for one-shot searches)
    extra: Option<crate::state::SharedQueries>,
}

impl EbaySource {
    pub fn new(
        config: EbayConfig,
        client: ScrapeClient,
        extra: Option<crate::state::SharedQueries>,
    ) -> Self {
        Self { config, client, extra }
    }

    async fn fetch_page(&self, url: &str) -> anyhow::Result<String> {
        if !self.config.fetch_command.is_empty() {
            return fetch_via_command(&self.config.fetch_command, url).await;
        }
        let get = || async {
            let parsed = Url::parse(url)?;
            let mut request = reqwest::Request::new(reqwest::Method::GET, parsed);
            request.headers_mut().insert(reqwest::header::USER_AGENT, USER_AGENT.parse()?);
            request
                .headers_mut()
                .insert(reqwest::header::ACCEPT_LANGUAGE, "fr-FR,fr;q=0.9".parse()?);
            let mut client = self.client.clone();
            anyhow::Ok(client.ready().await?.call(request).await?)
        };
        let response = get().await?;
        let response = if matches!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
        ) {
            tracing::debug!(status = %response.status(), "ebay rate-limited, retrying in 120s");
            tokio::time::sleep(RETRY_WAIT).await;
            get().await?
        } else {
            response
        };
        Ok(response.error_for_status()?.text().await?)
    }
}

/// Run the configured external fetcher (`{url}` substituted) and return
/// its stdout. This is how anti-bot sources plug in a stealth browser
/// without ferret embedding one.
async fn fetch_via_command(argv: &[String], url: &str) -> anyhow::Result<String> {
    let program = &argv[0];
    let args: Vec<String> = argv[1..].iter().map(|a| a.replace("{url}", url)).collect();
    let output = tokio::process::Command::new(program)
        .args(&args)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawning {program}: {e}"))?;
    anyhow::ensure!(
        output.status.success(),
        "{program} failed with {} on {url}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).chars().take(200).collect::<String>()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[async_trait::async_trait]
impl DealSource for EbaySource {
    fn id(&self) -> &str {
        SOURCE_ID
    }

    async fn fetch(&self) -> anyhow::Result<Vec<RawListing>> {
        let mut queries = self.config.queries.clone();
        if let Some(extra) = &self.extra {
            for q in extra.read().await.iter() {
                if !queries.contains(q) {
                    queries.push(q.clone());
                }
            }
        }
        let mut all = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for query in &queries {
            let html = self.fetch_page(&search_url(query)).await?;
            let listings = parse_search_page(&html)?;
            all.extend(listings.into_iter().filter(|l| seen.insert(l.url.clone())));
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_fixture() {
        let html = include_str!("../../tests/fixtures/ebay_search.html");
        let listings = parse_search_page(html).unwrap();

        // placeholder ✗ (no price), USD card ✗, duplicate item id ✗
        assert_eq!(listings.len(), 2);

        assert_eq!(listings[0].title, "NVIDIA RTX 3080 Founders Edition 10GB");
        assert_eq!(listings[0].price_text, "419,99 EUR");
        assert_eq!(listings[0].url, "https://www.ebay.fr/itm/335512340001");

        // "419,99 EUR" survives the shared pipeline's price parsing
        assert_eq!(
            ferret_domain::normalize::parse_price(&listings[0].price_text),
            Some((41_999, "EUR".into()))
        );

        assert_eq!(listings[1].url, "https://www.ebay.fr/itm/335512340002");
    }

    #[test]
    fn blocked_page_is_an_error() {
        assert!(parse_search_page("<html><body>Pardon our interruption</body></html>").is_err());
    }

    #[test]
    fn search_url_encodes() {
        assert_eq!(
            search_url("rtx 3080"),
            "https://www.ebay.fr/sch/i.html?_nkw=rtx+3080&_sop=15"
        );
    }

    #[tokio::test]
    async fn fetch_via_command_drives_the_full_source() {
        // the external-fetcher hook, exercised with `cat` as the "browser"
        let config = EbayConfig {
            enabled: true,
            queries: vec!["rtx 3080".into()],
            delay_ms: 0,
            interval_minutes: 60,
            fetch_command: vec!["cat".into(), "tests/fixtures/ebay_search.html".into()],
        };
        let source = EbaySource::new(
            config,
            crate::politeness::scrape_client(std::time::Duration::ZERO, 1),
            None,
        );
        let listings = source.fetch().await.unwrap();
        assert_eq!(listings.len(), 2);
        assert_eq!(listings[0].source_id, "ebay");
    }

    #[tokio::test]
    async fn failing_fetch_command_is_an_error() {
        let config = EbayConfig {
            enabled: true,
            queries: vec!["x".into()],
            delay_ms: 0,
            interval_minutes: 60,
            fetch_command: vec!["false".into()],
        };
        let source = EbaySource::new(
            config,
            crate::politeness::scrape_client(std::time::Duration::ZERO, 1),
            None,
        );
        assert!(source.fetch().await.is_err());
    }

    #[test]
    fn item_id_extraction() {
        assert_eq!(
            extract_item_id("https://www.ebay.fr/itm/335512340001?_skw=x"),
            Some("335512340001".into())
        );
        assert_eq!(extract_item_id("https://www.ebay.fr/sch/"), None);
    }
}
