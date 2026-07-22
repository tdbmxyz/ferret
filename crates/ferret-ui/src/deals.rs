//! Deals view: all deals or one watch's matches, newest first, with the
//! heuristic flag badges, the LLM verdict and the gone/active status.

use std::time::Duration;

use ferret_client::FerretClient;
use ferret_domain::{Deal, DealRow, DealStatus, Flag, LlmVerdict, MatchInfo, Moderation, Watch};
use leptos::prelude::*;
use leptos::task::spawn_local;
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
    let show_hidden = RwSignal::new(false);
    let deals = LocalResource::new(move || {
        tick.track();
        version.0.track();
        let client = client.clone();
        let watch_id = filter.get();
        let hidden = show_hidden.get();
        async move { client.deals(watch_id, hidden).await }
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
                " "
                <label class="spec">
                    <input type="checkbox" prop:checked=show_hidden
                        on:change=move |ev| show_hidden.set(event_target_checked(&ev))/>
                    "hidden only"
                </label>
            </div>
            {move || match deals.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="error">{format!("server unreachable: {e}")}</p> }.into_any(),
                Some(Ok(deals)) if deals.is_empty() && show_hidden.get() => {
                    view! { <p class="muted">"Nothing dismissed or banned."</p> }.into_any()
                }
                Some(Ok(deals)) if deals.is_empty() => {
                    view! { <p class="muted">"No deals yet — they appear as sources are scraped."</p> }
                        .into_any()
                }
                Some(Ok(deals)) => view! {
                    <ul class="deals">
                        {deals
                            .into_iter()
                            .map(|row| view! { <DealCard row=row/> })
                            .collect_view()}
                    </ul>
                }
                .into_any(),
            }}
        </section>
    }
}

#[component]
fn DealCard(row: DealRow) -> impl IntoView {
    let DealRow { deal, matches } = row;
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
    // the report verdict comes first: EVERY row says whether it was
    // pushed, and if not, why
    badges.push(report_outcome(&deal, &matches));
    for flag in &deal.flags {
        match flag {
            Flag::PossibleStuffing => badges.push(("possible stuffing".into(), "warn")),
            Flag::PriceOutlier => badges.push(("price outlier".into(), "warn")),
            Flag::WantedAd => badges.push(("wanted ad".into(), "muted")),
        }
    }
    match deal.llm_verdict {
        Some(LlmVerdict::Genuine) => badges.push(("llm: genuine".into(), "ok")),
        Some(LlmVerdict::StuffedTitle) => badges.push(("llm: stuffed title".into(), "warn")),
        Some(LlmVerdict::Scam) => badges.push(("llm: scam".into(), "bad")),
        Some(LlmVerdict::Irrelevant) => badges.push(("llm: not the product".into(), "muted")),
        None => {}
    }
    if gone {
        badges.push(("gone".into(), "muted"));
    }
    match deal.moderation {
        Moderation::Dismissed => badges.push(("dismissed".into(), "muted")),
        Moderation::Banned => badges.push(("banned".into(), "bad")),
        Moderation::None => {}
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
            {move || expanded.get().then(|| moderation_actions(deal_id, deal.moderation))}
        </li>
    }
}

/// One definitive answer per row: was this deal reported, and if not,
/// why not — no more unexplained silence.
fn report_outcome(deal: &Deal, matches: &[MatchInfo]) -> (String, &'static str) {
    match deal.moderation {
        Moderation::Dismissed => return ("not reported: dismissed by you".into(), "muted"),
        Moderation::Banned => return ("not reported: banned by you".into(), "muted"),
        Moderation::None => {}
    }
    if matches.is_empty() {
        return ("not reported: no watch matches".into(), "muted");
    }
    let names = || {
        matches.iter().map(|m| m.watch_name.as_str()).collect::<Vec<_>>().join(", ")
    };
    if matches.iter().any(|m| m.notified_price_cents.is_some()) {
        return (format!("reported: {}", names()), "ok");
    }
    // matched but silent — name the gate that held it back
    let reason = if deal.flags.contains(&Flag::WantedAd) {
        "wanted ad"
    } else {
        match deal.llm_verdict {
            Some(LlmVerdict::StuffedTitle) => "stuffed title",
            Some(LlmVerdict::Scam) => "scam verdict",
            Some(LlmVerdict::Irrelevant) => "not the product",
            Some(LlmVerdict::Genuine) => "pending",
            None => {
                if deal.flags.contains(&Flag::PossibleStuffing) {
                    "possible stuffing, no LLM verdict"
                } else {
                    "pending"
                }
            }
        }
    };
    (format!("matched {} — muted: {reason}", names()), "warn")
}

/// Dismiss / ban / restore buttons on an expanded card. Dismiss hides the
/// listing until it disappears and is re-acquired; ban is forever.
fn moderation_actions(deal_id: Uuid, current: Moderation) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let set = move |moderation: Moderation| {
        let client = client.clone();
        move |ev: web_sys::MouseEvent| {
            ev.stop_propagation();
            let client = client.clone();
            spawn_local(async move {
                let _ = client.set_moderation(deal_id, moderation).await;
                version.0.update(|v| *v += 1);
            });
        }
    };
    view! {
        <div class="watch-actions deal-actions">
            {(current != Moderation::Dismissed).then(|| view! {
                <button title="hide — comes back if the listing disappears and is re-listed"
                    on:click=set(Moderation::Dismissed)>
                    "dismiss"
                </button>
            })}
            {(current != Moderation::Banned).then(|| view! {
                <button class="danger" title="never show or match this listing again"
                    on:click=set(Moderation::Banned)>
                    "ban"
                </button>
            })}
            {(current != Moderation::None).then(|| view! {
                <button title="restore — it can match watches again"
                    on:click=set(Moderation::None)>
                    "restore"
                </button>
            })}
        </div>
    }
}
