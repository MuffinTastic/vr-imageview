use std::borrow::Cow;

use log::debug;

use crate::{constants::MULTIVIEW_MASK, image::Image, uniform::UniformBuffer};

pub struct Pipeline {
    _shader: wgpu::ShaderModule,
    _pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Pipeline {
    pub fn new(
        wgpu_device: &wgpu::Device,
        image: &Image,
        uniform: &UniformBuffer,
        swapchain_format: wgpu::TextureFormat,
    ) -> Self {
        // Unfortunately wgpu doesn't support skipping naga yet...
        // https://github.com/gfx-rs/wgpu/issues/9052

        let spirv: &[u32] = bytemuck::cast_slice(include_bytes!(env!("SHADER_SPV_PATH")));

        let _shader = unsafe {
            wgpu_device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                label: Some("combined shader"),
                spirv: Some(std::borrow::Cow::Borrowed(spirv)),
                entry_points: Cow::Borrowed(&[
                    wgpu::PassthroughShaderEntryPoint {
                        name: Cow::Borrowed("vs_main"),
                        workgroup_size: (0, 0, 0),
                    },
                    wgpu::PassthroughShaderEntryPoint {
                        name: Cow::Borrowed("fs_main"),
                        workgroup_size: (0, 0, 0),
                    },
                ]),
                dxil: None,
                metallib: None,
                msl: None,
                hlsl: None,
                glsl: None,
                wgsl: None,
            })
        };

        let _pipeline_layout =
            wgpu_device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Pipeline layout"),
                bind_group_layouts: &[
                    Some(image.bind_group_layout()),
                    Some(uniform.bind_group_layout()),
                ],
                immediate_size: 0,
            });

        let pipeline = wgpu_device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Pipeline"),
            layout: Some(&_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: swapchain_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: MULTIVIEW_MASK,
            cache: None,
        });

        debug!("Created pipeline");

        Pipeline {
            _shader,
            _pipeline_layout,
            pipeline,
        }
    }

    pub fn inner(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}
