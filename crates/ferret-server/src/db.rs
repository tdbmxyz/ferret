//! SQLite persistence. All row ↔ domain-type mapping happens here and only
//! here — handlers and the pipeline never see SQL types.

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use ferret_domain::{Deal, DealStatus, Flag, LlmVerdict, PricePoint, Watch, WatchRequest};
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

/// What `upsert_deal` did, and what the pipeline should react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    New,
    /// Same listing seen again with a different price.
    PriceChanged { old_price_cents: i64 },
    Unchanged,
}

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
            min_price_cents: req.min_price_cents,
            max_price_cents: req.max_price_cents,
            active: req.active,
            created_at: Utc::now(),
        };
        sqlx::query(
            "INSERT INTO watches (id, name, family, model, min_capacity_gb, min_price_cents,
             max_price_cents, active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(watch.id.to_string())
        .bind(&watch.name)
        .bind(&watch.family)
        .bind(&watch.model)
        .bind(watch.min_capacity_gb)
        .bind(watch.min_price_cents)
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
             min_price_cents = ?, max_price_cents = ?, active = ? WHERE id = ?",
        )
        .bind(&req.name)
        .bind(&req.family)
        .bind(&req.model)
        .bind(req.min_capacity_gb)
        .bind(req.min_price_cents)
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
    /// update its mutable fields keeping id and first_seen (and reviving a
    /// `gone` deal to `active`). Every new deal and every price change adds
    /// a `deal_prices` row (one per day, latest wins). Returns the stored
    /// deal and what happened.
    pub async fn upsert_deal(&self, deal: &Deal) -> Result<(Deal, UpsertOutcome)> {
        let existing = sqlx::query("SELECT * FROM deals WHERE source_id = ? AND canonical_url = ?")
            .bind(&deal.source_id)
            .bind(&deal.canonical_url)
            .fetch_optional(&self.pool)
            .await?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO deals (id, source_id, canonical_url, title, price_cents, currency,
                     family, models, capacity_gb, condition, stuffing_score, flags, status,
                     llm_verdict, llm_reason, first_seen, last_seen)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
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
                .bind(deal.llm_verdict.map(verdict_to_str))
                .bind(&deal.llm_reason)
                .bind(deal.first_seen.to_rfc3339())
                .bind(deal.last_seen.to_rfc3339())
                .execute(&self.pool)
                .await?;
                self.record_deal_price(deal.id, deal.price_cents).await?;
                let stored = Deal { status: DealStatus::Active, ..deal.clone() };
                Ok((stored, UpsertOutcome::New))
            }
            Some(row) => {
                let stored = row_to_deal(&row)?;
                sqlx::query(
                    // llm fields COALESCE: a re-scraped deal (in-memory llm
                    // fields None) must not wipe a stored refinement
                    "UPDATE deals SET title = ?, price_cents = ?, currency = ?, family = ?,
                     models = ?, capacity_gb = ?, condition = ?, stuffing_score = ?, flags = ?,
                     status = 'active',
                     llm_verdict = COALESCE(?, llm_verdict),
                     llm_reason = COALESCE(?, llm_reason),
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
                .bind(deal.llm_verdict.map(verdict_to_str))
                .bind(&deal.llm_reason)
                .bind(deal.last_seen.to_rfc3339())
                .bind(stored.id.to_string())
                .execute(&self.pool)
                .await?;
                let outcome = if stored.price_cents != deal.price_cents {
                    self.record_deal_price(stored.id, deal.price_cents).await?;
                    UpsertOutcome::PriceChanged { old_price_cents: stored.price_cents }
                } else {
                    UpsertOutcome::Unchanged
                };
                let merged = Deal {
                    id: stored.id,
                    first_seen: stored.first_seen,
                    status: DealStatus::Active,
                    llm_verdict: deal.llm_verdict.or(stored.llm_verdict),
                    llm_reason: deal.llm_reason.clone().or(stored.llm_reason),
                    ..deal.clone()
                };
                Ok((merged, outcome))
            }
        }
    }

    /// Mark every active deal of `source_id` whose canonical URL is not in
    /// `seen` as gone. Call only after a SUCCESSFUL full fetch — a failed
    /// scrape must not make the whole source "disappear". Returns how many
    /// deals went gone.
    pub async fn mark_gone(&self, source_id: &str, seen: &HashSet<String>) -> Result<u64> {
        let rows = sqlx::query(
            "SELECT id, canonical_url FROM deals WHERE source_id = ? AND status = 'active'",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;
        let mut gone = 0;
        for row in rows {
            let url: String = row.get("canonical_url");
            if !seen.contains(&url) {
                sqlx::query("UPDATE deals SET status = 'gone' WHERE id = ?")
                    .bind(row.get::<String, _>("id"))
                    .execute(&self.pool)
                    .await?;
                gone += 1;
            }
        }
        Ok(gone)
    }

    /// Price history of one deal, oldest first.
    pub async fn deal_prices(&self, deal_id: Uuid) -> Result<Vec<PricePoint>> {
        let rows = sqlx::query(
            "SELECT day, price_cents FROM deal_prices WHERE deal_id = ? ORDER BY day",
        )
        .bind(deal_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| PricePoint { day: r.get("day"), price_cents: r.get("price_cents") })
            .collect())
    }

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

    async fn record_deal_price(&self, deal_id: Uuid, price_cents: i64) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO deal_prices (deal_id, day, price_cents) VALUES (?, ?, ?)")
            .bind(deal_id.to_string())
            .bind(Utc::now().format("%Y-%m-%d").to_string())
            .bind(price_cents)
            .execute(&self.pool)
            .await?;
        Ok(())
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

    /// Record that a notification fired for this match, at this price —
    /// the reference against which later price drops are measured.
    pub async fn mark_notified(&self, deal_id: Uuid, watch_id: Uuid, price_cents: i64) -> Result<()> {
        sqlx::query(
            "UPDATE deal_matches SET notified = 1, notified_price_cents = ?
             WHERE deal_id = ? AND watch_id = ?",
        )
        .bind(price_cents)
        .bind(deal_id.to_string())
        .bind(watch_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Price at the last notification for this match; None when the match
    /// was never notified.
    pub async fn notified_price(&self, deal_id: Uuid, watch_id: Uuid) -> Result<Option<i64>> {
        let row = sqlx::query(
            "SELECT notified_price_cents FROM deal_matches
             WHERE deal_id = ? AND watch_id = ? AND notified = 1",
        )
        .bind(deal_id.to_string())
        .bind(watch_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.get::<Option<i64>, _>("notified_price_cents")))
    }

    /// Current match count per watch (watches with none are absent).
    pub async fn count_matches(&self) -> Result<std::collections::HashMap<Uuid, i64>> {
        let rows = sqlx::query(
            "SELECT watch_id, COUNT(*) AS n FROM deal_matches GROUP BY watch_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((parse_uuid(&r.get::<String, _>("watch_id"))?, r.get::<i64, _>("n"))))
            .collect()
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

fn verdict_to_str(verdict: LlmVerdict) -> &'static str {
    match verdict {
        LlmVerdict::Genuine => "genuine",
        LlmVerdict::StuffedTitle => "stuffed-title",
        LlmVerdict::Scam => "scam",
    }
}

fn verdict_from_str(s: &str) -> Result<LlmVerdict> {
    match s {
        "genuine" => Ok(LlmVerdict::Genuine),
        "stuffed-title" => Ok(LlmVerdict::StuffedTitle),
        "scam" => Ok(LlmVerdict::Scam),
        other => Err(DbError::Corrupt(format!("bad llm verdict {other:?}"))),
    }
}

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
        min_price_cents: row.get("min_price_cents"),
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
    let status = match row.get::<String, _>("status").as_str() {
        "active" => DealStatus::Active,
        "gone" => DealStatus::Gone,
        other => return Err(DbError::Corrupt(format!("bad deal status {other:?}"))),
    };
    let llm_verdict = row
        .get::<Option<String>, _>("llm_verdict")
        .map(|s| verdict_from_str(&s))
        .transpose()?;
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
        status,
        llm_verdict,
        llm_reason: row.get("llm_reason"),
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
            status: DealStatus::Active,
            llm_verdict: None,
            llm_reason: None,
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
            min_price_cents: Some(10_000),
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
        let (stored, outcome) = db.upsert_deal(&d).await.unwrap();
        assert_eq!(outcome, UpsertOutcome::New);
        assert_eq!(stored.flags, vec![Flag::PossibleStuffing]);

        // same (source, canonical_url), new price → update, keep first_seen/id
        let mut d2 = deal("https://ex.com/1", 42_000);
        d2.id = Uuid::new_v4();
        let (updated, outcome) = db.upsert_deal(&d2).await.unwrap();
        assert_eq!(outcome, UpsertOutcome::PriceChanged { old_price_cents: 45_000 });
        assert_eq!(updated.id, stored.id);
        assert_eq!(updated.price_cents, 42_000);
        assert_eq!(updated.first_seen, stored.first_seen);

        // same price again → unchanged
        let mut d3 = deal("https://ex.com/1", 42_000);
        d3.id = Uuid::new_v4();
        let (_, outcome) = db.upsert_deal(&d3).await.unwrap();
        assert_eq!(outcome, UpsertOutcome::Unchanged);

        assert_eq!(db.list_deals(None).await.unwrap().len(), 1);
        // price history: insert day + change day collapse to one row per
        // day — both writes happened today, latest wins
        let prices = db.deal_prices(stored.id).await.unwrap();
        assert_eq!(prices.len(), 1);
        assert_eq!(prices[0].price_cents, 42_000);
    }

    #[tokio::test]
    async fn mark_gone_and_revive() {
        let db = test_db().await;
        let (d1, _) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();
        let (d2, _) = db.upsert_deal(&deal("https://ex.com/2", 46_000)).await.unwrap();

        // a tick that only saw /2 → /1 goes gone
        let seen: HashSet<String> = ["https://ex.com/2".to_string()].into();
        assert_eq!(db.mark_gone("src", &seen).await.unwrap(), 1);
        let deals = db.list_deals(None).await.unwrap();
        let find = |id| deals.iter().find(|d| d.id == id).unwrap();
        assert_eq!(find(d1.id).status, DealStatus::Gone);
        assert_eq!(find(d2.id).status, DealStatus::Active);

        // /1 reappears → revived by upsert
        let (revived, outcome) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();
        assert_eq!(outcome, UpsertOutcome::Unchanged);
        assert_eq!(revived.status, DealStatus::Active);
        assert_eq!(revived.id, d1.id);

        // other sources are untouched
        assert_eq!(db.mark_gone("other-src", &HashSet::new()).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn refinement_fills_gaps_but_never_overwrites() {
        let db = test_db().await;
        let mut d = deal("https://ex.com/1", 45_000);
        d.condition = Some("used".into()); // heuristic already extracted this
        d.capacity_gb = None;
        let (stored, _) = db.upsert_deal(&d).await.unwrap();

        let refined = db
            .apply_refinement(
                stored.id,
                LlmVerdict::StuffedTitle,
                "title enumerates five sibling GPUs",
                Some(10),
                Some("new"), // must NOT replace the heuristic "used"
            )
            .await
            .unwrap();

        assert_eq!(refined.llm_verdict, Some(LlmVerdict::StuffedTitle));
        assert_eq!(refined.llm_reason.as_deref(), Some("title enumerates five sibling GPUs"));
        assert_eq!(refined.capacity_gb, Some(10), "empty capacity filled");
        assert_eq!(refined.condition.as_deref(), Some("used"), "heuristic kept");
        // heuristic flags untouched
        assert_eq!(refined.flags, vec![Flag::PossibleStuffing]);
    }

    #[tokio::test]
    async fn rescrape_keeps_llm_fields() {
        let db = test_db().await;
        let (stored, _) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();
        db.apply_refinement(stored.id, LlmVerdict::Genuine, "looks fine", None, None)
            .await
            .unwrap();

        // re-scrape with fresh (llm-empty) in-memory deal
        let mut d2 = deal("https://ex.com/1", 42_000);
        d2.id = Uuid::new_v4();
        let (updated, _) = db.upsert_deal(&d2).await.unwrap();
        assert_eq!(updated.llm_verdict, Some(LlmVerdict::Genuine));
        assert_eq!(updated.llm_reason.as_deref(), Some("looks fine"));

        // and it round-trips through list_deals
        let listed = db.list_deals(None).await.unwrap();
        assert_eq!(listed[0].llm_verdict, Some(LlmVerdict::Genuine));
    }

    #[tokio::test]
    async fn notified_price_round_trip() {
        let db = test_db().await;
        let w = db
            .create_watch(&WatchRequest {
                name: "w".into(),
                family: None,
                model: None,
                min_capacity_gb: None,
                min_price_cents: None,
                max_price_cents: None,
                active: true,
            })
            .await
            .unwrap();
        let (d, _) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();
        db.insert_match(d.id, w.id).await.unwrap();

        assert_eq!(db.notified_price(d.id, w.id).await.unwrap(), None);
        db.mark_notified(d.id, w.id, 45_000).await.unwrap();
        assert_eq!(db.notified_price(d.id, w.id).await.unwrap(), Some(45_000));
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
                min_price_cents: None,
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
