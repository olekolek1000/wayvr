use std::{rc::Rc, sync::Arc};

use super::egl_data;
use glow::HasContext;
use smithay::backend::egl as smithay_egl;

pub struct SurfaceData {
	pub surface: khronos_egl::Surface,
	pub width: i32,
	pub height: i32,
}

impl SurfaceData {
	pub fn new(data: &egl_data::EGLData, width: i32, height: i32) -> anyhow::Result<Self> {
		let egl_pbuffer_attribs = [
			khronos_egl::WIDTH,
			width,
			khronos_egl::HEIGHT,
			height,
			khronos_egl::NONE,
		];

		let surface =
			data
				.egl
				.create_pbuffer_surface(data.display, data.config, &egl_pbuffer_attribs)?;

		Ok(Self {
			surface,
			width,
			height,
		})
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

pub fn debug_save_pixmap(
	data: &egl_data::EGLData,
	surface: &SurfaceData,
	path: &str,
) -> anyhow::Result<()> {
	log::trace!("Saving pixmap image {}", path);

	data.make_current(&surface.surface)?;

	let mut pixels = vec![0u8; (surface.width * surface.height * 4) as usize];

	unsafe {
		data.gl.read_pixels(
			0,
			0,
			surface.width,
			surface.height,
			glow::RGBA,
			glow::UNSIGNED_BYTE,
			glow::PixelPackData::Slice(&mut pixels),
		);
	}

	image::save_buffer(
		path,
		&pixels,
		surface.width as u32,
		surface.height as u32,
		image::ExtendedColorType::Rgba8,
	)?;

	Ok(())
}
