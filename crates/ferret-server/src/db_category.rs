//! Category storage: categories + their spec definitions, loaded whole
//! (the table is small and read per tick / per interpret call).

use chrono::Utc;
use ferret_domain::{Category, CategoryOrigin, CategorySpec, CategoryStatus, SpecKind};
use sqlx::Row;

use crate::db::{Db, DbError, Result};

fn kind_to_str(kind: SpecKind) -> &'static str {
    match kind {
        SpecKind::Number => "number",
        SpecKind::Enum => "enum",
        SpecKind::Boolean => "boolean",
    }
}

fn kind_from_str(s: &str) -> Result<SpecKind> {
    match s {
        "number" => Ok(SpecKind::Number),
        "enum" => Ok(SpecKind::Enum),
        "boolean" => Ok(SpecKind::Boolean),
        other => Err(DbError::Corrupt(format!("bad spec kind {other:?}"))),
    }
}

impl Db {
    /// Every category (specs included), proposed ones last.
    pub async fn list_categories(&self) -> Result<Vec<Category>> {
        let rows = sqlx::query("SELECT * FROM categories ORDER BY status, slug")
            .fetch_all(self.pool())
            .await?;
        let mut categories = Vec::with_capacity(rows.len());
        for row in &rows {
            let slug: String = row.get("slug");
            let spec_rows = sqlx::query(
                "SELECT * FROM category_specs WHERE category_slug = ? ORDER BY position, key",
            )
            .bind(&slug)
            .fetch_all(self.pool())
            .await?;
            let specs = spec_rows
                .iter()
                .map(|s| {
                    Ok(CategorySpec {
                        key: s.get("key"),
                        label: s.get("label"),
                        kind: kind_from_str(&s.get::<String, _>("kind"))?,
                        unit: s.get("unit"),
                        allowed_values: serde_json::from_str(
                            &s.get::<String, _>("allowed_values"),
                        )
                        .map_err(|e| DbError::Corrupt(format!("bad allowed_values: {e}")))?,
                        extraction_hint: s.get("extraction_hint"),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            categories.push(Category {
                slug,
                label: row.get("label"),
                aliases: serde_json::from_str(&row.get::<String, _>("aliases"))
                    .map_err(|e| DbError::Corrupt(format!("bad aliases: {e}")))?,
                origin: match row.get::<String, _>("origin").as_str() {
                    "curated" => CategoryOrigin::Curated,
                    _ => CategoryOrigin::Llm,
                },
                status: match row.get::<String, _>("status").as_str() {
                    "proposed" => CategoryStatus::Proposed,
                    _ => CategoryStatus::Active,
                },
                specs,
                created_at: crate::db::parse_ts(&row.get::<String, _>("created_at"))?,
            });
        }
        Ok(categories)
    }

    /// Insert or fully replace a category and its specs.
    pub async fn upsert_category(&self, category: &Category) -> Result<()> {
        sqlx::query(
            "INSERT INTO categories (slug, label, aliases, origin, status, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(slug) DO UPDATE SET
               label=excluded.label, aliases=excluded.aliases,
               origin=excluded.origin, status=excluded.status",
        )
        .bind(&category.slug)
        .bind(&category.label)
        .bind(serde_json::to_string(&category.aliases).expect("aliases serialize"))
        .bind(match category.origin {
            CategoryOrigin::Curated => "curated",
            CategoryOrigin::Llm => "llm",
        })
        .bind(match category.status {
            CategoryStatus::Active => "active",
            CategoryStatus::Proposed => "proposed",
        })
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool())
        .await?;
        sqlx::query("DELETE FROM category_specs WHERE category_slug = ?")
            .bind(&category.slug)
            .execute(self.pool())
            .await?;
        for (position, spec) in category.specs.iter().enumerate() {
            sqlx::query(
                "INSERT INTO category_specs
                 (category_slug, key, label, kind, unit, allowed_values, extraction_hint, position)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&category.slug)
            .bind(&spec.key)
            .bind(&spec.label)
            .bind(kind_to_str(spec.kind))
            .bind(&spec.unit)
            .bind(serde_json::to_string(&spec.allowed_values).expect("values serialize"))
            .bind(&spec.extraction_hint)
            .bind(position as i64)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    pub async fn delete_category(&self, slug: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM categories WHERE slug = ?")
            .bind(slug)
            .execute(self.pool())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Insert curated categories that don't exist yet — never overwrites,
    /// so user edits survive restarts.
    pub async fn seed_categories(&self, seeds: &[Category]) -> Result<()> {
        for seed in seeds {
            let exists = sqlx::query("SELECT 1 FROM categories WHERE slug = ?")
                .bind(&seed.slug)
                .fetch_optional(self.pool())
                .await?
                .is_some();
            if !exists {
                self.upsert_category(seed).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn hdd() -> Category {
        Category {
            slug: "hdd".into(),
            label: "Hard drive".into(),
            aliases: vec!["hdd".into(), "disque dur".into()],
            origin: CategoryOrigin::Curated,
            status: CategoryStatus::Active,
            specs: vec![CategorySpec {
                key: "capacity".into(),
                label: "Capacity (GB)".into(),
                kind: SpecKind::Number,
                unit: Some("GB".into()),
                allowed_values: vec![],
                extraction_hint: None,
            }],
            created_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn category_round_trip_and_seed_semantics() {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        db.upsert_category(&hdd()).await.unwrap();

        let listed = db.list_categories().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].slug, "hdd");
        assert_eq!(listed[0].specs.len(), 1);
        assert_eq!(listed[0].specs[0].kind, SpecKind::Number);

        // user edit
        let mut edited = hdd();
        edited.label = "Spinning rust".into();
        db.upsert_category(&edited).await.unwrap();

        // seeding must NOT clobber the edit
        db.seed_categories(&[hdd()]).await.unwrap();
        assert_eq!(db.list_categories().await.unwrap()[0].label, "Spinning rust");

        db.delete_category("hdd").await.unwrap();
        assert!(db.list_categories().await.unwrap().is_empty());
        assert!(matches!(db.delete_category("hdd").await, Err(DbError::NotFound)));
    }
}
