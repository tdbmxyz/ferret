//! Categories management: review LLM-proposed categories, and create or
//! edit any category's aliases and spec table — the schema behind guided
//! watch filters.

use ferret_client::FerretClient;
use ferret_domain::{Category, CategoryOrigin, CategorySpec, CategoryStatus, SpecKind};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::DataVersion;

/// One editable spec line; every field is its own signal so typing never
/// re-renders (and never drops focus on) the row.
#[derive(Clone)]
struct SpecRow {
    id: u32,
    key: RwSignal<String>,
    label: RwSignal<String>,
    kind: RwSignal<String>,
    unit: RwSignal<String>,
    values: RwSignal<String>,
    hint: RwSignal<String>,
}

impl SpecRow {
    fn new(id: u32, spec: Option<&CategorySpec>) -> Self {
        let kind = match spec.map(|s| s.kind) {
            Some(SpecKind::Enum) => "enum",
            Some(SpecKind::Boolean) => "boolean",
            _ => "number",
        };
        Self {
            id,
            key: RwSignal::new(spec.map(|s| s.key.clone()).unwrap_or_default()),
            label: RwSignal::new(spec.map(|s| s.label.clone()).unwrap_or_default()),
            kind: RwSignal::new(kind.into()),
            unit: RwSignal::new(spec.and_then(|s| s.unit.clone()).unwrap_or_default()),
            values: RwSignal::new(spec.map(|s| s.allowed_values.join(", ")).unwrap_or_default()),
            hint: RwSignal::new(spec.and_then(|s| s.extraction_hint.clone()).unwrap_or_default()),
        }
    }

    fn to_spec(&self) -> Option<CategorySpec> {
        let key = self.key.get_untracked().trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        let label = {
            let l = self.label.get_untracked().trim().to_string();
            if l.is_empty() { key.clone() } else { l }
        };
        let kind = match self.kind.get_untracked().as_str() {
            "enum" => SpecKind::Enum,
            "boolean" => SpecKind::Boolean,
            _ => SpecKind::Number,
        };
        let unit = Some(self.unit.get_untracked().trim().to_string()).filter(|u| !u.is_empty());
        let hint = Some(self.hint.get_untracked().trim().to_string()).filter(|h| !h.is_empty());
        Some(CategorySpec {
            key,
            label,
            kind,
            unit,
            allowed_values: split_list(&self.values.get_untracked()),
            extraction_hint: hint,
        })
    }
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

/// The open editor: header signals + spec rows + what it edits.
#[derive(Clone)]
struct Editor {
    original: Option<Category>,
    slug: RwSignal<String>,
    label: RwSignal<String>,
    aliases: RwSignal<String>,
    rows: RwSignal<Vec<SpecRow>>,
    next_id: RwSignal<u32>,
}

impl Editor {
    fn open(category: Option<Category>) -> Self {
        let specs = category.as_ref().map(|c| c.specs.clone()).unwrap_or_default();
        let rows: Vec<SpecRow> =
            specs.iter().enumerate().map(|(i, s)| SpecRow::new(i as u32, Some(s))).collect();
        Self {
            slug: RwSignal::new(category.as_ref().map(|c| c.slug.clone()).unwrap_or_default()),
            label: RwSignal::new(category.as_ref().map(|c| c.label.clone()).unwrap_or_default()),
            aliases: RwSignal::new(
                category.as_ref().map(|c| c.aliases.join(", ")).unwrap_or_default(),
            ),
            next_id: RwSignal::new(rows.len() as u32),
            rows: RwSignal::new(rows),
            original: category,
        }
    }

    /// Load an LLM revision into the open editor (slug stays what it was).
    fn load_revision(&self, category: &Category) {
        self.label.set(category.label.clone());
        self.aliases.set(category.aliases.join(", "));
        let base = self.next_id.get_untracked();
        let rows: Vec<SpecRow> = category
            .specs
            .iter()
            .enumerate()
            .map(|(i, s)| SpecRow::new(base + i as u32, Some(s)))
            .collect();
        self.next_id.set(base + rows.len() as u32);
        self.rows.set(rows);
    }

    fn to_category(&self) -> Result<Category, String> {
        let slug = self.slug.get_untracked().trim().to_lowercase().replace(' ', "-");
        if slug.is_empty() {
            return Err("the category needs a slug".into());
        }
        let label = {
            let l = self.label.get_untracked().trim().to_string();
            if l.is_empty() { slug.clone() } else { l }
        };
        Ok(Category {
            slug,
            label,
            aliases: split_list(&self.aliases.get_untracked()),
            origin: self.original.as_ref().map(|c| c.origin).unwrap_or(CategoryOrigin::User),
            status: self.original.as_ref().map(|c| c.status).unwrap_or(CategoryStatus::Active),
            specs: self.rows.get_untracked().iter().filter_map(SpecRow::to_spec).collect(),
            created_at: self
                .original
                .as_ref()
                .map(|c| c.created_at)
                .unwrap_or_else(chrono::Utc::now),
        })
    }
}

#[derive(Clone, Copy)]
struct EditorSlot(RwSignal<Option<Editor>>);

#[component]
pub fn CategoriesView() -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let editor = EditorSlot(RwSignal::new(None));
    provide_context(editor);
    let categories = LocalResource::new({
        let client = client.clone();
        move || {
            version.0.track();
            let client = client.clone();
            async move { client.categories().await.unwrap_or_default() }
        }
    });

    view! {
        <section>
            <p class="muted">
                "Categories drive interpretation and filters. Proposed ones were drafted \
                 by the LLM and only start categorizing deals once approved."
            </p>
            <div class="toolbar">
                <button on:click=move |_| editor.0.set(Some(Editor::open(None)))>
                    "New category"
                </button>
            </div>
            {move || editor.0.get().map(editor_view)}
            <ul class="watches">
                {move || {
                    categories
                        .get()
                        .unwrap_or_default()
                        .into_iter()
                        .map(category_row)
                        .collect_view()
                }}
            </ul>
        </section>
    }
}

fn editor_view(editor: Editor) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let slot: EditorSlot = expect_context();
    let error = RwSignal::new(None::<String>);
    let is_new = editor.original.is_none();
    let instruction = RwSignal::new(String::new());
    let asking = RwSignal::new(false);
    let status_res: crate::status::StatusResource = expect_context();
    let ask_elapsed = crate::status::elapsed_while(asking);
    let sync_shared = RwSignal::new(true);
    // the revision conversation: every ask continues where the last ended
    let chat = RwSignal::new(Vec::<ferret_domain::ChatTurn>::new());

    let ask_llm = {
        let client = client.clone();
        let editor = editor.clone();
        move |_| {
            let text = instruction.get_untracked().trim().to_string();
            if text.is_empty() {
                return;
            }
            match editor.to_category() {
                Ok(current) => {
                    let client = client.clone();
                    let editor = editor.clone();
                    asking.set(true);
                    error.set(None);
                    spawn_local(async move {
                        let history = chat.get_untracked();
                        match client.revise_category(&current, &text, &history).await {
                            Ok(revised) => {
                                chat.update(|c| {
                                    c.push(ferret_domain::ChatTurn {
                                        role: "user".into(),
                                        content: text.clone(),
                                    });
                                    c.push(ferret_domain::ChatTurn {
                                        role: "assistant".into(),
                                        content: serde_json::to_string(&revised)
                                            .unwrap_or_default(),
                                    });
                                });
                                editor.load_revision(&revised);
                                instruction.set(String::new());
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                        asking.set(false);
                    });
                }
                Err(e) => error.set(Some(e)),
            }
        }
    };

    let add_row = {
        let editor = editor.clone();
        move |_| {
            let id = editor.next_id.get_untracked();
            editor.next_id.set(id + 1);
            editor.rows.update(|rows| rows.push(SpecRow::new(id, None)));
        }
    };
    let save = {
        let editor = editor.clone();
        move |_| match editor.to_category() {
            Ok(category) => {
                let client = client.clone();
                let sync = sync_shared.get_untracked();
                spawn_local(async move {
                    match client.upsert_category(&category).await {
                        Ok(_) => {
                            if sync {
                                sync_shared_specs(&client, &category).await;
                            }
                            slot.0.set(None);
                            version.0.update(|v| *v += 1);
                        }
                        Err(e) => error.set(Some(e.to_string())),
                    }
                });
            }
            Err(e) => error.set(Some(e)),
        }
    };
    let rows = editor.rows;

    view! {
        <div class="editor">
            <span class="settings-title">
                {if is_new { "New category" } else { "Edit category" }}
            </span>
            <div class="editor-head">
                <input placeholder="slug (stable id)" prop:value=editor.slug
                    disabled=!is_new
                    on:input=move |ev| editor.slug.set(event_target_value(&ev))/>
                <input placeholder="label" prop:value=editor.label
                    on:input=move |ev| editor.label.set(event_target_value(&ev))/>
                <input class="wide" placeholder="aliases (title words that identify it), comma-separated"
                    prop:value=editor.aliases
                    on:input=move |ev| editor.aliases.set(event_target_value(&ev))/>
            </div>
            <div class="editor-head">
                <input class="wide"
                    placeholder="ask the LLM to rework it — e.g. add an rpm spec, labels in French"
                    prop:value=instruction
                    on:input=move |ev| instruction.set(event_target_value(&ev))/>
                <button on:click=ask_llm disabled=move || asking.get()>
                    {move || if asking.get() {
                        crate::status::llm_progress_label(
                            "Asking",
                            ask_elapsed.get(),
                            crate::status::llm_avg_ms(&status_res, "revise"),
                        )
                    } else {
                        "Ask LLM".to_string()
                    }}
                </button>
            </div>
            {move || {
                let asked: Vec<String> = chat
                    .get()
                    .iter()
                    .filter(|t| t.role == "user")
                    .map(|t| t.content.clone())
                    .collect();
                (!asked.is_empty()).then(|| view! {
                    <span class="muted">
                        {format!("conversation so far: {}", asked.join(" · "))}
                    </span>
                })
            }}
            <span class="muted">"Specs — the filters buyers get for this category:"</span>
            {move || rows.get().into_iter().map(|row| spec_row_view(row, rows)).collect_view()}
            <div class="editor-actions">
                <button on:click=add_row>"+ spec"</button>
                <button on:click=save>"Save category"</button>
                <button on:click=move |_| slot.0.set(None)>"Cancel"</button>
                <label class="spec" title="e.g. renaming 'capacity' here renames it in every category that has it">
                    <input type="checkbox" prop:checked=sync_shared
                        on:change=move |ev| sync_shared.set(event_target_checked(&ev))/>
                    "propagate spec renames to other categories"
                </label>
            </div>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
        </div>
    }
}

/// One "capacity" lives in hdd, ssd AND ram — after a save, mirror
/// label/unit/hint edits onto same-key same-kind specs elsewhere so the
/// user never repeats a rename per category. Values (enum lists) stay
/// per-category.
async fn sync_shared_specs(client: &FerretClient, saved: &Category) {
    let Ok(all) = client.categories().await else { return };
    for mut other in all {
        if other.slug == saved.slug {
            continue;
        }
        let mut changed = false;
        for spec in &mut other.specs {
            if let Some(edited) =
                saved.specs.iter().find(|s| s.key == spec.key && s.kind == spec.kind)
                && (spec.label != edited.label
                    || spec.unit != edited.unit
                    || spec.extraction_hint != edited.extraction_hint)
            {
                spec.label = edited.label.clone();
                spec.unit = edited.unit.clone();
                spec.extraction_hint = edited.extraction_hint.clone();
                changed = true;
            }
        }
        if changed {
            let _ = client.upsert_category(&other).await;
        }
    }
}

fn spec_row_view(row: SpecRow, rows: RwSignal<Vec<SpecRow>>) -> impl IntoView {
    let id = row.id;
    let kind = row.kind;
    let is = move |k: &'static str| move || kind.get() == k;
    view! {
        <div class="spec-row">
            <input class="narrow" placeholder="key" prop:value=row.key
                on:input=move |ev| row.key.set(event_target_value(&ev))/>
            <input placeholder="label" prop:value=row.label
                on:input=move |ev| row.label.set(event_target_value(&ev))/>
            <select on:change=move |ev| kind.set(event_target_value(&ev))>
                <option value="number" selected=is("number")>"number"</option>
                <option value="enum" selected=is("enum")>"enum"</option>
                <option value="boolean" selected=is("boolean")>"boolean"</option>
            </select>
            {move || (kind.get() == "number").then(|| view! {
                <input class="narrow" placeholder="unit (GB…)" prop:value=row.unit
                    on:input=move |ev| row.unit.set(event_target_value(&ev))/>
            })}
            {move || (kind.get() == "enum").then(|| view! {
                <input class="wide" placeholder="allowed values, comma-separated" prop:value=row.values
                    on:input=move |ev| row.values.set(event_target_value(&ev))/>
            })}
            {move || (kind.get() == "boolean").then(|| view! {
                <input placeholder="title keywords meaning yes" prop:value=row.hint
                    on:input=move |ev| row.hint.set(event_target_value(&ev))/>
            })}
            <button class="danger"
                on:click=move |_| rows.update(|r| r.retain(|s| s.id != id))>
                "✕"
            </button>
        </div>
    }
}

fn category_row(category: Category) -> impl IntoView {
    let client: FerretClient = expect_context();
    let version: DataVersion = expect_context();
    let slot: EditorSlot = expect_context();
    let proposed = category.status == CategoryStatus::Proposed;

    let specs: Vec<String> = category
        .specs
        .iter()
        .map(|s| {
            let detail = match s.kind {
                SpecKind::Number => s.unit.clone().unwrap_or_else(|| "number".into()),
                SpecKind::Enum => format!("{} values", s.allowed_values.len()),
                SpecKind::Boolean => "yes/no".into(),
            };
            format!("{} ({detail})", s.label)
        })
        .collect();

    let approve = {
        let client = client.clone();
        let category = category.clone();
        move |_| {
            let mut approved = category.clone();
            approved.status = CategoryStatus::Active;
            let client = client.clone();
            spawn_local(async move {
                let _ = client.upsert_category(&approved).await;
                version.0.update(|v| *v += 1);
            });
        }
    };
    let edit = {
        let category = category.clone();
        move |_| slot.0.set(Some(Editor::open(Some(category.clone()))))
    };
    let remove = {
        let client = client.clone();
        let slug = category.slug.clone();
        move |_| {
            let client = client.clone();
            let slug = slug.clone();
            spawn_local(async move {
                let _ = client.delete_category(&slug).await;
                version.0.update(|v| *v += 1);
            });
        }
    };

    view! {
        <li class="watch" class:inactive=proposed>
            <div class="watch-main">
                <span class="watch-name">
                    {category.label.clone()}
                    " "
                    {proposed.then(|| view! { <span class="badge warn">"proposed"</span> })}
                </span>
                <span class="muted">
                    {format!(
                        "aliases: {} · filters: {}",
                        if category.aliases.is_empty() { "—".into() } else { category.aliases.join(", ") },
                        if specs.is_empty() { "—".into() } else { specs.join(" · ") },
                    )}
                </span>
            </div>
            <div class="watch-actions">
                {proposed.then(|| view! { <button on:click=approve.clone()>"approve"</button> })}
                <button on:click=edit>"edit"</button>
                <button class="danger" on:click=remove>"delete"</button>
            </div>
        </li>
    }
}
