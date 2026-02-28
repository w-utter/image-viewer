pub struct Metadata {
    pub width: u32,
    pub height: u32,
}

impl Metadata {
    pub fn dimensions_vec2(&self) -> egui::Vec2 {
        egui::Vec2::new(self.width as _, self.height as _)
    }
}
