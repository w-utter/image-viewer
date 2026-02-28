use crate::image::cleanup::ShutdownNotif;
use crate::image::metadata::Metadata;
use egui_wgpu::wgpu;

pub struct WebpTextureData {
    parser: image_webp::WebPDecoder<std::io::Cursor<Vec<u8>>>,
}

pub struct WebpTexture {
    pub metadata: Metadata,
    pub loaded: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub inner: wgpu::Texture,
}

impl super::Texture for WebpTexture {
    fn loaded(&self) -> bool {
        self.loaded.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

pub async fn load_webp(handle: rfd::FileHandle) -> WebpTextureData {
    let bytes = handle.read().await;
    tokio::task::spawn_blocking(move || {
        let parser = image_webp::WebPDecoder::new_with_options(
            std::io::Cursor::new(bytes),
            Default::default(),
        )
        .unwrap();

        WebpTextureData { parser }
    })
    .await
    .unwrap()
}

impl WebpTextureData {
    pub fn display(
        mut self,
        shutdown_tx: ShutdownNotif,
        device: &wgpu::Device,
        queue: std::sync::Arc<wgpu::Queue>,
        proxy: &egui_winit::winit::event_loop::EventLoopProxy<crate::app::UserEvent>,
    ) -> WebpTexture {
        let (width, height) = self.parser.dimensions();
        let size = wgpu::Extent3d {
            width: width as _,
            height: height as _,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            label: None,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });

        let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::from(false));

        let texture2 = texture.clone();
        let frame_loaded = loaded.clone();
        let proxy = proxy.clone();
        tokio::task::spawn(async move {
            let texture = texture2;

            let premul_alpha = |bytes: &mut [u8]| {
                // premultiply by alpha
                for rgba in bytes.chunks_exact_mut(4) {
                    let a = (rgba[3] as f32) / 255.;
                    rgba[0] = ((rgba[0] as f32) * a) as u8;
                    rgba[1] = ((rgba[1] as f32) * a) as u8;
                    rgba[2] = ((rgba[2] as f32) * a) as u8;
                }
            };

            if self.parser.is_animated() {
                let mut cached = vec![];

                let max_idx = self.parser.num_frames() as usize;

                let mut buf = vec![0; (width as usize) * (height as usize) * 4];
                let dur = self.parser.read_frame(&mut buf).unwrap();

                premul_alpha(&mut buf);

                wgpu_write_webp_texture(&queue, &texture, width, height, &buf);
                frame_loaded.store(true, std::sync::atomic::Ordering::SeqCst);

                let repaint_info = egui::RequestRepaintInfo {
                    viewport_id: egui::ViewportId::ROOT,
                    delay: std::time::Duration::ZERO,
                    current_cumulative_pass_nr: 0,
                };

                let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

                cached.push((dur, buf));

                use std::time::Duration;
                if shutdown_tx.sleep(Duration::from_millis(dur as u64)).await {
                    return;
                }

                let mut idx = 1;

                loop {
                    if let Some((duration, cached_bytes)) = cached.get(idx) {
                        wgpu_write_webp_texture(&queue, &texture, width, height, cached_bytes);

                        let repaint_info = egui::RequestRepaintInfo {
                            viewport_id: egui::ViewportId::ROOT,
                            delay: Duration::from_millis(*duration as u64),
                            current_cumulative_pass_nr: 0,
                        };

                        let _ = proxy
                            .send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

                        if shutdown_tx
                            .sleep(Duration::from_millis(*duration as u64))
                            .await
                        {
                            return;
                        }
                        idx = (idx + 1) % max_idx;
                        continue;
                    }

                    let now = std::time::Instant::now();

                    let mut buf = vec![0; (width as usize) * (height as usize) * 4];
                    let dur = self.parser.read_frame(&mut buf).unwrap();

                    premul_alpha(&mut buf);

                    wgpu_write_webp_texture(&queue, &texture, width, height, &buf);

                    let elapsed = std::time::Instant::now().duration_since(now);
                    let wait_time = std::time::Duration::from_millis(dur as u64)
                        .checked_sub(elapsed)
                        .unwrap_or(std::time::Duration::ZERO);

                    let repaint_info = egui::RequestRepaintInfo {
                        viewport_id: egui::ViewportId::ROOT,
                        delay: Duration::from_millis(dur as u64),
                        current_cumulative_pass_nr: 0,
                    };

                    let _ =
                        proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

                    cached.push((dur, buf));

                    if shutdown_tx.sleep(wait_time).await {
                        return;
                    }

                    idx = (idx + 1) % max_idx;
                }
            } else {
                let mut buf = vec![0; (width as usize) * (height as usize) * 4];
                self.parser.read_image(&mut buf).unwrap();

                premul_alpha(&mut buf);

                wgpu_write_webp_texture(&queue, &texture, width, height, &buf);
                frame_loaded.store(true, std::sync::atomic::Ordering::SeqCst);

                let repaint_info = egui::RequestRepaintInfo {
                    viewport_id: egui::ViewportId::ROOT,
                    delay: std::time::Duration::ZERO,
                    current_cumulative_pass_nr: 0,
                };

                let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));
            }
        });

        WebpTexture {
            inner: texture,
            loaded,
            metadata: Metadata { width, height },
        }
    }
}

pub fn wgpu_write_webp_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    data: &[u8],
) {
    /*
    {
        let clear = vec![0; (width * height * 4) as usize];

        let clear_size = wgpu::Extent3d {
            width: width as _,
            height: height as _,
            depth_or_array_layers: 1,
        };

        let clear_origin = wgpu::Origin3d::ZERO;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: clear_origin,
                aspect: wgpu::TextureAspect::All,
            },
            &clear,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * (width as u32)),
                rows_per_image: Some(height as _),
            },
            clear_size,
        );
    }
    */

    let size = wgpu::Extent3d {
        width: width as _,
        height: height as _,
        depth_or_array_layers: 1,
    };

    let origin = wgpu::Origin3d::ZERO;

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * (width as u32)),
            rows_per_image: Some(height as _),
        },
        size,
    );
}

pub struct EguiWebpView {
    pub inner: egui::TextureId,
}

impl EguiWebpView {
    pub fn from_texture(
        texture: &WebpTexture,
        renderer: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
    ) -> Self {
        let view = texture.inner.create_view(&Default::default());
        let inner = renderer.register_native_texture_with_sampler_options(
            device,
            &view,
            wgpu::SamplerDescriptor {
                lod_min_clamp: 0.,
                lod_max_clamp: 0.,
                min_filter: wgpu::FilterMode::Linear,
                mag_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            },
        );

        Self { inner }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        let img = egui::Image::new((self.inner, size));
        ui.add(img);
    }

    pub fn free_textures(&mut self, renderer: &mut egui_wgpu::Renderer) {
        renderer.free_texture(&self.inner)
    }
}
