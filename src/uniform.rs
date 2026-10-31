use log::debug;
use wgpu::util::DeviceExt;

use crate::{camera::Camera, image::Image};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UniformBufferData {
    camera_mat: [[f32; 4]; 3],
    camera_perspective_correction: u32,
    _pad: [u32; 1],
    image_scale: [f32; 2],
}

impl UniformBufferData {
    fn pad_matrix(mat: [[f32; 3]; 3]) -> [[f32; 4]; 3] {
        mat.map(|row| [row[0], row[1], row[2], 1.0f32])
    }
}

impl From<(&Camera, &Image)> for UniformBufferData {
    fn from((camera, image): (&Camera, &Image)) -> Self {
        UniformBufferData {
            camera_mat: UniformBufferData::pad_matrix(camera.matrix().into()),
            camera_perspective_correction: if camera.perspective_correction() {
                1
            } else {
                0
            },
            _pad: [0; 1],
            image_scale: image.calc_scale().into(),
        }
    }
}

pub struct UniformBuffer {
    buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
}

impl UniformBuffer {
    pub fn new(wgpu_device: &wgpu::Device, camera: &Camera, image: &Image) -> Self {
        debug!("Uniform buffer creation test: {:?}", UniformBufferData::from((camera, image)));

        let buffer = wgpu_device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform buffer"),
            contents: bytemuck::cast_slice(&[UniformBufferData::from((camera, image))]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let (bind_group_layout, bind_group) =
            UniformBuffer::create_bind_group(wgpu_device, &buffer);

        debug!("Created uniform buffer");

        UniformBuffer {
            buffer,
            bind_group_layout,
            bind_group,
        }
    }

    pub fn update(&self, wgpu_queue: &wgpu::Queue, camera: &Camera, image: &Image) {
        wgpu_queue.write_buffer(
            &self.buffer,
            0,
            bytemuck::cast_slice(&[UniformBufferData::from((camera, image))]),
        );
    }

    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    fn create_bind_group(
        wgpu_device: &wgpu::Device,
        buffer: &wgpu::Buffer,
    ) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
        let bind_group_layout =
            wgpu_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let bind_group = wgpu_device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        (bind_group_layout, bind_group)
    }
}
