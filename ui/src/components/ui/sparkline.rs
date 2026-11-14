use std::sync::atomic::{AtomicU64, Ordering};

use leptos::prelude::*;

static GRADIENT_SEQ: AtomicU64 = AtomicU64::new(0);

const PAD: f64 = 2.0;

fn next_gradient_id() -> String {
    format!("rt-grad-{}", GRADIENT_SEQ.fetch_add(1, Ordering::Relaxed))
}

fn sanitize(data: &[f64]) -> Vec<f64> {
    let mut values: Vec<f64> = data
        .iter()
        .map(|v| if v.is_finite() { *v } else { 0.0 })
        .collect();
    // Duplicate a lone sample so it renders as a flat line instead of a dot.
    if values.len() == 1 {
        if let Some(first) = values.first().copied() {
            values.push(first);
        }
    }
    values
}

fn bounds(values: &[f64]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in values {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    if min > max {
        (0.0, 0.0)
    } else {
        (min, max)
    }
}

fn points(values: &[f64], width: f64, height: f64, min: f64, max: f64) -> Vec<(f64, f64)> {
    let n = values.len();
    if n == 0 {
        return Vec::new();
    }
    let inner_w = (width - 2.0 * PAD).max(0.0);
    let inner_h = (height - 2.0 * PAD).max(0.0);
    let span = max - min;
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = if n == 1 {
                width / 2.0
            } else {
                PAD + inner_w * (i as f64 / (n - 1) as f64)
            };
            // Flat series: center the line instead of dividing by a zero span.
            let y = if span <= f64::EPSILON {
                height / 2.0
            } else {
                PAD + inner_h * (1.0 - (v - min) / span)
            };
            (x, y)
        })
        .collect()
}

fn smooth_line(pts: &[(f64, f64)]) -> String {
    let Some(&(x0, y0)) = pts.first() else {
        return String::new();
    };
    let mut d = format!("M {x0:.2} {y0:.2}");
    for window in pts.windows(3) {
        if let [_, (cx, cy), (nx, ny)] = window {
            let mx = (cx + nx) / 2.0;
            let my = (cy + ny) / 2.0;
            d.push_str(&format!(" Q {cx:.2} {cy:.2} {mx:.2} {my:.2}"));
        }
    }
    if let Some(&(lx, ly)) = pts.last() {
        d.push_str(&format!(" Q {lx:.2} {ly:.2} {lx:.2} {ly:.2}"));
    }
    d
}

fn area_path(line: &str, pts: &[(f64, f64)], height: f64) -> String {
    let (Some(&(fx, _)), Some(&(lx, _))) = (pts.first(), pts.last()) else {
        return String::new();
    };
    format!("{line} L {lx:.2} {height:.2} L {fx:.2} {height:.2} Z")
}

#[component]
pub fn Sparkline(
    data: Signal<Vec<f64>>,
    #[prop(default = "#6366f1")] color: &'static str,
    #[prop(default = 40.0)] height: f64,
    #[prop(default = 120.0)] width: f64,
) -> impl IntoView {
    let gradient_id = next_gradient_id();
    let gradient_fill = format!("url(#{gradient_id})");

    let paths = Memo::new(move |_| {
        let values = sanitize(&data.get());
        let (min, max) = bounds(&values);
        let pts = points(&values, width, height, min, max);
        let line = smooth_line(&pts);
        let area = area_path(&line, &pts, height);
        (line, area)
    });

    view! {
        <svg
            class="block w-full"
            width=width
            height=height
            viewBox=format!("0 0 {width} {height}")
            preserveAspectRatio="none"
            fill="none"
            aria-hidden="true"
        >
            <defs>
                <linearGradient id=gradient_id x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color=color stop-opacity="0.25" />
                    <stop offset="100%" stop-color=color stop-opacity="0" />
                </linearGradient>
            </defs>
            <path d=move || paths.get().1 fill=gradient_fill />
            <path
                d=move || paths.get().0
                stroke=color
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                fill="none"
            />
        </svg>
    }
}

#[component]
pub fn AreaChart(
    data: Signal<Vec<f64>>,
    #[prop(default = "#6366f1")] color: &'static str,
    #[prop(default = 220.0)] height: f64,
    #[prop(optional)] secondary: Option<Signal<Vec<f64>>>,
    #[prop(default = "#10b981")] secondary_color: &'static str,
) -> impl IntoView {
    // Fixed internal coordinate system; the SVG stretches to its container.
    const WIDTH: f64 = 600.0;

    let primary_id = next_gradient_id();
    let secondary_id = next_gradient_id();
    let primary_fill = format!("url(#{primary_id})");
    let secondary_fill = format!("url(#{secondary_id})");

    let paths = Memo::new(move |_| {
        let primary = sanitize(&data.get());
        let secondary_values = secondary.map(|s| sanitize(&s.get())).unwrap_or_default();
        let (mut min, mut max) = bounds(&primary);
        if !secondary_values.is_empty() {
            let (smin, smax) = bounds(&secondary_values);
            min = min.min(smin);
            max = max.max(smax);
        }
        let p_pts = points(&primary, WIDTH, height, min, max);
        let s_pts = points(&secondary_values, WIDTH, height, min, max);
        let p_line = smooth_line(&p_pts);
        let p_area = area_path(&p_line, &p_pts, height);
        let s_line = smooth_line(&s_pts);
        let s_area = area_path(&s_line, &s_pts, height);
        (p_line, p_area, s_line, s_area)
    });

    view! {
        <svg
            class="block w-full"
            width="100%"
            height=height
            viewBox=format!("0 0 {WIDTH} {height}")
            preserveAspectRatio="none"
            fill="none"
            aria-hidden="true"
        >
            <defs>
                <linearGradient id=primary_id x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color=color stop-opacity="0.25" />
                    <stop offset="100%" stop-color=color stop-opacity="0" />
                </linearGradient>
                <linearGradient id=secondary_id x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color=secondary_color stop-opacity="0.25" />
                    <stop offset="100%" stop-color=secondary_color stop-opacity="0" />
                </linearGradient>
            </defs>
            <path d=move || paths.get().3 fill=secondary_fill />
            <path
                d=move || paths.get().2
                stroke=secondary_color
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                vector-effect="non-scaling-stroke"
                fill="none"
            />
            <path d=move || paths.get().1 fill=primary_fill />
            <path
                d=move || paths.get().0
                stroke=color
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                vector-effect="non-scaling-stroke"
                fill="none"
            />
        </svg>
    }
}
