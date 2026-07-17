//! Deals view: all deals or one watch's matches, newest first, with the
//! heuristic flag badges, the LLM verdict and the gone/active status.

use std::time::Duration;

use ferret_client::FerretClient;
use ferret_domain::{Deal, DealStatus, Flag, LlmVerdict, Watch};
use leptos::prelude::*;
use uuid::Uuid;

use crate::{DataVersion, format_price};

/// Deals auto-refresh on this cadence (scrape ticks are minutes apart).
const REFRESH: Duration = Duration::from_secs(60);

#[component]
pub fn DealsView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let filter = RwSignal::new(None::<Uuid>);

    let tick = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(move || tick.update(|n| *n += 1), REFRESH) {
        on_cleanup(move || handle.clear());
    }

    let watches = LocalResource::new({
        let client = client.clone();
        move || {
            version.0.track();
            let client = client.clone();
            async move { client.watches().await.unwrap_or_default() }
        }
    });
    let deals = LocalResource::new(move || {
        tick.track();
        version.0.track();
        let client = client.clone();
        let watch_id = filter.get();
        async move { client.deals(watch_id).await }
    });

    view! {
        <section>
            <crate::status::SourcesStrip/>
            <div class="toolbar">
                <label>
                    "Watch: "
                    <select on:change=move |ev| {
                        let value = event_target_value(&ev);
                        filter.set(Uuid::parse_str(&value).ok());
                    }>
                        <option value="">"all deals"</option>
                        {move || {
                            watches
                                .get()
                                .unwrap_or_default()
                                .into_iter()
                                .map(|w: Watch| {
                                    view! { <option value=w.id.to_string()>{w.name}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
            </div>
            {move || match deals.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{format!("server unreachable: {e}")}</p> }.into_any(),
                Some(Ok(deals)) if deals.is_empty() => {
                    view! { <p class="muted">"No deals yet — they appear as sources are scraped."</p> }
                        .into_any()
                }
                Some(Ok(deals)) => view! {
                    <ul class="deals">
                        {deals
                            .into_iter()
                            .map(|deal| view! { <DealCard deal=deal/> })
                            .collect_view()}
                    </ul>
                }
                .into_any(),
            }}
        </section>
    }
}

#[component]
fn DealCard(deal: Deal) -> impl IntoView {
    let client: FerretClient = expect_context();
    let expanded = RwSignal::new(false);
    let deal_id = deal.id;
    // fetched lazily: nothing hits the network until the card is opened
    let prices = LocalResource::new(move || {
        let open = expanded.get();
        let client = client.clone();
        async move {
            if !open {
                return None;
            }
            client.deal_prices(deal_id).await.ok()
        }
    });
    let currency = deal.currency.clone();
    let gone = deal.status == DealStatus::Gone;
    let mut badges: Vec<(String, &'static str)> = Vec::new();
    for flag in &deal.flags {
        match flag {
            Flag::PossibleStuffing => badges.push(("possible stuffing".into(), "warn")),
            Flag::PriceOutlier => badges.push(("price outlier".into(), "warn")),
        }
    }
    match deal.llm_verdict {
        Some(LlmVerdict::Genuine) => badges.push(("llm: genuine".into(), "ok")),
        Some(LlmVerdict::StuffedTitle) => badges.push(("llm: stuffed title".into(), "warn")),
        Some(LlmVerdict::Scam) => badges.push(("llm: scam".into(), "bad")),
        None => {}
    }
    if gone {
        badges.push(("gone".into(), "muted"));
    }

    let mut details: Vec<String> = vec![deal.source_id.clone()];
    if let Some(gb) = deal.capacity_gb {
        details.push(if gb >= 1000 && gb % 1000 == 0 {
            format!("{} TB", gb / 1000)
        } else {
            format!("{gb} GB")
        });
    }
    if let Some(cond) = &deal.condition {
        details.push(cond.clone());
    }

    view! {
        <li class="deal" class:gone=gone on:click=move |_| expanded.update(|e| *e = !*e)>
            <div class="deal-main">
                <a href=deal.canonical_url.clone() target="_blank" rel="noreferrer"
                    on:click=move |ev| ev.stop_propagation()>
                    {deal.title.clone()}
                </a>
                <span class="price">{format_price(deal.price_cents, &deal.currency)}</span>
            </div>
            <div class="deal-meta">
                <span class="muted">{details.join(" · ")}</span>
                {badges
                    .into_iter()
                    .map(|(text, kind)| view! { <span class=format!("badge {kind}")>{text}</span> })
                    .collect_view()}
            </div>
            {deal.llm_reason.map(|reason| view! { <div class="muted reason">{reason}</div> })}
            {move || {
                expanded.get().then(|| match prices.get().flatten() {
                    None => view! { <p class="muted">"Loading price history…"</p> }.into_any(),
                    Some(points) => {
                        view! { <crate::sparkline::Sparkline prices=points currency=currency.clone()/> }
                            .into_any()
                    }
                })
            }}
        </li>
    }
}
