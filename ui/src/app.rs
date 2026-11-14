use leptos::prelude::*;
use leptos_router::components::{Redirect, Route, Router, Routes};
use leptos_router::path;

use crate::components::layout::AppShell;
use crate::pages::*;
use crate::state::*;

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();
    provide_context(state);
    crate::state::api::spawn_backend_bridge(state);

    view! {
        <Router>
            <AppShell>
                <Routes fallback=|| view! { <Redirect path="/"/> }>
                    <Route path=path!("/") view=DashboardPage/>
                    <Route path=path!("/explorer") view=ExplorerPage/>
                    <Route path=path!("/analytics") view=AnalyticsPage/>
                    <Route path=path!("/settings") view=SettingsPage/>
                </Routes>
            </AppShell>
        </Router>
    }
}
