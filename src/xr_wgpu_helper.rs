use std::ffi::c_void;

use ash::vk::{self, Handle};
use log::debug;
use openxr::sys::platform::VkGetInstanceProcAddr;

use crate::constants::{
    APPLICATION_NAME_CSTR, APPLICATION_VERSION, ENGINE_NAME_CSTR, ENGINE_VERSION,
    VK_TARGET_VERSION, VK_TARGET_VERSION_XR,
};

#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    #[error("ash loading error: {0}")]
    AshLoadingError(#[from] ash::LoadingError),

    #[error("XR error: {0}")]
    XrError(#[from] openxr::sys::Result),

    #[error("VK error: {0}")]
    Vk(#[from] vk::Result),

    #[error("wgpu_hal instance error: {0}")]
    WgpuHalInstanceError(#[from] wgpu_hal::InstanceError),

    #[error("Couldn't expose wgpu adapter")]
    WgpuExposeAdapterError,

    #[error("wgpu_hal device error: {0}")]
    WgpuHalDeviceError(#[from] wgpu_hal::DeviceError),

    #[error("wgpu requst device error: {0}")]
    WgpuRequestDeviceError(#[from] wgpu::RequestDeviceError),
}

pub fn create_ash_instance_from_xr(
    instance: &openxr::Instance,
    system: openxr::SystemId,
    vk_entry: &ash::Entry,
    vk_get_instance_proc_addr: VkGetInstanceProcAddr,
    wgpu_instance_exts: &Vec<&std::ffi::CStr>,
) -> Result<ash::Instance, HelperError> {
    unsafe {
        let vk_app_info = vk::ApplicationInfo::default()
            .application_name(APPLICATION_NAME_CSTR)
            .application_version(APPLICATION_VERSION)
            .engine_name(ENGINE_NAME_CSTR)
            .engine_version(ENGINE_VERSION)
            .api_version(VK_TARGET_VERSION);

        let wgpu_instance_exts_cchar: Vec<_> =
            wgpu_instance_exts.iter().map(|ext| ext.as_ptr()).collect();

        let vk_instance = instance
            .create_vulkan_instance(
                system,
                vk_get_instance_proc_addr,
                &vk::InstanceCreateInfo::default()
                    .application_info(&vk_app_info)
                    .enabled_extension_names(&wgpu_instance_exts_cchar) as *const _
                    as *const _,
            )?
            .map_err(vk::Result::from_raw)?;

        Ok(ash::Instance::load(
            vk_entry.static_fn(),
            vk::Instance::from_raw(vk_instance as _),
        ))
    }
}

pub fn create_ash_physical_device_from_xr(
    instance: &openxr::Instance,
    system: openxr::SystemId,
    vk_instance: &ash::Instance,
) -> Result<vk::PhysicalDevice, HelperError> {
    Ok(vk::PhysicalDevice::from_raw(unsafe {
        instance.vulkan_graphics_device(system, vk_instance.handle().as_raw() as _)? as _
    }))
}

pub fn create_wgpu_device_and_queue_from_xr(
    instance: &openxr::Instance,
    system: openxr::SystemId,
) -> Result<
    (
        ash::Entry,
        wgpu::Device,
        wgpu::Queue,
        openxr::vulkan::SessionCreateInfo,
    ),
    HelperError,
> {
    let vk_requirements = instance.graphics_requirements::<openxr::Vulkan>(system)?;

    if VK_TARGET_VERSION_XR < vk_requirements.min_api_version_supported
        || VK_TARGET_VERSION_XR > vk_requirements.max_api_version_supported
    {
        panic!("OpenXR version troubles");
    }
    debug!("Vk passes requirements");

    let vk_entry = unsafe { ash::Entry::load()? };

    // Convert from ash to OpenXR func pointer type, so OpenXR uses our entry
    let vk_get_instance_proc_addr = unsafe {
        std::mem::transmute::<
            ash::vk::PFN_vkGetInstanceProcAddr,
            openxr::sys::platform::VkGetInstanceProcAddr,
        >(vk_entry.static_fn().get_instance_proc_addr)
    };

    debug!("Got vk entry");

    let wgpu_instance_flags = wgpu::InstanceFlags::default();
    let wgpu_instance_exts =
        <wgpu_hal::vulkan::Api as wgpu_hal::Api>::Instance::desired_extensions(
            &vk_entry,
            VK_TARGET_VERSION,
            wgpu_instance_flags,
        )
        .expect("instance exts");
    debug!("Got wgpu extensions");

    // ---------------------------------------------------------------------------------
    // Get the underlying instance and device/adapter, and create a hal wrapper from it
    // We need the hal adapter first so we can access required logical device extensions
    // ---------------------------------------------------------------------------------

    // Raw ash instance/physical device

    let vk_instance = create_ash_instance_from_xr(
        instance,
        system,
        &vk_entry,
        vk_get_instance_proc_addr,
        &wgpu_instance_exts,
    )
    .expect("vk instance");
    let vk_instance_ptr = vk_instance.handle().as_raw() as *const c_void;
    debug!("Created vk instance");

    let vk_physical_device = create_ash_physical_device_from_xr(instance, system, &vk_instance)
        .expect("vk physical device");
    let vk_physical_device_ptr = vk_physical_device.as_raw() as *const c_void;
    debug!("Created vk physical device");

    let vk_device_properties =
        unsafe { vk_instance.get_physical_device_properties(vk_physical_device) };
    debug!("Got vk physical device props");

    if vk_device_properties.api_version < VK_TARGET_VERSION {
        panic!(
            "vk physical device doesn't support {}",
            VK_TARGET_VERSION_XR
        );
    }

    // wgpu hal instance/adapter

    let wgpu_vk_instance = unsafe {
        <wgpu_hal::vulkan::Api as wgpu_hal::Api>::Instance::from_raw(
            vk_entry.clone(),
            vk_instance.clone(),
            vk_device_properties.api_version,
            0,
            None,
            wgpu_instance_exts,
            wgpu_instance_flags,
            wgpu::MemoryBudgetThresholds::default(),
            false,
            None, // None on the drop callback causes wgpu to take ownership, so we don't have to return the instance
        )?
    };
    debug!("Created wgpu vk instance");

    let wgpu_exposed_adapter = wgpu_vk_instance
        .expose_adapter(vk_physical_device)
        .ok_or(HelperError::WgpuExposeAdapterError)?;
    debug!("Created exposed wgpu adapter");

    // -----------------------------------------------------------
    // Create logical device and wrap it in a wgpu hal open device
    // -----------------------------------------------------------

    // I would extract this into another function, but enabled_phd_features gives me lifetime headaches. Too bad

    let wgpu_device_features = wgpu_exposed_adapter.features;

    let wgpu_device_exts = wgpu_exposed_adapter
        .adapter
        .required_device_extensions(wgpu_device_features);

    let more_device_exts = [
        ash::khr::swapchain::NAME,
        ash::khr::timeline_semaphore::NAME, // Seems to be explicitly required on platforms like the Steam Frame
    ];

    let mut enabled_phd_features = wgpu_exposed_adapter
        .adapter
        .physical_device_features(&wgpu_device_exts, wgpu_device_features);

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

    let extension_names_cchar: Vec<_> = wgpu_device_exts
        .iter()
        .chain(&more_device_exts)
        .map(|ext| ext.as_ptr())
        .collect();

    let vk_device_create_info = enabled_phd_features
        .add_to_device_create(
            vk::DeviceCreateInfo::default()
                .queue_create_infos(&family_infos)
                .push_next(&mut timeline_features)
                .push_next(&mut phyical_device_multiview_features),
        )
        .enabled_extension_names(&extension_names_cchar);

    // Make it!

    let vk_device = unsafe {
        let vk_device = instance
            .create_vulkan_device(
                system,
                vk_get_instance_proc_addr,
                vk_physical_device.as_raw() as _,
                &vk_device_create_info as *const _ as *const _,
            )?
            .map_err(vk::Result::from_raw)?;

        ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(vk_device as _))
    };

    let vk_device_ptr = vk_device.handle().as_raw() as *const c_void;
    debug!("Created vk device");

    // Wrap it in the hal object

    let wgpu_open_device = unsafe {
        wgpu_exposed_adapter.adapter.device_from_raw(
            vk_device,
            None,
            &wgpu_device_exts,
            wgpu_device_features,
            &wgpu::Limits::defaults(),
            &wgpu::MemoryHints::default(),
            family_info.queue_family_index,
            0,
        )?
    };
    debug!("Created wgpu open device");

    // -------------------------------------------------------
    // Finally, wrap everything in the final wgpu abstractions
    // -------------------------------------------------------

    let wgpu_instance =
        unsafe { wgpu::Instance::from_hal::<wgpu_hal::api::Vulkan>(wgpu_vk_instance) };

    let wgpu_adapter = unsafe { wgpu_instance.create_adapter_from_hal(wgpu_exposed_adapter) };

    let mut limits = wgpu_adapter.limits();
    limits.max_buffer_size = u32::MAX as u64;

    let (wgpu_device, wgpu_queue) = unsafe {
        wgpu_adapter.create_device_from_hal(
            wgpu_open_device,
            &wgpu::DeviceDescriptor {
                label: Some("XR wgpu device"),
                required_features: wgpu_device_features,
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            },
        )?
    };

    debug!("Created wgpu device");

    // Give OpenXR the Vulkan pointers

    let xr_session_create_info = openxr::vulkan::SessionCreateInfo {
        instance: vk_instance_ptr,
        physical_device: vk_physical_device_ptr,
        device: vk_device_ptr,
        queue_family_index: family_info.queue_family_index,
        queue_index: 0,
    };

    Ok((vk_entry, wgpu_device, wgpu_queue, xr_session_create_info))
}
