use std::{
    ffi::c_void, io, ops::Mul, path::{Path, PathBuf}, sync::{
        Arc, atomic::{AtomicBool, AtomicU32, Ordering}, mpsc::RecvTimeoutError,
    }, time::Instant,
};

use ash::vk::{self, Handle};
use clap::Parser;
use env_logger::{DEFAULT_FILTER_ENV, Env};
use image::ImageReader;
use log::{debug, info, warn};
use wgpu::util::DeviceExt;

const VK_TARGET_VERSION_XR: openxr::Version = openxr::Version::new(1, 2, 0);
const VK_TARGET_VERSION: u32 = vk::make_api_version(0, 1, 2, 0);

const MULTIVIEW_MASK: Option<std::num::NonZeroU32> = std::num::NonZeroU32::new(0b11);

#[derive(Parser, Debug)]
#[command(name = "vr-imageview", about = "Display an image in VR")]
pub struct Args {
    #[arg(help = "The image to display in the headset")]
    pub image: PathBuf,

    #[arg(
        long,
        short,
        help = "Don't apply perspective correction by default while rendering (Y button to toggle)"
    )]
    pub no_perspective_correction: bool,

    #[arg(
        long,
        short,
        help = "Default to displaying with a 1:1 aspect ratio (B button to toggle)"
    )]
    pub square: bool,

    #[arg(long, short, help = "Print FPS counter")]
    pub fps: bool,

    #[arg(long, short, help = "Activate debug logging")]
    pub debug: bool,
}

fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::Relaxed);
    })
    .expect("setting Ctrl-C handler");

    let args = Args::parse();

    let default_log_level = if args.debug { "debug" } else { "info" };
    env_logger::init_from_env(Env::default().filter_or(DEFAULT_FILTER_ENV, default_log_level));

    let image_path = args.image;

    info!(
        "Starting VR image viewer with {}",
        image_path.as_path().to_string_lossy()
    );

    let (mut xr_context, wg_device, wg_queue) = XrContext::new();
    let mut scene = Scene::new(
        &wg_device,
        &wg_queue,
        &image_path,
        xr_context.swapchain_format(),
        !args.no_perspective_correction,
    );

    debug!("Starting main loop");

    let frames = Arc::new(AtomicU32::new(0));
    let f = frames.clone();
    let (frame_kill_send, frame_kill_recv) = std::sync::mpsc::channel();
    let frames_handle = args
        .fps
        .then(|| std::thread::spawn(move || fps_thread(f, frame_kill_recv)));

    let mut last_render = Instant::now();

    loop {
        if !running.load(Ordering::Relaxed) {
            info!("Received CTRL-C, shutting down");
            break;
        }

        match xr_context.poll_events() {
            true => {}
            false => {
                info!("XR session ended, shutting down");
                break;
            }
        }

        if !xr_context.is_session_running() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        let now = Instant::now();
        let delta = (now - last_render).as_secs_f32();
        last_render = now;

        let frame_state = xr_context.wait_frame();

        let xr_input_snapshot = xr_context.get_input_snapshot();
        scene.update(xr_input_snapshot, delta);

        render_frame(
            &mut xr_context,
            &wg_device,
            &wg_queue,
            &mut scene,
            delta,
            frame_state,
        );

        frames.fetch_add(1, Ordering::Relaxed);
    }

    debug!("Exited main loop, draining GPU work before quitting");

    if let Some(frames_handle) = frames_handle {
        let _ = frame_kill_send.send(());
        let _ = frames_handle.join();
    }

    let _ = wg_device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });

    let (entry, _instance) = xr_context.take_vk_instance();

    drop(scene);
    debug!("Dropped scene");
    xr_context.shutdown();
    debug!("Shut down xr context");
    drop(xr_context);
    debug!("Dropped xr context");
    drop(wg_queue);
    debug!("Dropped queue");
    drop(wg_device);
    debug!("Dropped device");
    drop(entry);

    debug!("Dropped everything")
}

fn fps_thread(frames: Arc<AtomicU32>, frame_kill_recv: std::sync::mpsc::Receiver<()>) {
    loop {
        let count = frames.swap(0, Ordering::Relaxed);
        info!("FPS: {count}");

        match frame_kill_recv.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => (),
        }
    }
}

fn render_frame(
    xr_context: &mut XrContext,
    wg_device: &wgpu::Device,
    wg_queue: &wgpu::Queue,
    scene: &mut Scene,
    _delta: f32,
    frame_state: openxr::FrameState,
) {
    xr_context
        .frame_stream()
        .begin()
        .expect("xr frame stream begin");

    // We shouldn't need this for a simple image viewer that doesn't care about head tracking
    /*
    if !frame_state.should_render {
        xr_context
            .frame_stream()
            .end(
                frame_state.predicted_display_time,
                openxr::EnvironmentBlendMode::OPAQUE,
                &[],
            )
            .expect("xr frame stream end");
        return;
    }
    */

    let (_view_state_flags, views) = xr_context
        .session()
        .locate_views(
            openxr::ViewConfigurationType::PRIMARY_STEREO,
            frame_state.predicted_display_time,
            xr_context.stage(),
        )
        .expect("xr locate views for render");

    /*
    if !view_state_flags.contains(
        openxr::ViewStateFlags::POSITION_VALID | openxr::ViewStateFlags::ORIENTATION_VALID,
    ) {
        xr_context
            .frame_stream()
            .end(
                frame_state.predicted_display_time,
                openxr::EnvironmentBlendMode::OPAQUE,
                &[],
            )
            .expect("xr frame stream end");
        return;
    }
    */

    xr_context.update_swapchain(wg_device);
    scene.update_camera_buffer(wg_queue);

    let image_index = xr_context
        .xr_swapchain
        .acquire_image()
        .expect("xr swapchain image acquire");
    xr_context
        .xr_swapchain
        .wait_image(openxr::Duration::INFINITE)
        .expect("xr swapchain wait");

    let swapchain_texture = &xr_context.wg_swapchain_buffers[image_index as usize];
    let resolution = xr_context.get_resolution();

    let swapchain_view = swapchain_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("Swapchain view"),
        format: Some(xr_context.swapchain_format()),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        usage: None,
        aspect: wgpu::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: None,
        base_array_layer: 0,
        array_layer_count: Some(2),
    });

    let mut encoder = wg_device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Render encoder"),
    });

    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("User image render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &swapchain_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: MULTIVIEW_MASK,
        });

        render_pass.set_pipeline(scene.image_pipeline());
        render_pass.set_bind_group(0, scene.image_bind_group(), &[]);
        render_pass.set_bind_group(1, scene.camera_bind_group(), &[]);

        render_pass.draw(0..3, 0..1);
    }

    wg_queue.submit(std::iter::once(encoder.finish()));

    xr_context
        .xr_swapchain
        .release_image()
        .expect("xr swapchain release");

    // Submit frames to OpenXR

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
                        .swapchain(&xr_context.xr_swapchain)
                        .image_array_index(view_index as u32)
                        .image_rect(rect),
                )
        })
        .collect();

    let projection_layer = openxr::CompositionLayerProjection::new()
        .space(&xr_context.xr_space)
        .views(&sub_images);

    xr_context
        .xr_frame_stream
        .end(
            frame_state.predicted_display_time,
            openxr::EnvironmentBlendMode::OPAQUE,
            &[&projection_layer],
        )
        .expect("xr frame stream end");
}

pub struct XrContext {
    session_running: bool,

    wg_swapchain_buffers: Vec<wgpu::Texture>,
    xr_swapchain: openxr::Swapchain<openxr::Vulkan>,

    xr_move_action: openxr::Action<openxr::Vector2f>,
    xr_zoom_action: openxr::Action<openxr::Vector2f>,
    xr_rotate_ccw_action: openxr::Action<f32>,
    xr_rotate_cw_action: openxr::Action<f32>,
    xr_clamp_action: openxr::Action<bool>,
    xr_perspective_action: openxr::Action<bool>,
    xr_reset_action: openxr::Action<bool>,
    xr_aspect_action: openxr::Action<bool>,
    xr_action_set: openxr::ActionSet,

    xr_space: openxr::Space,

    xr_resolution: nalgebra::Vector2<u32>,
    swapchain_dirty: bool,

    xr_frame_stream: openxr::FrameStream<openxr::Vulkan>,
    xr_frame_wait: openxr::FrameWaiter,
    xr_session: openxr::Session<openxr::Vulkan>,

    xr_instance: openxr::Instance,
    xr_system: openxr::SystemId,

    _vk_entry: Option<ash::Entry>,
    _vk_instance: Option<ash::Instance>,
}

impl XrContext {
    fn new() -> (Self, wgpu::Device, wgpu::Queue) {
        let xr_app_info = openxr::ApplicationInfo {
            application_name: "vr-imageview",
            application_version: 1,
            engine_name: "custom",
            engine_version: 1,
            api_version: openxr::Version::new(1, 0, 0),
        };

        let xr_platform_info = ();
        let xr_entry = openxr::Entry::linked(&xr_platform_info).expect("xr entry");
        let mut xr_requirements = openxr::ExtensionSet::default();

        debug!("Got XR entry");

        xr_requirements.khr_vulkan_enable2 = true;
        xr_requirements.ext_view_configuration_views_change = true; // Lets us update our swapchains on demand

        let xr_instance = xr_entry
            .create_instance(&xr_app_info, &xr_requirements, &[], &xr_platform_info)
            .expect("xr instance");
        debug!("Created XR instance");
        let xr_system = xr_instance
            .system(openxr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .expect("xr system");
        debug!("Created XR system");

        let vk_requirements = xr_instance
            .graphics_requirements::<openxr::Vulkan>(xr_system)
            .expect("xr graphics requirements");
        if VK_TARGET_VERSION_XR < vk_requirements.min_api_version_supported
            || VK_TARGET_VERSION_XR > vk_requirements.max_api_version_supported
        {
            panic!("OpenXR version troubles");
        }
        debug!("Vk passes requirements");

        let vk_entry = unsafe { ash::Entry::load() }.expect("vk entry");

        #[allow(clippy::missing_transmute_annotations)]
        let vk_get_instance_proc_addr = unsafe {
            std::mem::transmute::<
                _,
                unsafe extern "system" fn(
                    *const std::ffi::c_void,
                    *const std::ffi::c_char,
                ) -> Option<unsafe extern "system" fn()>,
            >(vk_entry.static_fn().get_instance_proc_addr)
        };

        debug!("Got vk entry");

        let wg_flags = wgpu::InstanceFlags::default();
        let wg_instance_exts =
            <wgpu_hal::vulkan::Api as wgpu_hal::Api>::Instance::desired_extensions(
                &vk_entry,
                VK_TARGET_VERSION,
                wg_flags,
            )
            .expect("instance exts");
        debug!("Got wgpu extensions");

        let vk_instance = unsafe {
            let vk_app_info = vk::ApplicationInfo::default()
                .application_name(c"vr-imageview")
                .application_version(1)
                .engine_name(c"custom")
                .engine_version(1)
                .api_version(VK_TARGET_VERSION);

            let wg_instance_exts_cchar: Vec<_> =
                wg_instance_exts.iter().map(|s| s.as_ptr()).collect();

            let vk_instance = xr_instance
                .create_vulkan_instance(
                    xr_system,
                    vk_get_instance_proc_addr,
                    &vk::InstanceCreateInfo::default()
                        .application_info(&vk_app_info)
                        .enabled_extension_names(&wg_instance_exts_cchar)
                        as *const _ as *const _,
                )
                .expect("xr vk instance")
                .map_err(vk::Result::from_raw)
                .expect("vk instance");

            ash::Instance::load(
                vk_entry.static_fn(),
                vk::Instance::from_raw(vk_instance as _),
            )
        };
        debug!("Created vk instance");

        let vk_physical_device = vk::PhysicalDevice::from_raw(unsafe {
            xr_instance
                .vulkan_graphics_device(xr_system, vk_instance.handle().as_raw() as _)
                .expect("xr vulkan physical device") as _
        });
        debug!("Created vk physical device");

        let vk_instance_ptr = vk_instance.handle().as_raw() as *const c_void;
        let vk_physical_device_ptr = vk_physical_device.as_raw() as *const c_void;
        let vk_device_properties =
            unsafe { vk_instance.get_physical_device_properties(vk_physical_device) };
        if vk_device_properties.api_version < VK_TARGET_VERSION {
            panic!(
                "vk physical device doesn't support {}",
                VK_TARGET_VERSION_XR
            );
        }
        debug!("Got vk physical device props");

        let wg_vk_instance = unsafe {
            <wgpu_hal::vulkan::Api as wgpu_hal::Api>::Instance::from_raw(
                vk_entry.clone(),
                vk_instance.clone(),
                vk_device_properties.api_version,
                0,
                None,
                wg_instance_exts,
                wg_flags,
                wgpu::MemoryBudgetThresholds::default(),
                false,
                None,
            )
            .expect("wgpu vk instance")
        };
        debug!("Created wgpu vk instance");

        let wg_exposed_adapter = wg_vk_instance
            .expose_adapter(vk_physical_device)
            .expect("expose wgpu adapter");
        debug!("Created exposed wgpu adapter");

        let wg_features = wg_exposed_adapter.features;
        let wg_enabled_exts = wg_exposed_adapter
            .adapter
            .required_device_extensions(wg_features);

        let vk_device_exts = [
            ash::khr::swapchain::NAME,
            ash::khr::timeline_semaphore::NAME,
        ];
        let (wg_open_device, vk_device_ptr, vk_queue_family_index) = {
            let vk_device_exts_cchar: Vec<_> = vk_device_exts.iter().map(|s| s.as_ptr()).collect();

            let mut enabled_phd_features = wg_exposed_adapter
                .adapter
                .physical_device_features(&wg_enabled_exts, wg_features);

            let family_index = 0;
            let family_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family_index)
                .queue_priorities(&[1.0]);

            let family_infos = [family_info];

            let mut timeline_features =
                vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true);

            let mut phyical_device_multiview_features = vk::PhysicalDeviceMultiviewFeatures {
                multiview: vk::TRUE,
                ..Default::default()
            };

            let info = enabled_phd_features
                .add_to_device_create(
                    vk::DeviceCreateInfo::default()
                        .queue_create_infos(&family_infos)
                        .push_next(&mut timeline_features)
                        .push_next(&mut phyical_device_multiview_features),
                )
                .enabled_extension_names(&vk_device_exts_cchar);

            let vk_device = unsafe {
                let vk_device = xr_instance
                    .create_vulkan_device(
                        xr_system,
                        vk_get_instance_proc_addr,
                        vk_physical_device.as_raw() as _,
                        &info as *const _ as *const _,
                    )
                    .expect("xr vk device")
                    .map_err(vk::Result::from_raw)
                    .expect("vk device");

                ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(vk_device as _))
            };
            let vk_device_ptr = vk_device.handle().as_raw() as *const c_void;
            debug!("Created vk device");

            let wg_open_device = unsafe {
                wg_exposed_adapter
                    .adapter
                    .device_from_raw(
                        vk_device,
                        None,
                        &wg_enabled_exts,
                        wg_features,
                        &wgpu::Limits::defaults(),
                        &wgpu::MemoryHints::default(),
                        family_info.queue_family_index,
                        0,
                    )
                    .expect("wgpu open device")
            };
            debug!("Created wgpu open device");

            (
                wg_open_device,
                vk_device_ptr,
                family_info.queue_family_index,
            )
        };

        let wg_instance =
            unsafe { wgpu::Instance::from_hal::<wgpu_hal::api::Vulkan>(wg_vk_instance) };

        let wg_adapter = unsafe { wg_instance.create_adapter_from_hal(wg_exposed_adapter) };

        let mut limits = wg_adapter.limits();
        limits.max_buffer_size = u32::MAX as u64;

        let (wg_device, wg_queue) = unsafe {
            wg_adapter
                .create_device_from_hal(
                    wg_open_device,
                    &wgpu::DeviceDescriptor {
                        label: Some("XR WGPU device"),
                        required_features: wg_features,
                        required_limits: limits,
                        experimental_features: wgpu::ExperimentalFeatures::disabled(),
                        memory_hints: wgpu::MemoryHints::Performance,
                        trace: wgpu::Trace::Off,
                    },
                )
                .expect("wgpu device and queue")
        };
        debug!("Created wgpu device");

        let (xr_session, xr_frame_wait, xr_frame_stream) = unsafe {
            xr_instance
                .create_session::<openxr::Vulkan>(
                    xr_system,
                    &openxr::vulkan::SessionCreateInfo {
                        instance: vk_instance_ptr,
                        physical_device: vk_physical_device_ptr,
                        device: vk_device_ptr,
                        queue_family_index: vk_queue_family_index,
                        queue_index: 0,
                    },
                )
                .expect("xr session")
        };
        debug!("Created xr session");

        let xr_action_set = xr_instance
            .create_action_set("gameplay", "Gameplay Actions", 0)
            .expect("xr action set");

        let xr_move_action = xr_action_set
            .create_action::<openxr::Vector2f>("move", "Move", &[])
            .expect("create move action");

        let xr_zoom_action = xr_action_set
            .create_action::<openxr::Vector2f>("zoom", "Zoom", &[])
            .expect("create zoom action");

        let xr_rotate_ccw_action = xr_action_set
            .create_action::<f32>("ccw", "Rotate CCW", &[])
            .expect("create ccw action");

        let xr_rotate_cw_action = xr_action_set
            .create_action::<f32>("cw", "Rotate CW", &[])
            .expect("create cw action");

        let xr_clamp_action = xr_action_set
            .create_action::<bool>("clamp", "Change Clamp", &[])
            .expect("create clamp action");

        let xr_perspective_action = xr_action_set
            .create_action::<bool>("perspective", "Toggle perspective correction", &[])
            .expect("create perspective action");

        let xr_reset_action = xr_action_set
            .create_action::<bool>("reset", "Reset", &[])
            .expect("create reset action");

        let xr_aspect_action = xr_action_set
            .create_action::<bool>("aspect", "Toggle Aspect Ratio", &[])
            .expect("create aspect action");

        xr_instance
            .suggest_interaction_profile_bindings(
                xr_instance
                    .string_to_path("/interaction_profiles/oculus/touch_controller")
                    .expect("touch_controller path"),
                &[
                    openxr::Binding::new(
                        &xr_move_action,
                        xr_instance
                            .string_to_path("/user/hand/left/input/thumbstick")
                            .expect("left thumbstick path"),
                    ),
                    openxr::Binding::new(
                        &xr_zoom_action,
                        xr_instance
                            .string_to_path("/user/hand/right/input/thumbstick")
                            .expect("right thumbstick path"),
                    ),
                    openxr::Binding::new(
                        &xr_rotate_ccw_action,
                        xr_instance
                            .string_to_path("/user/hand/left/input/trigger/value")
                            .expect("left trigger path"),
                    ),
                    openxr::Binding::new(
                        &xr_rotate_cw_action,
                        xr_instance
                            .string_to_path("/user/hand/right/input/trigger/value")
                            .expect("right trigger path"),
                    ),
                    openxr::Binding::new(
                        &xr_clamp_action,
                        xr_instance
                            .string_to_path("/user/hand/left/input/x/click")
                            .expect("left x path"),
                    ),
                    openxr::Binding::new(
                        &xr_perspective_action,
                        xr_instance
                            .string_to_path("/user/hand/left/input/y/click")
                            .expect("left y path"),
                    ),
                    openxr::Binding::new(
                        &xr_reset_action,
                        xr_instance
                            .string_to_path("/user/hand/right/input/a/click")
                            .expect("right a path"),
                    ),
                    openxr::Binding::new(
                        &xr_aspect_action,
                        xr_instance
                            .string_to_path("/user/hand/right/input/b/click")
                            .expect("right b path"),
                    ),
                ],
            )
            .expect("xr suggest profile bindings");

        xr_session
            .attach_action_sets(&[&xr_action_set])
            .expect("xr session action sets");

        debug!("Created xr action sets");

        let xr_space = xr_session
            .create_reference_space(openxr::ReferenceSpaceType::STAGE, openxr::Posef::IDENTITY)
            .expect("xr create space");

        let xr_resolution = XrContext::get_recommended_resolution(&xr_instance, xr_system);

        let (xr_swapchain, wg_swapchain_buffers) =
            XrContext::create_swapchain(&wg_device, &xr_session, xr_resolution);

        debug!("Created swapchain");

        let context = XrContext {
            session_running: true,
            _vk_entry: Some(vk_entry),
            _vk_instance: Some(vk_instance),
            xr_instance,
            xr_system,
            xr_session,
            xr_frame_wait,
            xr_frame_stream,
            xr_space,
            xr_resolution,
            xr_swapchain,
            wg_swapchain_buffers,
            swapchain_dirty: false,
            xr_action_set,
            xr_move_action,
            xr_zoom_action,
            xr_rotate_ccw_action,
            xr_rotate_cw_action,
            xr_clamp_action,
            xr_perspective_action,
            xr_reset_action,
            xr_aspect_action,
        };

        info!("XR context created");

        (context, wg_device, wg_queue)
    }

    fn take_vk_instance(&mut self) -> (ash::Entry, ash::Instance) {
        (
            self._vk_entry.take().unwrap(),
            self._vk_instance.take().unwrap(),
        )
    }

    fn get_recommended_resolution(
        xr_instance: &openxr::Instance,
        xr_system: openxr::SystemId,
    ) -> nalgebra::Vector2<u32> {
        let views = xr_instance
            .enumerate_view_configuration_views(
                xr_system,
                openxr::ViewConfigurationType::PRIMARY_STEREO,
            )
            .expect("xr views");

        nalgebra::Vector2::new(
            views[0].recommended_image_rect_width,
            views[0].recommended_image_rect_height,
        )
    }

    fn create_swapchain(
        wg_device: &wgpu::Device,
        xr_session: &openxr::Session<openxr::Vulkan>,
        xr_resolution: nalgebra::Vector2<u32>,
    ) -> (openxr::Swapchain<openxr::Vulkan>, Vec<wgpu::Texture>) {
        let xr_swapchain = xr_session
            .create_swapchain(&openxr::SwapchainCreateInfo {
                create_flags: openxr::SwapchainCreateFlags::EMPTY,
                usage_flags: openxr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | openxr::SwapchainUsageFlags::SAMPLED,
                format: vk::Format::R8G8B8A8_SRGB.as_raw() as _,
                sample_count: 1,
                width: xr_resolution.x,
                height: xr_resolution.y,
                face_count: 1,
                array_size: 2,
                mip_count: 1,
            })
            .expect("xr swapchain");
        let xr_swapchain_images = xr_swapchain.enumerate_images().expect("swapchain images");
        let wg_swapchain_buffers: Vec<wgpu::Texture> = xr_swapchain_images
            .into_iter()
            .map(|color_image| {
                let color_image = vk::Image::from_raw(color_image);

                let wg_hal_texture = unsafe {
                    let hal_dev = wg_device
                        .as_hal::<wgpu_hal::vulkan::Api>()
                        .expect("hal device for swapchain");
                    hal_dev.texture_from_raw(
                        color_image,
                        &wgpu_hal::TextureDescriptor {
                            label: Some("VR Swapchain"),
                            size: wgpu::Extent3d {
                                width: xr_resolution.x,
                                height: xr_resolution.y,
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
                    wg_device.create_texture_from_hal::<wgpu_hal::vulkan::Api>(
                        wg_hal_texture,
                        &wgpu::TextureDescriptor {
                            label: Some("VR Swapchain"),
                            size: wgpu::Extent3d {
                                width: xr_resolution.x,
                                height: xr_resolution.y,
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
        (xr_swapchain, wg_swapchain_buffers)
    }

    fn update_swapchain(&mut self, wg_device: &wgpu::Device) {
        if !self.swapchain_dirty {
            return;
        }

        let new_resolution =
            XrContext::get_recommended_resolution(&self.xr_instance, self.xr_system);

        if self.xr_resolution == new_resolution {
            // Resolution didn't actually change, bail
            return;
        }

        let (mut new_swapchain, mut new_swapchain_buffers) =
            XrContext::create_swapchain(wg_device, &self.xr_session, new_resolution);

        self.xr_resolution = new_resolution;
        std::mem::swap(&mut self.xr_swapchain, &mut new_swapchain);
        std::mem::swap(&mut self.wg_swapchain_buffers, &mut new_swapchain_buffers);
        self.swapchain_dirty = false;

        debug!(
            "Updated swapchain to ({}, {})",
            new_resolution.x, new_resolution.y
        );
    }

    pub fn poll_events(&mut self) -> bool {
        let mut buffer = openxr::EventDataBuffer::new();

        while let Some(event) = self
            .xr_instance
            .poll_event(&mut buffer)
            .expect("xr event poll")
        {
            match event {
                openxr::Event::SessionStateChanged(state_change) => {
                    log::info!("XR Session state changed to: {:?}", state_change.state());
                    match state_change.state() {
                        openxr::SessionState::READY => {
                            self.xr_session
                                .begin(openxr::ViewConfigurationType::PRIMARY_STEREO)
                                .expect("xr begin session");
                            self.session_running = true;
                            log::info!("XR Session started");
                        }
                        openxr::SessionState::STOPPING => {
                            self.session_running = false;
                            self.xr_session.end().expect("xr end session");
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
        self.wg_swapchain_buffers[0].format()
    }

    pub fn shutdown(&mut self) {
        if !self.session_running {
            return;
        }

        if let Err(error) = self.xr_session.request_exit() {
            warn!("error during xr session shutdown request: {error}");
            self.session_running = false;
            return;
        }

        let mut buffer = openxr::EventDataBuffer::new();

        for _ in 0..200 {
            match self.xr_instance.poll_event(&mut buffer) {
                Ok(Some(openxr::Event::SessionStateChanged(state_change))) => {
                    match state_change.state() {
                        openxr::SessionState::STOPPING => {
                            if let Err(error) = self.xr_session.end() {
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
        self.xr_frame_wait.wait().expect("xr frame wait")
    }

    pub fn frame_stream(&mut self) -> &mut openxr::FrameStream<openxr::Vulkan> {
        &mut self.xr_frame_stream
    }

    pub fn session(&self) -> &openxr::Session<openxr::Vulkan> {
        &self.xr_session
    }

    pub fn get_input_snapshot(&self) -> XrInputSnapshot {
        self.xr_session
            .sync_actions(&[(&self.xr_action_set).into()])
            .expect("xr sync actions");

        let move_state = self
            .xr_move_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr move action state");

        let zoom_state = self
            .xr_zoom_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr zoom action state");

        let ccw_state = self
            .xr_rotate_ccw_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr ccw action state");

        let cw_state = self
            .xr_rotate_cw_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr cw action state");

        let clamp_state = self
            .xr_clamp_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr clamp action state");

        let perspective_state = self
            .xr_perspective_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr perspective action state");

        let reset_state = self
            .xr_reset_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr reset action state");

        let aspect_state = self
            .xr_aspect_action
            .state(&self.xr_session, openxr::Path::NULL)
            .expect("xr aspect action state");

        let movement =
            nalgebra::Vector2::new(move_state.current_state.x, move_state.current_state.y);
        let movement = if movement.magnitude() > 0.1 {
            movement
        } else {
            nalgebra::Vector2::zeros()
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
            rot,
            change_clamp,
            toggle_perspective_correction,
            reset,
            toggle_aspect,
        }
    }

    fn get_resolution(&self) -> nalgebra::Vector2<u32> {
        self.xr_resolution
    }

    fn stage(&self) -> &openxr::Space {
        &self.xr_space
    }
}

pub struct XrInputSnapshot {
    movement: nalgebra::Vector2<f32>,
    zoom: f32,
    rot: f32,
    change_clamp: bool,
    reset: bool,
    toggle_perspective_correction: bool,
    toggle_aspect: bool,
}

pub struct Scene {
    camera: Camera,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,

    image_clamp_index: usize,

    _image_texture: wgpu::Texture,
    _image_texture_view: wgpu::TextureView,
    image_samplers: [wgpu::Sampler; 4],
    _image_bind_group_layout: wgpu::BindGroupLayout,
    image_bind_groups: [wgpu::BindGroup; 4],
    _image_shader: wgpu::ShaderModule,
    _image_pipeline_layout: wgpu::PipelineLayout,
    image_pipeline: wgpu::RenderPipeline,
}

impl Scene {
    pub fn new(
        wg_device: &wgpu::Device,
        wg_queue: &wgpu::Queue,
        image_path: &Path,
        swapchain_format: wgpu::TextureFormat,
        perspective_correction: bool,
    ) -> Scene {
        let image_data = read_image(image_path).expect("png image data");
        let (image_texture, image_texture_view, image_samplers) =
            Scene::create_and_upload_texture(wg_device, wg_queue, &image_data);

        let camera = Camera::new(perspective_correction, image_data.size);

        info!("Loaded texture");

        let image_bind_group_layout =
            wg_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                label: Some("Image bind group layout"),
            });

        let image_bind_groups = image_samplers.each_ref().map(|sampler| {
            wg_device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Image bind group"),
                layout: &image_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&image_texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });

        let camera_buffer = wg_device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera uniform buffer"),
            contents: bytemuck::cast_slice(&[CameraUniform::from(&camera)]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group_layout =
            wg_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Camera bind group layout"),
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

        let camera_bind_group = wg_device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera bind group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        debug!("Created bind groups");

        let image_shader = wg_device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Image shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });

        debug!("Loaded shader");

        let image_pipeline_layout =
            wg_device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Image pipeline layout"),
                bind_group_layouts: &[
                    Some(&image_bind_group_layout),
                    Some(&camera_bind_group_layout),
                ],
                immediate_size: 0,
            });

        let image_pipeline = wg_device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Image pipeline"),
            layout: Some(&image_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &image_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &image_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: swapchain_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList, // 1.
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw, // 2.
                cull_mode: Some(wgpu::Face::Back),
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

        Scene {
            camera,
            camera_buffer,
            camera_bind_group,

            image_clamp_index: 0,
            _image_texture: image_texture,
            _image_texture_view: image_texture_view,
            image_samplers,
            _image_bind_group_layout: image_bind_group_layout,
            image_bind_groups,
            _image_shader: image_shader,
            _image_pipeline_layout: image_pipeline_layout,
            image_pipeline,
        }
    }

    fn create_and_upload_texture(
        wg_device: &wgpu::Device,
        wg_queue: &wgpu::Queue,
        image_data: &ImageData,
    ) -> (wgpu::Texture, wgpu::TextureView, [wgpu::Sampler; 4]) {
        let texture_size = wgpu::Extent3d {
            width: image_data.size.x,
            height: image_data.size.y,
            depth_or_array_layers: 1,
        };

        let texture = wg_device.create_texture(&wgpu::TextureDescriptor {
            label: Some("User PNG"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        wg_queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image_data.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image_data.size.x),
                rows_per_image: Some(image_data.size.y),
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
            wg_device.create_sampler(&wgpu::SamplerDescriptor {
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

    pub fn update(&mut self, xr_input_snapshot: XrInputSnapshot, delta: f32) {
        if xr_input_snapshot.reset {
            self.camera.reset_position();
            info!("Reset camera");
        }

        if xr_input_snapshot.toggle_perspective_correction {
            self.camera.toggle_perspective_correction();
            info!("Toggled perspective correction");
        }

        if xr_input_snapshot.toggle_aspect {
            self.camera.toggle_aspect();
            info!("Toggled square aspect ratio");
        }

        self.camera.rotate(xr_input_snapshot.rot * delta);
        self.camera.zoom(xr_input_snapshot.zoom * delta);
        self.camera.translate(
            xr_input_snapshot
                .movement
                .component_mul(&nalgebra::Vector2::new(1.0f32, -1.0f32))
                * delta,
        );

        if xr_input_snapshot.change_clamp {
            self.image_clamp_index += 1;

            if self.image_clamp_index > 3 {
                self.image_clamp_index = 0;
            }

            info!("Changed image clamp setting");
        }
    }

    pub fn update_camera_buffer(&self, wg_queue: &wgpu::Queue) {
        wg_queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[CameraUniform::from(&self.camera)]),
        );
    }

    pub fn image_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.image_pipeline
    }

    pub fn image_sampler(&self) -> &wgpu::Sampler {
        &self.image_samplers[self.image_clamp_index]
    }

    pub fn image_bind_group(&self) -> &wgpu::BindGroup {
        &self.image_bind_groups[self.image_clamp_index]
    }

    pub fn camera_bind_group(&self) -> &wgpu::BindGroup {
        &self.camera_bind_group
    }
}

pub struct Camera {
    position: nalgebra::Vector2<f32>,
    zoom_raw: f32,
    zoom_vis: f32,
    rotate: f32,
    perspective_correction: bool,

    // I've stopped caring where data belongs at this point
    image_dims: nalgebra::Vector2<f32>,
    image_square: bool,
}

impl Camera {
    // We're not doing any scaling on the viewport so 2 u/s would span edge to edge in a single second
    const MOVE_SPEED_PER_S: f32 = 0.5;

    // Radians
    const ROT_SPEED_PER_S: f32 = 1.0;

    // Idk
    const ZOOM_SPEED_PER_S: f32 = 1.0;

    pub fn new(perspective_correction: bool, image_dims: nalgebra::Vector2<u32>) -> Camera {
        let image_dims = image_dims.cast();

        debug!("{image_dims}");

        Camera {
            position: nalgebra::Vector2::new(0.5, 0.5),
            zoom_raw: 0.0,
            zoom_vis: 1.0,
            rotate: 0.0,
            perspective_correction,

            image_dims,
            image_square: false,
        }
    }

    pub fn reset_position(&mut self) {
        self.position = nalgebra::Vector2::new(0.5, 0.5);
        self.zoom_raw = 0.0;
        self.zoom_vis = 1.0;
        self.rotate = 0.0;
    }

    pub fn toggle_perspective_correction(&mut self) {
        self.perspective_correction = !self.perspective_correction;
    }

    fn toggle_aspect(&mut self) {
        self.image_square = !self.image_square;
    }

    /// Affected by zoom level
    pub fn translate(&mut self, amount: nalgebra::Vector2<f32>) {
        let rot = nalgebra::Matrix3::new_rotation(self.rotate);
        let move_amount = (amount * self.zoom_vis) * Camera::MOVE_SPEED_PER_S;
        let rotated_move = rot * nalgebra::Vector3::new(move_amount.x, move_amount.y, 1.0);
        self.position += rotated_move.xy();
    }

    pub fn zoom(&mut self, amount: f32) {
        // Zoom scales exponentially
        self.zoom_raw = (self.zoom_raw + amount * Camera::ZOOM_SPEED_PER_S).clamp(-15.0, 15.0);
        self.zoom_vis = 1.5f32.powf(self.zoom_raw);
    }

    pub fn rotate(&mut self, amount: f32) {
        self.rotate += amount * Camera::ROT_SPEED_PER_S;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    mat: [[f32; 4]; 3],
    perspective_correction: u32,
    _pad: [u32; 1],
    image_scale: [f32; 2],
}

impl From<&Camera> for CameraUniform {
    fn from(camera: &Camera) -> Self {
        let rot = nalgebra::Matrix3::new_rotation(camera.rotate);
        let zoom = nalgebra::Matrix3::new_scaling(camera.zoom_vis);
        let trans = nalgebra::Matrix3::new_translation(&camera.position);

        let mat: [[f32; 3]; 3] = trans.mul(zoom).mul(rot).into();

        let mat_padded = mat.map(|r| [r[0], r[1], r[2], 1f32]);

        let image_scale = if camera.image_square {
            [1.0, 1.0]
        } else {
            let scale = 1.0 / camera.image_dims.max();
            (camera.image_dims * scale).into()
        };

        CameraUniform {
            mat: mat_padded,
            perspective_correction: if camera.perspective_correction { 1 } else { 0 },
            image_scale,
            _pad: [0; 1],
        }
    }
}

pub struct ImageData {
    size: nalgebra::Vector2<u32>,
    data: Vec<u8>,
}

fn read_image(path: &Path) -> io::Result<ImageData> {
    let img = ImageReader::open(path)?
        .decode()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "couldnt decode"))?;

    let size = nalgebra::Vector2::new(img.width(), img.height());

    if size.x > 16384 || size.y > 16384 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "png dimensions above 16384",
        ));
    }

    let data = img.to_rgba8().into_raw();

    Ok(ImageData { size, data })
}
