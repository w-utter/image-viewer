pub struct AvifTextureData {
    parser: zenavif_parse::AvifParser<'static>,
}

pub async fn load_avif(handle: rfd::FileHandle) -> AvifTextureData {
    let bytes = handle.read().await;
    tokio::task::spawn_blocking(move || {
        let bytes = bytes;
        let parser = zenavif_parse::AvifParser::from_owned(bytes).unwrap();

        AvifTextureData { parser }
    })
    .await
    .unwrap()
}

use super::cleanup::ShutdownNotif;
use super::metadata::Metadata;

use egui_wgpu::wgpu;

pub struct AvifTexture {
    pub metadata: Metadata,
    pub loaded: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub inner: wgpu::Texture,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub alpha: wgpu::Texture,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub color: wgpu::Texture,
}

impl super::Texture for AvifTexture {
    fn loaded(&self) -> bool {
        self.loaded.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

struct AvifDebug<'a> {
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub alpha: &'a wgpu::Texture,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub color: &'a wgpu::Texture,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub queue: &'a wgpu::Queue,
    _pd: core::marker::PhantomData<&'a ()>,
}

pub fn wgpu_write_avif_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    data: &[u8],
) {
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

pub struct EguiAvifView {
    pub inner: egui::TextureId,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub show_debug: bool,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub alpha: egui::TextureId,
    #[cfg(all(debug_assertions, feature = "debug-avif"))]
    pub color: egui::TextureId,
}

impl EguiAvifView {
    pub fn from_texture(
        texture: &AvifTexture,
        renderer: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
    ) -> Self {
        let mut create_id = |texture: &wgpu::Texture| {
            let view = texture.create_view(&Default::default());
            renderer.register_native_texture_with_sampler_options(
                device,
                &view,
                wgpu::SamplerDescriptor {
                    lod_min_clamp: 0.,
                    lod_max_clamp: 0.,
                    min_filter: wgpu::FilterMode::Linear,
                    mag_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                },
            )
        };

        let combined = create_id(&texture.inner);

        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let alpha = create_id(&texture.alpha);
        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let color = create_id(&texture.color);

        Self {
            inner: combined,
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            alpha,
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            color,
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            show_debug: false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        let img = egui::Image::new((self.inner, size));
        ui.add(img);

        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        {
            ui.checkbox(&mut self.show_debug, "debug");
            if self.show_debug {
                let img = egui::Image::new((self.color, size));
                ui.add(img);

                let img = egui::Image::new((self.alpha, size));
                ui.add(img);
            }
        }
    }

    pub fn free_textures(&mut self, renderer: &mut egui_wgpu::Renderer) {
        renderer.free_texture(&self.inner);
        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        {
            renderer.free_texture(&self.alpha);
            renderer.free_texture(&self.color);
        }
    }
}

impl AvifTextureData {
    pub fn display(
        self,
        shutdown_tx: ShutdownNotif,
        device: &wgpu::Device,
        queue: std::sync::Arc<wgpu::Queue>,
        proxy: &egui_winit::winit::event_loop::EventLoopProxy<crate::app::UserEvent>,
    ) -> AvifTexture {
        let metadata = self.parser.primary_metadata().unwrap();
        if let Some(grid) = self.parser.grid_config() {
            todo!("{grid:?}");
        }

        let width: u32 = metadata.max_frame_width.into();
        let height: u32 = metadata.max_frame_height.into();

        use dav1d::{Decoder, Settings};

        let mut main_decoder =
            Decoder::with_settings(&Settings::default()).expect("decoder creation failed");

        let mut alpha_decoder =
            Decoder::with_settings(&Settings::default()).expect("decoder creation failed");

        let size = wgpu::Extent3d {
            width: width as _,
            height: height as _,
            depth_or_array_layers: 1,
        };

        let alloc_texture = || {
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
            texture
        };

        let texture = alloc_texture();

        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let color_texture = alloc_texture();
        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let alpha_texture = alloc_texture();

        let texture2 = texture.clone();
        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let alpha_texture2 = alpha_texture.clone();
        #[cfg(all(debug_assertions, feature = "debug-avif"))]
        let color_texture2 = color_texture.clone();

        let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::from(false));

        let proxy = proxy.clone();
        let frame_loaded = loaded.clone();
        tokio::task::spawn(async move {
            let texture = texture2;
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            let alpha_texture = alpha_texture2;
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            let color_texture = color_texture2;

            let color_info = self.parser.color_info();
            let primary = self.parser.primary_data().unwrap();
            let alpha = self.parser.alpha_data().map(|res| res.unwrap());

            let debug = AvifDebug {
                #[cfg(all(debug_assertions, feature = "debug-avif"))]
                queue: &queue,
                #[cfg(all(debug_assertions, feature = "debug-avif"))]
                color: &color_texture,
                #[cfg(all(debug_assertions, feature = "debug-avif"))]
                alpha: &alpha_texture,
                _pd: core::marker::PhantomData,
            };

            let data_bytes = Self::decode_obu(
                &*primary,
                alpha.as_deref(),
                &mut main_decoder,
                &mut alpha_decoder,
                color_info,
                width,
                height,
                debug,
            );

            wgpu_write_avif_texture(&queue, &texture, width, height, &data_bytes);

            frame_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
            let repaint_info = egui::RequestRepaintInfo {
                viewport_id: egui::ViewportId::ROOT,
                delay: std::time::Duration::ZERO,
                current_cumulative_pass_nr: 0,
            };

            let _ = proxy.send_event(crate::app::UserEvent::RepaintRequest(repaint_info, None));

            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            {
                println!("animation: {:?}", self.parser.animation_info());
                println!("light: {:?}", self.parser.content_light_level());
                println!("premul: {:?}", self.parser.premultiplied_alpha());
                println!("color: {:?}", self.parser.color_info());
                println!("av1: {:?}", self.parser.av1_config());
            }

            if let Some(info) = self.parser.animation_info() {
                let mut cached = vec![];

                let max_idx = info.frame_count;
                let dur = self.parser.frame(0).unwrap().duration_ms;
                let color_info = self.parser.color_info();
                cached.push((dur, data_bytes));

                use std::time::Duration;
                if shutdown_tx.sleep(Duration::from_millis(dur as u64)).await {
                    return;
                }

                let mut idx = 1;

                loop {
                    if let Some((duration, cached_bytes)) = cached.get(idx) {
                        wgpu_write_avif_texture(&queue, &texture, width, height, cached_bytes);

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

                    let frame = self.parser.frame(idx).unwrap();

                    let data = frame.data;
                    let alpha = frame.alpha_data.map(|alpha| alpha);

                    let debug = AvifDebug {
                        #[cfg(all(debug_assertions, feature = "debug-avif"))]
                        queue: &queue,
                        #[cfg(all(debug_assertions, feature = "debug-avif"))]
                        alpha: &alpha_texture,
                        #[cfg(all(debug_assertions, feature = "debug-avif"))]
                        color: &color_texture,
                        _pd: core::marker::PhantomData,
                    };

                    let now = std::time::Instant::now();

                    let data_bytes = Self::decode_obu(
                        &*data,
                        alpha.as_deref(),
                        &mut main_decoder,
                        &mut alpha_decoder,
                        color_info,
                        width,
                        height,
                        debug,
                    );

                    wgpu_write_avif_texture(&queue, &texture, width, height, &data_bytes);

                    let dur = frame.duration_ms;

                    // if time to process frame was non-negligable
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

                    cached.push((dur, data_bytes));

                    if shutdown_tx.sleep(wait_time).await {
                        return;
                    }

                    idx = (idx + 1) % max_idx;
                }
            }
        });

        AvifTexture {
            metadata: Metadata { width, height },
            inner: texture,
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            color: color_texture,
            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            alpha: alpha_texture,
            loaded,
        }
    }

    fn decode_obu(
        bytes: &[u8],
        alpha: Option<&[u8]>,
        main_decoder: &mut dav1d::Decoder,
        alpha_decoder: &mut dav1d::Decoder,
        color_info: Option<&zenavif_parse::ColorInformation>,
        width: u32,
        height: u32,
        debug: AvifDebug<'_>,
    ) -> Vec<u8> {
        let primary = read_until_ready(main_decoder, bytes).unwrap();
        let alpha = alpha.map(|alpha| read_until_ready(alpha_decoder, alpha).unwrap());

        let depth = usize::from(primary.bit_depth() > 8) + 1;
        let mut buf = vec![0; usize::try_from(width * height * 4).unwrap() * depth];

        yuv::process_avif_frame(primary, alpha, color_info, width, height, &mut buf, debug)
            .unwrap();

        buf
    }
}

fn read_until_ready(
    decoder: &mut dav1d::Decoder,
    bytes: &[u8],
) -> Result<dav1d::Picture, dav1d::Error> {
    decoder.send_data(bytes.to_vec(), None, None, None).unwrap();

    loop {
        match decoder.get_picture() {
            Err(dav1d::Error::Again) => match decoder.send_pending_data() {
                Ok(_) | Err(dav1d::Error::Again) => (),
                Err(e) => return Err(e),
            },
            res => return res,
        }
    }
}

mod yuv {
    use num_traits::AsPrimitive;

    #[derive(Debug, Copy, Clone, PartialOrd, PartialEq)]
    /// Declares YUV range TV (limited) or PC (full),
    /// more info [ITU-R](https://www.itu.int/rec/T-REC-H.273/en)
    pub(crate) enum YuvIntensityRange {
        /// Limited range Y ∈ [16 << (depth - 8), 16 << (depth - 8) + 224 << (depth - 8)],
        /// UV ∈ [-1 << (depth - 1), -1 << (depth - 1) + 1 << (depth - 1)]
        Tv,
        /// Full range Y ∈ [0, 2^bit_depth - 1],
        /// UV ∈ [-1 << (depth - 1), -1 << (depth - 1) + 2^bit_depth - 1]
        Pc,
    }

    impl YuvIntensityRange {
        pub(crate) const fn get_yuv_range(self, depth: u32) -> YuvChromaRange {
            match self {
                YuvIntensityRange::Tv => YuvChromaRange {
                    bias_y: 16 << (depth - 8),
                    bias_uv: 1 << (depth - 1),
                    range_y: 219 << (depth - 8),
                    range_uv: 224 << (depth - 8),
                    range: self,
                },
                YuvIntensityRange::Pc => YuvChromaRange {
                    bias_y: 0,
                    bias_uv: 1 << (depth - 1),
                    range_uv: (1 << depth) - 1,
                    range_y: (1 << depth) - 1,
                    range: self,
                },
            }
        }
    }

    #[derive(Debug, Copy, Clone, PartialOrd, PartialEq)]
    pub(crate) struct YuvChromaRange {
        pub(crate) bias_y: u32,
        pub(crate) bias_uv: u32,
        pub(crate) range_y: u32,
        pub(crate) range_uv: u32,
        pub(crate) range: YuvIntensityRange,
    }

    pub(crate) struct YuvPlanarImage<'a, T> {
        pub(crate) y_plane: &'a [T],
        pub(crate) y_stride: usize,
        pub(crate) u_plane: &'a [T],
        pub(crate) u_stride: usize,
        pub(crate) v_plane: &'a [T],
        pub(crate) v_stride: usize,
        pub(crate) width: usize,
        pub(crate) height: usize,
    }

    use dav1d::{PixelLayout, PlanarImageComponent};
    pub(crate) fn process_avif_frame(
        primary: dav1d::Picture,
        alpha: Option<dav1d::Picture>,
        color_info: Option<&zenavif_parse::ColorInformation>,
        width: u32,
        height: u32,
        buf: &mut [u8],
        debug: super::AvifDebug<'_>,
    ) -> Result<(), ()> {
        let bit_depth = primary.bit_depth();

        let yuv_range = match primary.color_range() {
            dav1d::pixel::YUVRange::Limited => YuvIntensityRange::Tv,
            dav1d::pixel::YUVRange::Full => YuvIntensityRange::Pc,
        };

        let matrix_strategy = get_matrix(primary.matrix_coefficients()).unwrap();

        if matrix_strategy == YuvMatrixStrategy::Identity
            && primary.pixel_layout() != PixelLayout::I444
        {
            panic!()
        }

        if matrix_strategy == YuvMatrixStrategy::CgCo && primary.pixel_layout() == PixelLayout::I400
        {
            panic!()
        }

        if bit_depth == 8 {
            let ref_y = primary.plane(PlanarImageComponent::Y);
            let ref_u = primary.plane(PlanarImageComponent::U);
            let ref_v = primary.plane(PlanarImageComponent::V);

            let image = YuvPlanarImage {
                y_plane: ref_y.as_ref(),
                y_stride: primary.stride(PlanarImageComponent::Y) as usize,
                u_plane: ref_u.as_ref(),
                u_stride: primary.stride(PlanarImageComponent::U) as usize,
                v_plane: ref_v.as_ref(),
                v_stride: primary.stride(PlanarImageComponent::V) as usize,
                width: width as usize,
                height: height as usize,
            };

            match matrix_strategy {
                YuvMatrixStrategy::KrKb(standard) => {
                    let worker = match primary.pixel_layout() {
                        PixelLayout::I400 => yuv400_to_rgba8,
                        PixelLayout::I420 => yuv420_to_rgba8,
                        PixelLayout::I422 => yuv422_to_rgba8,
                        PixelLayout::I444 => yuv444_to_rgba8,
                    };

                    worker(image, buf, yuv_range, standard)?;
                }
                YuvMatrixStrategy::CgCo => {
                    let worker = match primary.pixel_layout() {
                        PixelLayout::I400 => unreachable!(),
                        PixelLayout::I420 => ycgcg::ycgco420_to_rgba8,
                        PixelLayout::I422 => ycgcg::ycgco422_to_rgba8,
                        PixelLayout::I444 => ycgcg::ycgco444_to_rgba8,
                    };

                    worker(image, buf, yuv_range)?;
                }
                YuvMatrixStrategy::Identity => {
                    let worker = match primary.pixel_layout() {
                        PixelLayout::I400 => unreachable!(),
                        PixelLayout::I420 => unreachable!(),
                        PixelLayout::I422 => unreachable!(),
                        PixelLayout::I444 => gbr_to_rgba8,
                    };

                    worker(image, buf, yuv_range)?;
                }
            }

            #[cfg(all(debug_assertions, feature = "debug-avif"))]
            super::wgpu_write_avif_texture(debug.queue, debug.color, width, height, buf);

            // Squashing alpha plane into a picture
            if let Some(picture) = alpha {
                if picture.pixel_layout() != PixelLayout::I400 {
                    panic!()
                }

                let stride = picture.stride(PlanarImageComponent::Y) as usize;
                let plane = picture.plane(PlanarImageComponent::Y);

                for (buf, slice) in Iterator::zip(
                    buf.chunks_exact_mut(width as usize * 4),
                    plane.as_ref().chunks_exact(stride),
                ) {
                    for (rgba, a_src) in buf.chunks_exact_mut(4).zip(slice) {
                        // premultiply by alpha
                        rgba[0] = ((rgba[0] as f32) * ((*a_src as f32) / 255.)) as u8;
                        rgba[1] = ((rgba[1] as f32) * ((*a_src as f32) / 255.)) as u8;
                        rgba[2] = ((rgba[2] as f32) * ((*a_src as f32) / 255.)) as u8;

                        rgba[3] = *a_src;
                    }
                }

                #[cfg(all(debug_assertions, feature = "debug-avif"))]
                {
                    let mut alpha = vec![255; buf.len()];

                    for (buf, slice) in Iterator::zip(
                        alpha.chunks_exact_mut(width as usize * 4),
                        plane.as_ref().chunks_exact(stride),
                    ) {
                        for (rgba, a_src) in buf.chunks_exact_mut(4).zip(slice) {
                            // premultiply by alpha
                            rgba[0] = ((rgba[0] as f32) * ((*a_src as f32) / 255.)) as u8;
                            rgba[1] = ((rgba[1] as f32) * ((*a_src as f32) / 255.)) as u8;
                            rgba[2] = ((rgba[2] as f32) * ((*a_src as f32) / 255.)) as u8;

                            rgba[3] = *a_src;
                        }
                    }

                    super::wgpu_write_avif_texture(debug.queue, debug.alpha, width, height, &alpha);
                }
            }
        } else {
            // // 8+ bit-depth case
            /*
            if let Ok(buf) = bytemuck::try_cast_slice_mut(buf) {
                let target_slice: &mut [u16] = buf;
                self.process_16bit_picture(target_slice, yuv_range, matrix_strategy)?;
            } else {
                // If buffer from Decoder is unaligned
                let mut aligned_store = vec![0u16; buf.len() / 2];
                self.process_16bit_picture(&mut aligned_store, yuv_range, matrix_strategy)?;
                for (dst, src) in buf.chunks_exact_mut(2).zip(aligned_store.iter()) {
                    let bytes = src.to_ne_bytes();
                    dst[0] = bytes[0];
                    dst[1] = bytes[1];
                }
            }
            */
            todo!()
        }
        Ok(())
    }

    #[derive(Debug, Copy, Clone, PartialOrd, PartialEq, Eq)]
    /// Declares standard prebuilt YUV conversion matrices,
    /// check [ITU-R](https://www.itu.int/rec/T-REC-H.273/en) information for more info
    pub(crate) enum YuvStandardMatrix {
        Bt601,
        Bt709,
        Bt2020,
        Smpte240,
        Bt470_6,
    }

    #[derive(Copy, Clone, Debug, PartialOrd, Eq, PartialEq)]
    enum YuvMatrixStrategy {
        KrKb(YuvStandardMatrix),
        CgCo,
        Identity,
    }

    #[derive(Debug, Copy, Clone, PartialOrd, PartialEq)]
    struct YuvBias {
        kr: f32,
        kb: f32,
    }

    impl YuvStandardMatrix {
        const fn get_kr_kb(self) -> YuvBias {
            match self {
                YuvStandardMatrix::Bt601 => YuvBias {
                    kr: 0.299f32,
                    kb: 0.114f32,
                },
                YuvStandardMatrix::Bt709 => YuvBias {
                    kr: 0.2126f32,
                    kb: 0.0722f32,
                },
                YuvStandardMatrix::Bt2020 => YuvBias {
                    kr: 0.2627f32,
                    kb: 0.0593f32,
                },
                YuvStandardMatrix::Smpte240 => YuvBias {
                    kr: 0.087f32,
                    kb: 0.212f32,
                },
                YuvStandardMatrix::Bt470_6 => YuvBias {
                    kr: 0.2220f32,
                    kb: 0.0713f32,
                },
            }
        }
    }

    /// Getting one of prebuilt matrix of fails
    fn get_matrix(david_matrix: dav1d::pixel::MatrixCoefficients) -> Result<YuvMatrixStrategy, ()> {
        match david_matrix {
            dav1d::pixel::MatrixCoefficients::Identity => Ok(YuvMatrixStrategy::Identity),
            dav1d::pixel::MatrixCoefficients::BT709 => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Bt709))
            }
            // This is arguable, some applications prefer to go with Bt.709 as default,
            // and some applications prefer Bt.601 as default.
            // For ex. `Chrome` always prefer Bt.709 even for SD content
            // However, nowadays standard should be Bt.709 for HD+ size otherwise Bt.601
            dav1d::pixel::MatrixCoefficients::Unspecified => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Bt709))
            }
            dav1d::pixel::MatrixCoefficients::Reserved => Err(()),
            dav1d::pixel::MatrixCoefficients::BT470M => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Bt470_6))
            }
            dav1d::pixel::MatrixCoefficients::BT470BG => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Bt601))
            }
            dav1d::pixel::MatrixCoefficients::ST170M => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Smpte240))
            }
            dav1d::pixel::MatrixCoefficients::ST240M => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Smpte240))
            }
            dav1d::pixel::MatrixCoefficients::YCgCo => Ok(YuvMatrixStrategy::CgCo),
            dav1d::pixel::MatrixCoefficients::BT2020NonConstantLuminance => {
                Ok(YuvMatrixStrategy::KrKb(YuvStandardMatrix::Bt2020))
            }
            dav1d::pixel::MatrixCoefficients::BT2020ConstantLuminance => {
                // This matrix significantly differs from others because linearize values is required
                // to compute Y instead of Y'.
                // Actually it is almost everywhere is not implemented.
                // Libavif + libheif missing this also so actually AVIF images
                // with CL BT.2020 might be made only by mistake
                Err(())
            }
            dav1d::pixel::MatrixCoefficients::ST2085 => Err(()),
            dav1d::pixel::MatrixCoefficients::ChromaticityDerivedConstantLuminance
            | dav1d::pixel::MatrixCoefficients::ChromaticityDerivedNonConstantLuminance => Err(()),
            dav1d::pixel::MatrixCoefficients::ICtCp => Err(()),
        }
    }

    #[inline]
    fn yuv400_to_rgbx_impl<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgba: &mut [V],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()>
    where
        i32: AsPrimitive<V>,
    {
        assert!(
            CHANNELS == 3 || CHANNELS == 4,
            "YUV 4:0:0 -> RGB is implemented only on 3 and 4 channels"
        );
        assert!(
            (8..=16).contains(&BIT_DEPTH),
            "Invalid bit depth is provided"
        );
        assert!(
            if BIT_DEPTH > 8 {
                size_of::<V>() == 2
            } else {
                size_of::<V>() == 1
            },
            "Unsupported bit depth and data type combination"
        );

        let y_plane = image.y_plane;
        let y_stride = image.y_stride;
        let height = image.height;
        let width = image.width;

        check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, height)?;
        check_rgb_preconditions(rgba, width * CHANNELS, height)?;

        let rgba_stride = width * CHANNELS;

        let max_value = (1 << BIT_DEPTH) - 1;

        // If luma plane is in full range it can be just redistributed across the image
        if range == YuvIntensityRange::Pc {
            let y_iter = y_plane.chunks_exact(y_stride);
            let rgb_iter = rgba.chunks_exact_mut(rgba_stride);

            // All branches on generic const will be optimized out.
            for (y_src, rgb) in y_iter.zip(rgb_iter) {
                let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);

                for (y_src, rgb_dst) in y_src.iter().zip(rgb_chunks) {
                    let r = *y_src;
                    rgb_dst[0] = r;
                    rgb_dst[1] = r;
                    rgb_dst[2] = r;
                    if CHANNELS == 4 {
                        rgb_dst[3] = max_value.as_();
                    }
                }
            }
            return Ok(());
        }

        let range = range.get_yuv_range(BIT_DEPTH as u32);
        let kr_kb = matrix.get_kr_kb();
        const PRECISION: i32 = 11;

        let inverse_transform = get_inverse_transform(
            (1 << BIT_DEPTH) - 1,
            range.range_y,
            range.range_uv,
            kr_kb.kr,
            kr_kb.kb,
            PRECISION as u32,
        );
        let y_coef = inverse_transform.y_coef;

        let bias_y = range.bias_y as i32;

        let y_iter = y_plane.chunks_exact(y_stride);
        let rgb_iter = rgba.chunks_exact_mut(rgba_stride);

        // All branches on generic const will be optimized out.
        for (y_src, rgb) in y_iter.zip(rgb_iter) {
            let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);

            for (y_src, rgb_dst) in y_src.iter().zip(rgb_chunks) {
                let y_value = (y_src.as_() - bias_y) * y_coef;

                let r = qrshr::<PRECISION, BIT_DEPTH>(y_value);
                rgb_dst[0] = r.as_();
                rgb_dst[1] = r.as_();
                rgb_dst[2] = r.as_();
                if CHANNELS == 4 {
                    rgb_dst[3] = max_value.as_();
                }
            }
        }

        Ok(())
    }

    pub(crate) fn yuv400_to_rgba8(
        image: YuvPlanarImage<u8>,
        rgba: &mut [u8],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()> {
        yuv400_to_rgbx_impl::<u8, 4, 8>(image, rgba, range, matrix)
    }

    pub(crate) type HalvedRowHandler<V> =
        fn(YuvPlanarImage<V>, &mut [V], &CbCrInverseTransform<i32>, &YuvChromaRange);

    pub(crate) fn yuv420_to_rgba8(
        image: YuvPlanarImage<u8>,
        rgb: &mut [u8],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()> {
        const P: i32 = 13;
        yuv420_to_rgbx_invoker::<u8, HalvedRowHandler<u8>, P, 4, 8>(
            image,
            rgb,
            range,
            matrix,
            process_halved_chroma_row_cbcr::<u8, P, 4, 8>,
        )
    }

    pub(crate) fn yuv420_to_rgbx_invoker<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        W: Fn(YuvPlanarImage<V>, &mut [V], &CbCrInverseTransform<i32>, &YuvChromaRange),
        const PRECISION: i32,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgb: &mut [V],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
        worker: W,
    ) -> Result<(), ()>
    where
        i32: AsPrimitive<V>,
    {
        assert!(
            CHANNELS == 3 || CHANNELS == 4,
            "YUV 4:2:0 -> RGB is implemented only on 3 and 4 channels"
        );
        assert!(
            (8..=16).contains(&BIT_DEPTH),
            "Invalid bit depth is provided"
        );
        assert!(
            if BIT_DEPTH > 8 {
                size_of::<V>() == 2
            } else {
                size_of::<V>() == 1
            },
            "Unsupported bit depth and data type combination"
        );
        let y_plane = image.y_plane;
        let u_plane = image.u_plane;
        let v_plane = image.v_plane;
        let y_stride = image.y_stride;
        let u_stride = image.u_stride;
        let v_stride = image.v_stride;
        let chroma_height = image.height.div_ceil(2);

        check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, image.height)?;
        check_yuv_plane_preconditions(u_plane, PlaneDefinition::U, u_stride, chroma_height)?;
        check_yuv_plane_preconditions(v_plane, PlaneDefinition::V, v_stride, chroma_height)?;

        check_rgb_preconditions(rgb, image.width * CHANNELS, image.height)?;

        let range = range.get_yuv_range(BIT_DEPTH as u32);
        let kr_kb = matrix.get_kr_kb();
        let inverse_transform = get_inverse_transform(
            (1 << BIT_DEPTH) - 1,
            range.range_y,
            range.range_uv,
            kr_kb.kr,
            kr_kb.kb,
            PRECISION as u32,
        );

        let rgb_stride = image.width * CHANNELS;

        let y_iter = y_plane.chunks_exact(y_stride * 2);
        let rgb_iter = rgb.chunks_exact_mut(rgb_stride * 2);
        let u_iter = u_plane.chunks_exact(u_stride);
        let v_iter = v_plane.chunks_exact(v_stride);

        /*
           Sample 4x4 YUV420 planar image
           start_y + 0:  Y00 Y01 Y02 Y03
           start_y + 4:  Y04 Y05 Y06 Y07
           start_y + 8:  Y08 Y09 Y10 Y11
           start_y + 12: Y12 Y13 Y14 Y15
           start_cb + 0: Cb00 Cb01
           start_cb + 2: Cb02 Cb03
           start_cr + 0: Cr00 Cr01
           start_cr + 2: Cr02 Cr03

           For 4 luma components (2x2 on rows and cols) there are 1 chroma Cb/Cr components.
           Luma channel must have always exact size as RGB target layout, but chroma is not.

           We're sectioning an image by pair of rows, then for each pair of luma and RGB row,
           there is one chroma row.

           As chroma is shrunk by factor of 2 then we're processing by pairs of RGB and luma,
           for each RGB and luma pair there is one chroma component.

           If image have odd width then luma channel must be exact, and we're replicating last
           chroma component.

           If image have odd height then luma channel is exact, and we're replicating last chroma rows.
        */

        // All branches on generic const will be optimized out.
        for (((y_src, u_src), v_src), rgb) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
            // Since we're processing two rows in one loop we need to re-slice once more
            let y_iter = y_src.chunks_exact(y_stride);
            let rgb_iter = rgb.chunks_exact_mut(rgb_stride);
            for (y_src, rgba) in y_iter.zip(rgb_iter) {
                let image = YuvPlanarImage {
                    y_plane: y_src,
                    y_stride: 0,
                    u_plane: u_src,
                    u_stride: 0,
                    v_plane: v_src,
                    v_stride: 0,
                    width: image.width,
                    height: image.height,
                };
                worker(image, rgba, &inverse_transform, &range);
            }
        }

        // Process remainder if height is odd

        let y_iter = y_plane
            .chunks_exact(y_stride * 2)
            .remainder()
            .chunks_exact(y_stride);
        let rgb_iter = rgb.chunks_exact_mut(rgb_stride).rev();
        let u_iter = u_plane.chunks_exact(u_stride).rev();
        let v_iter = v_plane.chunks_exact(v_stride).rev();

        for (((y_src, u_src), v_src), rgba) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
            let image = YuvPlanarImage {
                y_plane: y_src,
                y_stride: 0,
                u_plane: u_src,
                u_stride: 0,
                v_plane: v_src,
                v_stride: 0,
                width: image.width,
                height: image.height,
            };
            worker(image, rgba, &inverse_transform, &range);
        }

        Ok(())
    }

    #[inline]
    fn process_halved_chroma_row_cbcr<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        const PRECISION: i32,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgba: &mut [V],
        transform: &CbCrInverseTransform<i32>,
        range: &YuvChromaRange,
    ) where
        i32: AsPrimitive<V>,
    {
        // If the stride is larger than the plane size,
        // it might contain junk data beyond the actual valid region.
        // To avoid processing artifacts when working with odd-sized images,
        // the buffer is reshaped to its actual size,
        // preventing accidental use of invalid values from the trailing region.

        let y_plane = &image.y_plane[0..image.width];
        let chroma_size = image.width.div_ceil(2);
        let u_plane = &image.u_plane[0..chroma_size];
        let v_plane = &image.v_plane[0..chroma_size];
        let rgba = &mut rgba[0..image.width * CHANNELS];

        let bias_y = range.bias_y as i32;
        let bias_uv = range.bias_uv as i32;
        let y_iter = y_plane.chunks_exact(2);
        let rgb_chunks = rgba.chunks_exact_mut(CHANNELS * 2);
        for (((y_src, &u_src), &v_src), rgb_dst) in y_iter.zip(u_plane).zip(v_plane).zip(rgb_chunks)
        {
            let y_value0: i32 = y_src[0].as_() - bias_y;
            let cb_value: i32 = u_src.as_() - bias_uv;
            let cr_value: i32 = v_src.as_() - bias_uv;

            let dst0 = &mut rgb_dst[..CHANNELS];

            ycbcr_execute::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                dst0.try_into().unwrap(),
                y_value0,
                cb_value,
                cr_value,
                transform,
            );

            let y_value1 = y_src[1].as_() - bias_y;

            let dst1 = &mut rgb_dst[CHANNELS..2 * CHANNELS];

            ycbcr_execute::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                dst1.try_into().unwrap(),
                y_value1,
                cb_value,
                cr_value,
                transform,
            );
        }

        // Process remainder if width is odd.
        if image.width & 1 != 0 {
            let y_left = y_plane.chunks_exact(2).remainder();
            let rgb_chunks = rgba
                .chunks_exact_mut(CHANNELS * 2)
                .into_remainder()
                .chunks_exact_mut(CHANNELS);
            let u_iter = u_plane.iter().rev();
            let v_iter = v_plane.iter().rev();

            for (((y_src, u_src), v_src), rgb_dst) in
                y_left.iter().zip(u_iter).zip(v_iter).zip(rgb_chunks)
            {
                let y_value = y_src.as_() - bias_y;
                let cb_value = u_src.as_() - bias_uv;
                let cr_value = v_src.as_() - bias_uv;

                ycbcr_execute::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                    rgb_dst.try_into().unwrap(),
                    y_value,
                    cb_value,
                    cr_value,
                    transform,
                );
            }
        }
    }

    #[inline(always)]
    fn ycbcr_execute<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        const PRECISION: i32,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        dst: &mut [V; CHANNELS],
        y_value: i32,
        cb: i32,
        cr: i32,
        t: &CbCrInverseTransform<i32>,
    ) where
        i32: AsPrimitive<V>,
    {
        let y_scaled = y_value * t.y_coef;
        let r = qrshr::<PRECISION, BIT_DEPTH>(y_scaled + t.cr_coef * cr);
        let b = qrshr::<PRECISION, BIT_DEPTH>(y_scaled + t.cb_coef * cb);
        let g = qrshr::<PRECISION, BIT_DEPTH>(y_scaled - t.g_coeff_1 * cr - t.g_coeff_2 * cb);

        if CHANNELS == 4 {
            dst[0] = r.as_();
            dst[1] = g.as_();
            dst[2] = b.as_();
            dst[3] = ((1i32 << BIT_DEPTH) - 1).as_();
        } else if CHANNELS == 3 {
            dst[0] = r.as_();
            dst[1] = g.as_();
            dst[2] = b.as_();
        } else {
            unreachable!();
        }
    }

    pub(crate) fn yuv422_to_rgba8(
        image: YuvPlanarImage<u8>,
        rgb: &mut [u8],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()> {
        const P: i32 = 13;
        yuv422_to_rgbx_invoker::<u8, HalvedRowHandler<u8>, P, 4, 8>(
            image,
            rgb,
            range,
            matrix,
            process_halved_chroma_row_cbcr::<u8, P, 4, 8>,
        )
    }

    pub(crate) fn yuv422_to_rgbx_invoker<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        W: Fn(YuvPlanarImage<V>, &mut [V], &CbCrInverseTransform<i32>, &YuvChromaRange),
        const PRECISION: i32,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgb: &mut [V],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
        worker: W,
    ) -> Result<(), ()>
    where
        i32: AsPrimitive<V>,
    {
        assert!(
            CHANNELS == 3 || CHANNELS == 4,
            "YUV 4:2:2 -> RGB is implemented only on 3 and 4 channels"
        );
        assert!(
            (8..=16).contains(&BIT_DEPTH),
            "Invalid bit depth is provided"
        );
        assert!(PRECISION < 16);
        assert!(
            if BIT_DEPTH > 8 {
                size_of::<V>() == 2
            } else {
                size_of::<V>() == 1
            },
            "Unsupported bit depth and data type combination"
        );
        let y_plane = image.y_plane;
        let u_plane = image.u_plane;
        let v_plane = image.v_plane;
        let y_stride = image.y_stride;
        let u_stride = image.u_stride;
        let v_stride = image.v_stride;
        let width = image.width;

        check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, image.height)?;
        check_yuv_plane_preconditions(u_plane, PlaneDefinition::U, u_stride, image.height)?;
        check_yuv_plane_preconditions(v_plane, PlaneDefinition::V, v_stride, image.height)?;

        check_rgb_preconditions(rgb, image.width * CHANNELS, image.height)?;

        let range = range.get_yuv_range(BIT_DEPTH as u32);
        let kr_kb = matrix.get_kr_kb();

        let inverse_transform = get_inverse_transform(
            (1 << BIT_DEPTH) - 1,
            range.range_y,
            range.range_uv,
            kr_kb.kr,
            kr_kb.kb,
            PRECISION as u32,
        );

        /*
           Sample 4x4 YUV422 planar image
           start_y + 0:  Y00 Y01 Y02 Y03
           start_y + 4:  Y04 Y05 Y06 Y07
           start_y + 8:  Y08 Y09 Y10 Y11
           start_y + 12: Y12 Y13 Y14 Y15
           start_cb + 0: Cb00 Cb01
           start_cb + 2: Cb02 Cb03
           start_cb + 4: Cb04 Cb05
           start_cb + 6: Cb06 Cb07
           start_cr + 0: Cr00 Cr01
           start_cr + 2: Cr02 Cr03
           start_cr + 4: Cr04 Cr05
           start_cr + 6: Cr06 Cr07

           For 2 luma components there are 1 chroma Cb/Cr components.
           Luma channel must have always exact size as RGB target layout, but chroma is not.

           As chroma is shrunk by factor of 2 then we're processing by pairs of RGB and luma,
           for each RGB and luma pair there is one chroma component.

           If image have odd width then luma channel must be exact, and we're replicating last
           chroma component.
        */

        let rgb_stride = width * CHANNELS;

        let y_iter = y_plane.chunks_exact(y_stride);
        let rgb_iter = rgb.chunks_exact_mut(rgb_stride);
        let u_iter = u_plane.chunks_exact(u_stride);
        let v_iter = v_plane.chunks_exact(v_stride);

        // All branches on generic const will be optimized out.
        for (((y_src, u_src), v_src), rgba) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
            let image = YuvPlanarImage {
                y_plane: y_src,
                y_stride: 0,
                u_plane: u_src,
                u_stride: 0,
                v_plane: v_src,
                v_stride: 0,
                width: image.width,
                height: image.height,
            };
            worker(image, rgba, &inverse_transform, &range);
        }

        Ok(())
    }

    pub(crate) fn yuv444_to_rgba8(
        image: YuvPlanarImage<u8>,
        rgba: &mut [u8],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()> {
        yuv444_to_rgbx_impl::<u8, 4, 8>(image, rgba, range, matrix)
    }

    #[inline]
    fn yuv444_to_rgbx_impl<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgba: &mut [V],
        range: YuvIntensityRange,
        matrix: YuvStandardMatrix,
    ) -> Result<(), ()>
    where
        i32: AsPrimitive<V>,
    {
        assert!(
            CHANNELS == 3 || CHANNELS == 4,
            "YUV 4:4:4 -> RGB is implemented only on 3 and 4 channels"
        );
        assert!(
            (8..=16).contains(&BIT_DEPTH),
            "Invalid bit depth is provided"
        );
        assert!(
            if BIT_DEPTH > 8 {
                size_of::<V>() == 2
            } else {
                size_of::<V>() == 1
            },
            "Unsupported bit depth and data type combination"
        );

        let y_plane = image.y_plane;
        let u_plane = image.u_plane;
        let v_plane = image.v_plane;
        let y_stride = image.y_stride;
        let u_stride = image.u_stride;
        let v_stride = image.v_stride;
        let height = image.height;
        let width = image.width;

        check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, height)?;
        check_yuv_plane_preconditions(u_plane, PlaneDefinition::U, u_stride, height)?;
        check_yuv_plane_preconditions(v_plane, PlaneDefinition::V, v_stride, height)?;

        check_rgb_preconditions(rgba, image.width * CHANNELS, height)?;

        let range = range.get_yuv_range(BIT_DEPTH as u32);
        let kr_kb = matrix.get_kr_kb();
        const PRECISION: i32 = 13;

        let inverse_transform = get_inverse_transform(
            (1 << BIT_DEPTH) - 1,
            range.range_y,
            range.range_uv,
            kr_kb.kr,
            kr_kb.kb,
            PRECISION as u32,
        );

        let bias_y = range.bias_y as i32;
        let bias_uv = range.bias_uv as i32;

        let rgb_stride = width * CHANNELS;

        let y_iter = y_plane.chunks_exact(y_stride);
        let rgb_iter = rgba.chunks_exact_mut(rgb_stride);
        let u_iter = u_plane.chunks_exact(u_stride);
        let v_iter = v_plane.chunks_exact(v_stride);

        // All branches on generic const will be optimized out.
        for (((y_src, u_src), v_src), rgb) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
            let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);

            for (((y_src, u_src), v_src), rgb_dst) in
                y_src.iter().zip(u_src).zip(v_src).zip(rgb_chunks)
            {
                let y_value = y_src.as_() - bias_y;
                let cb_value = u_src.as_() - bias_uv;
                let cr_value = v_src.as_() - bias_uv;

                ycbcr_execute::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                    rgb_dst.try_into().unwrap(),
                    y_value,
                    cb_value,
                    cr_value,
                    &inverse_transform,
                );
            }
        }

        Ok(())
    }

    #[inline(always)]
    /// Saturating rounding shift right against bit depth
    pub(crate) fn qrshr<const PRECISION: i32, const BIT_DEPTH: usize>(val: i32) -> i32 {
        let rounding: i32 = 1 << (PRECISION - 1);
        let max_value: i32 = (1 << BIT_DEPTH) - 1;
        ((val + rounding) >> PRECISION).clamp(0, max_value)
    }

    /// Transformation YUV to RGB with coefficients as specified in [ITU-R](https://www.itu.int/rec/T-REC-H.273/en)
    fn get_inverse_transform(
        range_bgra: u32,
        range_y: u32,
        range_uv: u32,
        kr: f32,
        kb: f32,
        precision: u32,
    ) -> CbCrInverseTransform<i32> {
        let range_uv = range_bgra as f32 / range_uv as f32;
        let y_coef = range_bgra as f32 / range_y as f32;
        let cr_coeff = (2f32 * (1f32 - kr)) * range_uv;
        let cb_coeff = (2f32 * (1f32 - kb)) * range_uv;
        let kg = 1.0f32 - kr - kb;
        assert_ne!(kg, 0., "1.0f - kr - kg must not be 0");
        let g_coeff_1 = (2f32 * ((1f32 - kr) * kr / kg)) * range_uv;
        let g_coeff_2 = (2f32 * ((1f32 - kb) * kb / kg)) * range_uv;
        let exact_transform = CbCrInverseTransform {
            y_coef,
            cr_coef: cr_coeff,
            cb_coef: cb_coeff,
            g_coeff_1,
            g_coeff_2,
        };
        exact_transform.to_integers(precision)
    }

    #[derive(Debug, Copy, Clone)]
    /// Representation of inversion matrix
    pub(crate) struct CbCrInverseTransform<T> {
        y_coef: T,
        cr_coef: T,
        cb_coef: T,
        g_coeff_1: T,
        g_coeff_2: T,
    }

    impl CbCrInverseTransform<f32> {
        fn to_integers(self, precision: u32) -> CbCrInverseTransform<i32> {
            let precision_scale: i32 = 1i32 << (precision as i32);
            let cr_coef = (self.cr_coef * precision_scale as f32) as i32;
            let cb_coef = (self.cb_coef * precision_scale as f32) as i32;
            let y_coef = (self.y_coef * precision_scale as f32) as i32;
            let g_coef_1 = (self.g_coeff_1 * precision_scale as f32) as i32;
            let g_coef_2 = (self.g_coeff_2 * precision_scale as f32) as i32;
            CbCrInverseTransform::<i32> {
                y_coef,
                cr_coef,
                cb_coef,
                g_coeff_1: g_coef_1,
                g_coeff_2: g_coef_2,
            }
        }
    }

    #[inline]
    pub(crate) fn check_yuv_plane_preconditions<V>(
        plane: &[V],
        plane_definition: PlaneDefinition,
        stride: usize,
        height: usize,
    ) -> Result<(), ()> {
        if plane.len() != stride * height {
            return Err(());
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn check_rgb_preconditions<V>(
        rgb_data: &[V],
        stride: usize,
        height: usize,
    ) -> Result<(), ()> {
        if rgb_data.len() != stride * height {
            return Err(());
        }
        Ok(())
    }

    #[derive(Copy, Clone, Debug)]
    pub(crate) enum PlaneDefinition {
        Y,
        U,
        V,
    }

    mod ycgcg {
        use super::{
            CbCrInverseTransform, HalvedRowHandler, PlaneDefinition, YuvChromaRange,
            YuvIntensityRange, YuvPlanarImage, YuvStandardMatrix, check_rgb_preconditions,
            check_yuv_plane_preconditions, qrshr, yuv420_to_rgbx_invoker, yuv422_to_rgbx_invoker,
        };
        use num_traits::AsPrimitive;

        /// Computes YCgCo inverse in limited range
        /// # Arguments
        /// - `dst` - dest buffer
        /// - `y_value` - Y value with subtracted bias
        /// - `cb` - Cb value with subtracted bias
        /// - `cr` - Cr value with subtracted bias
        #[inline(always)]
        fn ycgco_execute_limited<
            V: Copy + AsPrimitive<i32> + 'static + Sized,
            const PRECISION: i32,
            const CHANNELS: usize,
            const BIT_DEPTH: usize,
        >(
            dst: &mut [V; CHANNELS],
            y_value: i32,
            cg: i32,
            co: i32,
            scale: i32,
        ) where
            i32: AsPrimitive<V>,
        {
            let t0 = y_value - cg;

            let r = qrshr::<PRECISION, BIT_DEPTH>((t0 + co) * scale);
            let b = qrshr::<PRECISION, BIT_DEPTH>((t0 - co) * scale);
            let g = qrshr::<PRECISION, BIT_DEPTH>((y_value + cg) * scale);

            if CHANNELS == 4 {
                dst[0] = r.as_();
                dst[1] = g.as_();
                dst[2] = b.as_();
                dst[3] = ((1i32 << BIT_DEPTH) - 1).as_();
            } else if CHANNELS == 3 {
                dst[0] = r.as_();
                dst[1] = g.as_();
                dst[2] = b.as_();
            } else {
                unreachable!();
            }
        }

        /// Computes YCgCo inverse in full range
        /// # Arguments
        /// - `dst` - dest buffer
        /// - `y_value` - Y value with subtracted bias
        /// - `cb` - Cb value with subtracted bias
        /// - `cr` - Cr value with subtracted bias
        #[inline(always)]
        fn ycgco_execute_full<
            V: Copy + AsPrimitive<i32> + 'static + Sized,
            const PRECISION: i32,
            const CHANNELS: usize,
            const BIT_DEPTH: usize,
        >(
            dst: &mut [V; CHANNELS],
            y_value: i32,
            cg: i32,
            co: i32,
        ) where
            i32: AsPrimitive<V>,
        {
            let t0 = y_value - cg;

            let max_value = (1i32 << BIT_DEPTH) - 1;

            let r = (t0 + co).clamp(0, max_value);
            let b = (t0 - co).clamp(0, max_value);
            let g = (y_value + cg).clamp(0, max_value);

            if CHANNELS == 4 {
                dst[0] = r.as_();
                dst[1] = g.as_();
                dst[2] = b.as_();
                dst[3] = max_value.as_();
            } else if CHANNELS == 3 {
                dst[0] = r.as_();
                dst[1] = g.as_();
                dst[2] = b.as_();
            } else {
                unreachable!();
            }
        }

        #[inline(always)]
        fn process_halved_chroma_row_cgco<
            V: Copy + AsPrimitive<i32> + 'static + Sized,
            const PRECISION: i32,
            const CHANNELS: usize,
            const BIT_DEPTH: usize,
        >(
            image: YuvPlanarImage<V>,
            rgba: &mut [V],
            _: &CbCrInverseTransform<i32>,
            range: &YuvChromaRange,
        ) where
            i32: AsPrimitive<V>,
        {
            let max_value = (1i32 << BIT_DEPTH) - 1;

            // If the stride is larger than the plane size,
            // it might contain junk data beyond the actual valid region.
            // To avoid processing artifacts when working with odd-sized images,
            // the buffer is reshaped to its actual size,
            // preventing accidental use of invalid values from the trailing region.

            let y_plane = &image.y_plane[..image.width];
            let chroma_size = image.width.div_ceil(2);
            let u_plane = &image.u_plane[..chroma_size];
            let v_plane = &image.v_plane[..chroma_size];
            let rgba = &mut rgba[..image.width * CHANNELS];

            let bias_y = range.bias_y as i32;
            let bias_uv = range.bias_uv as i32;
            let y_iter = y_plane.chunks_exact(2);
            let rgb_chunks = rgba.chunks_exact_mut(CHANNELS * 2);

            let scale_coef =
                ((max_value as f32 / range.range_y as f32) * (1 << PRECISION) as f32) as i32;

            for (((y_src, &u_src), &v_src), rgb_dst) in
                y_iter.zip(u_plane).zip(v_plane).zip(rgb_chunks)
            {
                let y_value0: i32 = y_src[0].as_() - bias_y;
                let cg_value: i32 = u_src.as_() - bias_uv;
                let co_value: i32 = v_src.as_() - bias_uv;

                let dst0 = &mut rgb_dst[..CHANNELS];

                ycgco_execute_limited::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                    dst0.try_into().unwrap(),
                    y_value0,
                    cg_value,
                    co_value,
                    scale_coef,
                );

                let y_value1 = y_src[1].as_() - bias_y;

                let dst1 = &mut rgb_dst[CHANNELS..2 * CHANNELS];

                ycgco_execute_limited::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                    dst1.try_into().unwrap(),
                    y_value1,
                    cg_value,
                    co_value,
                    scale_coef,
                );
            }

            // Process remainder if width is odd.
            if image.width & 1 != 0 {
                let y_left = y_plane.chunks_exact(2).remainder();
                let rgb_chunks = rgba
                    .chunks_exact_mut(CHANNELS * 2)
                    .into_remainder()
                    .chunks_exact_mut(CHANNELS);
                let u_iter = u_plane.iter().rev();
                let v_iter = v_plane.iter().rev();

                for (((y_src, u_src), v_src), rgb_dst) in
                    y_left.iter().zip(u_iter).zip(v_iter).zip(rgb_chunks)
                {
                    let y_value = y_src.as_() - bias_y;
                    let cg_value = u_src.as_() - bias_uv;
                    let co_value = v_src.as_() - bias_uv;

                    ycgco_execute_limited::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                        rgb_dst.try_into().unwrap(),
                        y_value,
                        cg_value,
                        co_value,
                        scale_coef,
                    );
                }
            }
        }

        /// Converts YCgCo 444 planar format to Rgba
        ///
        /// # Arguments
        ///
        /// * `image`: see [YuvPlanarImage]
        /// * `rgba`: RGB image layout
        /// * `range`: see [YuvIntensityRange]
        ///
        fn ycgco444_to_rgbx_impl<
            V: Copy + AsPrimitive<i32> + 'static + Sized,
            const CHANNELS: usize,
            const BIT_DEPTH: usize,
        >(
            image: YuvPlanarImage<V>,
            rgba: &mut [V],
            yuv_range: YuvIntensityRange,
        ) -> Result<(), ()>
        where
            i32: AsPrimitive<V>,
        {
            assert!(
                CHANNELS == 3 || CHANNELS == 4,
                "YUV 4:4:4 -> RGB is implemented only on 3 and 4 channels"
            );
            assert!(
                (8..=16).contains(&BIT_DEPTH),
                "Invalid bit depth is provided"
            );
            assert!(
                if BIT_DEPTH > 8 {
                    size_of::<V>() == 2
                } else {
                    size_of::<V>() == 1
                },
                "Unsupported bit depth and data type combination"
            );

            let y_plane = image.y_plane;
            let u_plane = image.u_plane;
            let v_plane = image.v_plane;
            let y_stride = image.y_stride;
            let u_stride = image.u_stride;
            let v_stride = image.v_stride;
            let height = image.height;
            let width = image.width;

            check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, height)?;
            check_yuv_plane_preconditions(u_plane, PlaneDefinition::U, u_stride, height)?;
            check_yuv_plane_preconditions(v_plane, PlaneDefinition::V, v_stride, height)?;

            check_rgb_preconditions(rgba, image.width * CHANNELS, height)?;

            let range = yuv_range.get_yuv_range(BIT_DEPTH as u32);
            const PRECISION: i32 = 13;

            let bias_y = range.bias_y as i32;
            let bias_uv = range.bias_uv as i32;

            let rgb_stride = width * CHANNELS;

            let y_iter = y_plane.chunks_exact(y_stride);
            let rgb_iter = rgba.chunks_exact_mut(rgb_stride);
            let u_iter = u_plane.chunks_exact(u_stride);
            let v_iter = v_plane.chunks_exact(v_stride);

            let max_value: i32 = (1 << BIT_DEPTH) - 1;

            // All branches on generic const will be optimized out.
            for (((y_src, u_src), v_src), rgb) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
                let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);
                match yuv_range {
                    YuvIntensityRange::Tv => {
                        let y_coef = ((max_value as f32 / range.range_y as f32)
                            * (1 << PRECISION) as f32) as i32;
                        for (((y_src, u_src), v_src), rgb_dst) in
                            y_src.iter().zip(u_src).zip(v_src).zip(rgb_chunks)
                        {
                            let y_value = y_src.as_() - bias_y;
                            let cg_value = u_src.as_() - bias_uv;
                            let co_value = v_src.as_() - bias_uv;

                            ycgco_execute_limited::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                                rgb_dst.try_into().unwrap(),
                                y_value,
                                cg_value,
                                co_value,
                                y_coef,
                            );
                        }
                    }
                    YuvIntensityRange::Pc => {
                        for (((y_src, u_src), v_src), rgb_dst) in
                            y_src.iter().zip(u_src).zip(v_src).zip(rgb_chunks)
                        {
                            let y_value = y_src.as_() - bias_y;
                            let cg_value = u_src.as_() - bias_uv;
                            let co_value = v_src.as_() - bias_uv;

                            ycgco_execute_full::<V, PRECISION, CHANNELS, BIT_DEPTH>(
                                rgb_dst.try_into().unwrap(),
                                y_value,
                                cg_value,
                                co_value,
                            );
                        }
                    }
                }
            }

            Ok(())
        }

        macro_rules! define_ycgco_half_chroma {
            ($name: ident, $invoker: ident, $storage: ident, $cn: expr, $bp: expr, $description: expr) => {
                #[doc = concat!($description, "
                
                # Arguments
                
                * `image`: see [YuvPlanarImage]
                * `rgb`: RGB image layout
                * `range`: see [YuvIntensityRange]
                * `matrix`: see [YuvStandardMatrix]")]
                pub(crate) fn $name(
                    image: YuvPlanarImage<$storage>,
                    rgb: &mut [$storage],
                    range: YuvIntensityRange,
                ) -> Result<(), ()> {
                    const P: i32 = 13;
                    $invoker::<$storage, HalvedRowHandler<$storage>, P, $cn, $bp>(
                        image,
                        rgb,
                        range,
                        YuvStandardMatrix::Bt709,
                        process_halved_chroma_row_cgco::<$storage, P, $cn, $bp>,
                    )
                }
            };
        }

        const RGBA_CN: usize = 4;

        define_ycgco_half_chroma!(
            ycgco420_to_rgba8,
            yuv420_to_rgbx_invoker,
            u8,
            RGBA_CN,
            8,
            "Converts YCgCo 420 8-bit planar format to Rgba 8-bit"
        );

        define_ycgco_half_chroma!(
            ycgco422_to_rgba8,
            yuv422_to_rgbx_invoker,
            u8,
            RGBA_CN,
            8,
            "Converts YCgCo 420 8-bit planar format to Rgba 8-bit"
        );

        define_ycgco_half_chroma!(
            ycgco420_to_rgba10,
            yuv420_to_rgbx_invoker,
            u16,
            RGBA_CN,
            10,
            "Converts YCgCo 420 10-bit planar format to Rgba 10-bit"
        );

        define_ycgco_half_chroma!(
            ycgco422_to_rgba10,
            yuv422_to_rgbx_invoker,
            u16,
            RGBA_CN,
            10,
            "Converts YCgCo 422 10-bit planar format to Rgba 10-bit"
        );

        define_ycgco_half_chroma!(
            ycgco420_to_rgba12,
            yuv420_to_rgbx_invoker,
            u16,
            RGBA_CN,
            12,
            "Converts YCgCo 420 12-bit planar format to Rgba 12-bit"
        );

        define_ycgco_half_chroma!(
            ycgco422_to_rgba12,
            yuv422_to_rgbx_invoker,
            u16,
            RGBA_CN,
            12,
            "Converts YCgCo 422 12-bit planar format to Rgba 12-bit"
        );

        macro_rules! define_ycgcg_full_chroma {
            ($name: ident, $storage: ident, $cn: expr, $bp: expr, $description: expr) => {
                #[doc = concat!($description, "
                
                # Arguments
                
                * `image`: see [YuvPlanarImage]
                * `rgba`: RGB image layout
                * `range`: see [YuvIntensityRange]
                * `matrix`: see [YuvStandardMatrix]
                ")]
                pub(crate) fn $name(
                    image: YuvPlanarImage<$storage>,
                    rgba: &mut [$storage],
                    range: YuvIntensityRange,
                ) -> Result<(), ()> {
                    ycgco444_to_rgbx_impl::<$storage, $cn, $bp>(image, rgba, range)
                }
            };
        }

        define_ycgcg_full_chroma!(
            ycgco444_to_rgba8,
            u8,
            RGBA_CN,
            8,
            "Converts YCgCo 444 planar format 8 bit-depth to Rgba 8 bit"
        );
        define_ycgcg_full_chroma!(
            ycgco444_to_rgba10,
            u16,
            RGBA_CN,
            10,
            "Converts YCgCo 444 planar format 10 bit-depth to Rgba 10 bit"
        );
        define_ycgcg_full_chroma!(
            ycgco444_to_rgba12,
            u16,
            RGBA_CN,
            12,
            "Converts YCgCo 444 planar format 12 bit-depth to Rgba 12 bit"
        );
    }

    pub(crate) fn gbr_to_rgba8(
        image: YuvPlanarImage<u8>,
        rgb: &mut [u8],
        range: YuvIntensityRange,
    ) -> Result<(), ()> {
        gbr_to_rgbx_impl::<u8, 4, 8>(image, rgb, range)
    }

    #[inline]
    fn gbr_to_rgbx_impl<
        V: Copy + AsPrimitive<i32> + 'static + Sized,
        const CHANNELS: usize,
        const BIT_DEPTH: usize,
    >(
        image: YuvPlanarImage<V>,
        rgba: &mut [V],
        yuv_range: YuvIntensityRange,
    ) -> Result<(), ()>
    where
        i32: AsPrimitive<V>,
    {
        assert!(
            CHANNELS == 3 || CHANNELS == 4,
            "GBR -> RGB is implemented only on 3 and 4 channels"
        );
        assert!(
            (8..=16).contains(&BIT_DEPTH),
            "Invalid bit depth is provided"
        );
        assert!(
            if BIT_DEPTH > 8 {
                size_of::<V>() == 2
            } else {
                size_of::<V>() == 1
            },
            "Unsupported bit depth and data type combination"
        );
        let y_plane = image.y_plane;
        let u_plane = image.u_plane;
        let v_plane = image.v_plane;
        let y_stride = image.y_stride;
        let u_stride = image.u_stride;
        let v_stride = image.v_stride;
        let height = image.height;
        let width = image.width;

        check_yuv_plane_preconditions(y_plane, PlaneDefinition::Y, y_stride, height)?;
        check_yuv_plane_preconditions(u_plane, PlaneDefinition::U, u_stride, height)?;
        check_yuv_plane_preconditions(v_plane, PlaneDefinition::V, v_stride, height)?;

        check_rgb_preconditions(rgba, width * CHANNELS, height)?;

        let max_value = (1 << BIT_DEPTH) - 1;

        let rgb_stride = width * CHANNELS;

        let y_iter = y_plane.chunks_exact(y_stride);
        let rgb_iter = rgba.chunks_exact_mut(rgb_stride);
        let u_iter = u_plane.chunks_exact(u_stride);
        let v_iter = v_plane.chunks_exact(v_stride);

        match yuv_range {
            YuvIntensityRange::Tv => {
                const PRECISION: i32 = 11;
                // All channels on identity should use Y range
                let range = yuv_range.get_yuv_range(BIT_DEPTH as u32);
                let range_rgba = (1 << BIT_DEPTH) - 1;
                let y_coef =
                    ((range_rgba as f32 / range.range_y as f32) * (1 << PRECISION) as f32) as i32;
                let y_bias = range.bias_y as i32;

                for (((y_src, u_src), v_src), rgb) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
                    let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);

                    for (((&y_src, &u_src), &v_src), rgb_dst) in
                        y_src.iter().zip(u_src).zip(v_src).zip(rgb_chunks)
                    {
                        rgb_dst[0] =
                            qrshr::<PRECISION, BIT_DEPTH>((v_src.as_() - y_bias) * y_coef).as_();
                        rgb_dst[1] =
                            qrshr::<PRECISION, BIT_DEPTH>((y_src.as_() - y_bias) * y_coef).as_();
                        rgb_dst[2] =
                            qrshr::<PRECISION, BIT_DEPTH>((u_src.as_() - y_bias) * y_coef).as_();
                        if CHANNELS == 4 {
                            rgb_dst[3] = max_value.as_();
                        }
                    }
                }
            }
            YuvIntensityRange::Pc => {
                for (((y_src, u_src), v_src), rgb) in y_iter.zip(u_iter).zip(v_iter).zip(rgb_iter) {
                    let rgb_chunks = rgb.chunks_exact_mut(CHANNELS);

                    for (((&y_src, &u_src), &v_src), rgb_dst) in
                        y_src.iter().zip(u_src).zip(v_src).zip(rgb_chunks)
                    {
                        rgb_dst[0] = v_src;
                        rgb_dst[1] = y_src;
                        rgb_dst[2] = u_src;
                        if CHANNELS == 4 {
                            rgb_dst[3] = max_value.as_();
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
