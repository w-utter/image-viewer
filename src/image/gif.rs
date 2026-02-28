use egui_wgpu::wgpu;
use crate::image::metadata::Metadata;
use crate::image::cleanup::ShutdownNotif;

pub struct GifTextureData {
    parser: gif::Decoder<std::io::Cursor<Vec<u8>>>,
}

pub async fn load_gif(handle: rfd::FileHandle) -> GifTextureData {
    let bytes = handle.read().await;
    tokio::task::spawn_blocking(move || {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);

        let parser = options.read_info(std::io::Cursor::new(bytes)).unwrap();

        GifTextureData {
            parser,
        }
    }).await.unwrap()
}

#[derive(Clone, Copy)]
struct GifFrameInfo {
    width: u32,
    height: u32,
    top: u32,
    left: u32,
    duration: u16,
    disposal: gif::DisposalMethod,
}

impl GifTextureData {
    pub fn display(mut self, shutdown_tx: ShutdownNotif, device: &wgpu::Device, queue: std::sync::Arc<wgpu::Queue>, proxy: &egui_winit::winit::event_loop::EventLoopProxy<crate::app::UserEvent>) -> GifTexture {
        let width = self.parser.width() as u32;
        let height = self.parser.height() as u32;

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

            let mut cached = vec![];

            let frame = self.parser.read_next_frame().unwrap().unwrap();
            let mut buf = frame.buffer.clone().into_owned();

            premul_alpha(&mut buf);

            let dur = frame.delay * 10;
            let frame_info = GifFrameInfo { width, height, top: frame.top as u32, left: frame.left as u32, duration: dur, disposal: frame.dispose};

            wgpu_write_gif_texture(&queue, &texture, &frame_info, width, height, &buf);

            frame_loaded.store(true, std::sync::atomic::Ordering::SeqCst);

            let repaint_info = egui::RequestRepaintInfo {
                viewport_id: egui::ViewportId::ROOT,
                delay: std::time::Duration::ZERO,
                current_cumulative_pass_nr: 0,
            };

            let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

            cached.push((frame_info, buf));

            use std::time::Duration;
            if shutdown_tx.sleep(Duration::from_millis(dur as u64)).await {
                return;
            }

            loop {
                let now = std::time::Instant::now();
                let Some(frame) = self.parser.read_next_frame().unwrap() else {
                    break;
                };

                let mut buf = frame.buffer.clone().into_owned();
                premul_alpha(&mut buf);

                let dur = frame.delay * 10;

                let frame_info = GifFrameInfo { width: frame.width as u32, height: frame.height as u32, top: frame.top as u32, left: frame.left as u32, duration: dur, disposal: frame.dispose};
                let (prev_info, prev_buf) = cached.last().unwrap();

                if matches!(prev_info.disposal, gif::DisposalMethod::Keep) {
                    blend_alpha(&mut buf, &frame_info, prev_buf, prev_info);
                }

                wgpu_write_gif_texture(&queue, &texture, &frame_info, width, height, &buf);

                let elapsed = std::time::Instant::now().duration_since(now);
                let wait_time = std::time::Duration::from_millis(dur as u64).checked_sub(elapsed).unwrap_or(std::time::Duration::ZERO);

                let repaint_info = egui::RequestRepaintInfo {
                    viewport_id: egui::ViewportId::ROOT,
                    delay: std::time::Duration::ZERO,
                    current_cumulative_pass_nr: 0,
                };

                let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

                cached.push((frame_info, buf));

                use std::time::Duration;
                if shutdown_tx.sleep(wait_time).await {
                    return;
                }
            }

            loop {
                for (info, cached_bytes) in &cached {
                    wgpu_write_gif_texture(&queue, &texture, info, width, height, cached_bytes);

                    let repaint_info = egui::RequestRepaintInfo {
                        viewport_id: egui::ViewportId::ROOT,
                        delay: Duration::from_millis(info.duration as u64),
                        current_cumulative_pass_nr: 0,
                    };

                    let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

                    if shutdown_tx.sleep(Duration::from_millis(info.duration as u64)).await {
                        return;
                    }
                }
            }
        });

        GifTexture {
            metadata: Metadata {
                width,
                height,
            },
            inner: texture,
            loaded,
        }
    }
}

fn blend_alpha(bytes: &mut [u8], byte_area: &GifFrameInfo, prev_bytes: &[u8], prev_byte_area: &GifFrameInfo) {
    if byte_area.left + byte_area.width < prev_byte_area.left || prev_byte_area.left + prev_byte_area.width < byte_area.left || byte_area.top + byte_area.height < prev_byte_area.top || prev_byte_area.top + prev_byte_area.height < byte_area.top {
        // no overlap, skip
        return;
    }

    let (h_offset, prev_h_offset, width) = get_horizontal_offset(byte_area, prev_byte_area);
    let (v_offset, prev_v_offset, height) = get_vertical_offset(byte_area, prev_byte_area);

    for x in 0..width {
        for y in 0..height {
            let byte_offset = (v_offset + h_offset + y * byte_area.width + x) as usize * 4;
            let bytes = &mut bytes[byte_offset..byte_offset + 4];

            let prev_byte_offset = (prev_v_offset + prev_h_offset + y * prev_byte_area.width + x) as usize * 4;
            let prev_bytes = &prev_bytes[prev_byte_offset..prev_byte_offset + 4];


            if bytes[3] == 0 {
                bytes[0] = prev_bytes[0];
                bytes[1] = prev_bytes[1];
                bytes[2] = prev_bytes[2];
                bytes[3] = prev_bytes[3];
            }
        }
    }
}

fn get_horizontal_offset(byte_area: &GifFrameInfo, prev_byte_area: &GifFrameInfo) -> (u32, u32, u32) {
    if byte_area.left ==  prev_byte_area.left {
        (0, 0, core::cmp::min(byte_area.width, prev_byte_area.width))
    } else if byte_area.left < prev_byte_area.left {
        (prev_byte_area.left - byte_area.left, 0,  core::cmp::min(prev_byte_area.left + prev_byte_area.width, byte_area.left + byte_area.width) - prev_byte_area.left)
    } else {
        (0, byte_area.left - prev_byte_area.left,  core::cmp::min(prev_byte_area.left + prev_byte_area.width, byte_area.left + byte_area.width) - byte_area.left)
    }
}

fn get_vertical_offset(byte_area: &GifFrameInfo, prev_byte_area: &GifFrameInfo) -> (u32, u32, u32) {
    let (offset_v, prev_offset_v, height) = if byte_area.top ==  prev_byte_area.top {
        (0, 0, core::cmp::min(byte_area.height, prev_byte_area.height))
    } else if byte_area.top < prev_byte_area.top {
        (prev_byte_area.top - byte_area.top, 0,  core::cmp::min(prev_byte_area.top + prev_byte_area.height, byte_area.top + byte_area.height) - prev_byte_area.top)
    } else {
        (0, byte_area.top - prev_byte_area.top,  core::cmp::min(prev_byte_area.top + prev_byte_area.height, byte_area.top + byte_area.height) - byte_area.top)
    };
    (offset_v * byte_area.width, prev_offset_v * prev_byte_area.width, height)
}

#[test]
fn gif_offsets() {
    fn test_frame(left: u32, top: u32, width: u32, height: u32) -> GifFrameInfo {
        GifFrameInfo {
            left,
            top,
            width,
            height,
            duration: 0,
            disposal: gif::DisposalMethod::Keep,
        }
    }

    // completely overlapping

    let f1 = test_frame(0, 0, 100, 100);
    let f2 = test_frame(0, 0, 50, 50);
    let hoffset = get_horizontal_offset(&f1, &f2);
    let voffset = get_vertical_offset(&f1, &f2);
    assert_eq!(get_horizontal_offset(&f1, &f2), get_horizontal_offset(&f2, &f1));
    assert_eq!(get_vertical_offset(&f1, &f2), get_vertical_offset(&f2, &f1));
    assert_eq!(hoffset, (0, 0, 50));
    assert_eq!(voffset, (0, 0, 50));

    let f3 = test_frame(50, 50, 50, 50);
    let hoffset = get_horizontal_offset(&f1, &f3);
    let voffset = get_vertical_offset(&f1, &f3);
    assert_eq!(hoffset, (50, 0, 50));
    assert_eq!(voffset, (50 * f1.width, 0, 50));
    let hoffset = get_horizontal_offset(&f3, &f1);
    let voffset = get_vertical_offset(&f3, &f1);
    assert_eq!(hoffset, (0, 50, 50));
    assert_eq!(voffset, (0, 50 * f1.width, 50));

    let f4 = test_frame(25, 25, 50, 50);
    let hoffset = get_horizontal_offset(&f1, &f4);
    let voffset = get_vertical_offset(&f1, &f4);
    assert_eq!(hoffset, (25, 0, 50));
    assert_eq!(voffset, (25 * f1.width, 0, 50));
    let hoffset = get_horizontal_offset(&f4, &f1);
    let voffset = get_vertical_offset(&f4, &f1);
    assert_eq!(hoffset, (0, 25, 50));
    assert_eq!(voffset, (0, 25 * f1.width, 50));

    // partially overlapping

    let f5 = test_frame(50, 50, 100, 100);
    let hoffset = get_horizontal_offset(&f1, &f5);
    let voffset = get_vertical_offset(&f1, &f5);
    assert_eq!(hoffset, (50, 0, 50));
    assert_eq!(voffset, (50 * f1.width, 0, 50));
    let hoffset = get_horizontal_offset(&f5, &f1);
    let voffset = get_vertical_offset(&f5, &f1);
    assert_eq!(hoffset, (0, 50, 50));
    assert_eq!(voffset, (0, 50 * f1.width, 50));

    let f6 = test_frame(100, 25, 100, 100);
    let hoffset = get_horizontal_offset(&f5, &f6);
    let voffset = get_vertical_offset(&f5, &f6);
    assert_eq!(hoffset, (50, 0, 50));
    assert_eq!(voffset, (0, 25 * f6.width, 75));
    let hoffset = get_horizontal_offset(&f6, &f5);
    let voffset = get_vertical_offset(&f6, &f5);
    assert_eq!(hoffset, (0, 50, 50));
    assert_eq!(voffset, (25 * f6.width, 0, 75));
}

pub fn wgpu_write_gif_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, GifFrameInfo { top, left, width, height, disposal, .. }: &GifFrameInfo, max_width: u32, max_height: u32, data: &[u8]) {
    let size = wgpu::Extent3d {
        width: *width as _,
        height: *height as _,
        depth_or_array_layers: 1,
    };

    let origin = wgpu::Origin3d {
        x: *left,
        y: *top,
        z: 0,
    };

    if !matches!(disposal, gif::DisposalMethod::Keep) && (*width < max_width || *height < max_height) {
        let clear = vec![0; (max_width * max_height * 4) as usize];

        let clear_size = wgpu::Extent3d {
            width: max_width as _,
            height: max_height as _,
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
                bytes_per_row: Some(4 * (max_width as u32)),
                rows_per_image: Some(max_height as _),
            },
            clear_size,
        );
    }

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
            bytes_per_row: Some(4 * (*width as u32)),
            rows_per_image: Some(*height as _),
        },
        size,
    );
}

pub struct GifTexture {
    pub metadata: Metadata,
    pub loaded: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub inner: wgpu::Texture,
}

impl super::Texture for GifTexture {
    fn loaded(&self) -> bool {
        self.loaded.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

pub struct EguiGifView {
    pub inner: egui::TextureId,
}

impl EguiGifView {
    pub fn from_texture(texture: &GifTexture, renderer: &mut egui_wgpu::Renderer, device: &wgpu::Device) -> Self {
        let view = texture.inner.create_view(&Default::default());
        let inner = renderer.register_native_texture_with_sampler_options(device, &view, wgpu::SamplerDescriptor { lod_min_clamp: 0., lod_max_clamp: 0., min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default()});

        Self {
            inner
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        let img = egui::Image::new((self.inner, size));
        ui.add(img);
    }
}
