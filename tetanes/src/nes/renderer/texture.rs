use egui::{load::SizedTexture, TextureId, Vec2};

#[derive(Debug)]
#[must_use]
pub struct Texture {
    pub label: Option<&'static str>,
    pub id: TextureId,
    pub texture: wgpu::Texture,
    pub size: wgpu::Extent3d,
    pub view: wgpu::TextureView,
    pub aspect_ratio: f32,
}

impl Texture {
    pub fn new(
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        width: u32,
        height: u32,
        aspect_ratio: f32,
        label: Option<&'static str>,
    ) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label,
            dimension: Some(wgpu::TextureViewDimension::D2),
            ..Default::default()
        });
        let sampler_descriptor = wgpu::SamplerDescriptor {
            label: Some("sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        };

        let id = renderer.register_native_texture_with_sampler_options(
            device,
            &view,
            sampler_descriptor,
        );

        Self {
            label,
            texture,
            size,
            view,
            aspect_ratio,
            id,
        }
    }

    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        width: u32,
        height: u32,
        aspect_ratio: f32,
    ) {
        renderer.free_texture(&self.id);
        *self = Self::new(device, renderer, width, height, aspect_ratio, self.label);
    }

    pub fn sized_texture(&self) -> SizedTexture {
        SizedTexture::new(
            self.id,
            Vec2 {
                x: self.size.width as f32 * self.aspect_ratio,
                y: self.size.height as f32,
            },
        )
    }

    pub fn update(&self, queue: &wgpu::Queue, bytes: &[u8]) {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                aspect: wgpu::TextureAspect::All,
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
            },
            bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * self.size.width),
                rows_per_image: Some(self.size.height),
            },
            self.size,
        );
    }
}
