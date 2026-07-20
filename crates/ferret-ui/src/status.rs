//! Sources liveness strip: is anything actually scraping, when did each
//! source last tick, and did it fail — the "return" a fresh watch lacked.

use ferret_client::FerretClient;
use ferret_domain::{SourceStatus, StatusResponse};
use leptos::prelude::*;

use crate::DataVersion;

/// Shared, periodically refreshed /api/status resource; provided once by
/// the App so both views read the same data.
#[derive(Clone, Copy)]
pub struct StatusResource(pub LocalResource<Option<StatusResponse>>);

pub fn provide_status(tick: RwSignal<u32>) {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let resource = LocalResource::new(move || {
        tick.track();
        version.0.track();
        let client = client.clone();
        async move { client.status().await.ok() }
    });
    provide_context(StatusResource(resource));
}

/// Seconds counter that runs while `busy` is true (0 otherwise) — drives
/// the "Interpreting… 12s" live feedback on LLM buttons.
pub fn elapsed_while(busy: RwSignal<bool>) -> RwSignal<u32> {
    let elapsed = RwSignal::new(0u32);
    if let Ok(handle) = set_interval_with_handle(
        move || {
            if busy.get_untracked() {
                elapsed.update(|e| *e += 1);
            } else if elapsed.get_untracked() != 0 {
                elapsed.set(0);
            }
        },
        std::time::Duration::from_secs(1),
    ) {
        on_cleanup(move || handle.clear());
    }
    elapsed
}

/// Historical average duration for one LLM call kind, from /api/status.
pub fn llm_avg_ms(status: &StatusResource, kind: &str) -> Option<i64> {
    status.0.get().flatten().and_then(|s| s.llm.avg_ms.get(kind).copied())
}

/// "Interpreting… 12s / ~63s" — elapsed now, expectation from history.
pub fn llm_progress_label(base: &str, elapsed: u32, avg_ms: Option<i64>) -> String {
    match avg_ms {
        Some(ms) if ms > 0 => {
            format!("{base}… {elapsed}s / ~{}s", (ms as f64 / 1000.0).round() as u64)
        }
        _ => format!("{base}… {elapsed}s"),
    }
}

fn ago(status: &SourceStatus) -> String {
    match status.last_tick {
        None => "waiting for first pass".into(),
        Some(t) => {
            let mins = (chrono::Utc::now() - t).num_minutes();
            match mins {
                m if m < 1 => "just now".into(),
                m if m < 60 => format!("{m} min ago"),
                m => format!("{}h{:02} ago", m / 60, m % 60),
            }
        }
    }
}

#[component]
pub fn SourcesStrip() -> impl IntoView {
    let status: StatusResource = expect_context();
    view! {
        <div class="sources">
            {move || match status.0.get().flatten() {
                None => view! { <span class="muted">"checking sources…"</span> }.into_any(),
                Some(s) if s.sources.is_empty() => view! {
                    <span class="badge bad">"no sources configured — nothing will be scraped"</span>
                }
                .into_any(),
                Some(s) => {
                    let llm_chip = if s.llm.enabled && s.llm.busy > 0 {
                        view! {
                            <span class="badge warn">
                                {format!(
                                    "LLM ⋯ working ({} call{})",
                                    s.llm.busy,
                                    if s.llm.busy == 1 { "" } else { "s" },
                                )}
                            </span>
                        }
                        .into_any()
                    } else if s.llm.enabled {
                        view! {
                            <span class="badge ok">
                                {format!("LLM ✓ {}", s.llm.model.clone().unwrap_or_default())}
                            </span>
                        }
                        .into_any()
                    } else {
                        view! {
                            <span class="badge muted" title="heuristics only — configure under ⚙">
                                "LLM off"
                            </span>
                        }
                        .into_any()
                    };
                    let sources = s
                    .sources
                    .iter()
                    .map(|src| {
                        let label = match (&src.last_error, &src.last_stats) {
                            (Some(_), _) => format!("{} ✗ failing, {}", src.source_id, ago(src)),
                            (None, Some(st)) => format!(
                                "{} ✓ {} listings, {}",
                                src.source_id, st.fetched, ago(src)
                            ),
                            (None, None) => {
                                format!("{} · every {} min, {}", src.source_id,
                                    src.interval_minutes, ago(src))
                            }
                        };
                        let bad = src.last_error.is_some();
                        view! {
                            <span class="badge" class:bad=bad class:ok=!bad
                                title=src.last_error.clone().unwrap_or_default()>
                                {label}
                            </span>
                        }
                    })
                    .collect_view();
                    view! { {sources} {llm_chip} }.into_any()
                }
            }}
        </div>
    }
}
