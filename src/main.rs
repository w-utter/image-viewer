mod app;
mod image;
mod viewer;

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (ev_loop, mut window_manager) =
        rt.block_on(app::WindowHandler::new(app::GlobalState::new(&rt)));

    ev_loop.run_app(&mut window_manager).unwrap();
}
