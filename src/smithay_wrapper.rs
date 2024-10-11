use super::egl_data;
use smithay::{
	backend::egl as smithay_egl, reexports::wayland_server::protocol::wl_surface::WlSurface,
	xwayland::X11Surface,
};

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
