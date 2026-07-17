//! Tiny key-less web context for LLM interpretation: one DuckDuckGo HTML
//! query, top result titles + snippets. Strictly fail-open — no results is
//! never an error, the LLM just answers without web context. Only called
//! on user-triggered interprets (never per listing), so one request per
//! guided search is the whole footprint.

use std::time::Duration;

use scraper::{Html, Selector};

const MAX_SNIPPETS: usize = 5;

pub async fn snippets(query: &str) -> Vec<String> {
    match fetch(query).await {
        Ok(results) => results,
        Err(e) => {
            tracing::debug!(error = %e, "web search failed — interpreting without context");
            Vec::new()
        }
    }
}

async fn fetch(query: &str) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let body = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse(&body))
}

pub(crate) fn parse(html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let result = Selector::parse(".result").expect("selector");
    let title = Selector::parse(".result__title").expect("selector");
    let snippet = Selector::parse(".result__snippet").expect("selector");
    document
        .select(&result)
        .filter(|r| {
            !r.value().attr("class").unwrap_or_default().contains("result--ad")
        })
        .filter_map(|r| {
            let title = r.select(&title).next()?.text().collect::<String>();
            let snippet =
                r.select(&snippet).next().map(|s| s.text().collect::<String>()).unwrap_or_default();
            let line = format!("{} — {}", title.trim(), snippet.trim());
            let line = line.trim_matches([' ', '—']).to_string();
            (!line.is_empty()).then_some(line)
        })
        .take(MAX_SNIPPETS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ddg_result_markup() {
        let html = r#"
            <div class="result">
              <h2 class="result__title"><a>NVIDIA GeForce RTX 3090</a></h2>
              <a class="result__snippet">The GeForce RTX 3090 is a big ferocious GPU.</a>
            </div>
            <div class="result">
              <h2 class="result__title"><a>RTX 3090 review</a></h2>
            </div>"#;
        let out = parse(html);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("ferocious GPU"));
        assert_eq!(out[1], "RTX 3090 review");
    }

    #[test]
    fn ads_are_skipped() {
        let html = r#"
            <div class="result results_links result--ad ">
              <h2 class="result__title"><a>Cheap prices on Amazon</a></h2>
            </div>
            <div class="result results_links web-result ">
              <h2 class="result__title"><a>RTX 3090 specs</a></h2>
            </div>"#;
        let out = parse(html);
        assert_eq!(out, vec!["RTX 3090 specs".to_string()]);
    }

    #[test]
    fn empty_page_yields_nothing() {
        assert!(parse("<html><body>captcha?</body></html>").is_empty());
    }
}
