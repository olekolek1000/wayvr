use smithay::{
	backend::renderer::{
		gles::{ffi, GlesRenderer, GlesTexture},
		Bind,
	},
	input::SeatState,
	reexports::wayland_server::{self},
	wayland::{
		compositor, selection::data_device::DataDeviceState, shell::xdg::XdgShellState, shm::ShmState,
	},
};

pub use crate::egl_data;

use crate::{client, comp::Application, smithay_wrapper, time::get_millis};

#[derive(Clone)]
pub struct WaylandEnv {
	pub display_num: u32,
}

impl WaylandEnv {
	pub fn display_num_string(&self) -> String {
		// e.g. "wayland-20"
		format!("wayland-{}", self.display_num)
	}
}

#[allow(dead_code)]
pub struct WayVR {
	time_start: u64,
	width: u32,
	height: u32,
	gles_renderer: GlesRenderer,
	egl_data: egl_data::EGLData,
	egl_image: khronos_egl::Image,
	dmabuf_data: egl_data::DMAbufData,

	client_manager: client::ClientManager,
}

pub enum MouseIndex {
	Left,
	Center,
	Right,
}

impl WayVR {
	pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
		let display: wayland_server::Display<Application> = wayland_server::Display::new()?;
		let dh = display.handle();
		let compositor = compositor::CompositorState::new::<Application>(&dh);
		let xdg_shell = XdgShellState::new::<Application>(&dh);
		let mut seat_state = SeatState::new();
		let shm = ShmState::new::<Application>(&dh, Vec::new());
		let data_device = DataDeviceState::new::<Application>(&dh);
		let mut seat = seat_state.new_wl_seat(&dh, "wayvr");

		// TODO: Keyboard repeat delay and rate?
		let seat_keyboard = seat.add_keyboard(Default::default(), 100, 100)?;
		let seat_pointer = seat.add_pointer();

		let state = Application {
			compositor,
			xdg_shell,
			seat_state,
			shm,
			data_device,
		};

		let time_start = get_millis();
		let egl_data = egl_data::EGLData::new()?;
		let smithay_display = smithay_wrapper::get_egl_display(&egl_data)?;
		let smithay_context = smithay_wrapper::get_egl_context(&egl_data, &smithay_display)?;
		let mut gles_renderer = unsafe { GlesRenderer::new(smithay_context)? };

		let tex_format = ffi::RGBA;
		let internal_format = ffi::RGBA8;

		let tex_id = gles_renderer.with_context(|gl| {
			smithay_wrapper::create_framebuffer_texture(gl, width, height, tex_format, internal_format)
		})?;
		let egl_image = egl_data.create_egl_image(tex_id, width, height)?;
		let dmabuf_data = egl_data.create_dmabuf_data(&egl_image)?;

		let opaque = false;
		let size = (width as i32, height as i32).into();
		let gles_texture =
			unsafe { GlesTexture::from_raw(&gles_renderer, Some(tex_format), opaque, tex_id, size) };

		gles_renderer.bind(gles_texture)?;

		Ok(Self {
			width,
			height,
			gles_renderer,
			time_start,
			egl_data,
			egl_image,
			dmabuf_data,
			client_manager: client::ClientManager::new(state, display, seat_keyboard, seat_pointer)?,
		})
	}

	pub fn spawn_process(
		&mut self,
		exec_path: &str,
		args: Vec<&str>,
		env: Vec<(&str, &str)>,
	) -> anyhow::Result<()> {
		self.client_manager.spawn_process(exec_path, args, env)
	}

	pub fn tick(&mut self) -> anyhow::Result<()> {
		// millis since the start of wayvr
		let time_ms = get_millis() - self.time_start;

		self
			.client_manager
			.tick_render(&mut self.gles_renderer, self.width, self.height, time_ms)?;
		self.client_manager.tick_wayland()?;

		self.gles_renderer.with_context(|gl| unsafe {
			gl.Flush();
			gl.Finish();
		})?;

		Ok(())
	}

	pub fn send_mouse_move(&mut self, x: u32, y: u32) {
		self.client_manager.send_mouse_move(x, y);
	}

	pub fn send_mouse_down(&mut self, index: MouseIndex) {
		self.client_manager.send_mouse_down(index);
	}

	pub fn send_mouse_up(&mut self, index: MouseIndex) {
		self.client_manager.send_mouse_up(index);
	}

	pub fn send_mouse_scroll(&mut self, delta: f32) {
		self.client_manager.send_mouse_scroll(delta);
	}

	pub fn get_dmabuf_data(&self) -> egl_data::DMAbufData {
		self.dmabuf_data.clone()
	}
}
