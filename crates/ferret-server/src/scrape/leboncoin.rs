//! Leboncoin (France) — hand-written `DealSource` for the occasion market,
//! ported from the approach validated in ent/veille-prix.
//!
//! Search result pages embed the full result set as JSON in a
//! `<script id="__NEXT_DATA__">` tag: we parse
//! `props.pageProps.searchData.ads` instead of scraping HTML. A search
//! without results simply has no `ads` key.
//!
//! Leboncoin is behind DataDome, which fingerprints the TLS stack: plain
//! HTTP clients can get 403 while curl passes. On 403/429 the fetch falls
//! back to a curl subprocess with the same headers.

use chrono::Utc;
use ferret_domain::RawListing;
use url::Url;

use crate::config::LeboncoinConfig;
use crate::politeness::ScrapeClient;
use crate::scrape::DealSource;

use tower::{Service, ServiceExt};

pub const SOURCE_ID: &str = "leboncoin";
const SEARCH_URL: &str = "https://www.leboncoin.fr/recherche";

/// Browser-like identity — the default `ferret/x.y` UA is an instant block.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const ACCEPT_LANGUAGE: &str = "fr-FR,fr;q=0.9";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8";

pub fn search_url(query: &str, page: u32) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    if page <= 1 {
        format!("{SEARCH_URL}?text={encoded}")
    } else {
        format!("{SEARCH_URL}?text={encoded}&page={page}")
    }
}

/// The curl invocation used when the plain client is fingerprint-blocked.
pub fn curl_args(url: &str) -> Vec<String> {
    vec![
        "-sL".into(),
        "--fail".into(),
        "-m".into(),
        "30".into(),
        "--compressed".into(),
        "-H".into(),
        format!("User-Agent: {USER_AGENT}"),
        "-H".into(),
        format!("Accept-Language: {ACCEPT_LANGUAGE}"),
        "-H".into(),
        format!("Accept: {ACCEPT}"),
        url.into(),
    ]
}

/// Parse one search page. Missing `ads` key = no results (not an error);
/// missing `__NEXT_DATA__` = blocked or restructured page → hard error so
/// the scheduler's backoff/alerting kicks in instead of silently seeing
/// "zero listings" and marking everything gone.
pub fn parse_search_page(html: &str) -> anyhow::Result<Vec<RawListing>> {
    let start_tag = r#"<script id="__NEXT_DATA__""#;
    let start = html
        .find(start_tag)
        .ok_or_else(|| anyhow::anyhow!("__NEXT_DATA__ not found (blocked page or new layout)"))?;
    let json_start = html[start..]
        .find('>')
        .map(|i| start + i + 1)
        .ok_or_else(|| anyhow::anyhow!("malformed __NEXT_DATA__ tag"))?;
    let json_end = html[json_start..]
        .find("</script>")
        .map(|i| json_start + i)
        .ok_or_else(|| anyhow::anyhow!("unterminated __NEXT_DATA__ tag"))?;
    let data: serde_json::Value = serde_json::from_str(&html[json_start..json_end])?;

    let ads = match data["props"]["pageProps"]["searchData"].get("ads") {
        Some(serde_json::Value::Array(ads)) => ads.as_slice(),
        _ => return Ok(Vec::new()), // no-result search: `ads` key absent
    };

    let now = Utc::now();
    let mut listings = Vec::new();
    for ad in ads {
        if ad["status"].as_str().unwrap_or("active") != "active" {
            continue;
        }
        let (Some(title), Some(url)) = (ad["subject"].as_str(), ad["url"].as_str()) else {
            continue;
        };
        // price_cents (integer cents) preferred; older shape: price = [euros]
        let cents = ad["price_cents"]
            .as_i64()
            .or_else(|| ad["price"][0].as_f64().map(|e| (e * 100.0).round() as i64));
        let Some(cents) = cents else {
            continue;
        };
        listings.push(RawListing {
            source_id: SOURCE_ID.into(),
            title: title.to_string(),
            price_text: format!("{:.2} €", cents as f64 / 100.0),
            url: url.to_string(),
            scraped_at: now,
        });
    }
    Ok(listings)
}

pub struct LeboncoinSource {
    config: LeboncoinConfig,
    client: ScrapeClient,
    /// live watch queries merged in at fetch time (None for one-shot searches)
    extra: Option<crate::state::SharedQueries>,
}

impl LeboncoinSource {
    pub fn new(
        config: LeboncoinConfig,
        client: ScrapeClient,
        extra: Option<crate::state::SharedQueries>,
    ) -> Self {
        Self { config, client, extra }
    }

    /// Fetch one URL: polite reqwest first; DataDome fingerprint blocks
    /// (403/429) fall back to curl, which passes.
    async fn fetch_page(&self, url: &str) -> anyhow::Result<String> {
        let parsed = Url::parse(url)?;
        let mut request = reqwest::Request::new(reqwest::Method::GET, parsed);
        let headers = request.headers_mut();
        headers.insert(reqwest::header::USER_AGENT, USER_AGENT.parse()?);
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, ACCEPT_LANGUAGE.parse()?);
        headers.insert(reqwest::header::ACCEPT, ACCEPT.parse()?);

        let mut client = self.client.clone();
        let response = client.ready().await?.call(request).await?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            tracing::debug!(%status, url, "leboncoin fingerprint-blocked, retrying via curl");
            return fetch_via_curl(url).await;
        }
        Ok(response.error_for_status()?.text().await?)
    }
}

async fn fetch_via_curl(url: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("curl")
        .args(curl_args(url))
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawning curl: {e}"))?;
    anyhow::ensure!(
        output.status.success(),
        "curl failed with {} on {url}",
        output.status
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[async_trait::async_trait]
impl DealSource for LeboncoinSource {
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
            for page in 1..=self.config.pages_per_query.max(1) {
                let html = self.fetch_page(&search_url(query, page)).await?;
                let listings = parse_search_page(&html)?;
                let count = listings.len();
                all.extend(listings.into_iter().filter(|l| seen.insert(l.url.clone())));
                // 35 ads per full page — a short page is the last one
                if count < 35 {
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

    #[test]
    fn parses_search_fixture() {
        let html = include_str!("../../tests/fixtures/leboncoin_search.html");
        let listings = parse_search_page(html).unwrap();

        assert_eq!(listings.len(), 2, "sold ad is skipped");

        assert_eq!(listings[0].title, "RTX 3080 Founders Edition occasion");
        assert_eq!(listings[0].price_text, "450.00 €");
        assert_eq!(listings[0].url, "https://www.leboncoin.fr/ad/informatique/2900000001");
        assert_eq!(listings[0].source_id, "leboncoin");

        // euros-array price shape
        assert_eq!(listings[1].price_text, "90.00 €");
    }

    #[test]
    fn price_text_round_trips_through_normalize() {
        // the plugin's price_text must stay parseable by the shared pipeline
        assert_eq!(
            ferret_domain::normalize::parse_price("450.00 €"),
            Some((45_000, "EUR".to_string()))
        );
    }

    #[test]
    fn no_results_is_empty_not_error() {
        let html = include_str!("../../tests/fixtures/leboncoin_empty.html");
        assert_eq!(parse_search_page(html).unwrap(), Vec::new());
    }

    #[test]
    fn blocked_page_is_an_error() {
        // a DataDome challenge page has no __NEXT_DATA__ — must be a fetch
        // failure (backoff + alert), never "zero listings"
        assert!(parse_search_page("<html><body>Please verify</body></html>").is_err());
    }

    #[test]
    fn search_url_encodes_query_and_pages() {
        assert_eq!(
            search_url("rtx 3080", 1),
            "https://www.leboncoin.fr/recherche?text=rtx+3080"
        );
        assert_eq!(
            search_url("rtx 3080", 2),
            "https://www.leboncoin.fr/recherche?text=rtx+3080&page=2"
        );
    }

    #[test]
    fn curl_args_carry_browser_identity() {
        let args = curl_args("https://www.leboncoin.fr/recherche?text=x");
        assert!(args.contains(&"--compressed".to_string()));
        assert!(args.iter().any(|a| a.starts_with("User-Agent: Mozilla/5.0")));
        assert_eq!(args.last().unwrap(), "https://www.leboncoin.fr/recherche?text=x");
    }
}
