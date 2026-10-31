use ash::vk::{self, Handle};
use log::{debug, info, warn};
use nalgebra::Vector2;

use crate::xr_wgpu_helper;

pub struct XrInputSnapshot {
    pub movement: Vector2<f32>,
    pub zoom: f32,
    pub rotate: f32,
    pub change_clamp: bool,
    pub reset: bool,
    pub toggle_perspective_correction: bool,
    pub toggle_aspect: bool,
}

pub struct XrContext {
    always_render: bool,
    session_running: bool,

    swapchain_buffers: Vec<wgpu::Texture>,
    swapchain: openxr::Swapchain<openxr::Vulkan>,

    action_set: XrActionSet,

    space: openxr::Space,

    resolution: Vector2<u32>,
    swapchain_dirty: bool,

    frame_stream: openxr::FrameStream<openxr::Vulkan>,
    frame_wait: openxr::FrameWaiter,
    session: openxr::Session<openxr::Vulkan>,

    instance: openxr::Instance,
    system: openxr::SystemId,

    vk_entry: Option<ash::Entry>,
}

impl XrContext {
    pub fn new(always_render: bool) -> (Self, wgpu::Device, wgpu::Queue) {
        let xr_app_info = openxr::ApplicationInfo {
            application_name: "vr-imageview",
            application_version: 1,
            engine_name: "custom",
            engine_version: 1,
            api_version: openxr::Version::new(1, 0, 0),
        };

        if always_render {
            debug!("always_render set to true");
        }

        let xr_platform_info = ();
        let xr_entry = openxr::Entry::linked(&xr_platform_info).expect("xr entry");
        let mut xr_requirements = openxr::ExtensionSet::default();

        debug!("Got XR entry");

        xr_requirements.khr_vulkan_enable2 = true; // We're using vulkan
        xr_requirements.ext_view_configuration_views_change = true; // Lets us update our swapchains on demand

        let instance = xr_entry
            .create_instance(&xr_app_info, &xr_requirements, &[], &xr_platform_info)
            .expect("xr instance");
        debug!("Created XR instance");

        let system = instance
            .system(openxr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .expect("xr system");
        debug!("Created XR system");

        let (vk_entry, wgpu_device, wgpu_queue, xr_session_create_info) =
            xr_wgpu_helper::create_wgpu_device_and_queue_from_xr(&instance, system).expect("wgpu");

        let (session, frame_wait, frame_stream) = unsafe {
            instance
                .create_session(system, &xr_session_create_info)
                .expect("xr session")
        };
        debug!("Created xr session");

        let action_set = XrContext::create_action_set(&instance, &session);

        debug!("Created xr action sets");

        let space = session
            .create_reference_space(openxr::ReferenceSpaceType::STAGE, openxr::Posef::IDENTITY)
            .expect("xr create space");

        let resolution = XrContext::get_recommended_resolution(&instance, system);

        let (swapchain, swapchain_buffers) =
            XrContext::create_swapchain(&wgpu_device, &session, resolution);

        debug!("Created swapchain");

        let context = XrContext {
            always_render,
            session_running: true,
            vk_entry: Some(vk_entry),
            instance,
            system,
            session,
            frame_wait,
            frame_stream,
            space,
            resolution,
            swapchain,
            swapchain_buffers,
            swapchain_dirty: false,
            action_set,
        };

        info!("XR context created");

        (context, wgpu_device, wgpu_queue)
    }

    pub fn take_vk_entry(&mut self) -> ash::Entry {
        self.vk_entry.take().expect("take vk entry")
    }

    fn get_recommended_resolution(
        instance: &openxr::Instance,
        system: openxr::SystemId,
    ) -> Vector2<u32> {
        let views = instance
            .enumerate_view_configuration_views(
                system,
                openxr::ViewConfigurationType::PRIMARY_STEREO,
            )
            .expect("xr views");

        Vector2::new(
            views[0].recommended_image_rect_width,
            views[0].recommended_image_rect_height,
        )
    }

    fn create_action_set(
        instance: &openxr::Instance,
        session: &openxr::Session<openxr::Vulkan>,
    ) -> XrActionSet {
        let action_set = instance
            .create_action_set("gameplay", "Gameplay Actions", 0)
            .expect("xr action set");

        let move_action = action_set
            .create_action::<openxr::Vector2f>("move", "Move", &[])
            .expect("create move action");

        let zoom_action = action_set
            .create_action::<openxr::Vector2f>("zoom", "Zoom", &[])
            .expect("create zoom action");

        let rotate_ccw_action = action_set
            .create_action::<f32>("ccw", "Rotate CCW", &[])
            .expect("create ccw action");

        let rotate_cw_action = action_set
            .create_action::<f32>("cw", "Rotate CW", &[])
            .expect("create cw action");

        let clamp_action = action_set
            .create_action::<bool>("clamp", "Change Clamp", &[])
            .expect("create clamp action");

        let perspective_action = action_set
            .create_action::<bool>("perspective", "Toggle perspective correction", &[])
            .expect("create perspective action");

        let reset_action = action_set
            .create_action::<bool>("reset", "Reset", &[])
            .expect("create reset action");

        let aspect_action = action_set
            .create_action::<bool>("aspect", "Toggle Aspect Ratio", &[])
            .expect("create aspect action");

        instance
            .suggest_interaction_profile_bindings(
                instance
                    .string_to_path("/interaction_profiles/oculus/touch_controller")
                    .expect("touch_controller path"),
                &[
                    openxr::Binding::new(
                        &move_action,
                        instance
                            .string_to_path("/user/hand/left/input/thumbstick")
                            .expect("left thumbstick path"),
                    ),
                    openxr::Binding::new(
                        &zoom_action,
                        instance
                            .string_to_path("/user/hand/right/input/thumbstick")
                            .expect("right thumbstick path"),
                    ),
                    openxr::Binding::new(
                        &rotate_ccw_action,
                        instance
                            .string_to_path("/user/hand/left/input/trigger/value")
                            .expect("left trigger path"),
                    ),
                    openxr::Binding::new(
                        &rotate_cw_action,
                        instance
                            .string_to_path("/user/hand/right/input/trigger/value")
                            .expect("right trigger path"),
                    ),
                    openxr::Binding::new(
                        &clamp_action,
                        instance
                            .string_to_path("/user/hand/left/input/x/click")
                            .expect("left x path"),
                    ),
                    openxr::Binding::new(
                        &perspective_action,
                        instance
                            .string_to_path("/user/hand/left/input/y/click")
                            .expect("left y path"),
                    ),
                    openxr::Binding::new(
                        &reset_action,
                        instance
                            .string_to_path("/user/hand/right/input/a/click")
                            .expect("right a path"),
                    ),
                    openxr::Binding::new(
                        &aspect_action,
                        instance
                            .string_to_path("/user/hand/right/input/b/click")
                            .expect("right b path"),
                    ),
                ],
            )
            .expect("xr suggest profile bindings");

        session
            .attach_action_sets(&[&action_set])
            .expect("xr session action sets");

        XrActionSet {
            move_action,
            zoom_action,
            rotate_ccw_action,
            rotate_cw_action,
            clamp_action,
            perspective_action,
            reset_action,
            aspect_action,
            action_set,
        }
    }

    fn create_swapchain(
        wgpu_device: &wgpu::Device,
        session: &openxr::Session<openxr::Vulkan>,
        resolution: Vector2<u32>,
    ) -> (openxr::Swapchain<openxr::Vulkan>, Vec<wgpu::Texture>) {
        let swapchain = session
            .create_swapchain(&openxr::SwapchainCreateInfo {
                create_flags: openxr::SwapchainCreateFlags::EMPTY,
                usage_flags: openxr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | openxr::SwapchainUsageFlags::SAMPLED,
                format: vk::Format::R8G8B8A8_SRGB.as_raw() as _,
                sample_count: 1,
                width: resolution.x,
                height: resolution.y,
                face_count: 1,
                array_size: 2,
                mip_count: 1,
            })
            .expect("xr swapchain");

        let swapchain_images = swapchain.enumerate_images().expect("swapchain images");

        let swapchain_buffers: Vec<wgpu::Texture> = swapchain_images
            .into_iter()
            .map(|color_image| {
                let color_image = vk::Image::from_raw(color_image);

                let wgpu_hal_texture = unsafe {
                    let hal_dev = wgpu_device
                        .as_hal::<wgpu_hal::vulkan::Api>()
                        .expect("hal device for swapchain");
                    hal_dev.texture_from_raw(
                        color_image,
                        &wgpu_hal::TextureDescriptor {
                            label: Some("VR Swapchain"),
                            size: wgpu::Extent3d {
                                width: resolution.x,
                                height: resolution.y,
                                depth_or_array_layers: 2,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8UnormSrgb,
                            usage: wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::COPY_DST,
                            memory_flags: wgpu_hal::MemoryFlags::empty(),
                            view_formats: vec![],
                        },
                        Some(Box::new(|| {})), // Necessary to prevent a double-free
                        wgpu_hal::vulkan::TextureMemory::External,
                    )
                };

                unsafe {
                    wgpu_device.create_texture_from_hal::<wgpu_hal::vulkan::Api>(
                        wgpu_hal_texture,
                        &wgpu::TextureDescriptor {
                            label: Some("VR Swapchain"),
                            size: wgpu::Extent3d {
                                width: resolution.x,
                                height: resolution.y,
                                depth_or_array_layers: 2,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8UnormSrgb,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                        wgpu::TextureUses::UNINITIALIZED,
                    )
                }
            })
            .collect();

        (swapchain, swapchain_buffers)
    }

    pub fn update_swapchain(&mut self, wgpu_device: &wgpu::Device) {
        if !self.swapchain_dirty {
            return;
        }

        let new_resolution = XrContext::get_recommended_resolution(&self.instance, self.system);

        if self.resolution == new_resolution {
            // Resolution didn't actually change, bail
            return;
        }

        let (mut new_swapchain, mut new_swapchain_buffers) =
            XrContext::create_swapchain(wgpu_device, &self.session, new_resolution);

        self.resolution = new_resolution;
        std::mem::swap(&mut self.swapchain, &mut new_swapchain);
        std::mem::swap(&mut self.swapchain_buffers, &mut new_swapchain_buffers);
        self.swapchain_dirty = false;

        debug!(
            "Updated swapchain to ({}, {})",
            new_resolution.x, new_resolution.y
        );
    }

    pub fn poll_events(&mut self) -> bool {
        let mut buffer = openxr::EventDataBuffer::new();

        while let Some(event) = self
            .instance
            .poll_event(&mut buffer)
            .expect("xr event poll")
        {
            match event {
                openxr::Event::SessionStateChanged(state_change) => {
                    log::info!("XR Session state changed to: {:?}", state_change.state());
                    match state_change.state() {
                        openxr::SessionState::READY => {
                            self.session
                                .begin(openxr::ViewConfigurationType::PRIMARY_STEREO)
                                .expect("xr begin session");
                            self.session_running = true;
                            log::info!("XR Session started");
                        }
                        openxr::SessionState::STOPPING => {
                            self.session_running = false;
                            self.session.end().expect("xr end session");
                            log::info!("XR Session ended");
                        }
                        openxr::SessionState::EXITING | openxr::SessionState::LOSS_PENDING => {
                            self.session_running = false;
                            log::info!("XR Session exiting");
                            return false;
                        }
                        _ => {}
                    }
                }
                openxr::Event::InstanceLossPending(_) => {
                    log::info!("XR Instance loss pending");
                    return false;
                }
                openxr::Event::ViewConfigurationViewsChangedEXT(_) => {
                    self.swapchain_dirty = true;
                }
                _ => {}
            }
        }

        true
    }

    pub fn is_session_running(&self) -> bool {
        self.session_running
    }

    pub fn swapchain_format(&self) -> wgpu::TextureFormat {
        self.swapchain_buffers[0].format()
    }

    pub fn shutdown(&mut self) {
        if !self.session_running {
            return;
        }

        if let Err(error) = self.session.request_exit() {
            warn!("error during xr session shutdown request: {error}");
            self.session_running = false;
            return;
        }

        let mut buffer = openxr::EventDataBuffer::new();

        for _ in 0..200 {
            match self.instance.poll_event(&mut buffer) {
                Ok(Some(openxr::Event::SessionStateChanged(state_change))) => {
                    match state_change.state() {
                        openxr::SessionState::STOPPING => {
                            if let Err(error) = self.session.end() {
                                warn!("error during xr session shutdown request: {error}");
                                break;
                            }
                        }
                        openxr::SessionState::EXITING => break,
                        _ => {}
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(error) => {
                    warn!("error during xr session shutdown request: {error}");
                    break;
                }
            }
        }

        self.session_running = false;
    }

    pub fn wait_frame(&mut self) -> openxr::FrameState {
        self.frame_wait.wait().expect("xr frame wait")
    }

    pub fn get_input_snapshot(&self) -> XrInputSnapshot {
        self.session
            .sync_actions(&[(&self.action_set.action_set).into()])
            .expect("xr sync actions");

        let move_state = self
            .action_set
            .move_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr move action state");

        let zoom_state = self
            .action_set
            .zoom_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr zoom action state");

        let ccw_state = self
            .action_set
            .rotate_ccw_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr ccw action state");

        let cw_state = self
            .action_set
            .rotate_cw_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr cw action state");

        let clamp_state = self
            .action_set
            .clamp_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr clamp action state");

        let perspective_state = self
            .action_set
            .perspective_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr perspective action state");

        let reset_state = self
            .action_set
            .reset_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr reset action state");

        let aspect_state = self
            .action_set
            .aspect_action
            .state(&self.session, openxr::Path::NULL)
            .expect("xr aspect action state");

        let movement = Vector2::new(move_state.current_state.x, move_state.current_state.y);
        let movement = if movement.magnitude() > 0.1 {
            movement
        } else {
            Vector2::zeros()
        };

        let zoom_x = zoom_state.current_state.x;
        let zoom_y = -zoom_state.current_state.y;

        let zoom_sign = if (zoom_x + zoom_y).is_sign_positive() {
            1.0
        } else {
            -1.0
        };
        let zoom_mag = (zoom_x * zoom_x + zoom_y * zoom_y).sqrt();

        let zoom = if (zoom_x + zoom_y).abs() > 0.1 {
            zoom_mag * zoom_sign
        } else {
            0.0
        };

        let rot = ccw_state.current_state - cw_state.current_state;

        let change_clamp = clamp_state.changed_since_last_sync && clamp_state.current_state;

        let toggle_perspective_correction =
            perspective_state.changed_since_last_sync && perspective_state.current_state;

        let reset = reset_state.changed_since_last_sync && reset_state.current_state;
        let toggle_aspect = aspect_state.changed_since_last_sync && aspect_state.current_state;

        XrInputSnapshot {
            movement,
            zoom,
            rotate: rot,
            change_clamp,
            toggle_perspective_correction,
            reset,
            toggle_aspect,
        }
    }

    pub fn get_resolution(&self) -> Vector2<u32> {
        self.resolution
    }

    pub fn swapchain_buffer(&self, index: u32) -> &wgpu::Texture {
        &self.swapchain_buffers[index as usize]
    }

    pub fn start_frame(
        &mut self,
        wgpu_device: &wgpu::Device,
        frame_state: openxr::FrameState,
    ) -> Option<(Vec<openxr::View>, wgpu::TextureView)> {
        self.frame_stream.begin().expect("xr frame stream begin");

        if !frame_state.should_render && !self.always_render {
            self.frame_stream
                .end(
                    frame_state.predicted_display_time,
                    openxr::EnvironmentBlendMode::OPAQUE,
                    &[],
                )
                .expect("xr frame stream end");

            return None;
        }

        let (view_state_flags, views) = self
            .session
            .locate_views(
                openxr::ViewConfigurationType::PRIMARY_STEREO,
                frame_state.predicted_display_time,
                &self.space,
            )
            .expect("xr locate views for render");

        if !view_state_flags.contains(
            openxr::ViewStateFlags::POSITION_VALID | openxr::ViewStateFlags::ORIENTATION_VALID,
        ) && !self.always_render
        {
            self.frame_stream
                .end(
                    frame_state.predicted_display_time,
                    openxr::EnvironmentBlendMode::OPAQUE,
                    &[],
                )
                .expect("xr frame stream end");

            return None;
        }

        self.update_swapchain(wgpu_device);

        let image_index = self
            .swapchain
            .acquire_image()
            .expect("xr swapchain image acquire");

        self.swapchain
            .wait_image(openxr::Duration::INFINITE)
            .expect("xr swapchain wait");

        let swapchain_texture = &self.swapchain_buffer(image_index);

        let swapchain_view = swapchain_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Swapchain view"),
            format: Some(self.swapchain_format()),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            usage: None,
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: 0,
            array_layer_count: Some(2),
        });

        Some((views, swapchain_view))
    }

    pub fn end_frame(&mut self, frame_state: openxr::FrameState, views: Vec<openxr::View>) {
        self.swapchain
            .release_image()
            .expect("xr swapchain release");

        let resolution = self.get_resolution();

        let rect = openxr::Rect2Di {
            offset: openxr::Offset2Di { x: 0, y: 0 },
            extent: openxr::Extent2Di {
                width: resolution.x as i32,
                height: resolution.y as i32,
            },
        };

        let sub_images: Vec<_> = views
            .iter()
            .enumerate()
            .map(|(view_index, view)| {
                openxr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(
                        openxr::SwapchainSubImage::new()
                            .swapchain(&self.swapchain)
                            .image_array_index(view_index as u32)
                            .image_rect(rect),
                    )
            })
            .collect();

        let projection_layer = openxr::CompositionLayerProjection::new()
            .space(&self.space)
            .views(&sub_images);

        self.frame_stream
            .end(
                frame_state.predicted_display_time,
                openxr::EnvironmentBlendMode::OPAQUE,
                &[&projection_layer],
            )
            .expect("xr frame stream end");
    }
}

struct XrActionSet {
    move_action: openxr::Action<openxr::Vector2f>,
    zoom_action: openxr::Action<openxr::Vector2f>,
    rotate_ccw_action: openxr::Action<f32>,
    rotate_cw_action: openxr::Action<f32>,
    clamp_action: openxr::Action<bool>,
    perspective_action: openxr::Action<bool>,
    reset_action: openxr::Action<bool>,
    aspect_action: openxr::Action<bool>,
    action_set: openxr::ActionSet,
}
