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
    timeout: Duration,
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
            timeout: Duration::from_secs(eff.timeout_secs),
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
        // ollama-style backends default to ~128 output tokens, which
        // truncates any JSON answer into a parse error — always be explicit
        "max_tokens": 600,
        "chat_template_kwargs": no_thinking(),
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
    let choice = &v["choices"][0];
    if let Some(content) = choice["message"]["content"].as_str()
        && !content.trim().is_empty()
    {
        return Ok(content.to_string());
    }
    // llama.cpp reasoning models put thoughts in reasoning_content; when
    // the token budget runs out mid-think, content stays empty
    if let Some(reasoning) = choice["message"]["reasoning_content"].as_str()
        && !reasoning.trim().is_empty()
    {
        let finish = choice["finish_reason"].as_str().unwrap_or("?");
        if finish == "stop" && reasoning.contains('{') {
            // finished cleanly — the answer may simply live in there
            return Ok(reasoning.to_string());
        }
        anyhow::bail!(
            "the model spent its whole token budget reasoning without answering \
             (finish_reason={finish}) — thinking should be disabled for this call"
        );
    }
    anyhow::bail!("no choices[0].message.content in llm response")
}

/// Ask reasoning-capable models (Qwen3 family on llama.cpp/vllm) to skip
/// the thinking phase: these calls want a small JSON object, and thoughts
/// otherwise eat the whole `max_tokens` budget before the answer starts.
/// Backends that reject the unknown field get a plain retry (see
/// `chat_json`), the rest ignore it.
fn no_thinking() -> serde_json::Value {
    serde_json::json!({ "enable_thinking": false })
}

/// Models love wrapping JSON in ```fences``` or prose despite instructions —
/// cut the answer down to its outermost object before parsing.
pub(crate) fn extract_json(content: &str) -> &str {
    // reasoning models prepend <think>…</think>, which may itself contain
    // braces — skip past it before hunting for the object
    let content = match content.rfind("</think>") {
        Some(i) => &content[i + "</think>".len()..],
        None => content,
    };
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
    async fn post_chat(
        &self,
        body: &serde_json::Value,
        timeout: Option<Duration>,
    ) -> anyhow::Result<String> {
        let mut request = self.http.post(&self.url).json(body);
        if let Some(t) = timeout {
            request = request.timeout(t);
        }
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

    /// One structured chat call, resilient to backends that reject OR
    /// silently mangle `response_format`: any failure (HTTP or parse) on
    /// the structured attempt gets one plain retry — the prompts already
    /// demand a bare JSON object.
    async fn chat_json<T: serde::de::DeserializeOwned>(
        &self,
        mut body: serde_json::Value,
        timeout: Option<Duration>,
    ) -> anyhow::Result<T> {
        fn parse<T: serde::de::DeserializeOwned>(content: &str) -> anyhow::Result<T> {
            Ok(serde_json::from_str(extract_json(content))?)
        }
        let first = match self.post_chat(&body, timeout).await {
            Ok(content) => match parse(&content) {
                Ok(v) => return Ok(v),
                Err(e) => anyhow::anyhow!("{e} (content: {})", content.chars().take(120).collect::<String>()),
            },
            Err(e) => e,
        };
        if body.get("response_format").is_none() && body.get("chat_template_kwargs").is_none() {
            return Err(first);
        }
        tracing::debug!(error = %first, "structured attempt failed — retrying plain");
        // strip everything a strict OpenAI-style backend might reject
        let object = body.as_object_mut().expect("chat body is an object");
        object.remove("response_format");
        object.remove("chat_template_kwargs");
        let content = self
            .post_chat(&body, timeout)
            .await
            .map_err(|e| anyhow::anyhow!("{first}; plain retry: {e}"))?;
        parse(&content).map_err(|e| anyhow::anyhow!("{first}; plain retry: {e}"))
    }

    /// Interprets/revisions generate a lot more than refinements — give
    /// slow local models room regardless of a tight refine timeout.
    fn long_timeout(&self) -> Option<Duration> {
        Some(self.timeout.max(Duration::from_secs(120)))
    }
}

#[async_trait::async_trait]
impl LlmRefiner for OpenAiRefiner {
    async fn refine(&self, input: &RefineInput<'_>) -> anyhow::Result<Refinement> {
        self.chat_json(request_body(&self.model, input), None).await
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
        "max_tokens": 100,
        "chat_template_kwargs": { "enable_thinking": false },
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
    /// Boolean kind: title words that mean "yes".
    #[serde(default)]
    pub extraction_hint: Option<String>,
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

    /// Rework a category per the user's instruction ("add an rpm spec",
    /// "labels in French"…), returning the full revised draft.
    async fn revise(
        &self,
        _category: &ferret_domain::Category,
        _instruction: &str,
    ) -> anyhow::Result<LlmProposal> {
        anyhow::bail!("category revision is not supported by this backend")
    }
}

/// The category shape shared by proposals (interpret) and revisions.
fn proposal_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
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
                    "allowed_values": { "type": "array", "items": { "type": "string" } },
                    "extraction_hint": { "type": ["string", "null"] }
                },
                "required": ["key", "label", "kind", "unit", "allowed_values", "extraction_hint"],
                "additionalProperties": false
            }}
        },
        "required": ["slug", "label", "aliases", "specs"],
        "additionalProperties": false
    })
}

fn interpret_schema() -> serde_json::Value {
    let mut proposal = proposal_schema();
    proposal["type"] = serde_json::json!(["object", "null"]);
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
            "proposal": proposal
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
        // explicit — ollama-style backends truncate at ~128 tokens otherwise
        "max_tokens": 2000,
        "chat_template_kwargs": no_thinking(),
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

/// Serialize a category the way revision prompts want to see it.
fn category_json(category: &ferret_domain::Category) -> serde_json::Value {
    serde_json::json!({
        "slug": category.slug,
        "label": category.label,
        "aliases": category.aliases,
        "specs": category.specs.iter().map(|s| serde_json::json!({
            "key": s.key, "label": s.label, "kind": s.kind, "unit": s.unit,
            "allowed_values": s.allowed_values, "extraction_hint": s.extraction_hint,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn revise_request_body(
    model: &str,
    category: &ferret_domain::Category,
    instruction: &str,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "temperature": 0,
        "max_tokens": 2000,
        "chat_template_kwargs": no_thinking(),
        "messages": [
            { "role": "system", "content":
                "You maintain product category definitions for a second-hand deal \
                 tracker. Given the current category (JSON) and the user's instruction, \
                 answer with the FULL revised category object — apply the instruction \
                 and keep everything else unchanged, including the slug. Specs are the \
                 filters buyers get: kind \"number\" carries a unit, \"enum\" carries \
                 allowed_values, \"boolean\" carries extraction_hint (title words that \
                 mean yes). Answer only with the JSON object." },
            { "role": "user", "content": serde_json::json!({
                "category": category_json(category),
                "instruction": instruction,
            }).to_string() }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": "category", "strict": true, "schema": proposal_schema() }
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
        self.chat_json(
            interpret_request_body(&self.model, text, categories, web_context),
            self.long_timeout(),
        )
        .await
    }

    async fn revise(
        &self,
        category: &ferret_domain::Category,
        instruction: &str,
    ) -> anyhow::Result<LlmProposal> {
        self.chat_json(
            revise_request_body(&self.model, category, instruction),
            self.long_timeout(),
        )
        .await
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
    fn extract_json_skips_think_blocks() {
        let content = "<think>Hmm {is it an hdd?} let me think</think>\n{\"a\": 1}";
        assert_eq!(extract_json(content), "{\"a\": 1}");
    }

    #[test]
    fn request_bodies_set_explicit_max_tokens() {
        assert!(request_body("m", &input())["max_tokens"].as_u64().unwrap() >= 500);
        assert!(
            interpret_request_body("m", "x", &[], &[])["max_tokens"].as_u64().unwrap() >= 1000
        );
    }

    #[test]
    fn request_bodies_disable_thinking() {
        for body in [
            request_body("m", &input()),
            interpret_request_body("m", "x", &[], &[]),
        ] {
            assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        }
    }

    #[test]
    fn empty_content_with_reasoning_is_a_clear_error() {
        // Qwen-style reasoning ate the whole budget (the real zeus failure)
        let body = r#"{"choices": [{"finish_reason": "length", "message":
            {"role": "assistant", "content": "", "reasoning_content": "Let me think..."}}]}"#;
        let err = content_of(body).unwrap_err().to_string();
        assert!(err.contains("token budget reasoning"), "got: {err}");
        assert!(err.contains("finish_reason=length"), "got: {err}");

        // finished cleanly but the answer hid in reasoning_content → salvaged
        let body = r#"{"choices": [{"finish_reason": "stop", "message":
            {"role": "assistant", "content": "", "reasoning_content": "here: {\"a\": 1}"}}]}"#;
        assert!(content_of(body).unwrap().contains("{\"a\": 1}"));
    }

    #[test]
    fn revise_request_carries_category_and_instruction() {
        let category = ferret_domain::Category {
            slug: "hdd".into(),
            label: "Hard drive".into(),
            aliases: vec!["hdd".into()],
            origin: ferret_domain::CategoryOrigin::Curated,
            status: ferret_domain::CategoryStatus::Active,
            specs: vec![],
            created_at: chrono::DateTime::UNIX_EPOCH,
        };
        let body = revise_request_body("qwen3", &category, "add an rpm spec");
        let user = body["messages"][1]["content"].as_str().unwrap();
        assert!(user.contains("\"slug\":\"hdd\""));
        assert!(user.contains("add an rpm spec"));
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert!(body["max_tokens"].as_u64().unwrap() >= 1000);
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
