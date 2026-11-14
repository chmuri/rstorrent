use leptos::prelude::*;

#[component]
pub fn Skeleton(#[prop(default = "h-4 w-full")] class: &'static str) -> impl IntoView {
    view! { <div class=format!("rounded-lg bg-white/[0.06] animate-pulse {class}") /> }
}
