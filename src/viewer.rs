pub struct Viewer {
    color_bg: egui::Color32,
}

use crate::app::{ControlFlow, GlobalState};

impl Viewer {
    pub fn new() -> Self {
        Self {
            color_bg: egui::Color32::GRAY,
        }
    }

    pub fn display(
        &mut self,
        ctx: &egui::Context,
        global_state: &mut GlobalState<'_>,
        wgpu: crate::app::WgpuRef<'_>,
        proxy: &egui_winit::winit::event_loop::EventLoopProxy<crate::app::UserEvent>,
    ) -> Option<ControlFlow> {
        if let Some((name, texture, cleanup)) = global_state.file_rx.try_recv().ok() {
            let view = crate::image::ImageView::from_texture(&texture, wgpu.renderer, &wgpu.device);
            global_state.files.push((name, texture, view, cleanup))
        }

        egui::CentralPanel::default()
            .frame(egui::Frame {
                fill: self.color_bg,
                ..Default::default()
            })
            .show(ctx, |ui| {
                egui::widgets::color_picker::color_picker_color32(
                    ui,
                    &mut self.color_bg,
                    egui::color_picker::Alpha::Opaque,
                );

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

                        let texture = crate::image::avif::AvifTextureData::display(
                            data,
                            notif_rx,
                            &wgpu.device,
                            wgpu.queue,
                            &proxy,
                        );
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

                        let texture = crate::image::webp::WebpTextureData::display(
                            data,
                            notif_rx,
                            &wgpu.device,
                            wgpu.queue,
                            &proxy,
                        );
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

                        let texture = crate::image::gif::GifTextureData::display(
                            data,
                            notif_rx,
                            &wgpu.device,
                            wgpu.queue,
                            &proxy,
                        );
                        let _ = file_tx.send((name, texture.into(), notif_tx));
                    });
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    use crate::image::Texture;

                    for (_, _, mut removed, _) in
                        global_state
                            .files
                            .extract_if(.., |(name, texture, view, _)| {
                                let size = texture.metadata().dimensions_vec2();
                                ui.label(&format!("{name}, {size}"));

                                if texture.loaded() {
                                    view.show(ui, size);
                                } else {
                                    ui.label("loading");
                                }

                                ui.button("remove").clicked()
                            })
                    {
                        removed.free_textures(wgpu.renderer);
                    }
                });
            });
        None
    }
}
