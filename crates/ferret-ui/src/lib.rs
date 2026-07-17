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

/// localStorage key for the manual server override (read at startup by
/// ferret-web's API-base resolution).
const API_BASE_KEY: &str = "ferret-api-base";

#[component]
pub fn App(config: AppConfig) -> impl IntoView {
    provide_context(FerretClient::new(config.api_base.clone()));
    provide_context(DataVersion(RwSignal::new(0)));
    let tab = RwSignal::new(Tab::Deals);
    let show_connect = RwSignal::new(false);
    let server = RwSignal::new(config.api_base.to_string());

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

    // Persist the override and reload so the whole app re-resolves — the
    // path for pointing the Android shell at the server.
    let save_server = move |_| {
        let value = server.get_untracked();
        let Some(window) = web_sys::window() else { return };
        if let Ok(Some(storage)) = window.local_storage() {
            if value.trim().is_empty() || Url::parse(value.trim()).is_err() {
                let _ = storage.remove_item(API_BASE_KEY);
            } else {
                let _ = storage.set_item(API_BASE_KEY, value.trim());
            }
            let _ = window.location().reload();
        }
    };

    view! {
        <header class="topbar">
            <span class="brand">"ferret"</span>
            <nav>
                {tab_button(Tab::Deals, "Deals")}
                {tab_button(Tab::Watches, "Watches")}
            </nav>
            <button class="connect-toggle" title="server address"
                on:click=move |_| show_connect.update(|s| *s = !*s)>
                "⚙"
            </button>
        </header>
        {move || show_connect.get().then(|| view! {
            <div class="connect">
                <label>"Server: "</label>
                <input prop:value=server placeholder="http://zeus:4800"
                    on:input=move |ev| server.set(event_target_value(&ev))/>
                <button on:click=save_server>"Save & reload"</button>
                <span class="muted">"empty = back to automatic"</span>
            </div>
        })}
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
