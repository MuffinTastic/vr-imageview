use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::RecvTimeoutError,
    },
    time::Instant,
};

use clap::Parser;
use env_logger::{DEFAULT_FILTER_ENV, Env};
use log::{debug, info};

use crate::{constants::MULTIVIEW_MASK, scene::Scene, xr::XrContext};

mod camera;
mod constants;
mod image;
mod pipeline;
mod scene;
mod uniform;
mod xr;
mod xr_wgpu_helper;

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

    #[arg(long, short, help = "Always render regardless of HMD presence")]
    pub always_render: bool,

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

    let (mut xr_context, wgpu_device, wgpu_queue) = XrContext::new(args.always_render);
    let mut scene = Scene::new(
        &wgpu_device,
        &wgpu_queue,
        &image_path,
        xr_context.swapchain_format(),
        !args.no_perspective_correction,
        args.square,
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

        // Sorry my beloved fixed timestep, not this time . . .
        scene.update(xr_input_snapshot, delta);

        render_frame(
            &mut xr_context,
            &wgpu_device,
            &wgpu_queue,
            &mut scene,
            delta,
            frame_state,
            &frames,
        );
    }

    debug!("Exited main loop, draining GPU work before quitting");

    if let Some(frames_handle) = frames_handle {
        let _ = frame_kill_send.send(());
        let _ = frames_handle.join();
    }

    let _ = wgpu_device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });

    let vk_entry = xr_context.take_vk_entry();

    // https://tenor.com/view/shut-it-down-dark-city-talking-shut-it-down-forever-gif-17544075

    drop(scene);
    debug!("Dropped scene");
    xr_context.shutdown();
    debug!("Shut down xr context");
    drop(xr_context);
    debug!("Dropped xr context");

    drop(wgpu_queue);
    debug!("Dropped queue");
    drop(wgpu_device);
    debug!("Dropped device");
    drop(vk_entry);

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
    wgpu_device: &wgpu::Device,
    wgpu_queue: &wgpu::Queue,
    scene: &mut Scene,
    _delta: f32,
    frame_state: openxr::FrameState,
    frames: &Arc<AtomicU32>
) {
    scene.pre_render(wgpu_queue);

    let Some((views, swapchain_view)) = xr_context.start_frame(wgpu_device, frame_state) else {
        return;
    };

    let mut encoder = wgpu_device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Render encoder"),
    });

    {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Scene render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &swapchain_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: MULTIVIEW_MASK,
        });

        scene.render(&mut render_pass);
    }

    wgpu_queue.submit(std::iter::once(encoder.finish()));

    xr_context.end_frame(frame_state, views);

    frames.fetch_add(1, Ordering::Relaxed);
}
