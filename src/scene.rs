use std::path::Path;

use log::info;
use nalgebra::Vector2;

use crate::{
    camera::Camera,
    image::{Image, ImageData},
    pipeline::Pipeline,
    uniform::UniformBuffer,
    xr::XrInputSnapshot,
};

pub struct Scene {
    camera: Camera,
    image: Image,
    uniform: UniformBuffer,
    pipeline: Pipeline,
}

impl Scene {
    pub fn new(
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        image_path: &Path,
        swapchain_format: wgpu::TextureFormat,
        perspective_correction: bool,
        square_aspect: bool,
    ) -> Scene {
        let image_data =
            ImageData::load_from(image_path, wgpu_device.limits().max_texture_dimension_2d)
                .expect("image data");

        let image = Image::new(wgpu_device, wgpu_queue, image_data, square_aspect);

        let mut camera = Camera::new(perspective_correction);
        camera.set_position(image.calc_scale() / 2.0);

        let uniform = UniformBuffer::new(wgpu_device, &camera, &image);

        let pipeline = Pipeline::new(wgpu_device, &image, &uniform, swapchain_format);

        Scene {
            camera,
            image,
            uniform,
            pipeline,
        }
    }

    pub fn update(&mut self, xr_input_snapshot: XrInputSnapshot, delta: f32) {
        if xr_input_snapshot.reset {
            self.camera.set_position(self.image.calc_scale() / 2.0);
            self.camera.set_rotation(0.0);
            self.camera.set_zoom(0.0);
            info!("Reset camera");
        }

        if xr_input_snapshot.toggle_perspective_correction {
            self.camera.toggle_perspective_correction();
            info!("Toggled perspective correction");
        }

        if xr_input_snapshot.toggle_aspect {
            self.image.toggle_aspect();
            info!("Toggled square aspect ratio");
        }

        self.camera.rotate(xr_input_snapshot.rotate * delta);
        self.camera.zoom(xr_input_snapshot.zoom * delta);
        self.camera.translate(
            xr_input_snapshot
                .movement
                .component_mul(&Vector2::new(1.0f32, -1.0f32))
                * delta,
        );

        if xr_input_snapshot.change_clamp {
            self.image.cycle_clamp();
            info!("Changed image clamp setting");
        }
    }

    pub fn pre_render(&self, wgpu_queue: &wgpu::Queue) {
        self.uniform.update(wgpu_queue, &self.camera, &self.image);
    }

    pub fn render(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.set_pipeline(self.pipeline.inner());
        render_pass.set_bind_group(0, self.image.bind_group(), &[]);
        render_pass.set_bind_group(1, self.uniform.bind_group(), &[]);
        render_pass.draw(0..3, 0..1);
    }
}
