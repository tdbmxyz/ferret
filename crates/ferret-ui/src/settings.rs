//! Server-side LLM settings, edited from the ⚙ panel. The TOML config is
//! the base; saving here stores a DB override that applies immediately.

use ferret_client::FerretClient;
use ferret_domain::{LlmProbeRequest, LlmSettings, LlmSettingsUpdate, PromptSet};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The three LLM system prompts, editable with a reset to factory defaults.
#[component]
pub fn PromptsPanel() -> impl IntoView {
    let client: FerretClient = expect_context();

    let refine = RwSignal::new(String::new());
    let interpret = RwSignal::new(String::new());
    let revise = RwSignal::new(String::new());
    let defaults = RwSignal::new(None::<PromptSet>);
    let message = RwSignal::new(None::<String>);

    let apply = move |current: PromptSet, def: PromptSet| {
        refine.set(current.refine);
        interpret.set(current.interpret);
        revise.set(current.revise);
        defaults.set(Some(def));
    };

    {
        let client = client.clone();
        spawn_local(async move {
            match client.prompts().await {
                Ok(r) => apply(r.current, r.default),
                Err(e) => message.set(Some(format!("couldn't load prompts: {e}"))),
            }
        });
    }

    let save = {
        let client = client.clone();
        move |_| {
            let set = PromptSet {
                refine: refine.get_untracked(),
                interpret: interpret.get_untracked(),
                revise: revise.get_untracked(),
            };
            let client = client.clone();
            spawn_local(async move {
                match client.update_prompts(&set).await {
                    Ok(r) => {
                        apply(r.current, r.default);
                        message.set(Some("prompts saved — applied immediately".into()));
                    }
                    Err(e) => message.set(Some(e.to_string())),
                }
            });
        }
    };
    let reset = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                match client.reset_prompts().await {
                    Ok(r) => {
                        apply(r.current, r.default);
                        message.set(Some("back to the default prompts".into()));
                    }
                    Err(e) => message.set(Some(e.to_string())),
                }
            });
        }
    };

    let changed = move |value: RwSignal<String>, pick: fn(&PromptSet) -> &String| {
        move || defaults.get().is_some_and(|d| value.get().trim() != pick(&d).trim())
    };
    let prompt_area = move |label: &'static str,
                            hint: &'static str,
                            value: RwSignal<String>,
                            pick: fn(&PromptSet) -> &String| {
        view! {
            <label class="prompt-label">
                {label} " " <span class="muted">{hint}</span>
                {move || changed(value, pick)().then(|| view! {
                    <span class="badge warn">"edited"</span>
                })}
            </label>
            <textarea prop:value=value rows=5
                on:input=move |ev| value.set(event_target_value(&ev))>
            </textarea>
        }
    };

    view! {
        <div class="settings-block prompts">
            <span class="settings-title">
                "System prompts — how the LLM is instructed (empty or unchanged = default)"
            </span>
            {prompt_area("interpret", "(search text → category)", interpret, |d| &d.interpret)}
            {prompt_area("revise", "(category rework instructions)", revise, |d| &d.revise)}
            {prompt_area("refine", "(listing review: genuine/stuffed/scam)", refine, |d| &d.refine)}
            <div class="editor-actions">
                <button on:click=save>"Save prompts"</button>
                <button on:click=reset>"Reset to defaults"</button>
                {move || message.get().map(|m| view! { <span class="muted">{m}</span> })}
            </div>
        </div>
    }
}

#[component]
pub fn LlmSettingsPanel() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: crate::DataVersion = expect_context();

    let enabled = RwSignal::new(false);
    let base_url = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let current = RwSignal::new(None::<LlmSettings>);
    let message = RwSignal::new(None::<String>);
    let models = RwSignal::new(Vec::<String>::new());

    // form values → probe body; empty fields fall back server-side to the
    // stored settings (including the saved API key)
    let probe_request = move || {
        let key = api_key.get_untracked();
        LlmProbeRequest {
            base_url: Some(base_url.get_untracked().trim().to_string()).filter(|s| !s.is_empty()),
            model: Some(model.get_untracked().trim().to_string()).filter(|s| !s.is_empty()),
            api_key: (!key.is_empty()).then_some(key),
        }
    };

    let apply = move |settings: LlmSettings| {
        enabled.set(settings.enabled);
        base_url.set(settings.base_url.clone());
        model.set(settings.model.clone());
        api_key.set(String::new());
        current.set(Some(settings));
    };

    // load once on mount
    {
        let client = client.clone();
        spawn_local(async move {
            match client.llm_settings().await {
                Ok(settings) => apply(settings),
                Err(e) => message.set(Some(format!("couldn't load LLM settings: {e}"))),
            }
        });
    }

    let load_models = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            let request = probe_request();
            message.set(Some("asking the endpoint for its models…".into()));
            spawn_local(async move {
                match client.llm_models(&request).await {
                    Ok(found) if found.is_empty() => {
                        message.set(Some("the endpoint answered but lists no models".into()));
                    }
                    Ok(found) => {
                        message.set(Some(format!("{} models found", found.len())));
                        if model.get_untracked().trim().is_empty()
                            && let Some(first) = found.first()
                        {
                            model.set(first.clone());
                        }
                        models.set(found);
                    }
                    Err(e) => message.set(Some(format!("model listing failed — {e}"))),
                }
            });
        }
    };

    let test = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            let request = probe_request();
            message.set(Some("testing — one real completion…".into()));
            spawn_local(async move {
                match client.test_llm(&request).await {
                    Ok(result) if result.ok => message.set(Some("LLM answered ✓".into())),
                    Ok(result) => message.set(Some(format!(
                        "LLM test failed — {}",
                        result.error.unwrap_or_else(|| "unknown error".into())
                    ))),
                    Err(e) => message.set(Some(format!("LLM test failed — {e}"))),
                }
            });
        }
    };

    let save = {
        let client = client.clone();
        move |_| {
            let key = api_key.get_untracked();
            let update = LlmSettingsUpdate {
                enabled: enabled.get_untracked(),
                base_url: base_url.get_untracked().trim().to_string(),
                model: model.get_untracked().trim().to_string(),
                // empty input = keep the stored key
                api_key: (!key.is_empty()).then_some(key),
            };
            let client = client.clone();
            spawn_local(async move {
                match client.update_llm_settings(&update).await {
                    Ok(settings) => {
                        apply(settings);
                        message.set(Some("saved — applies to the next interpretation/tick".into()));
                        version.0.update(|v| *v += 1);
                    }
                    Err(e) => message.set(Some(e.to_string())),
                }
            });
        }
    };

    let clear_key = {
        let client = client.clone();
        move |_| {
            let update = LlmSettingsUpdate {
                enabled: enabled.get_untracked(),
                base_url: base_url.get_untracked().trim().to_string(),
                model: model.get_untracked().trim().to_string(),
                api_key: Some(String::new()),
            };
            let client = client.clone();
            spawn_local(async move {
                match client.update_llm_settings(&update).await {
                    Ok(settings) => {
                        apply(settings);
                        message.set(Some("stored key cleared".into()));
                    }
                    Err(e) => message.set(Some(e.to_string())),
                }
            });
        }
    };

    let reset = {
        let client = client.clone();
        move |_| {
            let client = client.clone();
            spawn_local(async move {
                match client.reset_llm_settings().await {
                    Ok(settings) => {
                        apply(settings);
                        message.set(Some("override dropped — back to the server config".into()));
                        version.0.update(|v| *v += 1);
                    }
                    Err(e) => message.set(Some(e.to_string())),
                }
            });
        }
    };

    view! {
        <div class="settings-block">
            <span class="settings-title">
                "LLM (stored on the server) — interprets free-text searches and \
                 reviews ambiguous listings"
            </span>
            <label class="spec">
                <input type="checkbox" prop:checked=enabled
                    on:change=move |ev| enabled.set(event_target_checked(&ev))/>
                "enabled"
            </label>
            <input placeholder="base URL, e.g. http://zeus:8080/v1" prop:value=base_url
                on:input=move |ev| base_url.set(event_target_value(&ev))/>
            <input placeholder="model" prop:value=model
                on:input=move |ev| model.set(event_target_value(&ev))/>
            {move || {
                let found = models.get();
                (!found.is_empty()).then(|| view! {
                    <select on:change=move |ev| model.set(event_target_value(&ev))>
                        {found.iter().map(|m| {
                            let (value, selected) = (m.clone(), m.clone());
                            view! {
                                <option value=value.clone()
                                    selected=move || model.get() == selected>
                                    {value.clone()}
                                </option>
                            }
                        }).collect_view()}
                    </select>
                })
            }}
            <input type="password"
                placeholder=move || {
                    if current.get().is_some_and(|s| s.api_key_set) {
                        "API key stored — empty keeps it"
                    } else {
                        "API key (optional)"
                    }
                }
                prop:value=api_key
                on:input=move |ev| api_key.set(event_target_value(&ev))/>
            <button on:click=load_models title="query {base URL}/models">"List models"</button>
            <button on:click=test title="one real completion round-trip">"Test"</button>
            <button on:click=save>"Save"</button>
            {move || current.get().is_some_and(|s| s.api_key_set).then(|| view! {
                <button on:click=clear_key.clone()>"Clear key"</button>
            })}
            {move || current.get().is_some_and(|s| s.from_override).then(|| view! {
                <button on:click=reset.clone() title="drop the override, use ferret.toml">
                    "Use server config"
                </button>
            })}
            {move || message.get().map(|m| view! { <span class="muted">{m}</span> })}
        </div>
    }
}
