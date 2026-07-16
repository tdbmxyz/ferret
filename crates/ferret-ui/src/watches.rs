//! Watches view: create form + list with activate/deactivate and delete.

use ferret_client::FerretClient;
use ferret_domain::{Watch, WatchRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::{DataVersion, format_price};

/// "450" or "450.50" (euros) → cents; empty/garbage → None.
fn parse_euros(input: &str) -> Option<i64> {
    let trimmed = input.trim().replace(',', ".");
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f64>().ok().map(|e| (e * 100.0).round() as i64)
}

#[component]
pub fn WatchesView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();

    let name = RwSignal::new(String::new());
    let family = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let min_capacity = RwSignal::new(String::new());
    let min_price = RwSignal::new(String::new());
    let max_price = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);

    let families = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.families().await.unwrap_or_default() }
        }
    });
    let watches = LocalResource::new({
        let client = client.clone();
        move || {
            version.0.track();
            let client = client.clone();
            async move { client.watches().await }
        }
    });

    let create = {
        let client = client.clone();
        move |_| {
            let request = WatchRequest {
                name: name.get_untracked().trim().to_string(),
                family: Some(family.get_untracked()).filter(|s| !s.is_empty()),
                model: Some(model.get_untracked().trim().to_string()).filter(|s| !s.is_empty()),
                min_capacity_gb: min_capacity.get_untracked().trim().parse().ok(),
                min_price_cents: parse_euros(&min_price.get_untracked()),
                max_price_cents: parse_euros(&max_price.get_untracked()),
                active: true,
            };
            if request.name.is_empty() {
                error.set(Some("a watch needs a name".into()));
                return;
            }
            let client = client.clone();
            spawn_local(async move {
                match client.create_watch(&request).await {
                    Ok(_) => {
                        error.set(None);
                        name.set(String::new());
                        model.set(String::new());
                        min_capacity.set(String::new());
                        min_price.set(String::new());
                        max_price.set(String::new());
                        version.0.update(|v| *v += 1);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    };

    view! {
        <section>
            <form class="watch-form" on:submit=move |ev| ev.prevent_default()>
                <input placeholder="name (e.g. RTX 3080)" prop:value=name
                    on:input=move |ev| name.set(event_target_value(&ev))/>
                <select on:change=move |ev| family.set(event_target_value(&ev))>
                    <option value="">"any family"</option>
                    {move || {
                        families
                            .get()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|f| view! { <option value=f.name.clone()>{f.name.clone()}</option> })
                            .collect_view()
                    }}
                </select>
                <input placeholder="model (e.g. 3080)" prop:value=model
                    on:input=move |ev| model.set(event_target_value(&ev))/>
                <input placeholder="min capacity (GB)" prop:value=min_capacity
                    on:input=move |ev| min_capacity.set(event_target_value(&ev))/>
                <input placeholder="min price (€)" prop:value=min_price
                    on:input=move |ev| min_price.set(event_target_value(&ev))/>
                <input placeholder="max price (€)" prop:value=max_price
                    on:input=move |ev| max_price.set(event_target_value(&ev))/>
                <button on:click=create>"Add watch"</button>
            </form>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            {move || match watches.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => {
                    view! { <p class="error">{format!("server unreachable: {e}")}</p> }.into_any()
                }
                Some(Ok(watches)) if watches.is_empty() => {
                    view! { <p class="muted">"No watches yet — add one above."</p> }.into_any()
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

fn watch_row(watch: Watch) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();

    let mut filters: Vec<String> = Vec::new();
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

    let toggle = {
        let client = client.clone();
        let watch = watch.clone();
        move |_| {
            let client = client.clone();
            let request = WatchRequest {
                name: watch.name.clone(),
                family: watch.family.clone(),
                model: watch.model.clone(),
                min_capacity_gb: watch.min_capacity_gb,
                min_price_cents: watch.min_price_cents,
                max_price_cents: watch.max_price_cents,
                active: !watch.active,
            };
            let id = watch.id;
            spawn_local(async move {
                let _ = client.update_watch(id, &request).await;
                version.0.update(|v| *v += 1);
            });
        }
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

    view! {
        <li class="watch" class:inactive=!watch.active>
            <div class="watch-main">
                <span class="watch-name">{watch.name.clone()}</span>
                <span class="muted">{filters.join(" · ")}</span>
            </div>
            <div class="watch-actions">
                <button on:click=toggle>
                    {if watch.active { "pause" } else { "resume" }}
                </button>
                <button class="danger" on:click=delete>"delete"</button>
            </div>
        </li>
    }
}
