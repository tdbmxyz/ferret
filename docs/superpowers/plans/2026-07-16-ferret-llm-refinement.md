# ferret LLM Refinement Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the optional LLM refinement pass from the design spec: a single JSON-schema-constrained call to an OpenAI-compatible backend for *ambiguous* listings only, refining attributes (fill-only) and attaching a relevance verdict (genuine / stuffed-title / scam) alongside — never instead of — the heuristic flags. Fail-open on any LLM error.

**Architecture:** A `needs_refinement` pure predicate in `ferret-domain` gates the pass (family matched AND (≥2 models OR price-outlier flag) — the clean single-model case never touches the LLM). The server gets an `LlmRefiner` trait (mockable, like `Notify`) with an `OpenAiRefiner` impl calling `{base_url}/chat/completions` with `response_format: json_schema`. The pipeline refines only NEW ambiguous deals (re-scrapes never re-call), persisting via a fill-only SQL `COALESCE` update so heuristic values are never overwritten.

**Tech Stack:** existing workspace deps only (reqwest, serde_json, async-trait, sqlx).

---

## File structure

```
crates/ferret-domain/src/
  deal.rs        # + LlmVerdict enum; Deal.llm_verdict / Deal.llm_reason (Task 1)
  refine.rs      # needs_refinement() predicate (Task 1)
crates/ferret-server/
  migrations/0003_llm.sql        # llm_verdict, llm_reason columns (Task 2)
  src/db.rs      # persist/load llm fields (COALESCE on update), apply_refinement (Task 2)
  src/config.rs  # [llm] LlmConfig (Task 3)
  ferret.example.toml            # [llm] block (Task 3)
  src/llm.rs     # RefineInput, Refinement, LlmRefiner trait, OpenAiRefiner (Task 4)
  src/pipeline.rs # refine-on-new-ambiguous integration, fail-open (Task 5)
  src/scheduler.rs, src/main.rs  # wiring (Task 5)
```

---

### Task 1: Domain — LlmVerdict, Deal fields, needs_refinement

**Files:** Modify `deal.rs`, Create `refine.rs`, Modify `lib.rs` (+ fix Deal initializers in `matching.rs` tests)

`deal.rs` additions:

```rust
/// LLM relevance verdict for an ambiguous listing. A second, independent
/// signal — heuristic flags are kept untouched next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmVerdict {
    Genuine,
    StuffedTitle,
    Scam,
}
```

Deal gains (after `status`):

```rust
    /// Verdict of the optional LLM refinement pass; None when the listing
    /// was unambiguous or the pass is disabled/failed.
    pub llm_verdict: Option<LlmVerdict>,
    /// Short model-written justification for the verdict.
    pub llm_reason: Option<String>,
```

`refine.rs`:

```rust
//! Gate for the optional LLM refinement pass: only AMBIGUOUS listings are
//! worth a model call — the common case must never touch the LLM.

use crate::deal::Flag;

/// A listing is ambiguous when it matched a product family AND either
/// enumerates several sibling models (stuffed title? genuine bundle?) or
/// carries a price-outlier flag (scam? genuine deal?). No family match =
/// irrelevant listing = never refined.
pub fn needs_refinement(family: Option<&str>, models: &[String], flags: &[Flag]) -> bool {
    family.is_some() && (models.len() >= 2 || flags.contains(&Flag::PriceOutlier))
}
```

Tests (in `refine.rs`): single clean model → false; two models → true; outlier flag single model → true; no family → false even with outlier. Plus in `deal.rs`/`watch.rs` tests: `LlmVerdict` serializes to `"stuffed-title"` etc.

Run: `nix develop -c cargo test -p ferret-domain` — all pass.
Commit: `feat(domain): LlmVerdict, llm fields on Deal, needs_refinement gate`

---

### Task 2: Storage — migration 0003 + fill-only refinement update

**Files:** Create `migrations/0003_llm.sql`, Modify `db.rs`

```sql
-- Optional LLM refinement pass: verdict + reason stored alongside the
-- heuristic flags, never replacing them.
ALTER TABLE deals ADD COLUMN llm_verdict TEXT;
ALTER TABLE deals ADD COLUMN llm_reason TEXT;
```

`db.rs`:
- INSERT: bind both fields (verdict via helper `verdict_to_str`).
- UPDATE (re-scrape): `llm_verdict = COALESCE(?, llm_verdict), llm_reason = COALESCE(?, llm_reason)` — a re-scraped deal (whose in-memory llm fields are None) must not wipe a stored refinement.
- `row_to_deal`: parse verdict string ("genuine" | "stuffed-title" | "scam") → enum, error → `DbError::Corrupt`.
- New method (fill-only semantics in SQL):

```rust
    /// Persist an LLM refinement: verdict + reason, and attribute fills for
    /// values the heuristics left empty. Never overwrites a heuristic
    /// capacity/condition (COALESCE keeps the stored value). Returns the
    /// refined deal.
    pub async fn apply_refinement(
        &self,
        deal_id: Uuid,
        verdict: LlmVerdict,
        reason: &str,
        capacity_gb: Option<i64>,
        condition: Option<&str>,
    ) -> Result<Deal> {
        sqlx::query(
            "UPDATE deals SET llm_verdict = ?, llm_reason = ?,
             capacity_gb = COALESCE(capacity_gb, ?),
             condition = COALESCE(condition, ?) WHERE id = ?",
        )
        .bind(verdict_to_str(verdict))
        .bind(reason)
        .bind(capacity_gb)
        .bind(condition)
        .bind(deal_id.to_string())
        .execute(&self.pool)
        .await?;
        let row = sqlx::query("SELECT * FROM deals WHERE id = ?")
            .bind(deal_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        row_to_deal(&row)
    }
```

Tests: `apply_refinement` fills empty capacity but does NOT overwrite an existing condition; llm fields survive a later `upsert_deal` re-scrape; round-trip through `list_deals`.

Run: `nix develop -c cargo test -p ferret-server db::` — pass.
Commit: `feat(server): persist LLM verdicts with fill-only attribute refinement`

---

### Task 3: `[llm]` config

**Files:** Modify `config.rs`, `ferret.example.toml`

```rust
/// Optional LLM refinement pass. Off unless `enabled = true`; points at
/// any OpenAI-compatible chat-completions API (default shape: self-hosted
/// llama-cpp on zeus).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub enabled: bool,
    /// API root, e.g. `http://zeus:8080/v1` — `/chat/completions` is appended.
    pub base_url: String,
    pub model: String,
    /// Bearer token file (agenix) — only for external backends.
    pub api_key_file: Option<PathBuf>,
    pub timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "http://localhost:8080/v1".into(),
            model: "local".into(),
            api_key_file: None,
            timeout_secs: 30,
        }
    }
}
```

`Config` gains `pub llm: LlmConfig`. Example TOML gets a commented `[llm]` block. Config tests: defaults disabled; example parses.

Commit: `feat(server): [llm] config block`

---

### Task 4: LlmRefiner trait + OpenAI-compatible client

**Files:** Create `src/llm.rs`, Modify `main.rs` (mod decl)

```rust
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
```

Tests (`#[cfg(test)]` in `llm.rs`): `parse_response` happy path (canned chat-completions JSON with a stuffed-title verdict); `parse_response` rejects garbage and missing-content bodies; `request_body` carries the model, the title, and strict json_schema; `OpenAiRefiner::new` returns None when disabled.

Commit: `feat(server): LlmRefiner trait + OpenAI-compatible structured-output client`

---

### Task 5: Pipeline integration (fail-open) + wiring

**Files:** Modify `pipeline.rs`, `scheduler.rs`, `main.rs`

`process_listings` gains `refiner: Option<&dyn LlmRefiner>`. After upsert, before watch matching:

```rust
        // -- optional LLM refinement: new ambiguous deals only (a re-scrape
        //    never re-asks), fail-open on any error --
        let mut stored = stored;
        if let Some(refiner) = refiner
            && was_new
            && let Some(family_name) = stored.family.clone()
            && refine::needs_refinement(Some(&family_name), &stored.models, &stored.flags)
        {
            let input = crate::llm::RefineInput {
                title: &stored.title,
                price_cents: stored.price_cents,
                currency: &stored.currency,
                family: &family_name,
                models: &stored.models,
                flags: &stored.flags,
            };
            match refiner.refine(&input).await {
                Ok(r) => {
                    stored = db
                        .apply_refinement(
                            stored.id, r.verdict, &r.reason, r.capacity_gb,
                            r.condition.as_deref(),
                        )
                        .await?;
                    stats.refined += 1;
                }
                Err(e) => {
                    tracing::warn!(deal = %stored.id, error = %e,
                        "llm refinement failed — keeping heuristic-only verdict");
                }
            }
        }
```

`PipelineStats` gains `pub refined: u64`. Matching then runs on the refined deal (an LLM capacity fill can enable a capacity watch). `scheduler::spawn_all`/`run_source` thread `Option<Arc<dyn LlmRefiner>>` through (log `refined` in the tick line); `main.rs` builds `OpenAiRefiner::new(&config.llm)` and passes it.

Tests (pipeline, with a `MockRefiner { calls: Mutex<u32>, result: Result-producing closure or canned }`):
1. Stuffed new listing + refiner returning `stuffed-title` + capacity fill → deal carries verdict, reason, filled capacity; heuristic `possible-stuffing` flag still present; refiner called exactly once.
2. Refiner returning condition when heuristics already extracted one → stored condition unchanged (fill-only).
3. Erroring refiner → deal persisted, notification still fires, llm fields None (fail-open).
4. Clean single-model listing → refiner never called.
5. Re-scrape of a refined deal → refiner not re-called, llm fields survive.

Final verification: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, smoke test with `enabled = false` (default path).

Commit: `feat(server): gated fail-open LLM refinement in the ETL pipeline`
