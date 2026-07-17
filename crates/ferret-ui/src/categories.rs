//! Categories management: review LLM-proposed categories (approve/reject)
//! and inspect the active ones.

use ferret_client::FerretClient;
use ferret_domain::{Category, CategoryStatus, SpecKind};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::DataVersion;

#[component]
pub fn CategoriesView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let categories = LocalResource::new({
        let client = client.clone();
        move || {
            version.0.track();
            let client = client.clone();
            async move { client.categories().await.unwrap_or_default() }
        }
    });

    view! {
        <section>
            <p class="muted">
                "Categories drive interpretation and filters. Proposed ones were drafted \
                 by the LLM and only start categorizing deals once approved."
            </p>
            <ul class="watches">
                {move || {
                    categories
                        .get()
                        .unwrap_or_default()
                        .into_iter()
                        .map(category_row)
                        .collect_view()
                }}
            </ul>
        </section>
    }
}

fn category_row(category: Category) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let proposed = category.status == CategoryStatus::Proposed;

    let specs: Vec<String> = category
        .specs
        .iter()
        .map(|s| {
            let detail = match s.kind {
                SpecKind::Number => s.unit.clone().unwrap_or_else(|| "number".into()),
                SpecKind::Enum => format!("{} values", s.allowed_values.len()),
                SpecKind::Boolean => "yes/no".into(),
            };
            format!("{} ({detail})", s.label)
        })
        .collect();

    let approve = {
        let client = client.clone();
        let category = category.clone();
        move |_| {
            let mut approved = category.clone();
            approved.status = CategoryStatus::Active;
            let client = client.clone();
            spawn_local(async move {
                let _ = client.upsert_category(&approved).await;
                version.0.update(|v| *v += 1);
            });
        }
    };
    let remove = {
        let client = client.clone();
        let slug = category.slug.clone();
        move |_| {
            let client = client.clone();
            let slug = slug.clone();
            spawn_local(async move {
                let _ = client.delete_category(&slug).await;
                version.0.update(|v| *v += 1);
            });
        }
    };

    view! {
        <li class="watch" class:inactive=proposed>
            <div class="watch-main">
                <span class="watch-name">
                    {category.label.clone()}
                    " "
                    {proposed.then(|| view! { <span class="badge warn">"proposed"</span> })}
                </span>
                <span class="muted">
                    {format!(
                        "aliases: {} · filters: {}",
                        if category.aliases.is_empty() { "—".into() } else { category.aliases.join(", ") },
                        if specs.is_empty() { "—".into() } else { specs.join(" · ") },
                    )}
                </span>
            </div>
            <div class="watch-actions">
                {proposed.then(|| view! { <button on:click=approve.clone()>"approve"</button> })}
                <button class="danger" on:click=remove>"delete"</button>
            </div>
        </li>
    }
}
