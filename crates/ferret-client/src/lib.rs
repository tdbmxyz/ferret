//! Typed client for the ferret HTTP API.
//!
//! Compiles on both native (tokio + rustls) and wasm (browser fetch)
//! targets thanks to reqwest's dual backend. The UI crates go through this
//! client so the API surface is exercised from exactly one place.

use std::time::Duration;

use ferret_domain::{
    Deal, HealthResponse, PricePoint, ProductFamily, StatusResponse, Watch, WatchRequest,
};
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
    pub async fn deals(&self, watch_id: Option<Uuid>) -> Result<Vec<Deal>> {
        match watch_id {
            Some(id) => self.get(&format!("api/deals?watch_id={id}")).await,
            None => self.get("api/deals").await,
        }
    }

    pub async fn deal_prices(&self, deal_id: Uuid) -> Result<Vec<PricePoint>> {
        self.get(&format!("api/deals/{deal_id}/prices")).await
    }

    pub async fn families(&self) -> Result<Vec<ProductFamily>> {
        self.get("api/families").await
    }
}
