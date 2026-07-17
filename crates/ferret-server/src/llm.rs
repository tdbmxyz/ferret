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
    fn from_effective(eff: &EffectiveLlm) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(eff.timeout_secs))
                .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building llm http client"),
            url: format!("{}/chat/completions", eff.base_url.trim_end_matches('/')),
            model: eff.model.clone(),
            api_key: eff.api_key.clone(),
        }
    }
}

// ---- runtime configuration: TOML base + DB override, hot-swappable ----

/// DB-stored override of the `[llm]` TOML section (settings key "llm").
/// Saved wholesale from the UI; empty url/model fields fall back to TOML.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct LlmOverride {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

pub const LLM_SETTINGS_KEY: &str = "llm";

/// Fully resolved LLM configuration, ready to build clients from.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveLlm {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u64,
    pub api_key: Option<String>,
    pub from_override: bool,
    /// The override carries its own key (drives the UI's "clear key").
    pub override_key_set: bool,
}

/// Merge the TOML base with an optional DB override. The key file is only
/// read when the result is enabled — a broken path never blocks startup of
/// a disabled pass.
pub fn effective(base: &LlmConfig, o: Option<&LlmOverride>) -> anyhow::Result<EffectiveLlm> {
    let pick = |over: &str, conf: &str| {
        if over.trim().is_empty() { conf.to_string() } else { over.trim().to_string() }
    };
    let (enabled, base_url, model) = match o {
        Some(o) => (o.enabled, pick(&o.base_url, &base.base_url), pick(&o.model, &base.model)),
        None => (base.enabled, base.base_url.clone(), base.model.clone()),
    };
    let override_key = o.and_then(|o| o.api_key.clone()).filter(|k| !k.is_empty());
    let override_key_set = override_key.is_some();
    let api_key = match (&override_key, enabled) {
        (Some(key), _) => Some(key.clone()),
        (None, true) => match &base.api_key_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading llm api key {}: {e}", path.display()))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        },
        (None, false) => None,
    };
    Ok(EffectiveLlm {
        enabled,
        base_url,
        model,
        timeout_secs: base.timeout_secs,
        api_key,
        from_override: o.is_some(),
        override_key_set,
    })
}

/// The live LLM layer, swapped in place when settings change so the
/// scheduler and API handlers pick the new backend up without a restart.
#[derive(Clone, Default)]
pub struct LlmRuntime {
    pub refiner: Option<std::sync::Arc<dyn LlmRefiner>>,
    pub interpreter: Option<std::sync::Arc<dyn LlmInterpret>>,
    pub status: ferret_domain::LlmStatus,
    pub settings: ferret_domain::LlmSettings,
}

pub type LlmHandle = std::sync::Arc<tokio::sync::RwLock<LlmRuntime>>;

pub fn build_runtime(eff: EffectiveLlm) -> LlmRuntime {
    let client = eff.enabled.then(|| std::sync::Arc::new(OpenAiRefiner::from_effective(&eff)));
    LlmRuntime {
        refiner: client.clone().map(|c| c as std::sync::Arc<dyn LlmRefiner>),
        interpreter: client.map(|c| c as std::sync::Arc<dyn LlmInterpret>),
        status: ferret_domain::LlmStatus {
            enabled: eff.enabled,
            model: eff.enabled.then(|| eff.model.clone()),
        },
        settings: ferret_domain::LlmSettings {
            enabled: eff.enabled,
            base_url: eff.base_url,
            model: eff.model,
            api_key_set: eff.override_key_set,
            from_override: eff.from_override,
        },
    }
}

pub async fn load_override(db: &crate::db::Db) -> Option<LlmOverride> {
    let raw = db.get_setting(LLM_SETTINGS_KEY).await.ok()??;
    serde_json::from_str(&raw)
        .map_err(|e| tracing::warn!(error = %e, "ignoring corrupt llm settings override"))
        .ok()
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

/// The assistant text of a chat-completions response body.
pub(crate) fn content_of(body: &str) -> anyhow::Result<String> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("no choices[0].message.content in llm response"))
}

/// Models love wrapping JSON in ```fences``` or prose despite instructions —
/// cut the answer down to its outermost object before parsing.
pub(crate) fn extract_json(content: &str) -> &str {
    match (content.find('{'), content.rfind('}')) {
        (Some(start), Some(end)) if end > start => &content[start..=end],
        _ => content,
    }
}

/// Parse a chat-completions response body into a `Refinement`.
#[cfg(test)]
pub(crate) fn parse_response(body: &str) -> anyhow::Result<Refinement> {
    let content = content_of(body)?;
    Ok(serde_json::from_str(extract_json(&content))?)
}

impl OpenAiRefiner {
    async fn post_chat(&self, body: &serde_json::Value) -> anyhow::Result<String> {
        let mut request = self.http.post(&self.url).json(body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // surface the backend's own message ("model X not found"…)
            anyhow::bail!("{status}: {}", text.chars().take(300).collect::<String>().trim());
        }
        content_of(&text)
    }

    /// One chat call, resilient to backends that reject structured output:
    /// a 4xx with `response_format` set retries once without it (the
    /// prompts already demand a bare JSON object).
    async fn chat(&self, mut body: serde_json::Value) -> anyhow::Result<String> {
        match self.post_chat(&body).await {
            Ok(content) => Ok(content),
            Err(e)
                if body.get("response_format").is_some()
                    && e.to_string().starts_with('4') =>
            {
                tracing::debug!(error = %e, "structured output rejected — retrying plain");
                body.as_object_mut().expect("chat body is an object").remove("response_format");
                self.post_chat(&body).await
            }
            Err(e) => Err(e),
        }
    }
}

#[async_trait::async_trait]
impl LlmRefiner for OpenAiRefiner {
    async fn refine(&self, input: &RefineInput<'_>) -> anyhow::Result<Refinement> {
        let content = self.chat(request_body(&self.model, input)).await?;
        Ok(serde_json::from_str(extract_json(&content))?)
    }
}

// ---- endpoint discovery & probing (settings UI helpers) ----

fn probe_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.min(15)))
        .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("building llm probe client")
}

/// `GET {base_url}/models` — the standard OpenAI-compatible catalog.
pub async fn list_models(base_url: &str, api_key: Option<&str>) -> anyhow::Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = probe_client(10).get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("{status}: {}", text.chars().take(300).collect::<String>().trim());
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let mut models: Vec<String> = v["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no data[] in {url} response"))?
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    models.sort();
    Ok(models)
}

/// One tiny real completion against the endpoint — the settings panel's
/// "Test" button. Errors carry the backend's message verbatim.
pub async fn probe(base_url: &str, model: &str, api_key: Option<&str>) -> anyhow::Result<()> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 20,
        "messages": [{ "role": "user", "content": "Reply with the single word: ok" }],
    });
    let mut request = probe_client(15).post(&url).json(&body);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("{status}: {}", text.chars().take(300).collect::<String>().trim());
    }
    content_of(&text).map(|_| ())
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
    /// `web_context`: search-result snippets about the text, possibly empty.
    async fn interpret(
        &self,
        text: &str,
        categories: &[ferret_domain::Category],
        web_context: &[String],
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
    web_context: &[String],
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
                 allowed_values, or boolean). `web_search_results`, when present, are \
                 snippets about the search — use them to identify what the product is. \
                 Answer only with the JSON object." },
            { "role": "user", "content": serde_json::json!({
                "search": text,
                "known_categories": known,
                "web_search_results": web_context,
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
        web_context: &[String],
    ) -> anyhow::Result<LlmInterpretation> {
        let content = self
            .chat(interpret_request_body(&self.model, text, categories, web_context))
            .await?;
        Ok(serde_json::from_str(extract_json(&content))?)
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
    fn extract_json_strips_fences_and_prose() {
        assert_eq!(extract_json("{\"a\": 1}"), "{\"a\": 1}");
        assert_eq!(extract_json("```json\n{\"a\": 1}\n```"), "{\"a\": 1}");
        assert_eq!(extract_json("Sure! Here is it: {\"a\": {\"b\": 2}} hope it helps"),
            "{\"a\": {\"b\": 2}}");
        assert_eq!(extract_json("no json at all"), "no json at all");
    }

    #[test]
    fn parses_fenced_content() {
        let body = r#"{
            "choices": [{ "message": { "role": "assistant", "content":
                "```json\n{\"verdict\": \"genuine\", \"reason\": \"looks fine\", \"capacity_gb\": 4000, \"condition\": \"used\"}\n```"
            }}]
        }"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.verdict, LlmVerdict::Genuine);
        assert_eq!(r.capacity_gb, Some(4000));
    }

    #[test]
    fn interpret_request_carries_web_context() {
        let body = interpret_request_body(
            "qwen3",
            "RTX 3090",
            &[],
            &["NVIDIA GeForce RTX 3090 — a graphics card".into()],
        );
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("web_search_results"));
        assert!(user.contains("a graphics card"));
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
        let eff = effective(&crate::config::LlmConfig::default(), None).unwrap();
        let runtime = build_runtime(eff);
        assert!(runtime.refiner.is_none() && runtime.interpreter.is_none());
        assert!(!runtime.status.enabled && runtime.status.model.is_none());
    }

    #[test]
    fn override_supersedes_config_blank_fields_fall_back() {
        let base = crate::config::LlmConfig {
            model: "conf-model".into(),
            ..Default::default()
        };
        let o = LlmOverride {
            enabled: true,
            base_url: "http://ollama:11434/v1".into(),
            model: String::new(),
            api_key: Some("sk-x".into()),
        };
        let eff = effective(&base, Some(&o)).unwrap();
        assert!(eff.enabled, "override enables a config-disabled pass");
        assert_eq!(eff.base_url, "http://ollama:11434/v1");
        assert_eq!(eff.model, "conf-model", "blank override field falls back to TOML");
        assert_eq!(eff.api_key.as_deref(), Some("sk-x"));
        assert!(eff.from_override && eff.override_key_set);

        let runtime = build_runtime(eff);
        assert!(runtime.refiner.is_some() && runtime.interpreter.is_some());
        assert_eq!(runtime.status.model.as_deref(), Some("conf-model"));
        assert!(runtime.settings.api_key_set && runtime.settings.from_override);
    }
}
