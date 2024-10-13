use std::sync::Arc;

use smithay::{
	backend::renderer::{
		element::{
			surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
			Kind,
		},
		gles::GlesRenderer,
		utils::draw_render_elements,
		Color32F, Frame, Renderer,
	},
	input::{self, keyboard::KeyboardHandle, pointer::PointerHandle},
	reexports::wayland_server,
	utils::{Logical, Point, Rectangle, SerialCounter, Size, Transform},
};

use crate::{
	comp::{self, send_frames_surface_tree},
	wayvr::{self, WaylandEnv},
	window,
};

struct Process {
	child: std::process::Child,
}

impl Drop for Process {
	fn drop(&mut self) {
		let _dont_care = self.child.kill();
	}
}

pub struct ClientManager {
	state: comp::Application,
	display: wayland_server::Display<comp::Application>,
	listener: wayland_server::ListeningSocket,
	wayland_env: WaylandEnv,
	serial_counter: SerialCounter,
	seat_keyboard: KeyboardHandle<comp::Application>,
	seat_pointer: PointerHandle<comp::Application>,

	clients: Vec<wayland_server::Client>,
	wm: window::WindowManager,
	processes: Vec<Process>,
}

impl ClientManager {
	pub fn new(
		state: comp::Application,
		display: wayland_server::Display<comp::Application>,
		seat_keyboard: KeyboardHandle<comp::Application>,
		seat_pointer: PointerHandle<comp::Application>,
		disp_width: u32,
		disp_height: u32,
	) -> anyhow::Result<Self> {
		let (wayland_env, listener) = create_wayland_listener()?;

		Ok(Self {
			state,
			display,
			seat_keyboard,
			seat_pointer,
			listener,
			wayland_env,
			serial_counter: SerialCounter::new(),
			processes: Vec::new(),
			clients: Vec::new(),
			wm: window::WindowManager::new(disp_width, disp_height),
		})
	}

	fn configure_env(&self, cmd: &mut std::process::Command) {
		cmd.env_remove("DISPLAY"); // Goodbye X11
		cmd.env("WAYLAND_DISPLAY", self.wayland_env.display_num_string());
	}

	pub fn spawn_process(
		&mut self,
		exec_path: &str,
		args: Vec<&str>,
		env: Vec<(&str, &str)>,
	) -> anyhow::Result<()> {
		log::info!("Spawning subprocess with exec path \"{}\"", exec_path);
		let mut cmd = std::process::Command::new(exec_path);
		self.configure_env(&mut cmd);
		cmd.args(args);

		for e in &env {
			cmd.env(e.0, e.1);
		}

		match cmd.spawn() {
			Ok(child) => {
				self.processes.push(Process { child });
			}
			Err(e) => {
				anyhow::bail!(
					"Failed to launch process with path \"{}\": {}. Make sure your exec path exists.",
					exec_path,
					e
				);
			}
		}

		Ok(())
	}

	pub fn tick_render(&mut self, renderer: &mut GlesRenderer, time_ms: u64) -> anyhow::Result<()> {
		let size = Size::from((self.wm.disp_width as i32, self.wm.disp_height as i32));
		let damage: Rectangle<i32, smithay::utils::Physical> =
			Rectangle::from_loc_and_size((0, 0), size);

		let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = self
			.state
			.xdg_shell
			.toplevel_surfaces()
			.iter()
			.flat_map(|toplevel_surf| {
				let win = self.wm.get_window(toplevel_surf);

				render_elements_from_surface_tree(
					renderer,
					toplevel_surf.wl_surface(),
					(win.pos_x, win.pos_y),
					1.0,
					1.0,
					Kind::Unspecified,
				)
			})
			.collect();

		let mut frame = renderer.render(size, Transform::Normal)?;
		frame.clear(Color32F::new(0.0, 0.0, 0.0, 0.5), &[damage])?;

		draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;

		let _sync_point = frame.finish()?;

		for surface in self.state.xdg_shell.toplevel_surfaces() {
			send_frames_surface_tree(surface.wl_surface(), time_ms as u32);
		}

		Ok(())
	}

	fn accept_connections(&mut self) -> anyhow::Result<()> {
		if let Some(stream) = self.listener.accept()? {
			log::debug!("Stream accepted: {:?}", stream);

			let client = self
				.display
				.handle()
				.insert_client(stream, Arc::new(comp::ClientState::default()))
				.unwrap();
			self.clients.push(client);
		}

		Ok(())
	}

	pub fn tick_wayland(&mut self) -> anyhow::Result<()> {
		if let Err(e) = self.accept_connections() {
			log::error!("accept_connections failed: {}", e);
		}

		self.display.dispatch_clients(&mut self.state)?;
		self.display.flush_clients()?;

		Ok(())
	}

	fn get_mouse_index_number(index: wayvr::MouseIndex) -> u32 {
		match index {
			wayvr::MouseIndex::Left => 0x110,   /* BTN_LEFT */
			wayvr::MouseIndex::Center => 0x112, /* BTN_MIDDLE */
			wayvr::MouseIndex::Right => 0x111,  /* BTN_RIGHT */
		}
	}

	fn get_hovered_window(&mut self, cursor_x: u32, cursor_y: u32) -> Option<&window::Window> {
		for cell in self.wm.windows.vec.iter().flatten() {
			let window = &cell.obj;
			if (cursor_x as i32) >= window.pos_x
				&& (cursor_x as i32) < window.pos_x + window.size_x as i32
				&& (cursor_y as i32) >= window.pos_y
				&& (cursor_y as i32) < window.pos_y + window.size_y as i32
			{
				return Some(window);
			}
		}
		None
	}

	pub fn send_mouse_move(&mut self, x: u32, y: u32) {
		if let Some(window) = self.get_hovered_window(x, y) {
			let surf = window.toplevel.wl_surface().clone();
			let point = Point::<f64, Logical>::from((
				(x as i32 - window.pos_x) as f64,
				(y as i32 - window.pos_y) as f64,
			));

			self.seat_pointer.motion(
				&mut self.state,
				Some((surf, Point::from((0.0, 0.0)))),
				&input::pointer::MotionEvent {
					serial: self.serial_counter.next_serial(),
					time: 0,
					location: point,
				},
			);

			self.seat_pointer.frame(&mut self.state);
		}
	}

	pub fn send_mouse_down(&mut self, index: wayvr::MouseIndex) {
		// Change keyboard focus to pressed window
		let loc = self.seat_pointer.current_location();

		if let Some(window) = self.get_hovered_window(loc.x.max(0.0) as u32, loc.y.max(0.0) as u32) {
			let surf = window.toplevel.wl_surface().clone();

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
			&input::pointer::ButtonEvent {
				button: Self::get_mouse_index_number(index),
				serial: self.serial_counter.next_serial(),
				time: 0,
				state: smithay::backend::input::ButtonState::Pressed,
			},
		);

		self.seat_pointer.frame(&mut self.state);
	}

	pub fn send_mouse_up(&mut self, index: wayvr::MouseIndex) {
		self.seat_pointer.button(
			&mut self.state,
			&input::pointer::ButtonEvent {
				button: Self::get_mouse_index_number(index),
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
			input::pointer::AxisFrame {
				source: None,
				relative_direction: (
					smithay::backend::input::AxisRelativeDirection::Identical,
					smithay::backend::input::AxisRelativeDirection::Identical,
				),
				time: 0,
				axis: (0.0, -delta as f64),
				v120: Some((0, (delta * -120.0) as i32)),
				stop: (false, false),
			},
		);
		self.seat_pointer.frame(&mut self.state);
	}
}

const STARTING_WAYLAND_ADDR_IDX: u32 = 20;

fn create_wayland_listener() -> anyhow::Result<(WaylandEnv, wayland_server::ListeningSocket)> {
	let mut env = WaylandEnv {
		display_num: STARTING_WAYLAND_ADDR_IDX,
	};

	let listener = loop {
		let display_str = env.display_num_string();
		log::debug!("Trying to open socket \"{}\"", display_str);
		match wayland_server::ListeningSocket::bind(display_str.as_str()) {
			Ok(listener) => {
				log::debug!("Listening to {}", display_str);
				break listener;
			}
			Err(e) => {
				log::debug!(
					"Failed to open socket \"{}\" (reason: {}), trying next...",
					display_str,
					e
				);

				env.display_num += 1;
				if env.display_num > STARTING_WAYLAND_ADDR_IDX + 20 {
					// Highly unlikely for the user to have 20 Wayland displays enabled at once. Return error instead.
					anyhow::bail!("Failed to create wayland-server socket")
				}
			}
		}
	};

	Ok((env, listener))
}
