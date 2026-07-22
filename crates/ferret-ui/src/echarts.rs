//! Minimal bindings to the vendored Apache ECharts bundle (loaded globally
//! from index.html) plus a reusable `ChartCanvas` — ported from chaos,
//! trimmed (no connect-groups, no window tooltip formatters). Options are
//! built as JSON with serde_json and parsed on the JS side.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub type EChart;

    /// `echarts.init(el)` — one chart instance bound to a DOM element.
    #[wasm_bindgen(js_namespace = echarts, catch)]
    pub fn init(el: &web_sys::HtmlElement) -> Result<EChart, JsValue>;

    /// `chart.setOption(option, opts)` — `replaceMerge: ["series"]` so
    /// series dropped from the option are actually removed.
    #[wasm_bindgen(method, js_name = setOption, catch)]
    pub fn set_option_with(this: &EChart, option: &JsValue, opts: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, js_name = dispatchAction, catch)]
    pub fn dispatch_action(this: &EChart, action: &JsValue) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn resize(this: &EChart) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn dispose(this: &EChart) -> Result<(), JsValue>;

    /// The chart's zrender handle — raw canvas events (dblclick).
    pub type ZRender;

    #[wasm_bindgen(method, js_name = getZr)]
    pub fn get_zr(this: &EChart) -> ZRender;

    #[wasm_bindgen(method)]
    pub fn on(this: &ZRender, event: &str, handler: &js_sys::Function);
}

// wasm-bindgen doesn't derive Clone for extern types; ChartCanvas caches
// the instance in a StoredValue, which needs it.
impl Clone for EChart {
    fn clone(&self) -> Self {
        use wasm_bindgen::JsCast;
        JsValue::from(self).unchecked_into()
    }
}

/// Parse a JSON string into a JS object (NULL on bad input — the chart
/// just stays empty).
pub fn json(raw: &str) -> JsValue {
    js_sys::JSON::parse(raw).unwrap_or(JsValue::NULL)
}

/// The "inside" dataZoom fragment: wheel zooms around the cursor, drag
/// pans; wheel never pans so page scroll stays predictable.
pub(crate) fn inside_zoom() -> serde_json::Value {
    serde_json::json!([{
        "type": "inside",
        "xAxisIndex": 0,
        "zoomOnMouseWheel": true,
        "moveOnMouseMove": true,
        "moveOnMouseWheel": false,
    }])
}

/// A CSS custom property from the active theme (empty if unset). DOM-only.
fn css_var(name: &str) -> String {
    web_sys::window()
        .and_then(|w| {
            let body = w.document()?.body()?;
            w.get_computed_style(&body).ok().flatten()
        })
        .and_then(|style| style.get_property_value(name).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// Theme colours injected into option builders so those stay pure.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChartColors {
    pub text: String,
    pub muted: String,
    pub line: String,
    pub panel: String,
    pub accent: String,
}

impl ChartColors {
    pub(crate) fn from_theme() -> Self {
        Self {
            text: css_var("--text"),
            muted: css_var("--muted"),
            line: css_var("--line"),
            panel: css_var("--panel"),
            accent: css_var("--accent"),
        }
    }
}

fn zoom_to(chart: &EChart, (start, end): (f64, f64)) {
    let _ = chart.dispatch_action(&json(&format!(
        r#"{{"type":"dataZoom","start":{start},"end":{end}}}"#
    )));
}

/// A mounted ECharts instance: init, reactive option updates, dblclick
/// zoom reset, window resize, disposal. `class` sizes the container.
#[component]
pub fn ChartCanvas(
    option: Callback<(), serde_json::Value>,
    class: &'static str,
) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    let chart = StoredValue::new_local(None::<EChart>);
    let dblclick = StoredValue::new_local(None::<Closure<dyn FnMut()>>);
    let failed = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
        let instance = match chart.get_value() {
            Some(instance) => instance,
            None => match init(&el) {
                Ok(instance) => {
                    let reset = {
                        let instance = instance.clone();
                        Closure::wrap(Box::new(move || {
                            zoom_to(&instance, (0.0, 100.0));
                        }) as Box<dyn FnMut()>)
                    };
                    {
                        use wasm_bindgen::JsCast;
                        instance.get_zr().on("dblclick", reset.as_ref().unchecked_ref());
                    }
                    dblclick.set_value(Some(reset));
                    chart.set_value(Some(instance.clone()));
                    instance
                }
                Err(_) => {
                    failed.set(true);
                    return;
                }
            },
        };
        let opt = json(&option.run(()).to_string());
        let _ = instance.set_option_with(&opt, &json(r#"{"replaceMerge":["series"]}"#));
    });

    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if let Some(instance) = chart.get_value() {
            let _ = instance.resize();
        }
    });
    on_cleanup(move || {
        resize.remove();
        if let Some(instance) = chart.get_value() {
            let _ = instance.dispose();
        }
        dblclick.set_value(None);
    });

    view! {
        <div class=class node_ref=node></div>
        {move || {
            failed.get().then(|| view! {
                <p class="error">"Chart failed to load (echarts bundle missing?)"</p>
            })
        }}
    }
}
