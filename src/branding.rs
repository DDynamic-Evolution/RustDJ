//! Embedded identity assets; no external files are required at runtime.
use eframe::egui;

const ICON: &[u8] = include_bytes!("../assets/branding/rustdj-window-icon.png");
const LOGO: &[u8] = include_bytes!("../assets/branding/rustdj-logo.png");

pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(ICON).expect("valid embedded RustDJ icon")
}

pub fn logo(ctx: &egui::Context) -> egui::TextureHandle {
    let image = eframe::icon_data::from_png_bytes(LOGO).expect("valid embedded RustDJ logo");
    ctx.load_texture(
        "rustdj-logo",
        egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        ),
        egui::TextureOptions::LINEAR,
    )
}
