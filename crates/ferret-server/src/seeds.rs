//! Curated category seeds: the starting point of the guided-watch system.
//! Config `[[families]]` fold in as categories carrying a `model` enum
//! spec, so one system drives categorization, stuffing and filters.
//! Seeding never overwrites user edits (see `Db::seed_categories`).

use chrono::Utc;
use ferret_domain::{
    Category, CategoryOrigin, CategorySpec, CategoryStatus, ProductFamily, SpecKind,
};

fn spec_number(key: &str, label: &str, unit: &str) -> CategorySpec {
    CategorySpec {
        key: key.into(),
        label: label.into(),
        kind: SpecKind::Number,
        unit: Some(unit.into()),
        allowed_values: vec![],
        extraction_hint: None,
    }
}

fn category(slug: &str, label: &str, aliases: &[&str], specs: Vec<CategorySpec>) -> Category {
    Category {
        slug: slug.into(),
        label: label.into(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        origin: CategoryOrigin::Curated,
        status: CategoryStatus::Active,
        specs,
        created_at: Utc::now(),
    }
}

pub fn builtin(families: &[ProductFamily]) -> Vec<Category> {
    let mut seeds = vec![
        category(
            "hdd",
            "Hard drive",
            &["hdd", "disque dur", "hard drive", "ironwolf", "wd red", "barracuda", "exos", "nas"],
            vec![spec_number("capacity", "Capacity", "GB"), spec_number("rpm", "Rotation speed", "rpm")],
        ),
        category(
            "ssd",
            "SSD",
            &["ssd", "nvme", "m.2"],
            vec![spec_number("capacity", "Capacity", "GB")],
        ),
        category(
            "ram",
            "RAM",
            &["ram", "ddr3", "ddr4", "ddr5", "dimm", "sodimm", "mémoire vive"],
            vec![spec_number("capacity", "Capacity", "GB")],
        ),
    ];
    // each family table becomes a category with its model enum; aliases
    // come from the family's context words (categorize requires an alias
    // hit — bare model numbers are too ambiguous)
    for family in families {
        let aliases: Vec<&str> =
            family.context.iter().map(String::as_str).chain([family.name.as_str()]).collect();
        seeds.push(category(
            &family.name,
            &family.name,
            &aliases,
            vec![CategorySpec {
                key: "model".into(),
                label: "Model".into(),
                kind: SpecKind::Enum,
                unit: None,
                allowed_values: family.models.clone(),
                extraction_hint: None,
            }],
        ));
    }
    seeds
}
