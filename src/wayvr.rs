#![allow(dead_code)]

use std::sync::Arc;

use anyhow::bail;
use smithay::{
	backend::{
		egl::ffi::{
			egl::types::{EGLBoolean, EGLImageKHR, EGLuint64KHR},
			EGLint,
		},
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
	input::SeatState,
	reexports::wayland_server::{self, ListeningSocket},
	utils::{Rectangle, Size, Transform},
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
	egl_image: khronos_egl::Image,
	dmabuf_data: DMAbufData,

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

//eglExportDMABUFImageQueryMESA
pub type PFNEGLEXPORTDMABUFIMAGEQUERYMESAPROC = Option<
	unsafe extern "C" fn(
		dpy: khronos_egl::EGLDisplay,
		image: khronos_egl::EGLImage,
		fourcc: *mut std::ffi::c_int,
		num_planes: *mut std::ffi::c_int,
		modifiers: *mut EGLuint64KHR,
	) -> EGLBoolean,
>;

const FOURCC: u32 = 0x34324258;

const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: usize = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: usize = 0x3444;

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

		let tex_format = ffi::RGB;
		let internal_format = ffi::RGB8;

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
			/*let egl_export_dmabuf_image_query_mesa = bind_egl_function!(
							PFNEGLEXPORTDMABUFIMAGEQUERYMESAPROC,
							&egl_data.load_func("eglExportDMABUFImageQueryMESA")?
						);

						let mut fourcc = FOURCC as std::ffi::c_int;
						let mut num_planes: std::ffi::c_int = 1;
						let mut modifiers: [EGLuint64KHR; 16] = [0; 16];

						if egl_export_dmabuf_image_query_mesa(
							egl_data.display.as_ptr(),
							egl_image.as_ptr(),
							&mut fourcc,
							&mut num_planes,
							modifiers.as_mut_ptr(),
						) != khronos_egl::TRUE
						{
							bail!("eglExportDMABUFImageQueryMESA failed")
						}

						//let modifier = 0x20000002086bf04;

						if num_planes != 1 {
							bail!("expected exactly one dmabuf plane")
						}

						let modifier = modifiers[0];
						if modifier == 0xFFFFFFFFFFFFFF {
							bail!("modifier is -1");
						}
			*/
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
				log::info!(
					"idx {}, format {}{}{}{} (hex {:#x})",
					idx,
					bytes[0] as char,
					bytes[1] as char,
					bytes[2] as char,
					bytes[3] as char,
					format
				);
			}

			let target_format = 0x34325258; //XR24
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
				bail!("mods is -1")
			}

			log::info!("Mods:");
			for modifier in &mods {
				log::info!("{:#x}", modifier);
			}

			dmabuf_data.modifiers = mods;
			dmabuf_data.fourcc = target_format;
		}

		gles_renderer.bind(gles_texture)?;

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
			egl_image,
			dmabuf_data,
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

		let mut frame = self.gles_renderer.render(size, Transform::Normal)?;
		frame.clear(Color32F::new(1.0, 1.0, 1.0, 0.1), &[damage])?;

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

	pub fn send_mouse_move(&mut self, _x: u32, _y: u32) {}

	pub fn send_mouse_down(&mut self, _index: MouseIndex) {}

	pub fn send_mouse_up(&mut self, _index: MouseIndex) {}

	pub fn send_mouse_scroll(&mut self, _delta: f32) {}

	pub fn get_dmabuf_data(&self) -> DMAbufData {
		self.dmabuf_data.clone()
	}
}
