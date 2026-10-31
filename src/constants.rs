use std::ffi::CStr;

use ash::vk;

pub const VK_TARGET_VERSION_XR: openxr::Version = openxr::Version::new(1, 2, 0);
pub const VK_TARGET_VERSION: u32 = vk::make_api_version(0, 1, 2, 0);

pub const MULTIVIEW_MASK: Option<std::num::NonZeroU32> = std::num::NonZeroU32::new(0b11);

pub const APPLICATION_NAME_CSTR: &CStr = c"vr-imageview";
pub const APPLICATION_VERSION: u32 = 1;
pub const ENGINE_NAME_CSTR: &CStr = c"custom";
pub const ENGINE_VERSION: u32 = 1;
