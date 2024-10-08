use std::rc::Rc;

use anyhow::anyhow;

pub struct EGLData {
	pub egl: khronos_egl::Instance<khronos_egl::Static>,
	pub display: khronos_egl::Display,
	pub config: khronos_egl::Config,
	pub context: khronos_egl::Context,

	#[allow(dead_code)]
	pub gl: Rc<glow::Context>,
}

impl EGLData {
	pub fn new() -> anyhow::Result<EGLData> {
		unsafe {
			let egl = khronos_egl::Instance::new(khronos_egl::Static);

			let display = egl
				.get_display(khronos_egl::DEFAULT_DISPLAY)
				.ok_or(anyhow!("eglGetDisplay failed"))?;

			let (major, minor) = egl.initialize(display)?;
			log::debug!("EGL version: {}.{}", major, minor);

			let attrib_list = [
				khronos_egl::RED_SIZE,
				8,
				khronos_egl::GREEN_SIZE,
				8,
				khronos_egl::BLUE_SIZE,
				8,
				khronos_egl::SURFACE_TYPE,
				khronos_egl::WINDOW_BIT,
				khronos_egl::RENDERABLE_TYPE,
				khronos_egl::OPENGL_BIT,
				khronos_egl::NONE,
			];

			let config = egl
				.choose_first_config(display, &attrib_list)?
				.ok_or(anyhow!("Failed to get EGL config"))?;

			egl.bind_api(khronos_egl::OPENGL_ES_API)?;

			log::debug!("eglCreateContext");

			// Require OpenGL ES 3.0
			let context_attrib_list = [
				khronos_egl::CONTEXT_MAJOR_VERSION,
				3,
				khronos_egl::CONTEXT_MINOR_VERSION,
				0,
				khronos_egl::NONE,
			];

			let context = egl.create_context(display, config, None, &context_attrib_list)?;

			egl.make_current(display, None, None, Some(context))?;

			let gl = Rc::new(glow::Context::from_loader_function(|name| {
				egl.get_proc_address(name).unwrap() as *const std::ffi::c_void
			}));

			Ok(EGLData {
				egl,
				display,
				config,
				context,
				gl,
			})
		}
	}

	#[allow(dead_code)]
	pub fn make_current(&self, surface: &khronos_egl::Surface) -> anyhow::Result<()> {
		self.egl.make_current(
			self.display,
			Some(*surface),
			Some(*surface),
			Some(self.context),
		)?;

		Ok(())
	}
}
