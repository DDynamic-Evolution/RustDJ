pub const COUNT: usize = 4;
/// Internal deck IDs are zero based; presentation is 1, 3, 4, 2.
pub const ORDER: [usize; COUNT] = [0, 2, 3, 1];
pub const COLORS: [eframe::egui::Color32; COUNT] = [
    eframe::egui::Color32::from_rgb(174, 119, 255), // 1: purple
    eframe::egui::Color32::from_rgb(247, 202, 85),  // 2: yellow
    eframe::egui::Color32::from_rgb(247, 114, 188), // 3: pink
    eframe::egui::Color32::from_rgb(79, 211, 202),  // 4: turquoise
];

pub fn sync_reference(
    follower: usize,
    loaded: [bool; COUNT],
    playing: [bool; COUNT],
    gains: [f32; COUNT],
) -> Option<usize> {
    (0..COUNT)
        .filter(|i| *i != follower && loaded[*i])
        .max_by(|a, b| {
            playing[*a]
                .cmp(&playing[*b])
                .then_with(|| gains[*a].total_cmp(&gains[*b]))
                .then_with(|| b.cmp(a))
        })
}
