//! Typed client for the ferret HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch)
//! targets thanks to reqwest's dual backend. The UI crates go through this
//! client so the API surface is exercised from exactly one place.

use std::time::Duration;

use ferret_domain::{
    Category, ChatTurn, Deal, HealthResponse, Interpretation, LlmProbeRequest, LlmProbeResult,
    LlmSettings, LlmSettingsUpdate, PricePoint, ProductFamily, PromptSet, PromptsResponse,
    SearchJob, StatusResponse, Watch, WatchRequest,
};
use serde::Serialize;
use url::Url;
use uuid::Uuid;

/// Deadline for data requests: generous for a LAN server, short enough
/// that an unreachable host fails the page fast instead of hanging.
const DATA_TIMEOUT: Duration = Duration::from_secs(8);
/// The health probe decides connectivity; it must answer (or fail) fast.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

/// Errors are stringly-typed on purpose: they cross into UI code that only
/// needs to display them, and `reqwest::Error` is neither `Clone` nor
/// available identically on wasm.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("request failed: {0}")]
    Transport(String),
    #[error("server returned {status}: {message}")]
    Api { status: u16, message: String },
    #[error("invalid response body: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[derive(Debug, Clone)]
pub struct FerretClient {
    base: Url,
    http: reqwest::Client,
}

impl FerretClient {
    /// `base` is the server origin, e.g. `http://zeus:4800` — without the
    /// `/api` prefix, which the client appends itself.
    pub fn new(base: Url) -> Self {
        Self { base, http: reqwest::Client::new() }
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path)
            .map_err(|e| ClientError::Transport(format!("bad url {path:?}: {e}")))
    }

    /// Execute a request; map non-2xx into `Api`, body decode into `Decode`.
    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        timeout: Duration,
    ) -> Result<T> {
        // reqwest's builder-level `.timeout()` isn't available on wasm;
        // the request-level one is (chaos pattern).
        let mut request = request.build().map_err(|e| ClientError::Transport(e.to_string()))?;
        *request.timeout_mut() = Some(timeout);
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        response.json().await.map_err(|e| ClientError::Decode(e.to_string()))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.http.get(self.url(path)?), DATA_TIMEOUT).await
    }

    pub async fn health(&self) -> Result<HealthResponse> {
        self.send(self.http.get(self.url("api/health")?), HEALTH_TIMEOUT).await
    }

    /// Scheduler liveness per source + match counts per watch.
    pub async fn status(&self) -> Result<StatusResponse> {
        self.get("api/status").await
    }

    // ---- watches ----

    pub async fn watches(&self) -> Result<Vec<Watch>> {
        self.get("api/watches").await
    }

    pub async fn create_watch(&self, request: &WatchRequest) -> Result<Watch> {
        self.send(self.http.post(self.url("api/watches")?).json(request), DATA_TIMEOUT)
            .await
    }

    pub async fn update_watch(&self, id: Uuid, request: &WatchRequest) -> Result<Watch> {
        self.send(
            self.http.put(self.url(&format!("api/watches/{id}"))?).json(request),
            DATA_TIMEOUT,
        )
        .await
    }

    pub async fn delete_watch(&self, id: Uuid) -> Result<()> {
        let request = self.http.delete(self.url(&format!("api/watches/{id}"))?);
        let mut request = request.build().map_err(|e| ClientError::Transport(e.to_string()))?;
        *request.timeout_mut() = Some(DATA_TIMEOUT);
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Ok(())
    }

    // ---- deals ----

    /// All deals, or one watch's matches when `watch_id` is set.
    /// `hidden = true` lists ONLY dismissed/banned deals (review view).
    pub async fn deals(&self, watch_id: Option<Uuid>, hidden: bool) -> Result<Vec<Deal>> {
        let path = match (watch_id, hidden) {
            (Some(id), h) => format!("api/deals?watch_id={id}&hidden={h}"),
            (None, true) => "api/deals?hidden=true".into(),
            (None, false) => "api/deals".into(),
        };
        self.get(&path).await
    }

    /// Set the user verdict on a deal (dismiss / ban / restore to none).
    pub async fn set_moderation(
        &self,
        deal_id: Uuid,
        moderation: ferret_domain::Moderation,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Body {
            moderation: ferret_domain::Moderation,
        }
        let request = self
            .http
            .put(self.url(&format!("api/deals/{deal_id}/moderation"))?)
            .json(&Body { moderation });
        let mut request = request.build().map_err(|e| ClientError::Transport(e.to_string()))?;
        *request.timeout_mut() = Some(DATA_TIMEOUT);
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, message });
        }
        Ok(())
    }

    pub async fn deal_prices(&self, deal_id: Uuid) -> Result<Vec<PricePoint>> {
        self.get(&format!("api/deals/{deal_id}/prices")).await
    }

    pub async fn families(&self) -> Result<Vec<ProductFamily>> {
        self.get("api/families").await
    }

    // ---- guided watch creation ----

    pub async fn categories(&self) -> Result<Vec<Category>> {
        self.get("api/categories").await
    }

    /// Create/replace a category — also approves an LLM proposal when
    /// posted back with status "active".
    pub async fn upsert_category(&self, category: &Category) -> Result<Category> {
        self.send(self.http.post(self.url("api/categories")?).json(category), DATA_TIMEOUT)
            .await
    }

    pub async fn delete_category(&self, slug: &str) -> Result<()> {
        let request = self.http.delete(self.url(&format!("api/categories/{slug}"))?);
        let mut request = request.build().map_err(|e| ClientError::Transport(e.to_string()))?;
        *request.timeout_mut() = Some(DATA_TIMEOUT);
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, message });
        }
        Ok(())
    }

    /// Ask the LLM to rework a category ("add an rpm spec"…). Returns the
    /// revised draft — nothing is saved until the user does.
    pub async fn revise_category(
        &self,
        category: &Category,
        instruction: &str,
        history: &[ChatTurn],
    ) -> Result<Category> {
        #[derive(Serialize)]
        struct Body<'a> {
            category: &'a Category,
            instruction: &'a str,
            history: &'a [ChatTurn],
        }
        self.send(
            self.http
                .post(self.url("api/categories/revise")?)
                .json(&Body { category, instruction, history }),
            Duration::from_secs(330),
        )
        .await
    }

    // ---- system prompts ----

    pub async fn prompts(&self) -> Result<PromptsResponse> {
        self.get("api/settings/prompts").await
    }

    pub async fn update_prompts(&self, set: &PromptSet) -> Result<PromptsResponse> {
        self.send(self.http.put(self.url("api/settings/prompts")?).json(set), DATA_TIMEOUT)
            .await
    }

    pub async fn reset_prompts(&self) -> Result<PromptsResponse> {
        self.send(self.http.delete(self.url("api/settings/prompts")?), DATA_TIMEOUT)
            .await
    }

    /// What product is this text about? Instant for known categories, may
    /// take LLM latency otherwise.
    pub async fn interpret(&self, text: &str) -> Result<Interpretation> {
        #[derive(Serialize)]
        struct Body<'a> {
            text: &'a str,
        }
        self.send(
            self.http.post(self.url("api/interpret")?).json(&Body { text }),
            // interpret may sit on a slow local LLM — give it real room
            Duration::from_secs(330),
        )
        .await
    }

    // ---- server settings ----

    /// Effective LLM settings (TOML base + DB override).
    pub async fn llm_settings(&self) -> Result<LlmSettings> {
        self.get("api/settings/llm").await
    }

    /// Store an override and apply it live; answers the new effective view.
    pub async fn update_llm_settings(&self, update: &LlmSettingsUpdate) -> Result<LlmSettings> {
        self.send(self.http.put(self.url("api/settings/llm")?).json(update), DATA_TIMEOUT)
            .await
    }

    /// Drop the override — back to what the TOML config says.
    pub async fn reset_llm_settings(&self) -> Result<LlmSettings> {
        self.send(self.http.delete(self.url("api/settings/llm")?), DATA_TIMEOUT)
            .await
    }

    /// Ask the endpoint (typed or stored values) for its model catalog.
    pub async fn llm_models(&self, probe: &LlmProbeRequest) -> Result<Vec<String>> {
        self.send(
            self.http.post(self.url("api/settings/llm/models")?).json(probe),
            Duration::from_secs(15),
        )
        .await
    }

    /// One real completion round-trip — is this endpoint/model usable?
    pub async fn test_llm(&self, probe: &LlmProbeRequest) -> Result<LlmProbeResult> {
        self.send(
            self.http.post(self.url("api/settings/llm/test")?).json(probe),
            Duration::from_secs(120),
        )
        .await
    }

    /// Kick off a background ad-hoc search; poll `search_progress`.
    pub async fn start_search(&self, queries: &[String]) -> Result<Uuid> {
        #[derive(Serialize)]
        struct Body<'a> {
            queries: &'a [String],
        }
        #[derive(serde::Deserialize)]
        struct Created {
            id: Uuid,
        }
        let created: Created = self
            .send(
                self.http.post(self.url("api/searches")?).json(&Body { queries }),
                DATA_TIMEOUT,
            )
            .await?;
        Ok(created.id)
    }

    pub async fn search_progress(&self, id: Uuid) -> Result<SearchJob> {
        self.get(&format!("api/searches/{id}")).await
    }
}
