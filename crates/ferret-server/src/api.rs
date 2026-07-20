//! REST API. Single-user LAN/tailnet trust model — no auth (same as chaos
//! pre-auth). Errors map: NotFound → 404, Corrupt/Sqlx → 500.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ferret_domain::WatchRequest;
use serde::Deserialize;
use uuid::Uuid;

use crate::db::DbError;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/watches", get(list_watches).post(create_watch))
        .route("/api/watches/{id}", axum::routing::put(update_watch).delete(delete_watch))
        .route("/api/deals", get(list_deals))
        .route("/api/deals/{id}/prices", get(deal_prices))
        .route("/api/families", get(list_families))
        .route("/api/categories", get(list_categories).post(upsert_category))
        .route("/api/categories/{slug}", axum::routing::delete(delete_category))
        .route("/api/categories/revise", axum::routing::post(revise_category))
        .route("/api/interpret", axum::routing::post(interpret_text))
        .route(
            "/api/settings/llm",
            get(get_llm_settings).put(put_llm_settings).delete(delete_llm_settings),
        )
        .route("/api/settings/llm/models", axum::routing::post(list_llm_models))
        .route("/api/settings/llm/test", axum::routing::post(test_llm))
        .route(
            "/api/settings/prompts",
            get(get_prompts).put(put_prompts).delete(delete_prompts),
        )
        .route("/api/searches", axum::routing::post(start_search))
        .route("/api/searches/{id}", get(search_progress))
        .with_state(state)
}

async fn health() -> Response {
    Json(ferret_domain::HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: Some(env!("FERRET_COMMIT").into()),
    })
    .into_response()
}

struct ApiError(DbError);

impl From<DbError> for ApiError {
    fn from(e: DbError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            DbError::NotFound => StatusCode::NOT_FOUND,
            _ => {
                tracing::error!(error = %self.0, "api database error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, self.0.to_string()).into_response()
    }
}

async fn status(State(state): State<AppState>) -> Result<Response, ApiError> {
    let mut sources: Vec<_> = state.statuses.read().await.values().cloned().collect();
    sources.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    let watch_matches = state.db.count_matches().await?;
    let llm = {
        let runtime = state.llm.read().await;
        let mut llm = runtime.status.clone();
        llm.busy = runtime.busy.load(std::sync::atomic::Ordering::SeqCst);
        llm
    };
    let llm = ferret_domain::LlmStatus { avg_ms: state.db.llm_avg_ms().await?, ..llm };
    Ok(Json(ferret_domain::StatusResponse { sources, watch_matches, llm }).into_response())
}

async fn list_watches(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_watches().await?).into_response())
}

async fn create_watch(
    State(state): State<AppState>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    let watch = state.db.create_watch(&req).await?;
    if let Err(e) =
        crate::watches::retro_match(&state.db, state.notifier.as_ref(), &watch, "created").await
    {
        // feedback is best-effort; the watch itself is saved
        tracing::warn!(error = %e, "retro-match after create failed");
    }
    let _ = crate::state::refresh_watch_queries(&state.db, &state.shared_queries).await;
    Ok((StatusCode::CREATED, Json(watch)).into_response())
}

async fn update_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    let watch = state.db.update_watch(id, &req).await?;
    if let Err(e) =
        crate::watches::retro_match(&state.db, state.notifier.as_ref(), &watch, "updated").await
    {
        tracing::warn!(error = %e, "retro-match after update failed");
    }
    let _ = crate::state::refresh_watch_queries(&state.db, &state.shared_queries).await;
    Ok(Json(watch).into_response())
}

async fn delete_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    state.db.delete_watch(id).await?;
    let _ = crate::state::refresh_watch_queries(&state.db, &state.shared_queries).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct DealsQuery {
    watch_id: Option<Uuid>,
}

async fn list_deals(
    State(state): State<AppState>,
    Query(q): Query<DealsQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_deals(q.watch_id).await?).into_response())
}

async fn deal_prices(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    Ok(Json(state.db.deal_prices(id).await?).into_response())
}

async fn list_families(State(state): State<AppState>) -> Response {
    Json(state.families.as_ref().clone()).into_response()
}

// ---- guided watch creation ----

async fn list_categories(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_categories().await?).into_response())
}

/// Create/replace a category — also how an LLM proposal gets approved
/// (the UI posts it back with status="active", possibly edited).
async fn upsert_category(
    State(state): State<AppState>,
    Json(category): Json<ferret_domain::Category>,
) -> Result<Response, ApiError> {
    state.db.upsert_category(&category).await?;
    Ok((StatusCode::CREATED, Json(category)).into_response())
}

async fn delete_category(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    state.db.delete_category(&slug).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct ReviseRequest {
    category: ferret_domain::Category,
    instruction: String,
    /// Earlier turns of this revision conversation.
    #[serde(default)]
    history: Vec<ferret_domain::ChatTurn>,
}

/// Let the LLM rework a category draft ("add an rpm spec", "labels in
/// French"…). Nothing is persisted — the revision loads into the editor
/// for the user to review and save.
async fn revise_category(
    State(state): State<AppState>,
    Json(req): Json<ReviseRequest>,
) -> Response {
    let Some(llm) = state.llm.read().await.interpreter.clone() else {
        return (StatusCode::CONFLICT, "no LLM configured — set one under ⚙").into_response();
    };
    match llm.revise(&req.category, &req.instruction, &req.history).await {
        Ok(draft) => {
            // the slug, origin, status and age of the category are not the
            // LLM's to change
            let revised = ferret_domain::Category {
                slug: req.category.slug,
                label: if draft.label.trim().is_empty() { req.category.label } else { draft.label },
                aliases: draft.aliases,
                origin: req.category.origin,
                status: req.category.status,
                specs: draft.specs.iter().filter_map(crate::interpret::to_spec).collect(),
                created_at: req.category.created_at,
            };
            Json(revised).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct InterpretRequest {
    text: String,
}

async fn interpret_text(
    State(state): State<AppState>,
    Json(req): Json<InterpretRequest>,
) -> Result<Response, ApiError> {
    let categories = state.db.list_categories().await?;
    let interpreter = state.llm.read().await.interpreter.clone();
    let out = crate::interpret::interpret(
        &req.text,
        &categories,
        interpreter.as_deref(),
        |q: String| async move { crate::websearch::snippets(&q).await },
    )
    .await;
    Ok(Json(out).into_response())
}

// ---- LLM settings (TOML base + DB override, applied live) ----

async fn get_llm_settings(State(state): State<AppState>) -> Response {
    Json(state.llm.read().await.settings.clone()).into_response()
}

async fn put_llm_settings(
    State(state): State<AppState>,
    Json(update): Json<ferret_domain::LlmSettingsUpdate>,
) -> Result<Response, ApiError> {
    // `api_key: None` keeps the currently stored key; `Some("")` clears it
    let api_key = match update.api_key {
        Some(key) if key.is_empty() => None,
        Some(key) => Some(key),
        None => crate::llm::load_override(&state.db).await.and_then(|o| o.api_key),
    };
    let over = crate::llm::LlmOverride {
        enabled: update.enabled,
        base_url: update.base_url,
        model: update.model,
        api_key,
    };
    state
        .db
        .put_setting(
            crate::llm::LLM_SETTINGS_KEY,
            &serde_json::to_string(&over).expect("override serializes"),
        )
        .await?;
    apply_llm(&state, Some(&over)).await
}

async fn delete_llm_settings(State(state): State<AppState>) -> Result<Response, ApiError> {
    state.db.delete_setting(crate::llm::LLM_SETTINGS_KEY).await?;
    apply_llm(&state, None).await
}

/// Resolve probe inputs: fields typed in the form win, missing ones fall
/// back to the effective settings (and the stored/config API key).
async fn probe_target(
    state: &AppState,
    req: ferret_domain::LlmProbeRequest,
) -> (String, String, Option<String>) {
    let over = crate::llm::load_override(&state.db).await;
    let eff = crate::llm::effective(&state.search.config.llm, over.as_ref())
        .unwrap_or_else(|_| crate::llm::EffectiveLlm {
            enabled: false,
            base_url: state.search.config.llm.base_url.clone(),
            model: state.search.config.llm.model.clone(),
            timeout_secs: state.search.config.llm.timeout_secs,
            api_key: None,
            from_override: false,
            override_key_set: false,
        });
    let pick = |v: Option<String>, fallback: String| {
        v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or(fallback)
    };
    (
        pick(req.base_url, eff.base_url),
        pick(req.model, eff.model),
        req.api_key.filter(|k| !k.is_empty()).or(eff.api_key),
    )
}

async fn list_llm_models(
    State(state): State<AppState>,
    Json(req): Json<ferret_domain::LlmProbeRequest>,
) -> Response {
    let (base_url, _, api_key) = probe_target(&state, req).await;
    match crate::llm::list_models(&base_url, api_key.as_deref()).await {
        Ok(models) => Json(models).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

async fn test_llm(
    State(state): State<AppState>,
    Json(req): Json<ferret_domain::LlmProbeRequest>,
) -> Response {
    let (base_url, model, api_key) = probe_target(&state, req).await;
    let result = match crate::llm::probe(&base_url, &model, api_key.as_deref()).await {
        Ok(()) => ferret_domain::LlmProbeResult { ok: true, error: None },
        Err(e) => ferret_domain::LlmProbeResult { ok: false, error: Some(e.to_string()) },
    };
    Json(result).into_response()
}

/// Rebuild the live LLM layer and answer with the new effective settings.
async fn apply_llm(
    state: &AppState,
    over: Option<&crate::llm::LlmOverride>,
) -> Result<Response, ApiError> {
    match crate::llm::effective(&state.search.config.llm, over) {
        Ok(eff) => {
            let prompts =
                crate::llm::effective_prompts(crate::llm::load_prompts(&state.db).await.as_ref());
            let runtime = crate::llm::build_runtime(eff, prompts, Some(state.db.clone()));
            let settings = runtime.settings.clone();
            *state.llm.write().await = runtime;
            Ok(Json(settings).into_response())
        }
        Err(e) => Ok((StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response()),
    }
}

// ---- system prompt overrides ----

async fn get_prompts(State(state): State<AppState>) -> Response {
    let current =
        crate::llm::effective_prompts(crate::llm::load_prompts(&state.db).await.as_ref());
    Json(ferret_domain::PromptsResponse { current, default: crate::llm::default_prompts() })
        .into_response()
}

async fn put_prompts(
    State(state): State<AppState>,
    Json(mut set): Json<ferret_domain::PromptSet>,
) -> Result<Response, ApiError> {
    // fields left at (or reset to) the default are stored empty, so future
    // default improvements reach them
    let defaults = crate::llm::default_prompts();
    for (field, default) in [
        (&mut set.refine, &defaults.refine),
        (&mut set.interpret, &defaults.interpret),
        (&mut set.revise, &defaults.revise),
    ] {
        if field.trim() == default.trim() {
            field.clear();
        }
    }
    state
        .db
        .put_setting(
            crate::llm::PROMPTS_SETTINGS_KEY,
            &serde_json::to_string(&set).expect("prompt set serializes"),
        )
        .await?;
    rebuild_prompts(&state).await;
    Ok(get_prompts(State(state)).await)
}

async fn delete_prompts(State(state): State<AppState>) -> Result<Response, ApiError> {
    state.db.delete_setting(crate::llm::PROMPTS_SETTINGS_KEY).await?;
    rebuild_prompts(&state).await;
    Ok(get_prompts(State(state)).await)
}

async fn rebuild_prompts(state: &AppState) {
    let over = crate::llm::load_override(&state.db).await;
    if let Ok(eff) = crate::llm::effective(&state.search.config.llm, over.as_ref()) {
        let prompts =
            crate::llm::effective_prompts(crate::llm::load_prompts(&state.db).await.as_ref());
        *state.llm.write().await = crate::llm::build_runtime(eff, prompts, Some(state.db.clone()));
    }
}

#[derive(Deserialize)]
struct SearchRequest {
    queries: Vec<String>,
}

async fn start_search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Response, ApiError> {
    let sources = crate::search::one_shot_sources(&state.search, &req.queries);
    let id = crate::search::spawn_job(
        state.db.clone(),
        &state.search,
        state.notifier.clone(),
        state.jobs.clone(),
        sources,
    )
    .await;
    Ok(Json(serde_json::json!({ "id": id })).into_response())
}

async fn search_progress(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    match state.jobs.read().await.get(&id) {
        Some(job) => Ok(Json(job.clone()).into_response()),
        None => Err(ApiError(DbError::NotFound)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as FsPath;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use ferret_domain::Watch;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::db::Db;

    async fn app() -> Router {
        let db = Db::connect(FsPath::new(":memory:")).await.unwrap();
        router(AppState {
            db,
            families: Arc::new(Vec::new()),
            notifier: Arc::new(crate::notify::NoopNotifier),
            statuses: Arc::new(tokio::sync::RwLock::new(Default::default())),
            llm: Default::default(),
            search: Arc::new(crate::search::SearchContext {
                config: crate::config::Config::default(),
                families: Arc::new(vec![]),
                scrape: Default::default(),
            }),
            jobs: Arc::new(tokio::sync::RwLock::new(Default::default())),
            shared_queries: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        })
    }

    async fn body_json<T: serde::de::DeserializeOwned>(resp: Response) -> T {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn watch_lifecycle_over_http() {
        let app = app().await;

        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/watches")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": "4TB HDD", "min_capacity_gb": 4000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: Watch = body_json(resp).await;
        assert_eq!(created.name, "4TB HDD");

        let resp = app
            .clone()
            .oneshot(Request::get("/api/watches").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let listed: Vec<Watch> = body_json(resp).await;
        assert_eq!(listed.len(), 1);

        let resp = app
            .clone()
            .oneshot(
                Request::delete(format!("/api/watches/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .oneshot(
                Request::delete(format!("/api/watches/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deal_prices_endpoint() {
        let db = Db::connect(FsPath::new(":memory:")).await.unwrap();
        let deal = ferret_domain::Deal {
            id: uuid::Uuid::new_v4(),
            source_id: "src".into(),
            canonical_url: "https://ex.com/1".into(),
            title: "RTX 3080".into(),
            price_cents: 45_000,
            currency: "EUR".into(),
            family: None,
            models: vec![],
            capacity_gb: None,
            condition: None,
            stuffing_score: 0.0,
            flags: vec![],
            status: ferret_domain::DealStatus::Active,
            llm_verdict: None,
            llm_reason: None,
            category: None,
            specs: Default::default(),
            first_seen: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
        };
        let (stored, _) = db.upsert_deal(&deal).await.unwrap();
        let app = router(AppState {
            db,
            families: Arc::new(Vec::new()),
            notifier: Arc::new(crate::notify::NoopNotifier),
            statuses: Arc::new(tokio::sync::RwLock::new(Default::default())),
            llm: Default::default(),
            search: Arc::new(crate::search::SearchContext {
                config: crate::config::Config::default(),
                families: Arc::new(vec![]),
                scrape: Default::default(),
            }),
            jobs: Arc::new(tokio::sync::RwLock::new(Default::default())),
            shared_queries: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        });

        let resp = app
            .oneshot(
                Request::get(format!("/api/deals/{}/prices", stored.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let prices: Vec<ferret_domain::PricePoint> = body_json(resp).await;
        assert_eq!(prices.len(), 1);
        assert_eq!(prices[0].price_cents, 45_000);
    }

    #[tokio::test]
    async fn llm_settings_override_lifecycle() {
        let app = app().await;

        // default: config-disabled, no override
        let resp = app
            .clone()
            .oneshot(Request::get("/api/settings/llm").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let settings: ferret_domain::LlmSettings = body_json(resp).await;
        assert!(!settings.enabled && !settings.from_override);

        // enable via override, with a key
        let resp = app
            .clone()
            .oneshot(
                Request::put("/api/settings/llm")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"enabled": true, "base_url": "http://ollama:11434/v1",
                            "model": "qwen3", "api_key": "sk-test"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let settings: ferret_domain::LlmSettings = body_json(resp).await;
        assert!(settings.enabled && settings.from_override && settings.api_key_set);
        assert_eq!(settings.model, "qwen3");

        // update without api_key keeps the stored key
        let resp = app
            .clone()
            .oneshot(
                Request::put("/api/settings/llm")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"enabled": true, "base_url": "http://ollama:11434/v1", "model": "qwen3-big"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let settings: ferret_domain::LlmSettings = body_json(resp).await;
        assert!(settings.api_key_set, "omitted api_key keeps the stored one");
        assert_eq!(settings.model, "qwen3-big");

        // /api/status reflects the live layer
        let resp = app
            .clone()
            .oneshot(Request::get("/api/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status: ferret_domain::StatusResponse = body_json(resp).await;
        assert!(status.llm.enabled);
        assert_eq!(status.llm.model.as_deref(), Some("qwen3-big"));

        // back to TOML config
        let resp = app
            .clone()
            .oneshot(Request::delete("/api/settings/llm").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let settings: ferret_domain::LlmSettings = body_json(resp).await;
        assert!(!settings.enabled && !settings.from_override && !settings.api_key_set);
    }

    #[tokio::test]
    async fn prompt_override_lifecycle() {
        let app = app().await;

        let resp = app
            .clone()
            .oneshot(Request::get("/api/settings/prompts").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let prompts: ferret_domain::PromptsResponse = body_json(resp).await;
        assert_eq!(prompts.current, prompts.default, "starts at factory defaults");

        // override one prompt, leave the others default (empty)
        let resp = app
            .clone()
            .oneshot(
                Request::put("/api/settings/prompts")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"refine": "", "interpret": "my custom interpret prompt", "revise": ""}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let prompts: ferret_domain::PromptsResponse = body_json(resp).await;
        assert_eq!(prompts.current.interpret, "my custom interpret prompt");
        assert_eq!(prompts.current.refine, prompts.default.refine, "untouched = default");

        // reset
        let resp = app
            .clone()
            .oneshot(Request::delete("/api/settings/prompts").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let prompts: ferret_domain::PromptsResponse = body_json(resp).await;
        assert_eq!(prompts.current, prompts.default);
    }

    #[tokio::test]
    async fn deals_endpoint_returns_empty_list() {
        let resp = app()
            .await
            .oneshot(Request::get("/api/deals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let deals: Vec<ferret_domain::Deal> = body_json(resp).await;
        assert!(deals.is_empty());
    }
}
