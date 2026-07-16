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
        .route("/api/watches", get(list_watches).post(create_watch))
        .route("/api/watches/{id}", axum::routing::put(update_watch).delete(delete_watch))
        .route("/api/deals", get(list_deals))
        .route("/api/deals/{id}/prices", get(deal_prices))
        .route("/api/families", get(list_families))
        .with_state(state)
}

async fn health() -> Response {
    Json(ferret_domain::HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
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

async fn list_watches(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_watches().await?).into_response())
}

async fn create_watch(
    State(state): State<AppState>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    let watch = state.db.create_watch(&req).await?;
    Ok((StatusCode::CREATED, Json(watch)).into_response())
}

async fn update_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    Ok(Json(state.db.update_watch(id, &req).await?).into_response())
}

async fn delete_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    state.db.delete_watch(id).await?;
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
        router(AppState { db, families: Arc::new(Vec::new()) })
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
            first_seen: chrono::Utc::now(),
            last_seen: chrono::Utc::now(),
        };
        let (stored, _) = db.upsert_deal(&deal).await.unwrap();
        let app = router(AppState { db, families: Arc::new(Vec::new()) });

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
