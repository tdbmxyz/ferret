//! Shared Leptos UI: the same `App` component is mounted by the web bundle
//! (trunk) and rendered inside the Tauri webview. Anything platform-specific
//! (where the API lives) is injected from the outside via [`AppConfig`].

mod deals;
mod watches;

use ferret_client::FerretClient;
use leptos::prelude::*;
use url::Url;

/// Platform-provided configuration, put into the reactive context so every
/// component can reach the API client without prop-drilling.
#[derive(Clone)]
pub struct AppConfig {
    pub api_base: Url,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Deals,
    Watches,
}

/// Bumped after every mutation (watch created/updated/deleted) so list
/// resources reload.
#[derive(Clone, Copy)]
pub(crate) struct DataVersion(pub(crate) RwSignal<u32>);

#[component]
pub fn App(config: AppConfig) -> impl IntoView {
    provide_context(FerretClient::new(config.api_base));
    provide_context(DataVersion(RwSignal::new(0)));
    let tab = RwSignal::new(Tab::Deals);

    let tab_button = move |target: Tab, label: &'static str| {
        view! {
            <button
                class:active=move || tab.get() == target
                on:click=move |_| tab.set(target)
            >
                {label}
            </button>
        }
    };

    view! {
        <header class="topbar">
            <span class="brand">"ferret"</span>
            <nav>
                {tab_button(Tab::Deals, "Deals")}
                {tab_button(Tab::Watches, "Watches")}
            </nav>
        </header>
        <main>
            <div style:display=move || if tab.get() == Tab::Deals { "" } else { "none" }>
                <deals::DealsView/>
            </div>
            <div style:display=move || if tab.get() == Tab::Watches { "" } else { "none" }>
                <watches::WatchesView/>
            </div>
        </main>
    }
}

/// "45000 EUR cents" → "450.00 €" (only EUR gets the symbol treatment;
/// other currencies keep their code).
pub(crate) fn format_price(cents: i64, currency: &str) -> String {
    let value = cents as f64 / 100.0;
    match currency {
        "EUR" => format!("{value:.2} €"),
        other => format!("{value:.2} {other}"),
    }
}
