use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[component]
pub fn Button(
    #[prop(into)] label: String,
    #[prop(default = ButtonVariant::Primary)] variant: ButtonVariant,
    #[prop(into, optional)] on_click: Option<Callback<()>>,
    #[prop(optional)] icon: Option<AnyView>,
    #[prop(default = false)] disabled: bool,
) -> impl IntoView {
    let base = match variant {
        ButtonVariant::Primary => {
            "inline-flex items-center justify-center gap-2 rounded-xl bg-[var(--accent)] px-4 py-2 \
             text-sm font-medium text-white shadow-[inset_0_1px_0_0_rgba(255,255,255,0.2)] \
             hover:brightness-110 active:scale-[0.98] transition-all cursor-pointer"
        }
        ButtonVariant::Secondary => {
            "inline-flex items-center justify-center gap-2 rounded-xl bg-white/[0.04] \
             hover:bg-white/[0.08] border border-white/[0.08] px-3.5 py-2 text-sm font-medium \
             text-zinc-200 active:scale-[0.98] transition-all cursor-pointer"
        }
        ButtonVariant::Ghost => {
            "inline-flex items-center justify-center gap-2 rounded-lg px-2.5 py-1.5 text-sm \
             font-medium text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.04] \
             active:scale-[0.98] transition-all cursor-pointer"
        }
        ButtonVariant::Danger => {
            "inline-flex items-center justify-center gap-2 rounded-xl bg-rose-600/90 \
             hover:bg-rose-500 px-3.5 py-2 text-sm font-medium text-white active:scale-[0.98] \
             transition-all cursor-pointer"
        }
    };
    let class = if disabled {
        format!("{base} opacity-50 pointer-events-none")
    } else {
        base.to_string()
    };

    view! {
        <button
            type="button"
            class=class
            disabled=disabled
            on:click=move |_| {
                if let Some(on_click) = on_click {
                    on_click.run(());
                }
            }
        >
            {icon}
            <span>{label}</span>
        </button>
    }
}
