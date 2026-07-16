//! Regex-driven attribute extraction from listing titles: capacity and
//! condition. Model extraction is family-table-driven — see `family`.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedAttributes {
    /// Decimal gigabytes (1 TB = 1000 GB, 1 To = 1000 Go).
    pub capacity_gb: Option<i64>,
    /// "new" | "used" | "refurbished".
    pub condition: Option<String>,
}

static CAPACITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 4TB / 4 To / 16GB / 16 Go / 1.5TB / 1,5 To. The unit is required and
    // word-bounded on the right; the number is deliberately NOT left-bounded
    // so kit notation like "2x8 Go" still matches ("x8" has no \b before 8).
    Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*(tb|to|gb|go)\b").unwrap()
});

/// (keyword, canonical condition) — first match in table order wins.
const CONDITIONS: &[(&str, &str)] = &[
    ("reconditionné", "refurbished"),
    ("reconditionne", "refurbished"),
    ("refurbished", "refurbished"),
    ("refurb", "refurbished"),
    ("occasion", "used"),
    ("used", "used"),
    ("neuf", "new"),
    ("neuve", "new"),
    ("brand new", "new"),
    (" new", "new"),
];

/// Extract capacity and condition from a (cleaned) listing title.
pub fn extract(title: &str) -> ExtractedAttributes {
    let capacity_gb = CAPACITY_RE.captures(title).and_then(|caps| {
        let n: f64 = caps[1].replace(',', ".").parse().ok()?;
        let gb = match caps[2].to_lowercase().as_str() {
            "tb" | "to" => n * 1000.0,
            _ => n,
        };
        Some(gb.round() as i64)
    });

    let lower = title.to_lowercase();
    let condition = CONDITIONS
        .iter()
        .find(|(kw, _)| lower.contains(kw))
        .map(|(_, canon)| canon.to_string());

    ExtractedAttributes { capacity_gb, condition }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(title: &str) -> Option<i64> {
        extract(title).capacity_gb
    }

    #[test]
    fn extracts_tb_capacity() {
        assert_eq!(cap("Seagate IronWolf 4TB NAS"), Some(4000));
        assert_eq!(cap("WD Red 4 To 3.5\""), Some(4000));
    }

    #[test]
    fn extracts_gb_capacity() {
        assert_eq!(cap("Crucial 16GB DDR4 3200"), Some(16));
        assert_eq!(cap("Kit 2x8 Go DDR4"), Some(8)); // first capacity token wins
    }

    #[test]
    fn extracts_fractional_tb() {
        assert_eq!(cap("SSD 1.5TB"), Some(1500));
        assert_eq!(cap("SSD 1,5 To"), Some(1500));
    }

    #[test]
    fn no_capacity_in_gpu_title() {
        // "3080" must not parse as capacity
        assert_eq!(cap("RTX 3080 Founders Edition"), None);
    }

    #[test]
    fn detects_condition() {
        assert_eq!(extract("RTX 3080 occasion").condition.as_deref(), Some("used"));
        assert_eq!(extract("RTX 3080 used, works").condition.as_deref(), Some("used"));
        assert_eq!(extract("Disque neuf sous blister").condition.as_deref(), Some("new"));
        assert_eq!(
            extract("Serveur reconditionné DL380").condition.as_deref(),
            Some("refurbished")
        );
        assert_eq!(
            extract("HP refurb server").condition.as_deref(),
            Some("refurbished")
        );
        assert_eq!(extract("RTX 3080").condition, None);
    }
}
