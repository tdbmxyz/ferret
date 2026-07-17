//! `POST /api/interpret` — the brain of guided watch creation. Heuristics
//! answer instantly when the text matches a known category; otherwise the
//! local LLM maps it or drafts a proposed new category. Every failure is
//! fail-open down to via="none" (the UI then offers manual creation).

use ferret_domain::{
    Category, CategoryOrigin, CategoryStatus, Interpretation, SpecFilter, SpecKind, category,
};

use crate::llm::{LlmConstraint, LlmInterpret, LlmInterpretation, LlmProposalSpec};

fn to_filter(c: &LlmConstraint) -> Option<SpecFilter> {
    let key = c.key.clone();
    match c.op.as_str() {
        "min" => c.value.as_f64().map(|value| SpecFilter::Min { key, value }),
        "max" => c.value.as_f64().map(|value| SpecFilter::Max { key, value }),
        "eq" => c.value.as_str().map(|v| SpecFilter::Eq { key, value: v.to_string() }),
        _ => None,
    }
}

fn to_spec(s: &LlmProposalSpec) -> Option<ferret_domain::CategorySpec> {
    let kind = match s.kind.as_str() {
        "number" => SpecKind::Number,
        "enum" => SpecKind::Enum,
        "boolean" => SpecKind::Boolean,
        _ => return None,
    };
    Some(ferret_domain::CategorySpec {
        key: s.key.clone(),
        label: s.label.clone(),
        kind,
        unit: s.unit.clone(),
        allowed_values: s.allowed_values.clone(),
        extraction_hint: None,
    })
}

/// Derive the scheduled search queries for a text + optional category.
fn queries_for(text: &str, category: Option<&Category>) -> Vec<String> {
    let mut queries = vec![text.trim().to_lowercase()];
    if let Some(cat) = category
        && !queries.contains(&cat.label.to_lowercase())
    {
        queries.push(cat.label.to_lowercase());
    }
    queries
}

/// The full interpretation ladder. `interpreter` is None when the LLM pass
/// is disabled.
pub async fn interpret(
    text: &str,
    categories: &[Category],
    interpreter: Option<&dyn LlmInterpret>,
) -> Interpretation {
    let llm_active = interpreter.is_some();
    // 1. instant heuristic
    if let Some((cat, constraints)) = category::interpret_heuristic(text, categories) {
        return Interpretation {
            queries: queries_for(text, Some(cat)),
            category: Some(cat.clone()),
            constraints,
            proposal: None,
            via: "heuristic".into(),
            llm_active,
        };
    }
    // 2. LLM mapping / proposal (fail-open)
    if let Some(llm) = interpreter {
        match llm.interpret(text, categories).await {
            Ok(answer) => return from_llm(text, categories, answer),
            Err(e) => tracing::warn!(error = %e, "llm interpret failed — falling through"),
        }
    }
    // 3. nothing — the UI offers manual creation / cancel
    Interpretation {
        category: None,
        constraints: vec![],
        queries: queries_for(text, None),
        proposal: None,
        via: "none".into(),
        llm_active,
    }
}

fn from_llm(text: &str, categories: &[Category], answer: LlmInterpretation) -> Interpretation {
    let category = answer
        .category_slug
        .as_ref()
        .and_then(|slug| {
            categories
                .iter()
                .find(|c| &c.slug == slug && c.status == CategoryStatus::Active)
        })
        .cloned();
    let constraints = answer.constraints.iter().filter_map(to_filter).collect();
    let proposal = match (&category, answer.proposal) {
        (None, Some(draft)) => Some(Category {
            slug: draft.slug.clone(),
            label: draft.label.clone(),
            aliases: draft.aliases.clone(),
            origin: CategoryOrigin::Llm,
            status: CategoryStatus::Proposed,
            specs: draft.specs.iter().filter_map(to_spec).collect(),
            created_at: chrono::Utc::now(),
        }),
        _ => None,
    };
    Interpretation {
        queries: queries_for(text, category.as_ref()),
        via: if category.is_some() || proposal.is_some() { "llm" } else { "none" }.into(),
        category,
        constraints,
        proposal,
        llm_active: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferret_domain::CategorySpec;

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

    struct MockLlm(anyhow::Result<LlmInterpretation>);

    #[async_trait::async_trait]
    impl LlmInterpret for MockLlm {
        async fn interpret(
            &self,
            _text: &str,
            _categories: &[Category],
        ) -> anyhow::Result<LlmInterpretation> {
            match &self.0 {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    #[tokio::test]
    async fn heuristic_answers_without_llm() {
        let out = interpret("4to disque dur", &[hdd()], None).await;
        assert_eq!(out.via, "heuristic");
        assert_eq!(out.category.as_ref().unwrap().slug, "hdd");
        assert_eq!(
            out.constraints,
            vec![SpecFilter::Min { key: "capacity".into(), value: 4000.0 }]
        );
        assert!(out.queries.contains(&"4to disque dur".to_string()));
    }

    #[tokio::test]
    async fn llm_maps_unknown_phrasing_to_known_category() {
        let llm = MockLlm(Ok(LlmInterpretation {
            category_slug: Some("hdd".into()),
            constraints: vec![LlmConstraint {
                op: "min".into(),
                key: "capacity".into(),
                value: serde_json::json!(8000),
            }],
            proposal: None,
        }));
        let out = interpret("spinning rust for my nas box", &[hdd()], Some(&llm)).await;
        assert_eq!(out.via, "llm");
        assert_eq!(out.category.as_ref().unwrap().slug, "hdd");
        assert_eq!(
            out.constraints,
            vec![SpecFilter::Min { key: "capacity".into(), value: 8000.0 }]
        );
    }

    #[tokio::test]
    async fn llm_drafts_proposal_for_unknown_product() {
        let llm = MockLlm(Ok(LlmInterpretation {
            category_slug: None,
            constraints: vec![],
            proposal: Some(crate::llm::LlmProposal {
                slug: "espresso-machine".into(),
                label: "Espresso machine".into(),
                aliases: vec!["espresso".into(), "machine à café".into()],
                specs: vec![LlmProposalSpec {
                    key: "pressure".into(),
                    label: "Pressure (bar)".into(),
                    kind: "number".into(),
                    unit: Some("bar".into()),
                    allowed_values: vec![],
                }],
            }),
        }));
        let out = interpret("machine à café DeLonghi", &[hdd()], Some(&llm)).await;
        assert_eq!(out.via, "llm");
        assert!(out.category.is_none());
        let proposal = out.proposal.unwrap();
        assert_eq!(proposal.status, CategoryStatus::Proposed);
        assert_eq!(proposal.origin, CategoryOrigin::Llm);
        assert_eq!(proposal.specs.len(), 1);
    }

    #[tokio::test]
    async fn llm_failure_is_fail_open_to_none() {
        let llm = MockLlm(Err(anyhow::anyhow!("backend down")));
        let out = interpret("machine à café", &[hdd()], Some(&llm)).await;
        assert_eq!(out.via, "none");
        assert!(out.category.is_none() && out.proposal.is_none());
        assert_eq!(out.queries, vec!["machine à café".to_string()]);
    }
}
