use std::sync::Arc;

use anyhow::bail;
use smithay::{
	backend::{
		egl::ffi::{
			egl::types::{EGLBoolean, EGLImageKHR, EGLuint64KHR},
			EGLint,
		},
		input::{Axis, AxisRelativeDirection},
		renderer::{
			element::{
				surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
				Kind,
			},
			gles::{ffi, GlesRenderer, GlesTexture},
			utils::draw_render_elements,
			Bind, Color32F, Frame, Renderer,
		},
	},
	input::{
		keyboard::KeyboardHandle,
		pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle, PointerTarget},
		SeatState,
	},
	reexports::wayland_server::{self, ListeningSocket},
	utils::{Logical, Physical, Point, Rectangle, SerialCounter, Size, Transform},
	wayland::{
		compositor, selection::data_device::DataDeviceState, shell::xdg::XdgShellState, shm::ShmState,
	},
};

pub use crate::egl_data;

use crate::{
	bind_egl_function,
	comp::{send_frames_surface_tree, Application, ClientState},
	smithay_wrapper,
	time::get_millis,
};

const STARTING_WAYLAND_ADDR_IDX: u32 = 20;

#[derive(Clone)]
pub struct DMAbufData {
	pub fd: i32,
	pub stride: i32,
	pub offset: i32,
	pub modifiers: Vec<u64>,
	pub fourcc: std::ffi::c_int,
}

#[allow(dead_code)]
pub struct WayVR {
	time_start: u64,
	width: u32,
	height: u32,
	wayland_display_addr: String, // e.g. "wayland-20"
	wayland_display_num: u32,
	state: Application,
	gles_renderer: GlesRenderer,
	listener: ListeningSocket,
	display: wayland_server::Display<Application>,
	egl_data: egl_data::EGLData,
	egl_image: khronos_egl::Image,
	dmabuf_data: DMAbufData,
	seat_keyboard: KeyboardHandle<Application>,
	seat_pointer: PointerHandle<Application>,
	serial_counter: SerialCounter,

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

//eglExportDMABUFImageMESA
pub type PFNEGLEXPORTDMABUFIMAGEMESAPROC = Option<
	unsafe extern "C" fn(
		dpy: khronos_egl::EGLDisplay,
		image: EGLImageKHR,
		fds: *mut i32,
		strides: *mut EGLint,
		offsets: *mut EGLint,
	) -> EGLBoolean,
>;

//eglQueryDmaBufModifiersEXT
pub type PFNEGLQUERYDMABUFMODIFIERSEXTPROC = Option<
	unsafe extern "C" fn(
		dpy: khronos_egl::EGLDisplay,
		format: EGLint,
		max_modifiers: EGLint,
		modifiers: *mut EGLuint64KHR,
		external_only: *mut EGLBoolean,
		num_modifiers: *mut EGLint,
	) -> EGLBoolean,
>;

//eglQueryDmaBufFormatsEXT
pub type PFNEGLQUERYDMABUFFORMATSEXTPROC = Option<
	unsafe extern "C" fn(
		dpy: khronos_egl::EGLDisplay,
		max_formats: EGLint,
		formats: *mut EGLint,
		num_formats: *mut EGLint,
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

		// Try "wayland-20", "wayland-21", "wayland-22"...
		let mut display_addr_idx = STARTING_WAYLAND_ADDR_IDX;
		let mut wayland_display_addr;
		let mut wayland_display_num;
		let listener = loop {
			wayland_display_addr = format!("wayland-{}", display_addr_idx);
			wayland_display_num = display_addr_idx;
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

					display_addr_idx += 1;
					if display_addr_idx > STARTING_WAYLAND_ADDR_IDX + 20 {
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

		let tex_format = ffi::RGBA;
		let internal_format = ffi::RGBA8;

		// Create framebuffer texture
		let tex = gles_renderer.with_context(|gl| unsafe {
			let mut tex = 0;
			gl.GenTextures(1, &mut tex);
			gl.BindTexture(ffi::TEXTURE_2D, tex);
			gl.TexParameteri(
				ffi::TEXTURE_2D,
				ffi::TEXTURE_MIN_FILTER,
				ffi::NEAREST as i32,
			);
			gl.TexParameteri(
				ffi::TEXTURE_2D,
				ffi::TEXTURE_MAG_FILTER,
				ffi::NEAREST as i32,
			);
			gl.TexImage2D(
				ffi::TEXTURE_2D,
				0,
				internal_format as i32,
				width as i32,
				height as i32,
				0,
				tex_format,
				ffi::UNSIGNED_BYTE,
				std::ptr::null(),
			);
			gl.BindTexture(ffi::TEXTURE_2D, 0);
			tex
		})?;

		let opaque = false;
		let size = (width as i32, height as i32).into();
		let gles_texture =
			unsafe { GlesTexture::from_raw(&gles_renderer, Some(tex_format), opaque, tex, size) };

		// Create EGL image from texture
		let egl_image = unsafe {
			egl_data.egl.create_image(
				egl_data.display,
				egl_data.context,
				khronos_egl::GL_TEXTURE_2D as std::ffi::c_uint,
				khronos_egl::ClientBuffer::from_ptr(gles_texture.tex_id() as *mut std::ffi::c_void),
				&[
					khronos_egl::WIDTH as usize,
					width as usize,
					khronos_egl::HEIGHT as usize,
					height as usize,
					khronos_egl::ATTRIB_NONE,
				],
			)?
		};

		// Create dmabuf from EGL image

		let mut dmabuf_data = unsafe {
			let egl_export_dmabuf_image_mesa = bind_egl_function!(
				PFNEGLEXPORTDMABUFIMAGEMESAPROC,
				&egl_data.load_func("eglExportDMABUFImageMESA")?
			);

			let mut fds: [i32; 3] = [0; 3];
			let mut strides: [i32; 3] = [0; 3];
			let mut offsets: [i32; 3] = [0; 3];

			if egl_export_dmabuf_image_mesa(
				egl_data.display.as_ptr(),
				egl_image.as_ptr(),
				fds.as_mut_ptr(),
				strides.as_mut_ptr(),
				offsets.as_mut_ptr(),
			) != khronos_egl::TRUE
			{
				anyhow::bail!("eglExportDMABUFImageMESA failed");
			}

			// many planes in RGB data?
			debug_assert!(fds[1] == 0);
			debug_assert!(strides[1] == 0);
			debug_assert!(offsets[1] == 0);

			DMAbufData {
				fd: fds[0],
				stride: strides[0],
				offset: offsets[0],
				fourcc: 0,
				modifiers: Vec::new(),
			}
		};

		unsafe {
			let egl_query_dmabuf_formats_ext = bind_egl_function!(
				PFNEGLQUERYDMABUFFORMATSEXTPROC,
				&egl_data.load_func("eglQueryDmaBufFormatsEXT")?
			);

			// Query format count
			let mut num_formats: EGLint = 0;
			egl_query_dmabuf_formats_ext(
				egl_data.display.as_ptr(),
				0,
				std::ptr::null_mut(),
				&mut num_formats,
			);

			// Retrieve formt list
			let mut formats: Vec<i32> = vec![0; num_formats as usize];
			egl_query_dmabuf_formats_ext(
				egl_data.display.as_ptr(),
				num_formats,
				formats.as_mut_ptr(),
				&mut num_formats,
			);

			for (idx, format) in formats.iter().enumerate() {
				let bytes = format.to_le_bytes();
				log::trace!(
					"idx {}, format {}{}{}{} (hex {:#x})",
					idx,
					bytes[0] as char,
					bytes[1] as char,
					bytes[2] as char,
					bytes[3] as char,
					format
				);
			}

			let target_format = 0x34324258; //XB24
			let egl_query_dmabuf_modifiers_ext = bind_egl_function!(
				PFNEGLQUERYDMABUFMODIFIERSEXTPROC,
				&egl_data.load_func("eglQueryDmaBufModifiersEXT")?
			);

			let mut num_mods: EGLint = 0;

			// Query modifier count
			egl_query_dmabuf_modifiers_ext(
				egl_data.display.as_ptr(),
				target_format,
				0,
				std::ptr::null_mut(),
				std::ptr::null_mut(),
				&mut num_mods,
			);

			if num_mods == 0 {
				bail!("eglQueryDmaBufModifiersEXT modifier count is zero");
			}

			let mut mods: Vec<u64> = vec![0; num_mods as usize];
			egl_query_dmabuf_modifiers_ext(
				egl_data.display.as_ptr(),
				target_format,
				num_mods,
				mods.as_mut_ptr(),
				std::ptr::null_mut(),
				&mut num_mods,
			);

			if mods[0] == 0xFFFFFFFFFFFFFFFF {
				bail!("modifier is -1")
			}

			log::trace!("Modifier list:");
			for modifier in &mods {
				log::trace!("{:#x}", modifier);
			}

			// We should not change these modifier values. Passing all of them to the Vulkan dmabuf
			// texture system causes significant graphical corruption due to invalid memory layout and
			// tiling on this specific GPU model (very probably others also have the same issue).
			// It is not guaranteed that this modifier will be present in other models.
			// If not, the full list of modifiers will be passed. Further testing is required.
			let mod_whitelist: [u64; 1] = [0x20000002086bf04 /* AMD RX 7800 XT */];

			for modifier in &mod_whitelist {
				if mods.contains(modifier) {
					log::warn!("Using whitelisted dmabuf tiling modifier: {:#x}", modifier);
					mods = vec![*modifier, 0x0 /* also important (???) */];
					break;
				}
			}

			dmabuf_data.modifiers = mods;
			dmabuf_data.fourcc = target_format;
		}

		gles_renderer.bind(gles_texture)?;

		Ok(Self {
			state,
			wayland_display_addr,
			wayland_display_num,
			width,
			height,
			gles_renderer,
			time_start,
			listener,
			display,
			clients: Vec::new(),
			process_children: Vec::new(),
			egl_data,
			egl_image,
			dmabuf_data,
			seat_keyboard,
			seat_pointer,
			serial_counter: SerialCounter::new(),
		})
	}

	fn set_default_env(&self, cmd: &mut std::process::Command) {
		cmd.env_remove("DISPLAY"); // Goodbye X11
		cmd.env("WAYLAND_DISPLAY", self.wayland_display_addr.as_str());
	}

	pub fn spawn_process(
		&mut self,
		exec_path: &str,
		args: Vec<&str>,
		env: Vec<(&str, &str)>,
	) -> anyhow::Result<()> {
		log::info!("Spawning subprocess with exec path \"{}\"", exec_path);
		let mut cmd = std::process::Command::new(exec_path);
		self.set_default_env(&mut cmd);
		cmd.args(args);

		for e in &env {
			cmd.env(e.0, e.1);
		}

		match cmd.spawn() {
			Ok(child) => {
				self.process_children.push(child);
			}
			Err(e) => {
				bail!(
					"Failed to launch process with path \"{}\": {}. Make sure your exec path exists.",
					exec_path,
					e
				);
			}
		}

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

		let mut frame = self.gles_renderer.render(size, Transform::Normal)?;
		frame.clear(Color32F::new(0.0, 0.0, 0.0, 0.5), &[damage])?;

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

		self.gles_renderer.with_context(|gl| unsafe {
			gl.Flush();
			gl.Finish();
		})?;

		Ok(())
	}

	fn get_mouse_index_number(index: MouseIndex) -> u32 {
		match index {
			MouseIndex::Left => 0x110,   /* BTN_LEFT */
			MouseIndex::Center => 0x112, /* BTN_MIDDLE */
			MouseIndex::Right => 0x111,  /* BTN_RIGHT */
		}
	}

	pub fn send_mouse_move(&mut self, x: u32, y: u32) {
		let point = Point::<f64, Logical>::from((x as f64, y as f64));
		if let Some(surf) = self.get_focus_surface(point) {
			self.seat_pointer.motion(
				&mut self.state,
				Some((surf, Point::from((0.0, 0.0)))),
				&MotionEvent {
					serial: self.serial_counter.next_serial(),
					time: 0,
					location: point,
				},
			);

			self.seat_pointer.frame(&mut self.state);
		}
	}

	fn get_focus_surface(
		&mut self,
		_pos: Point<f64, Logical>,
	) -> Option<wayland_server::protocol::wl_surface::WlSurface> {
		self
			.state
			.xdg_shell
			.toplevel_surfaces()
			.iter()
			.next()
			.cloned()
			.map(|surface| surface.wl_surface().clone())
	}

	pub fn send_mouse_down(&mut self, index: MouseIndex) {
		// Change keyboard focus to pressed window
		if let Some(surf) = self.get_focus_surface(self.seat_pointer.current_location()) {
			if self.seat_keyboard.current_focus().is_none() {
				self.seat_keyboard.set_focus(
					&mut self.state,
					Some(surf),
					self.serial_counter.next_serial(),
				);
			}
		}

		self.seat_pointer.button(
			&mut self.state,
			&ButtonEvent {
				button: WayVR::get_mouse_index_number(index),
				serial: self.serial_counter.next_serial(),
				time: 0,
				state: smithay::backend::input::ButtonState::Pressed,
			},
		);

		self.seat_pointer.frame(&mut self.state);
	}

	pub fn send_mouse_up(&mut self, index: MouseIndex) {
		self.seat_pointer.button(
			&mut self.state,
			&ButtonEvent {
				button: WayVR::get_mouse_index_number(index),
				serial: self.serial_counter.next_serial(),
				time: 0,
				state: smithay::backend::input::ButtonState::Released,
			},
		);

		self.seat_pointer.frame(&mut self.state);
	}

	pub fn send_mouse_scroll(&mut self, delta: f32) {
		self.seat_pointer.axis(
			&mut self.state,
			AxisFrame {
				source: None,
				relative_direction: (
					AxisRelativeDirection::Identical,
					AxisRelativeDirection::Identical,
				),
				time: 0,
				axis: (0.0, -delta as f64),
				v120: Some((0, (delta * -120.0) as i32)),
				stop: (false, false),
			},
		);
		self.seat_pointer.frame(&mut self.state);
	}

	pub fn get_dmabuf_data(&self) -> DMAbufData {
		self.dmabuf_data.clone()
	}
}
