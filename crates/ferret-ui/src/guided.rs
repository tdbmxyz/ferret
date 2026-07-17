//! Guided watch creation: one text input → interpretation (category +
//! pre-filled constraints) → confirmation with matching stored deals while
//! a live background search streams more in → typed spec filters → saved
//! (or updated) watch.

use std::collections::HashMap;

use ferret_client::FerretClient;
use ferret_domain::{
    Category, CategoryStatus, Interpretation, SearchJob, SourceProgress, SpecFilter, SpecKind,
    Watch, WatchRequest,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

use crate::{DataVersion, format_price};

/// Set by the watches list to load an existing watch into the flow.
#[derive(Clone, Copy)]
pub struct EditRequest(pub RwSignal<Option<Watch>>);

/// String-typed control state for the dynamic spec filter UI.
#[derive(Clone, Default)]
struct FilterState {
    num_min: HashMap<String, String>,
    num_max: HashMap<String, String>,
    enums: HashMap<String, String>,
    bools: HashMap<String, bool>,
}

impl FilterState {
    fn from_filters(filters: &[SpecFilter]) -> Self {
        let mut s = Self::default();
        for f in filters {
            match f {
                SpecFilter::Min { key, value } => {
                    s.num_min.insert(key.clone(), trim_float(*value));
                }
                SpecFilter::Max { key, value } => {
                    s.num_max.insert(key.clone(), trim_float(*value));
                }
                SpecFilter::Eq { key, value } => {
                    s.enums.insert(key.clone(), value.clone());
                }
                SpecFilter::AnyOf { key, values } => {
                    if let Some(first) = values.first() {
                        s.enums.insert(key.clone(), first.clone());
                    }
                }
                SpecFilter::Is { key, value } => {
                    s.bools.insert(key.clone(), *value);
                }
            }
        }
        s
    }

    fn to_filters(&self) -> Vec<SpecFilter> {
        let mut filters = Vec::new();
        for (key, raw) in &self.num_min {
            if let Ok(value) = raw.trim().replace(',', ".").parse::<f64>() {
                filters.push(SpecFilter::Min { key: key.clone(), value });
            }
        }
        for (key, raw) in &self.num_max {
            if let Ok(value) = raw.trim().replace(',', ".").parse::<f64>() {
                filters.push(SpecFilter::Max { key: key.clone(), value });
            }
        }
        for (key, value) in &self.enums {
            if !value.is_empty() {
                filters.push(SpecFilter::Eq { key: key.clone(), value: value.clone() });
            }
        }
        for (key, value) in &self.bools {
            if *value {
                filters.push(SpecFilter::Is { key: key.clone(), value: true });
            }
        }
        filters
    }
}

fn trim_float(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") }
}

fn parse_euros(input: &str) -> Option<i64> {
    let trimmed = input.trim().replace(',', ".");
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f64>().ok().map(|e| (e * 100.0).round() as i64)
}

#[component]
pub fn GuidedCreate() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let edit: EditRequest = expect_context();

    let text = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let interpretation = RwSignal::new(None::<Interpretation>);
    let filters = RwSignal::new(FilterState::default());
    let name = RwSignal::new(String::new());
    let min_price = RwSignal::new(String::new());
    let max_price = RwSignal::new(String::new());
    let queries = RwSignal::new(String::new());
    let editing = RwSignal::new(None::<Uuid>);
    let job_id = RwSignal::new(None::<Uuid>);
    let job = RwSignal::new(None::<SearchJob>);
    let deals_tick = RwSignal::new(0u32);

    // an edit request loads the watch into the flow
    {
        let client = client.clone();
        Effect::new(move |_| {
            let Some(watch) = edit.0.get() else { return };
            edit.0.set(None);
            editing.set(Some(watch.id));
            name.set(watch.name.clone());
            text.set(watch.name.clone());
            min_price.set(watch.min_price_cents.map(|c| trim_float(c as f64 / 100.0)).unwrap_or_default());
            max_price.set(watch.max_price_cents.map(|c| trim_float(c as f64 / 100.0)).unwrap_or_default());
            queries.set(watch.queries.join(", "));
            filters.set(FilterState::from_filters(&watch.spec_filters));
            let category_slug = watch.category.clone();
            let client = client.clone();
            spawn_local(async move {
                let cats = client.categories().await.unwrap_or_default();
                let category = cats.into_iter().find(|c| Some(&c.slug) == category_slug.as_ref());
                interpretation.set(Some(Interpretation {
                    category,
                    constraints: vec![],
                    queries: vec![],
                    proposal: None,
                    via: "edit".into(),
                    llm_active: true,
                }));
            });
        });
    }

    // poll the background search while it runs
    {
        let client = client.clone();
        let handle = set_interval_with_handle(
            move || {
                let Some(id) = job_id.get_untracked() else { return };
                if job.get_untracked().as_ref().is_some_and(|j| j.done) {
                    return;
                }
                let client = client.clone();
                spawn_local(async move {
                    if let Ok(j) = client.search_progress(id).await {
                        job.set(Some(j));
                        deals_tick.update(|n| *n += 1);
                    }
                });
            },
            std::time::Duration::from_secs(3),
        );
        if let Ok(h) = handle {
            on_cleanup(move || h.clear());
        }
    }

    // stored deals matching the current draft (client-side preview)
    let preview = LocalResource::new({
        let client = client.clone();
        move || {
            deals_tick.track();
            interpretation.track();
            filters.track();
            min_price.track();
            max_price.track();
            let client = client.clone();
            async move { client.deals(None).await.unwrap_or_default() }
        }
    });
    let preview_matches = move || {
        let Some(interp) = interpretation.get() else { return Vec::new() };
        let Some(category) = interp.category else { return Vec::new() };
        let active_filters = filters.get().to_filters();
        let min = parse_euros(&min_price.get());
        let max = parse_euros(&max_price.get());
        preview
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| {
                d.category.as_ref() == Some(&category.slug)
                    && ferret_domain::category::filters_match(&active_filters, &d.specs)
                    && min.is_none_or(|m| d.price_cents >= m)
                    && max.is_none_or(|m| d.price_cents <= m)
            })
            .collect::<Vec<_>>()
    };

    let run_interpret = {
        let client = client.clone();
        move |_| {
            let query_text = text.get_untracked().trim().to_string();
            if query_text.is_empty() {
                return;
            }
            busy.set(true);
            error.set(None);
            let client = client.clone();
            spawn_local(async move {
                match client.interpret(&query_text).await {
                    Ok(out) => {
                        if name.get_untracked().is_empty() {
                            name.set(query_text.clone());
                        }
                        queries.set(out.queries.join(", "));
                        filters.set(FilterState::from_filters(&out.constraints));
                        // DB-first, live behind: kick the background search now
                        if (out.category.is_some() || out.proposal.is_some())
                            && let Ok(id) = client.start_search(&out.queries).await
                        {
                            job_id.set(Some(id));
                            job.set(None);
                        }
                        interpretation.set(Some(out));
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };

    let approve_proposal = {
        let client = client.clone();
        move |_| {
            let Some(mut interp) = interpretation.get_untracked() else { return };
            let Some(mut proposal) = interp.proposal.take() else { return };
            proposal.status = CategoryStatus::Active;
            let client = client.clone();
            spawn_local(async move {
                match client.upsert_category(&proposal).await {
                    Ok(approved) => {
                        if let Ok(id) = client.start_search(&interp.queries).await {
                            job_id.set(Some(id));
                        }
                        interp.category = Some(approved);
                        interp.proposal = None;
                        interpretation.set(Some(interp));
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    };

    let reset = move || {
        interpretation.set(None);
        editing.set(None);
        job_id.set(None);
        job.set(None);
        text.set(String::new());
        name.set(String::new());
        min_price.set(String::new());
        max_price.set(String::new());
        queries.set(String::new());
        filters.set(FilterState::default());
        error.set(None);
    };

    let save = {
        let client = client.clone();
        move |_| {
            let interp = interpretation.get_untracked();
            let request = WatchRequest {
                name: {
                    let n = name.get_untracked().trim().to_string();
                    if n.is_empty() { text.get_untracked().trim().to_string() } else { n }
                },
                family: None,
                model: None,
                min_capacity_gb: None,
                min_price_cents: parse_euros(&min_price.get_untracked()),
                max_price_cents: parse_euros(&max_price.get_untracked()),
                category: interp.as_ref().and_then(|i| i.category.as_ref()).map(|c| c.slug.clone()),
                spec_filters: filters.get_untracked().to_filters(),
                queries: queries
                    .get_untracked()
                    .split(',')
                    .map(|q| q.trim().to_lowercase())
                    .filter(|q| !q.is_empty())
                    .collect(),
                active: true,
            };
            if request.name.is_empty() {
                error.set(Some("the watch needs a name".into()));
                return;
            }
            let client = client.clone();
            let update_id = editing.get_untracked();
            spawn_local(async move {
                let result = match update_id {
                    Some(id) => client.update_watch(id, &request).await,
                    None => client.create_watch(&request).await,
                };
                match result {
                    Ok(_) => {
                        version.0.update(|v| *v += 1);
                        reset();
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    };

    view! {
        <div class="guided">
            <form class="guided-input" on:submit=move |ev| ev.prevent_default()>
                <input class="guided-text" placeholder="What are you hunting? (e.g. 4TB HDD)"
                    prop:value=text on:input=move |ev| text.set(event_target_value(&ev))/>
                <button on:click=run_interpret.clone() disabled=move || busy.get()>
                    {move || if busy.get() { "Interpreting…" } else { "Search" }}
                </button>
                {move || interpretation.get().map(|_| view! {
                    <button type="button" on:click=move |_| reset()>"Cancel"</button>
                })}
            </form>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}

            {move || {
                let interp = interpretation.get()?;
                Some(view! {
                    <div class="guided-result">
                        {confirmation_header(&interp)}
                        {interp.proposal.clone().map(|p| proposal_card(p, approve_proposal.clone()))}
                        {interp.category.clone().map(|category| view! {
                            <div>
                                {spec_controls(category.clone(), filters)}
                                <div class="guided-save">
                                    <input placeholder="watch name" prop:value=name
                                        on:input=move |ev| name.set(event_target_value(&ev))/>
                                    <input placeholder="min €" prop:value=min_price
                                        on:input=move |ev| min_price.set(event_target_value(&ev))/>
                                    <input placeholder="max €" prop:value=max_price
                                        on:input=move |ev| max_price.set(event_target_value(&ev))/>
                                    <input placeholder="search queries, comma-separated" prop:value=queries
                                        on:input=move |ev| queries.set(event_target_value(&ev))/>
                                    <button on:click=save.clone()>
                                        {move || if editing.get().is_some() { "Update watch" } else { "Create watch" }}
                                    </button>
                                </div>
                                {search_progress_line(job)}
                                {preview_list(preview_matches())}
                            </div>
                        })}
                    </div>
                })
            }}
        </div>
    }
}

fn confirmation_header(interp: &Interpretation) -> impl IntoView + use<> {
    let via = interp.via.clone();
    match (&interp.category, &interp.proposal) {
        (Some(c), _) => view! {
            <p>
                "Understood as " <span class="badge ok">{c.label.clone()}</span>
                " " <span class="muted">{format!("(via {via})")}</span>
            </p>
        }
        .into_any(),
        (None, Some(_)) => view! {
            <p>
                <span class="badge warn">"unknown product"</span>
                " — ferret drafted a new category for review:"
            </p>
        }
        .into_any(),
        (None, None) if !interp.llm_active => view! {
            <p class="muted">
                "No category matched, and no LLM is configured to interpret free text — \
                 ferret only knows the categories in the Categories tab. Add one there, \
                 or set up an LLM under ⚙ (or [llm] in ferret.toml)."
            </p>
        }
        .into_any(),
        (None, None) => view! {
            <p class="muted">
                "Couldn't identify a product behind that search — try other words, or cancel."
            </p>
        }
        .into_any(),
    }
}

fn proposal_card(
    proposal: Category,
    approve: impl Fn(web_sys::MouseEvent) + Clone + 'static,
) -> impl IntoView {
    let specs: Vec<String> = proposal
        .specs
        .iter()
        .map(|s| {
            let kind = match s.kind {
                SpecKind::Number => s.unit.clone().unwrap_or_else(|| "number".into()),
                SpecKind::Enum => s.allowed_values.join("/"),
                SpecKind::Boolean => "yes/no".into(),
            };
            format!("{} ({kind})", s.label)
        })
        .collect();
    view! {
        <div class="proposal">
            <span class="watch-name">{proposal.label.clone()}</span>
            <span class="muted">{format!("aliases: {}", proposal.aliases.join(", "))}</span>
            <span class="muted">{format!("filters: {}", specs.join(" · "))}</span>
            <button on:click=approve>"Approve category & continue"</button>
        </div>
    }
}

fn spec_controls(category: Category, filters: RwSignal<FilterState>) -> impl IntoView {
    view! {
        <div class="spec-controls">
            {category
                .specs
                .into_iter()
                .map(|spec| {
                    let key = spec.key.clone();
                    match spec.kind {
                        SpecKind::Number => {
                            let (kmin, kmax) = (key.clone(), key.clone());
                            let unit = spec.unit.clone().unwrap_or_default();
                            view! {
                                <label class="spec">
                                    {format!("{} ({unit})", spec.label)}
                                    <input class="narrow" placeholder="min"
                                        prop:value=move || filters.with(|f| f.num_min.get(&kmin).cloned().unwrap_or_default())
                                        on:input={let k = key.clone(); move |ev| filters.update(|f| { f.num_min.insert(k.clone(), event_target_value(&ev)); })}/>
                                    <input class="narrow" placeholder="max"
                                        prop:value=move || filters.with(|f| f.num_max.get(&kmax).cloned().unwrap_or_default())
                                        on:input={let k = key.clone(); move |ev| filters.update(|f| { f.num_max.insert(k.clone(), event_target_value(&ev)); })}/>
                                </label>
                            }
                            .into_any()
                        }
                        SpecKind::Enum => {
                            let ksel = key.clone();
                            view! {
                                <label class="spec">
                                    {spec.label.clone()}
                                    <select on:change={let k = key.clone(); move |ev| filters.update(|f| { f.enums.insert(k.clone(), event_target_value(&ev)); })}>
                                        <option value="" selected=move || filters.with(|f| f.enums.get(&ksel).is_none_or(|v| v.is_empty()))>"any"</option>
                                        {spec.allowed_values.iter().map(|v| {
                                            let (val, kcur) = (v.clone(), key.clone());
                                            let vsel = v.clone();
                                            view! {
                                                <option value=val.clone()
                                                    selected=move || filters.with(|f| f.enums.get(&kcur) == Some(&vsel))>
                                                    {val.clone()}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </select>
                                </label>
                            }
                            .into_any()
                        }
                        SpecKind::Boolean => {
                            let kb = key.clone();
                            view! {
                                <label class="spec">
                                    {spec.label.clone()}
                                    <input type="checkbox"
                                        prop:checked=move || filters.with(|f| f.bools.get(&kb).copied().unwrap_or(false))
                                        on:change={let k = key.clone(); move |ev| filters.update(|f| { f.bools.insert(k.clone(), event_target_checked(&ev)); })}/>
                                </label>
                            }
                            .into_any()
                        }
                    }
                })
                .collect_view()}
        </div>
    }
}

fn search_progress_line(job: RwSignal<Option<SearchJob>>) -> impl IntoView {
    move || {
        job.get().map(|j| {
            let items = j
                .sources
                .iter()
                .map(|(source, progress)| match progress {
                    SourceProgress::Pending => format!("{source}: searching…"),
                    SourceProgress::Done { listings } => format!("{source}: {listings} found"),
                    SourceProgress::Error { .. } => format!("{source}: failed"),
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let done = j.done;
            view! {
                <p class="muted">
                    {if done { "Live search done — " } else { "Live search running — " }}
                    {items}
                </p>
            }
        })
    }
}

fn preview_list(deals: Vec<ferret_domain::Deal>) -> impl IntoView {
    let count = deals.len();
    view! {
        <div class="preview">
            <p class="muted">{format!("{count} matching deal{} right now", if count == 1 { "" } else { "s" })}</p>
            <ul class="deals">
                {deals
                    .into_iter()
                    .take(8)
                    .map(|d| view! {
                        <li class="deal">
                            <div class="deal-main">
                                <a href=d.canonical_url.clone() target="_blank" rel="noreferrer">
                                    {d.title.clone()}
                                </a>
                                <span class="price">{format_price(d.price_cents, &d.currency)}</span>
                            </div>
                        </li>
                    })
                    .collect_view()}
            </ul>
        </div>
    }
}
