use std::{io, path::Path};

use image::ImageReader;
use log::debug;
use nalgebra::Vector2;

pub struct ImageData {
    pub dimensions: Vector2<u32>,
    pub data: Vec<u8>,
}

impl ImageData {
    pub fn load_from(path: &Path, max_dimensions: u32) -> io::Result<Self> {
        let img = ImageReader::open(path)?
            .decode()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "couldnt decode"))?;

        let dimensions = Vector2::new(img.width(), img.height());

        if dimensions.x > max_dimensions || dimensions.y > max_dimensions {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("png dimensions above max dimensions ({max_dimensions}^2)"),
            ));
        }

        let data = img.to_rgba8().into_raw();

        debug!("Loaded image data with size {dimensions}");

        Ok(ImageData { dimensions, data })
    }
}

pub struct Image {
    dimensions: Vector2<f32>,
    square_aspect: bool,
    clamp_index: usize,

    _texture: wgpu::Texture,
    _texture_view: wgpu::TextureView,
    _samplers: [wgpu::Sampler; 4],
    bind_group_layout: wgpu::BindGroupLayout,
    bind_groups: [wgpu::BindGroup; 4],
}

impl Image {
    pub fn new(
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        data: ImageData,
        square_aspect: bool,
    ) -> Self {
        let dimensions = data.dimensions.cast();

        let (_texture, _texture_view, _samplers) =
            Image::create_and_upload_texture(wgpu_device, wgpu_queue, &data);
        let (bind_group_layout, bind_groups) =
            Image::create_bind_groups(wgpu_device, &_texture_view, &_samplers);

        debug!("Created image");

        let image = Image {
            dimensions,
            square_aspect,
            clamp_index: wgpu::AddressMode::ClampToBorder as usize,
            _texture,
            _texture_view,
            _samplers,
            bind_group_layout,
            bind_groups,
        };

        debug!("Image scale calc test: {}", image.calc_scale());

        image
    }

    pub fn toggle_aspect(&mut self) {
        self.square_aspect = !self.square_aspect;
    }

    pub fn cycle_clamp(&mut self) {
        self.clamp_index += 1;

        if self.clamp_index > 3 {
            self.clamp_index = 0;
        }
    }

    pub fn calc_scale(&self) -> Vector2<f32> {
        if self.square_aspect {
            Vector2::new(1.0, 1.0)
        } else {
            let scale = 1.0 / self.dimensions.max();
            self.dimensions * scale
        }
    }

    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_groups[self.clamp_index]
    }

    fn create_and_upload_texture(
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        image_data: &ImageData,
    ) -> (wgpu::Texture, wgpu::TextureView, [wgpu::Sampler; 4]) {
        let texture_size = wgpu::Extent3d {
            width: image_data.dimensions.x,
            height: image_data.dimensions.y,
            depth_or_array_layers: 1,
        };

        let texture = wgpu_device.create_texture(&wgpu::TextureDescriptor {
            label: Some("User PNG"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        wgpu_queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image_data.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image_data.dimensions.x),
                rows_per_image: Some(image_data.dimensions.y),
            },
            texture_size,
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let samplers = [
            wgpu::AddressMode::ClampToEdge,
            wgpu::AddressMode::Repeat,
            wgpu::AddressMode::MirrorRepeat,
            wgpu::AddressMode::ClampToBorder,
        ]
        .map(|mode| {
            wgpu_device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        });

        (texture, texture_view, samplers)
    }

    fn create_bind_groups(
        wgpu_device: &wgpu::Device,
        texture_view: &wgpu::TextureView,
        samplers: &[wgpu::Sampler; 4],
    ) -> (wgpu::BindGroupLayout, [wgpu::BindGroup; 4]) {
        let bind_group_layout =
            wgpu_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Image bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let bind_groups = samplers.each_ref().map(|sampler| {
            wgpu_device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Image bind group"),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });

        (bind_group_layout, bind_groups)
    }
}
