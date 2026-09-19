// Real component tree (Header/TimeControls/RiverLanes/LiveFeed/WindowCounts/
// EndpointsChart) and fetch orchestration land in phases 4-5. This scaffold
// just proves the Leptos CSR + Trunk build pipeline works end to end.
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Info);
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    view! { <p>"aimon-ui scaffold"</p> }
}
