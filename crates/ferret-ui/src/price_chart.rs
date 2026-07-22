//! Price-history chart for one watch, chaos-temperature-chart style:
//! daily best (min) and median price over every matched deal, with
//! hover tooltip and wheel zoom. The option builder is pure JSON —
//! unit-tested off-wasm; colors are injected.

use ferret_domain::WatchPricePoint;
use leptos::prelude::*;
use uuid::Uuid;

use crate::echarts::{ChartCanvas, ChartColors, inside_zoom};

/// Build the full ECharts option. One axis, two lines: "best" (accent)
/// is the price you could actually pay that day, "median" (muted) is the
/// market. Prices in euros.
pub(crate) fn chart_option(points: &[WatchPricePoint], colors: &ChartColors) -> serde_json::Value {
    let days: Vec<&str> = points.iter().map(|p| p.day.as_str()).collect();
    let best: Vec<f64> = points.iter().map(|p| p.min_cents as f64 / 100.0).collect();
    let median: Vec<f64> = points.iter().map(|p| p.median_cents as f64 / 100.0).collect();
    serde_json::json!({
        "backgroundColor": "transparent",
        "grid": { "left": 60, "right": 16, "top": 30, "bottom": 24 },
        "legend": {
            "data": ["best", "median"],
            "textStyle": { "color": colors.muted },
            "top": 0,
        },
        "tooltip": {
            "trigger": "axis",
            "backgroundColor": colors.panel,
            "borderColor": colors.line,
            "textStyle": { "color": colors.text },
            "valueFormatter": null,
        },
        "xAxis": {
            "type": "category",
            "data": days,
            "axisLine": { "lineStyle": { "color": colors.line } },
            "axisLabel": { "color": colors.muted },
        },
        "yAxis": {
            "type": "value",
            "scale": true,
            "axisLabel": { "color": colors.muted, "formatter": "{value} €" },
            "splitLine": { "lineStyle": { "color": colors.line } },
        },
        "dataZoom": inside_zoom(),
        "series": [
            {
                "name": "best",
                "type": "line",
                "data": best,
                "showSymbol": false,
                "lineStyle": { "width": 2, "color": colors.accent },
                "itemStyle": { "color": colors.accent },
            },
            {
                "name": "median",
                "type": "line",
                "data": median,
                "showSymbol": false,
                "lineStyle": { "width": 2, "color": colors.muted },
                "itemStyle": { "color": colors.muted },
            },
        ],
    })
}

#[component]
pub fn WatchPriceChart(watch_id: Uuid) -> impl IntoView {
    let client: ferret_client::FerretClient = expect_context();
    let points = LocalResource::new(move || {
        let client = client.clone();
        async move { client.watch_prices(watch_id).await }
    });

    view! {
        {move || match points.get() {
            None => view! { <p class="muted">"Loading price history…"</p> }.into_any(),
            Some(Err(e)) => {
                view! { <p class="error">{format!("history unavailable: {e}")}</p> }.into_any()
            }
            Some(Ok(pts)) if pts.len() < 2 => view! {
                <p class="muted">
                    "Not enough history yet — the chart appears after a couple of days \
                     of observations."
                </p>
            }
            .into_any(),
            Some(Ok(pts)) => {
                let option = Callback::new(move |()| {
                    chart_option(&pts, &ChartColors::from_theme())
                });
                view! { <ChartCanvas option=option class="chart"/> }.into_any()
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_builds_two_series_over_days() {
        let points = vec![
            WatchPricePoint {
                day: "2026-07-20".into(),
                min_cents: 100_000,
                median_cents: 150_000,
                count: 3,
            },
            WatchPricePoint {
                day: "2026-07-21".into(),
                min_cents: 90_000,
                median_cents: 149_000,
                count: 4,
            },
        ];
        let option = chart_option(&points, &ChartColors::default());
        assert_eq!(option["xAxis"]["data"][1], "2026-07-21");
        assert_eq!(option["series"][0]["name"], "best");
        assert_eq!(option["series"][0]["data"][1], 900.0);
        assert_eq!(option["series"][1]["data"][0], 1500.0);
        assert_eq!(option["series"].as_array().unwrap().len(), 2, "one axis, two lines");
        assert!(option["yAxis"]["scale"].as_bool().unwrap(), "zoomed to data range");
    }
}
