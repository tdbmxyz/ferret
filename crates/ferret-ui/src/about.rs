//! About: client + server versions with their git commits, connection
//! and LLM state — the "what exactly am I running?" tab.

use ferret_client::FerretClient;
use leptos::prelude::*;

#[component]
pub fn AboutView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let status: crate::status::StatusResource = expect_context();
    let server_url = client.base().to_string();

    let health = LocalResource::new({
        let client = client.clone();
        move || {
            let client = client.clone();
            async move { client.health().await }
        }
    });

    view! {
        <section class="about">
            <p>
                <span class="muted">"Client: "</span>
                {format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("FERRET_COMMIT"))}
            </p>
            <p>
                <span class="muted">"Server: "</span>
                {move || match health.get() {
                    None => "checking…".to_string(),
                    Some(Ok(h)) => format!(
                        "{} ({}) at {server_url}",
                        h.version,
                        h.commit.unwrap_or_else(|| "unknown".into()),
                    ),
                    Some(Err(e)) => format!("unreachable ({e})"),
                }}
            </p>
            <p>
                <span class="muted">"LLM: "</span>
                {move || match status.0.get().flatten() {
                    None => "…".to_string(),
                    Some(s) if s.llm.enabled => {
                        format!("enabled — {}", s.llm.model.unwrap_or_default())
                    }
                    Some(_) => "disabled (heuristics only)".to_string(),
                }}
            </p>
            <p class="muted">
                "ferret sniffs second-hand marketplaces for the deals you describe. "
                <a href="https://github.com/tdbmxyz/ferret" target="_blank" rel="noreferrer">
                    "Source"
                </a>
            </p>
        </section>
    }
}
