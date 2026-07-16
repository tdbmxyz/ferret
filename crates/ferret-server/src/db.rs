//! SQLite persistence. All row ↔ domain-type mapping happens here and only
//! here — handlers and the pipeline never see SQL types.

use std::path::Path;

use chrono::{DateTime, Utc};
use ferret_domain::{Deal, Flag, Watch, WatchRequest};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("not found")]
    NotFound,
    #[error("invalid stored data: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn connect(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1) // single-user app; avoids SQLite write contention
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    // ---- watches ----

    pub async fn create_watch(&self, req: &WatchRequest) -> Result<Watch> {
        let watch = Watch {
            id: Uuid::new_v4(),
            name: req.name.clone(),
            family: req.family.clone(),
            model: req.model.clone(),
            min_capacity_gb: req.min_capacity_gb,
            max_price_cents: req.max_price_cents,
            active: req.active,
            created_at: Utc::now(),
        };
        sqlx::query(
            "INSERT INTO watches (id, name, family, model, min_capacity_gb, max_price_cents, active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(watch.id.to_string())
        .bind(&watch.name)
        .bind(&watch.family)
        .bind(&watch.model)
        .bind(watch.min_capacity_gb)
        .bind(watch.max_price_cents)
        .bind(watch.active)
        .bind(watch.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(watch)
    }

    pub async fn list_watches(&self) -> Result<Vec<Watch>> {
        let rows = sqlx::query("SELECT * FROM watches ORDER BY created_at")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_watch).collect()
    }

    pub async fn update_watch(&self, id: Uuid, req: &WatchRequest) -> Result<Watch> {
        let result = sqlx::query(
            "UPDATE watches SET name = ?, family = ?, model = ?, min_capacity_gb = ?,
             max_price_cents = ?, active = ? WHERE id = ?",
        )
        .bind(&req.name)
        .bind(&req.family)
        .bind(&req.model)
        .bind(req.min_capacity_gb)
        .bind(req.max_price_cents)
        .bind(req.active)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        let row = sqlx::query("SELECT * FROM watches WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
        row_to_watch(&row)
    }

    pub async fn delete_watch(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM watches WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    // ---- deals ----

    /// Insert the deal or, when (source_id, canonical_url) already exists,
    /// update its mutable fields keeping id and first_seen. Returns the
    /// stored deal and whether it was new.
    pub async fn upsert_deal(&self, deal: &Deal) -> Result<(Deal, bool)> {
        let existing = sqlx::query("SELECT * FROM deals WHERE source_id = ? AND canonical_url = ?")
            .bind(&deal.source_id)
            .bind(&deal.canonical_url)
            .fetch_optional(&self.pool)
            .await?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO deals (id, source_id, canonical_url, title, price_cents, currency,
                     family, models, capacity_gb, condition, stuffing_score, flags, first_seen, last_seen)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(deal.id.to_string())
                .bind(&deal.source_id)
                .bind(&deal.canonical_url)
                .bind(&deal.title)
                .bind(deal.price_cents)
                .bind(&deal.currency)
                .bind(&deal.family)
                .bind(serde_json::to_string(&deal.models).expect("serializing models"))
                .bind(deal.capacity_gb)
                .bind(&deal.condition)
                .bind(deal.stuffing_score)
                .bind(serde_json::to_string(&deal.flags).expect("serializing flags"))
                .bind(deal.first_seen.to_rfc3339())
                .bind(deal.last_seen.to_rfc3339())
                .execute(&self.pool)
                .await?;
                Ok((deal.clone(), true))
            }
            Some(row) => {
                let stored = row_to_deal(&row)?;
                sqlx::query(
                    "UPDATE deals SET title = ?, price_cents = ?, currency = ?, family = ?,
                     models = ?, capacity_gb = ?, condition = ?, stuffing_score = ?, flags = ?,
                     last_seen = ? WHERE id = ?",
                )
                .bind(&deal.title)
                .bind(deal.price_cents)
                .bind(&deal.currency)
                .bind(&deal.family)
                .bind(serde_json::to_string(&deal.models).expect("serializing models"))
                .bind(deal.capacity_gb)
                .bind(&deal.condition)
                .bind(deal.stuffing_score)
                .bind(serde_json::to_string(&deal.flags).expect("serializing flags"))
                .bind(deal.last_seen.to_rfc3339())
                .bind(stored.id.to_string())
                .execute(&self.pool)
                .await?;
                let merged = Deal {
                    id: stored.id,
                    first_seen: stored.first_seen,
                    ..deal.clone()
                };
                Ok((merged, false))
            }
        }
    }

    /// Deals, newest last_seen first; filtered to one watch's matches when
    /// `watch_id` is set.
    pub async fn list_deals(&self, watch_id: Option<Uuid>) -> Result<Vec<Deal>> {
        let rows = match watch_id {
            Some(w) => {
                sqlx::query(
                    "SELECT d.* FROM deals d
                     JOIN deal_matches m ON m.deal_id = d.id
                     WHERE m.watch_id = ? ORDER BY d.last_seen DESC",
                )
                .bind(w.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query("SELECT * FROM deals ORDER BY last_seen DESC")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(row_to_deal).collect()
    }

    // ---- matches ----

    /// Record that a deal matches a watch. Returns true when the match is
    /// new (i.e. a notification should fire), false when already known.
    pub async fn insert_match(&self, deal_id: Uuid, watch_id: Uuid) -> Result<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO deal_matches (deal_id, watch_id, matched_at, notified)
             VALUES (?, ?, ?, 0)",
        )
        .bind(deal_id.to_string())
        .bind(watch_id.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_notified(&self, deal_id: Uuid, watch_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE deal_matches SET notified = 1 WHERE deal_id = ? AND watch_id = ?")
            .bind(deal_id.to_string())
            .bind(watch_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- price history ----

    pub async fn record_price(&self, family: &str, model: &str, price_cents: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO price_history (family, model, price_cents, observed_at) VALUES (?, ?, ?, ?)",
        )
        .bind(family)
        .bind(model)
        .bind(price_cents)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Most recent `limit` observed prices for (family, model).
    pub async fn recent_prices(&self, family: &str, model: &str, limit: u32) -> Result<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT price_cents FROM price_history WHERE family = ? AND model = ?
             ORDER BY observed_at DESC LIMIT ?",
        )
        .bind(family)
        .bind(model)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get::<i64, _>("price_cents")).collect())
    }
}

// ---- row mapping ----

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| DbError::Corrupt(format!("bad uuid {s:?}: {e}")))
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| DbError::Corrupt(format!("bad timestamp {s:?}: {e}")))
}

fn row_to_watch(row: &sqlx::sqlite::SqliteRow) -> Result<Watch> {
    Ok(Watch {
        id: parse_uuid(&row.get::<String, _>("id"))?,
        name: row.get("name"),
        family: row.get("family"),
        model: row.get("model"),
        min_capacity_gb: row.get("min_capacity_gb"),
        max_price_cents: row.get("max_price_cents"),
        active: row.get("active"),
        created_at: parse_ts(&row.get::<String, _>("created_at"))?,
    })
}

fn row_to_deal(row: &sqlx::sqlite::SqliteRow) -> Result<Deal> {
    let models: Vec<String> = serde_json::from_str(&row.get::<String, _>("models"))
        .map_err(|e| DbError::Corrupt(format!("bad models json: {e}")))?;
    let flags: Vec<Flag> = serde_json::from_str(&row.get::<String, _>("flags"))
        .map_err(|e| DbError::Corrupt(format!("bad flags json: {e}")))?;
    Ok(Deal {
        id: parse_uuid(&row.get::<String, _>("id"))?,
        source_id: row.get("source_id"),
        canonical_url: row.get("canonical_url"),
        title: row.get("title"),
        price_cents: row.get("price_cents"),
        currency: row.get("currency"),
        family: row.get("family"),
        models,
        capacity_gb: row.get("capacity_gb"),
        condition: row.get("condition"),
        stuffing_score: row.get("stuffing_score"),
        flags,
        first_seen: parse_ts(&row.get::<String, _>("first_seen"))?,
        last_seen: parse_ts(&row.get::<String, _>("last_seen"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ferret_domain::{Flag, WatchRequest};

    async fn test_db() -> Db {
        Db::connect(Path::new(":memory:")).await.unwrap()
    }

    fn deal(url: &str, price: i64) -> Deal {
        Deal {
            id: Uuid::new_v4(),
            source_id: "src".into(),
            canonical_url: url.into(),
            title: "RTX 3080".into(),
            price_cents: price,
            currency: "EUR".into(),
            family: Some("nvidia-rtx".into()),
            models: vec!["3080".into()],
            capacity_gb: None,
            condition: None,
            stuffing_score: 0.0,
            flags: vec![Flag::PossibleStuffing],
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    #[tokio::test]
    async fn watch_crud_round_trip() {
        let db = test_db().await;
        let req = WatchRequest {
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
        };
        let created = db.create_watch(&req).await.unwrap();
        let listed = db.list_watches().await.unwrap();
        assert_eq!(listed, vec![created.clone()]);

        let mut update = req.clone();
        update.active = false;
        let updated = db.update_watch(created.id, &update).await.unwrap();
        assert!(!updated.active);

        db.delete_watch(created.id).await.unwrap();
        assert!(db.list_watches().await.unwrap().is_empty());
        assert!(matches!(
            db.delete_watch(created.id).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn upsert_deal_inserts_then_updates() {
        let db = test_db().await;
        let d = deal("https://ex.com/1", 45_000);
        let (stored, was_new) = db.upsert_deal(&d).await.unwrap();
        assert!(was_new);
        assert_eq!(stored.flags, vec![Flag::PossibleStuffing]);

        // same (source, canonical_url), new price → update, keep first_seen/id
        let mut d2 = deal("https://ex.com/1", 42_000);
        d2.id = Uuid::new_v4();
        let (updated, was_new) = db.upsert_deal(&d2).await.unwrap();
        assert!(!was_new);
        assert_eq!(updated.id, stored.id);
        assert_eq!(updated.price_cents, 42_000);
        assert_eq!(updated.first_seen, stored.first_seen);

        assert_eq!(db.list_deals(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn price_history_round_trip() {
        let db = test_db().await;
        for p in [40_000, 45_000, 50_000] {
            db.record_price("nvidia-rtx", "3080", p).await.unwrap();
        }
        let prices = db.recent_prices("nvidia-rtx", "3080", 50).await.unwrap();
        assert_eq!(prices.len(), 3);
        assert!(db.recent_prices("nvidia-rtx", "3090", 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deal_match_insert_is_idempotent() {
        let db = test_db().await;
        let w = db
            .create_watch(&WatchRequest {
                name: "w".into(),
                family: None,
                model: None,
                min_capacity_gb: None,
                max_price_cents: None,
                active: true,
            })
            .await
            .unwrap();
        let (d, _) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();

        assert!(db.insert_match(d.id, w.id).await.unwrap()); // new match
        assert!(!db.insert_match(d.id, w.id).await.unwrap()); // already known

        let deals = db.list_deals(Some(w.id)).await.unwrap();
        assert_eq!(deals.len(), 1);
    }
}
