use leptos::prelude::*;

#[component]
pub fn BentoCard(
    #[prop(into)] title: String,
    #[prop(optional)] subtitle: Option<String>,
    #[prop(default = "col-span-12")] col_span: &'static str,
    #[prop(optional)] actions: Option<AnyView>,
    children: Children,
) -> impl IntoView {
    let class = format!(
        "glass-surface relative overflow-hidden rounded-2xl bg-zinc-900/50 backdrop-blur-xl \
         border border-white/[0.08] shadow-[0_8px_32px_rgba(0,0,0,0.4)] transition-all \
         duration-200 hover:border-white/[0.14] {col_span}"
    );

    view! {
        <section class=class>
            <div class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-white/10 to-transparent" />
            <header class="flex items-start justify-between gap-3 px-5 pt-4 pb-2">
                <div class="min-w-0">
                    <h3 class="text-sm font-semibold tracking-tight text-zinc-100">{title}</h3>
                    {subtitle.map(|subtitle| {
                        view! { <p class="mt-0.5 text-xs text-zinc-500">{subtitle}</p> }
                    })}
                </div>
                {actions.map(|actions| {
                    view! { <div class="flex shrink-0 items-center gap-2">{actions}</div> }
                })}
            </header>
            <div class="px-5 pb-5">{children()}</div>
        </section>
    }
}
