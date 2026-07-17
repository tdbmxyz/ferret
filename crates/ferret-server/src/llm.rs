//! Optional LLM refinement of ambiguous listings via any OpenAI-compatible
//! chat-completions API (llama-cpp on zeus by default). One structured-
//! output call per ambiguous listing; the pipeline treats every error as
//! fail-open — the LLM is a refinement layer, never a dependency.

use std::time::Duration;

use ferret_domain::{Flag, LlmVerdict};
use serde::Deserialize;

use crate::config::LlmConfig;

/// What the pipeline knows about an ambiguous listing.
pub struct RefineInput<'a> {
    pub title: &'a str,
    pub price_cents: i64,
    pub currency: &'a str,
    pub family: &'a str,
    pub models: &'a [String],
    pub flags: &'a [Flag],
}

/// Structured verdict returned by the model (schema-constrained).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Refinement {
    pub verdict: LlmVerdict,
    pub reason: String,
    #[serde(default)]
    pub capacity_gb: Option<i64>,
    #[serde(default)]
    pub condition: Option<String>,
}

#[async_trait::async_trait]
pub trait LlmRefiner: Send + Sync {
    async fn refine(&self, input: &RefineInput<'_>) -> anyhow::Result<Refinement>;
}

pub struct OpenAiRefiner {
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiRefiner {
    /// `None` when the pass is disabled.
    pub fn new(config: &LlmConfig) -> anyhow::Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }
        let api_key = match &config.api_key_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading llm api key {}: {e}", path.display()))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        };
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_secs))
                .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building llm http client"),
            url: format!("{}/chat/completions", config.base_url.trim_end_matches('/')),
            model: config.model.clone(),
            api_key,
        }))
    }
}

/// The JSON schema the model must answer with (strict structured output).
fn response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["genuine", "stuffed-title", "scam"] },
            "reason": { "type": "string" },
            "capacity_gb": { "type": ["integer", "null"] },
            "condition": { "type": ["string", "null"], "enum": ["new", "used", "refurbished", null] }
        },
        "required": ["verdict", "reason", "capacity_gb", "condition"],
        "additionalProperties": false
    })
}

pub(crate) fn request_body(model: &str, input: &RefineInput<'_>) -> serde_json::Value {
    let listing = serde_json::json!({
        "title": input.title,
        "price": format!("{:.2} {}", input.price_cents as f64 / 100.0, input.currency),
        "product_family": input.family,
        "models_mentioned": input.models,
        "heuristic_flags": input.flags,
    });
    serde_json::json!({
        "model": model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content":
                "You review second-hand hardware marketplace listings. Given a listing whose \
                 title mentions the models listed, decide: is it a genuine offer for one \
                 product (\"genuine\"), a title stuffed with sibling model names for search \
                 visibility or an accessory/bundle mentioning many models (\"stuffed-title\"), \
                 or a likely scam, e.g. an implausibly low price (\"scam\")? Also extract the \
                 storage/RAM capacity in decimal gigabytes and the condition when the title \
                 states them, else null. Answer only with the JSON object." },
            { "role": "user", "content": listing.to_string() }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": "refinement", "strict": true, "schema": response_schema() }
        }
    })
}

/// Parse a chat-completions response body into a `Refinement`.
pub(crate) fn parse_response(body: &str) -> anyhow::Result<Refinement> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no choices[0].message.content in llm response"))?;
    Ok(serde_json::from_str(content)?)
}

#[async_trait::async_trait]
impl LlmRefiner for OpenAiRefiner {
    async fn refine(&self, input: &RefineInput<'_>) -> anyhow::Result<Refinement> {
        let mut request = self.http.post(&self.url).json(&request_body(&self.model, input));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let body = request.send().await?.error_for_status()?.text().await?;
        parse_response(&body)
    }
}

// ---- guided-creation interpretation ----

/// Structured answer to "what product is this text about?".
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LlmInterpretation {
    /// Slug of a PROVIDED category, or null when none fits.
    pub category_slug: Option<String>,
    #[serde(default)]
    pub constraints: Vec<LlmConstraint>,
    /// Drafted new category when nothing provided fits.
    pub proposal: Option<LlmProposal>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LlmConstraint {
    /// "min" | "max" | "eq"
    pub op: String,
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LlmProposal {
    pub slug: String,
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub specs: Vec<LlmProposalSpec>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LlmProposalSpec {
    pub key: String,
    pub label: String,
    /// "number" | "enum" | "boolean"
    pub kind: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub allowed_values: Vec<String>,
}

#[async_trait::async_trait]
pub trait LlmInterpret: Send + Sync {
    async fn interpret(
        &self,
        text: &str,
        categories: &[ferret_domain::Category],
    ) -> anyhow::Result<LlmInterpretation>;
}

fn interpret_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "category_slug": { "type": ["string", "null"] },
            "constraints": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["min", "max", "eq"] },
                    "key": { "type": "string" },
                    "value": {}
                },
                "required": ["op", "key", "value"],
                "additionalProperties": false
            }},
            "proposal": { "type": ["object", "null"], "properties": {
                "slug": { "type": "string" },
                "label": { "type": "string" },
                "aliases": { "type": "array", "items": { "type": "string" } },
                "specs": { "type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" },
                        "label": { "type": "string" },
                        "kind": { "type": "string", "enum": ["number", "enum", "boolean"] },
                        "unit": { "type": ["string", "null"] },
                        "allowed_values": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["key", "label", "kind", "unit", "allowed_values"],
                    "additionalProperties": false
                }}
            },
            "required": ["slug", "label", "aliases", "specs"],
            "additionalProperties": false }
        },
        "required": ["category_slug", "constraints", "proposal"],
        "additionalProperties": false
    })
}

pub(crate) fn interpret_request_body(
    model: &str,
    text: &str,
    categories: &[ferret_domain::Category],
) -> serde_json::Value {
    let known: Vec<serde_json::Value> = categories
        .iter()
        .map(|c| {
            serde_json::json!({
                "slug": c.slug,
                "label": c.label,
                "specs": c.specs.iter().map(|s| serde_json::json!({
                    "key": s.key, "kind": s.kind, "unit": s.unit,
                    "allowed_values": s.allowed_values,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "model": model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content":
                "A user typed a product search into a second-hand deal tracker. Map it to ONE \
                 of the known product categories (by slug) and derive spec constraints from the \
                 text using that category's spec keys (a quantity the user typed is a minimum: \
                 op \"min\"; a named variant is op \"eq\"). If NO known category fits, set \
                 category_slug to null and draft a `proposal`: a kebab-case slug, a short \
                 label, title words that identify the product (aliases), and 1-4 spec \
                 dimensions buyers filter on (kind number with a unit, enum with \
                 allowed_values, or boolean). Answer only with the JSON object." },
            { "role": "user", "content": serde_json::json!({
                "search": text,
                "known_categories": known,
            }).to_string() }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": "interpretation", "strict": true, "schema": interpret_schema() }
        }
    })
}

#[async_trait::async_trait]
impl LlmInterpret for OpenAiRefiner {
    async fn interpret(
        &self,
        text: &str,
        categories: &[ferret_domain::Category],
    ) -> anyhow::Result<LlmInterpretation> {
        let mut request = self
            .http
            .post(&self.url)
            .json(&interpret_request_body(&self.model, text, categories));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let body = request.send().await?.error_for_status()?.text().await?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no choices[0].message.content in llm response"))?;
        Ok(serde_json::from_str(content)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> RefineInput<'static> {
        RefineInput {
            title: "Brackets for 3070 3080 3090",
            price_cents: 40_000,
            currency: "EUR",
            family: "nvidia-rtx",
            models: Box::leak(Box::new(["3070".to_string(), "3080".into(), "3090".into()])),
            flags: &[Flag::PossibleStuffing],
        }
    }

    #[test]
    fn parses_chat_completions_response() {
        let body = r#"{
            "choices": [{ "message": { "role": "assistant", "content":
                "{\"verdict\": \"stuffed-title\", \"reason\": \"bracket accessory, not a GPU\", \"capacity_gb\": null, \"condition\": null}"
            }}]
        }"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.verdict, LlmVerdict::StuffedTitle);
        assert_eq!(r.reason, "bracket accessory, not a GPU");
        assert_eq!(r.capacity_gb, None);
    }

    #[test]
    fn rejects_malformed_responses() {
        assert!(parse_response("not json").is_err());
        assert!(parse_response(r#"{"choices": []}"#).is_err());
        let bad_verdict = r#"{"choices": [{"message": {"content": "{\"verdict\": \"maybe\", \"reason\": \"\"}"}}]}"#;
        assert!(parse_response(bad_verdict).is_err());
    }

    #[test]
    fn request_carries_model_listing_and_strict_schema() {
        let body = request_body("qwen3", &input());
        assert_eq!(body["model"], "qwen3");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("Brackets for 3070 3080 3090"));
        assert!(user.contains("400.00 EUR"));
        assert!(user.contains("possible-stuffing"));
    }

    #[test]
    fn disabled_config_builds_no_refiner() {
        let refiner = OpenAiRefiner::new(&crate::config::LlmConfig::default()).unwrap();
        assert!(refiner.is_none());
    }
}
