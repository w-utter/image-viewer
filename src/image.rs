pub mod avif;
pub mod webp;
pub mod cleanup;
pub mod metadata;
pub mod gif;

use egui_wgpu::wgpu;

pub trait Texture {
    fn loaded(&self) -> bool;
    fn metadata(&self) -> &metadata::Metadata;
}

pub enum ImageTexture {
    Avif(avif::AvifTexture),
    WebP(webp::WebpTexture),
    Gif(gif::GifTexture),
}

impl Texture for ImageTexture {
    fn loaded(&self) -> bool {
        match self {
            Self::Avif(a) => a.loaded(),
            Self::WebP(w) => w.loaded(),
            Self::Gif(g) => g.loaded(),
        }
    }

    fn metadata(&self) -> &metadata::Metadata {
        match self {
            Self::Avif(a) => a.metadata(),
            Self::WebP(w) => w.metadata(),
            Self::Gif(g) => g.metadata(),
        }
    }
}

impl From<avif::AvifTexture> for ImageTexture {
    fn from(f: avif::AvifTexture) -> Self {
        Self::Avif(f)
    }
}

impl From<webp::WebpTexture> for ImageTexture {
    fn from(f: webp::WebpTexture) -> Self {
        Self::WebP(f)
    }
}

impl From<gif::GifTexture> for ImageTexture {
    fn from(f: gif::GifTexture) -> Self {
        Self::Gif(f)
    }
}

pub enum ImageView {
    Avif(avif::EguiAvifView),
    WebP(webp::EguiWebpView),
    Gif(gif::EguiGifView),
}

impl ImageView {
    pub fn from_texture(texture: &ImageTexture, renderer: &mut egui_wgpu::Renderer, device: &wgpu::Device) -> Self {
        match texture {
            ImageTexture::Avif(a) => ImageView::Avif(avif::EguiAvifView::from_texture(a, renderer, device)),
            ImageTexture::WebP(w) => ImageView::WebP(webp::EguiWebpView::from_texture(w, renderer, device)),
            ImageTexture::Gif(g) => ImageView::Gif(gif::EguiGifView::from_texture(g, renderer, device)),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        match self {
            Self::Avif(a) => a.show(ui, size),
            Self::WebP(w) => w.show(ui, size),
            Self::Gif(g) => g.show(ui, size),
        }
    }
}
