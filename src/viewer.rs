use std::sync::Arc;

pub struct Viewer {
    img: Option<egui::TextureHandle>,
    color_bg: egui::Color32,
}

use crate::app::{ControlFlow, GlobalState};

impl Viewer {
    pub fn new() -> Self {
        Self {
            img: None,
            color_bg: egui::Color32::GRAY,
        }
    }

    pub fn display(&mut self, ctx: &egui::Context, global_state: &mut GlobalState<'_>, wgpu: crate::app::WgpuRef<'_>, proxy: &egui_winit::winit::event_loop::EventLoopProxy<crate::app::UserEvent>) -> Option<ControlFlow> {
        if let Some((name, texture, cleanup)) = global_state.file_rx.try_recv().ok() {
            use egui_wgpu::wgpu;
            //let view = texture.inner.create_view(&Default::default());
            //let id = wgpu.renderer.register_native_texture_with_sampler_options(&wgpu.device, &view, wgpu::SamplerDescriptor { lod_min_clamp: 0., lod_max_clamp: 0., min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default()});

            let view = crate::image::ImageView::from_texture(&texture, wgpu.renderer, &wgpu.device);
            //let avif_view = crate::image::avif::EguiAvifView::from_texture(&texture, wgpu.renderer, &wgpu.device);

            global_state.files.push((name, texture, view, cleanup))
        }

        egui::CentralPanel::default().frame(egui::Frame { fill: self.color_bg, ..Default::default() }).show(ctx, |ui| {
            egui::widgets::color_picker::color_picker_color32(ui, &mut self.color_bg, egui::color_picker::Alpha::Opaque);

            if ui.button("open avif").clicked() {
                let task = rfd::AsyncFileDialog::new()
                    .add_filter("avif files", &["avif"])
                    .pick_file();
                
                let file_tx = global_state.file_tx.clone();
                let proxy = proxy.clone();
                global_state.rt.spawn(async move {
                    let Some(file) = task.await else {
                        return;
                    };

                    let name = file.file_name();

                    let (notif_tx, notif_rx) = crate::image::cleanup::channel();
                    let data = crate::image::avif::load_avif(file).await;

                    let texture = crate::image::avif::AvifTextureData::display(data, notif_rx, &wgpu.device, wgpu.queue, &proxy);
                    let _ = file_tx.send((name, texture.into(), notif_tx));
                });
            } else if ui.button("open webp").clicked() {
                let task = rfd::AsyncFileDialog::new()
                    .add_filter("webp files", &["webp"])
                    .pick_file();
                
                let file_tx = global_state.file_tx.clone();
                let proxy = proxy.clone();
                global_state.rt.spawn(async move {
                    let Some(file) = task.await else {
                        return;
                    };

                    let name = file.file_name();

                    let (notif_tx, notif_rx) = crate::image::cleanup::channel();
                    let data = crate::image::webp::load_webp(file).await;

                    let texture = crate::image::webp::WebpTextureData::display(data, notif_rx, &wgpu.device, wgpu.queue, &proxy);
                    let _ = file_tx.send((name, texture.into(), notif_tx));
                });
            } else if ui.button("open gif").clicked() {
                let task = rfd::AsyncFileDialog::new()
                    .add_filter("gif files", &["gif"])
                    .pick_file();
                
                let file_tx = global_state.file_tx.clone();
                let proxy = proxy.clone();
                global_state.rt.spawn(async move {
                    let Some(file) = task.await else {
                        return;
                    };

                    let name = file.file_name();

                    let (notif_tx, notif_rx) = crate::image::cleanup::channel();
                    let data = crate::image::gif::load_gif(file).await;

                    let texture = crate::image::gif::GifTextureData::display(data, notif_rx, &wgpu.device, wgpu.queue, &proxy);
                    let _ = file_tx.send((name, texture.into(), notif_tx));
                });
            }


            egui::ScrollArea::vertical().show(ui, |ui| {
                /*
                if self.img.is_none() {
                    use image::ImageDecoder;
                    let a = image::codecs::avif::AvifDecoder::new(std::fs::File::open("./HeWasSuspended-4x.avif").unwrap()).unwrap();
                    let mut buf = vec![0; a.total_bytes() as usize];
                    let (width, height) = a.dimensions();
                    a.read_image(&mut buf).unwrap();
                    let img = egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], &buf);
                    //self.img = Some(img.into())
                    let texture = ctx.load_texture("a", img, egui::TextureOptions::NEAREST);
                    self.img = Some(texture);
                }

                if let Some(texture) = self.img.as_ref() {
                    let size = texture.size_vec2();
                    let sized_texture = egui::load::SizedTexture::new(texture, size);
                    ui.add(egui::Image::new(sized_texture).fit_to_exact_size(size));
                }
                */

                use crate::image::Texture;

                global_state.files.retain_mut(|(name, texture, view, _)| {
                    let size = texture.metadata().dimensions_vec2();
                    ui.label(&format!("{name}, {size}"));

                    if texture.loaded() {
                        view.show(ui, size);
                    } else {
                        ui.label("loading");
                    }
                    //let img = egui::Image::new((*id, egui::Vec2::new(*width as _, *height as _)));
                    //ui.add(img);

                    !ui.button("remove").clicked()
                });
            });
        });
        None
    }
}
