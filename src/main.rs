#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1680.0, 1000.0])
            .with_min_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustDJ 0.18",
        options,
        Box::new(|cc| Ok(Box::new(rustdj::app::RustDj::new(cc)))),
    )
}
