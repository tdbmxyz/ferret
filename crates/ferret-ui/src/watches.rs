//! Watches view: guided creation on top, management list below
//! (edit / pause / delete, live match counts).

use ferret_client::FerretClient;
use ferret_domain::{Watch, WatchRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::{DataVersion, format_price};

#[component]
pub fn WatchesView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();

    let watches = LocalResource::new({
        let client = client.clone();
        move || {
            version.0.track();
            let client = client.clone();
            async move { client.watches().await }
        }
    });

    view! {
        <section>
            <crate::guided::GuidedCreate/>
            {move || match watches.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => {
                    view! { <p class="error">{format!("server unreachable: {e}")}</p> }.into_any()
                }
                Some(Ok(watches)) if watches.is_empty() => {
                    view! { <p class="muted">"No watches yet — describe a product above."</p> }
                        .into_any()
                }
                Some(Ok(watches)) => view! {
                    <ul class="watches">
                        {watches.into_iter().map(watch_row).collect_view()}
                    </ul>
                }
                .into_any(),
            }}
        </section>
    }
}

fn request_from(watch: &Watch, active: bool) -> WatchRequest {
    WatchRequest {
        name: watch.name.clone(),
        family: watch.family.clone(),
        model: watch.model.clone(),
        min_capacity_gb: watch.min_capacity_gb,
        min_price_cents: watch.min_price_cents,
        max_price_cents: watch.max_price_cents,
        category: watch.category.clone(),
        spec_filters: watch.spec_filters.clone(),
        queries: watch.queries.clone(),
        active,
    }
}

fn watch_row(watch: Watch) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let edit: crate::guided::EditRequest = expect_context();
    let status: crate::status::StatusResource = expect_context();
    let watch_id = watch.id;
    let match_count = move || {
        status
            .0
            .get()
            .flatten()
            .and_then(|s| s.watch_matches.get(&watch_id).copied())
            .unwrap_or(0)
    };

    let mut filters: Vec<String> = Vec::new();
    if let Some(c) = &watch.category {
        filters.push(c.clone());
    }
    for f in &watch.spec_filters {
        filters.push(match f {
            ferret_domain::SpecFilter::Min { key, value } => format!("{key} ≥ {value}"),
            ferret_domain::SpecFilter::Max { key, value } => format!("{key} ≤ {value}"),
            ferret_domain::SpecFilter::Eq { key, value } => format!("{key} = {value}"),
            ferret_domain::SpecFilter::AnyOf { key, values } => {
                format!("{key} ∈ {}", values.join("/"))
            }
            ferret_domain::SpecFilter::Is { key, value } => format!("{key}: {value}"),
        });
    }
    if let Some(f) = &watch.family {
        filters.push(f.clone());
    }
    if let Some(m) = &watch.model {
        filters.push(format!("model {m}"));
    }
    if let Some(gb) = watch.min_capacity_gb {
        filters.push(format!("≥ {gb} GB"));
    }
    if let Some(min) = watch.min_price_cents {
        filters.push(format!("≥ {}", format_price(min, "EUR")));
    }
    if let Some(max) = watch.max_price_cents {
        filters.push(format!("≤ {}", format_price(max, "EUR")));
    }
    if !watch.queries.is_empty() {
        filters.push(format!("searches: {}", watch.queries.join(", ")));
    }

    let toggle = {
        let client = client.clone();
        let watch = watch.clone();
        move |_| {
            let client = client.clone();
            let request = request_from(&watch, !watch.active);
            let id = watch.id;
            spawn_local(async move {
                let _ = client.update_watch(id, &request).await;
                version.0.update(|v| *v += 1);
            });
        }
    };
    let start_edit = {
        let watch = watch.clone();
        move |_| edit.0.set(Some(watch.clone()))
    };
    let delete = {
        let client = client.clone();
        let id = watch.id;
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                let _ = client.delete_watch(id).await;
                version.0.update(|v| *v += 1);
            });
        }
    };

    let show_chart = RwSignal::new(false);
    view! {
        <li class="watch watch-block" class:inactive=!watch.active>
            <div class="watch-row">
                <div class="watch-main">
                    <span class="watch-name">
                        {watch.name.clone()}
                        " "
                        <span class="badge ok">{move || format!("{} deals", match_count())}</span>
                    </span>
                    <span class="muted">{filters.join(" · ")}</span>
                </div>
                <div class="watch-actions">
                    <button on:click=move |_| show_chart.update(|s| *s = !*s)
                        title="daily best/median price over this watch's matches">
                        {move || if show_chart.get() { "hide history" } else { "history" }}
                    </button>
                    <button on:click=start_edit>"edit"</button>
                    <button on:click=toggle>
                        {if watch.active { "pause" } else { "resume" }}
                    </button>
                    <button class="danger" on:click=delete>"delete"</button>
                </div>
            </div>
            {move || show_chart.get().then(|| view! {
                <crate::price_chart::WatchPriceChart watch_id=watch_id/>
            })}
        </li>
    }
}
