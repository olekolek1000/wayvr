use std::{rc::Rc, sync::Arc};

use super::egl_data;
use smithay::backend::egl as smithay_egl;

pub struct SurfaceData {
	pub surface: khronos_egl::Surface,
}

impl SurfaceData {
	pub fn new(data: &egl_data::EGLData, width: u32, height: u32) -> anyhow::Result<Self> {
		let egl_pbuffer_attribs = [
			khronos_egl::WIDTH,
			width as i32,
			khronos_egl::HEIGHT,
			height as i32,
			khronos_egl::NONE,
		];

		let surface =
			data
				.egl
				.create_pbuffer_surface(data.display, data.config, &egl_pbuffer_attribs)?;

		Ok(Self { surface })
	}
}

pub struct NativeSurfaceWrapper {
	surface: Rc<SurfaceData>,
}

// required for *mut c_void!!
unsafe impl Send for NativeSurfaceWrapper {}
unsafe impl Sync for NativeSurfaceWrapper {}

impl NativeSurfaceWrapper {
	pub fn new(surface: Rc<SurfaceData>) -> anyhow::Result<Self> {
		Ok(Self { surface })
	}
}

unsafe impl smithay_egl::native::EGLNativeSurface for NativeSurfaceWrapper {
	unsafe fn create(
		&self,
		_display: &Arc<smithay_egl::display::EGLDisplayHandle>,
		_config_id: smithay_egl::ffi::egl::types::EGLConfig,
	) -> Result<*const std::os::raw::c_void, smithay_egl::EGLError> {
		// Return previously created surface
		// Always succeeds, for now we don't support pixmap resizing
		Ok(self.surface.surface.as_ptr())
	}
}

pub fn get_egl_display(data: &egl_data::EGLData) -> anyhow::Result<smithay_egl::EGLDisplay> {
	Ok(unsafe { smithay_egl::EGLDisplay::from_raw(data.display.as_ptr(), data.config.as_ptr())? })
}

pub fn get_egl_context(
	data: &egl_data::EGLData,
	display: &smithay_egl::EGLDisplay,
) -> anyhow::Result<smithay_egl::EGLContext> {
	let display_ptr = display.get_display_handle().handle;
	debug_assert!(display_ptr == data.display.as_ptr());
	let config_ptr = data.config.as_ptr();
	let context_ptr = data.context.as_ptr();
	Ok(unsafe { smithay_egl::EGLContext::from_raw(display_ptr, config_ptr, context_ptr)? })
}

pub fn create_egl_surface(
	data: &egl_data::EGLData,
	display: &smithay_egl::EGLDisplay,
	pixel_format: smithay_egl::display::PixelFormat,
	surface_data: Rc<SurfaceData>,
) -> anyhow::Result<smithay_egl::EGLSurface> {
	let native = NativeSurfaceWrapper::new(surface_data)?;
	Ok(unsafe { smithay_egl::EGLSurface::new(display, pixel_format, data.config.as_ptr(), native)? })
}
