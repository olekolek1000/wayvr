#![allow(dead_code)]

use std::{rc::Rc, sync::Arc};

use smithay::{
	backend::{
		egl::{
			ffi::{
				egl::types::{EGLBoolean, EGLImageKHR},
				EGLint,
			},
			EGLDisplay,
		},
		renderer::{
			element::{
				surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
				Kind,
			},
			gles::GlesRenderer,
			utils::draw_render_elements,
			Bind, Color32F, Frame, Renderer,
		},
	},
	input::SeatState,
	reexports::wayland_server::{self, ListeningSocket},
	utils::{Rectangle, Size, Transform},
	wayland::{
		compositor, selection::data_device::DataDeviceState, shell::xdg::XdgShellState, shm::ShmState,
	},
};

pub use crate::egl_data;

use crate::{
	comp::{send_frames_surface_tree, Application, ClientState},
	smithay_wrapper,
	time::get_millis,
};

const STARTING_WAYLAND_ADDR_IDX: u32 = 20;

pub struct WayVR {
	time_start: u64,
	width: u32,
	height: u32,
	wayland_display_addr: String, // e.g. "wayland-20"
	state: Application,
	gles_renderer: GlesRenderer,
	listener: ListeningSocket,
	display: wayland_server::Display<Application>,
	egl_data: egl_data::EGLData,
	surface_data: Rc<smithay_wrapper::SurfaceData>,

	// TODO: Cleanup of expired clients
	clients: Vec<wayland_server::Client>,

	// TODO: Cleanup of child processes
	process_children: Vec<std::process::Child>,
}

pub enum MouseIndex {
	Left,
	Center,
	Right,
}

pub type PFNEGLEXPORTDMABUFIMAGEMESAPROC = Option<
	unsafe extern "C" fn(
		dpy: EGLDisplay,
		image: EGLImageKHR,
		fds: *mut i32,
		strides: *mut EGLint,
		offsets: *mut EGLint,
	) -> EGLBoolean,
>;

impl WayVR {
	pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
		let display: wayland_server::Display<Application> = wayland_server::Display::new()?;
		let dh = display.handle();
		let compositor = compositor::CompositorState::new::<Application>(&dh);
		let xdg_shell = XdgShellState::new::<Application>(&dh);
		let mut seat_state = SeatState::new();
		let shm = ShmState::new::<Application>(&dh, Vec::new());
		let data_device = DataDeviceState::new::<Application>(&dh);
		let _seat = seat_state.new_wl_seat(&dh, "wayvr");

		let state = Application {
			compositor,
			xdg_shell,
			seat_state,
			shm,
			data_device,
		};

		// Try "wayland-20", "wayland-21", "wayland-22"...
		let display_addr_idx = STARTING_WAYLAND_ADDR_IDX;
		let mut try_idx = 0;
		let mut wayland_display_addr;
		let listener = loop {
			wayland_display_addr = format!("wayland-{}", display_addr_idx);
			log::debug!("Trying to open socket \"{}\"", wayland_display_addr);
			match ListeningSocket::bind(wayland_display_addr.as_str()) {
				Ok(listener) => {
					log::debug!("Listening to {}", wayland_display_addr);
					break listener;
				}
				Err(e) => {
					log::debug!(
						"Failed to open socket \"{}\" (reason: {}), trying next...",
						wayland_display_addr,
						e
					);

					try_idx += 1;
					if try_idx > 20 {
						// Highly unlikely for the user to have 20 Wayland displays enabled at once. Return error instead.
						return Err(anyhow::anyhow!("Failed to create wayland-server socket"))?;
					}
				}
			}
		};

		log::debug!("Starting loop");

		let time_start = get_millis();

		// Init EGL display and context
		let egl_data = egl_data::EGLData::new()?;

		let smithay_display = smithay_wrapper::get_egl_display(&egl_data)?;
		let smithay_context = smithay_wrapper::get_egl_context(&egl_data, &smithay_display)?;
		let mut gles_renderer = unsafe { GlesRenderer::new(smithay_context)? };

		let pixel_format = gles_renderer
			.egl_context()
			.pixel_format()
			.ok_or(anyhow::anyhow!("Failed to get pixel format"))?;

		let surface_data = Rc::new(smithay_wrapper::SurfaceData::new(&egl_data, width, height)?);

		let func_name = "eglExportDMABUFImageMESA";
		let raw_fn_egl_export_dmabuf_image_mesa =
			egl_data
				.egl
				.get_proc_address(func_name)
				.ok_or(anyhow::anyhow!(
					"Required EGL function {} not found",
					func_name
				))?;

		let fn_egl_export_dmabuf_image_mesa = unsafe {
			std::mem::transmute_copy::<_, PFNEGLEXPORTDMABUFIMAGEMESAPROC>(
				&raw_fn_egl_export_dmabuf_image_mesa,
			)
			.unwrap() /* should never fail */
		};

		//fixme EGL_BAD_PARAMETER
		let image = unsafe {
			egl_data.egl.create_image(
				egl_data.display,
				egl_data.context,
				khronos_egl::GL_TEXTURE_2D as std::ffi::c_uint,
				khronos_egl::ClientBuffer::from_ptr(surface_data.surface.as_ptr()),
				&[khronos_egl::ATTRIB_NONE],
			)?
		};

		/*fn_egl_export_dmabuf_image_mesa(
			egl_data.display.as_ptr(), /* EGLDisplay dpy */
			surface_data.surface.as_ptr(),
		);*/

		let smithay_surface = Rc::new(smithay_wrapper::create_egl_surface(
			&egl_data,
			&smithay_display,
			pixel_format,
			surface_data.clone(),
		)?);

		gles_renderer.bind(smithay_surface)?;

		Ok(Self {
			state,
			wayland_display_addr,
			width,
			height,
			gles_renderer,
			time_start,
			listener,
			display,
			clients: Vec::new(),
			process_children: Vec::new(),
			egl_data,
			surface_data,
		})
	}

	pub fn spawn_process(&mut self, exec_path: &str, args: Vec<String>) -> anyhow::Result<()> {
		log::debug!("Spawning process");
		let mut cmd = std::process::Command::new(exec_path);
		cmd.env_remove("DISPLAY"); // prevent running XWayland apps for now
		cmd.args(args);
		cmd.env("WAYLAND_DISPLAY", self.wayland_display_addr.as_str());
		let child = cmd.spawn()?;
		self.process_children.push(child);
		Ok(())
	}

	pub fn tick(&mut self) -> anyhow::Result<()> {
		let size = Size::from((self.width as i32, self.height as i32));
		let damage: Rectangle<i32, smithay::utils::Physical> =
			Rectangle::from_loc_and_size((0, 0), size);

		let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = self
			.state
			.xdg_shell
			.toplevel_surfaces()
			.iter()
			.flat_map(|surface| {
				render_elements_from_surface_tree(
					&mut self.gles_renderer,
					surface.wl_surface(),
					(0, 0),
					1.0,
					1.0,
					Kind::Unspecified,
				)
			})
			.collect();

		let mut frame = self.gles_renderer.render(size, Transform::Flipped180)?;
		frame.clear(Color32F::new(0.3, 0.3, 0.3, 1.0), &[damage])?;

		draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;

		let _sync_point = frame.finish()?;

		let time_cur = get_millis();

		for surface in self.state.xdg_shell.toplevel_surfaces() {
			send_frames_surface_tree(surface.wl_surface(), (time_cur - self.time_start) as u32);
		}

		if let Some(stream) = self.listener.accept()? {
			log::debug!("Stream accepted: {:?}", stream);

			let client = self
				.display
				.handle()
				.insert_client(stream, Arc::new(ClientState::default()))
				.unwrap();
			self.clients.push(client);
		}

		self.display.dispatch_clients(&mut self.state)?;
		self.display.flush_clients()?;

		Ok(())
	}

	pub fn get_image_data(&mut self) -> (&egl_data::EGLData, &khronos_egl::Surface) {
		(&self.egl_data, &self.surface_data.surface)
	}

	pub fn send_mouse_move(&mut self, _x: u32, _y: u32) {}

	pub fn send_mouse_down(&mut self, _index: MouseIndex) {}

	pub fn send_mouse_up(&mut self, _index: MouseIndex) {}

	pub fn send_mouse_scroll(&mut self, _delta: f32) {}
}
