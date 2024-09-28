use anyhow::anyhow;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;

use smithay::backend::renderer::element::surface::{
	render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::{draw_render_elements, on_commit_buffer_handler};
use smithay::backend::renderer::{Bind, Color32F, Frame, Renderer};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_seat, wl_surface};
use smithay::reexports::wayland_server::{self, ListeningSocket};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
	delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
};

use smithay::utils::{Rectangle, Serial, Size, Transform};
use smithay::wayland::compositor::{
	self, with_surface_tree_downward, SurfaceAttributes, TraversalAction,
};

use smithay::wayland::selection::data_device::{
	ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::xdg::{
	PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::Client;

use crate::time::get_millis;
use crate::{egl_data, smithay_wrapper};

pub struct Application {
	compositor: compositor::CompositorState,
	xdg_shell: XdgShellState,
	seat_state: SeatState<Application>,
	shm: ShmState,
	data_device: DataDeviceState,
}

impl compositor::CompositorHandler for Application {
	fn compositor_state(&mut self) -> &mut compositor::CompositorState {
		&mut self.compositor
	}

	fn client_compositor_state<'a>(
		&self,
		client: &'a Client,
	) -> &'a compositor::CompositorClientState {
		&client.get_data::<ClientState>().unwrap().compositor_state
	}

	fn commit(&mut self, surface: &WlSurface) {
		on_commit_buffer_handler::<Self>(surface);
	}
}

impl SeatHandler for Application {
	type KeyboardFocus = WlSurface;
	type PointerFocus = WlSurface;
	type TouchFocus = WlSurface;

	fn seat_state(&mut self) -> &mut SeatState<Self> {
		&mut self.seat_state
	}

	fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
	fn cursor_image(
		&mut self,
		_seat: &Seat<Self>,
		_image: smithay::input::pointer::CursorImageStatus,
	) {
	}
}

impl BufferHandler for Application {
	fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ClientDndGrabHandler for Application {}

impl ServerDndGrabHandler for Application {
	fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl DataDeviceHandler for Application {
	fn data_device_state(&self) -> &DataDeviceState {
		&self.data_device
	}
}

impl SelectionHandler for Application {
	type SelectionUserData = ();
}

#[derive(Default)]
struct ClientState {
	compositor_state: compositor::CompositorClientState,
}

impl ClientData for ClientState {
	fn initialized(&self, client_id: ClientId) {
		log::debug!("Client ID {:?} connected", client_id);
	}

	fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
		log::debug!(
			"Client ID {:?} disconnected. Reason: {:?}",
			client_id,
			reason
		);
	}
}

impl AsMut<compositor::CompositorState> for Application {
	fn as_mut(&mut self) -> &mut compositor::CompositorState {
		&mut self.compositor
	}
}

impl XdgShellHandler for Application {
	fn xdg_shell_state(&mut self) -> &mut XdgShellState {
		&mut self.xdg_shell
	}

	fn new_toplevel(&mut self, surface: ToplevelSurface) {
		surface.with_pending_state(|state| {
			state.states.set(xdg_toplevel::State::Activated);
		});
		surface.send_configure();
	}

	fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
		// Handle popup creation here
	}

	fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
		// Handle popup grab here
	}

	fn reposition_request(
		&mut self,
		_surface: PopupSurface,
		_positioner: PositionerState,
		_token: u32,
	) {
		// Handle popup reposition here
	}
}

impl ShmHandler for Application {
	fn shm_state(&self) -> &ShmState {
		&self.shm
	}
}

delegate_xdg_shell!(Application);
delegate_compositor!(Application);
delegate_shm!(Application);
delegate_seat!(Application);
delegate_data_device!(Application);

pub fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
	with_surface_tree_downward(
		surface,
		(),
		|_, _, &()| TraversalAction::DoChildren(()),
		|_surf, states, &()| {
			// the surface may not have any user_data if it is a subsurface and has not
			// yet been commited
			for callback in states
				.cached_state
				.get::<SurfaceAttributes>()
				.current()
				.frame_callbacks
				.drain(..)
			{
				callback.done(time);
			}
		},
		|_, _, &()| true,
	);
}

#[allow(unreachable_code)]
pub fn run(display_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
	log::debug!("Initializing Wayland display");
	let mut display: wayland_server::Display<Application> = wayland_server::Display::new()?;
	let dh = display.handle();
	let compositor = compositor::CompositorState::new::<Application>(&dh);
	let xdg_shell = XdgShellState::new::<Application>(&dh);
	let mut seat_state = SeatState::new();
	let shm = ShmState::new::<Application>(&dh, Vec::new());
	let data_device = DataDeviceState::new::<Application>(&dh);
	let _seat = seat_state.new_wl_seat(&dh, "wayvr");

	let mut state = Application {
		compositor,
		xdg_shell,
		seat_state,
		shm,
		data_device,
	};

	log::debug!("Opening socket \"{}\"", display_addr);
	let listener = ListeningSocket::bind(display_addr)?;
	log::debug!("Listening to {}", display_addr);

	let mut clients = Vec::new();

	log::debug!("Spawning process");
	let mut cmd = std::process::Command::new("konsole");
	cmd.env_remove("DISPLAY"); // prevent running x11 apps
	cmd.env("WAYLAND_DISPLAY", display_addr);
	cmd.spawn()?;

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
		.ok_or(anyhow!("Failed to get pixel format"))?;

	let size_w: i32 = 1280;
	let size_h: i32 = 720;

	let surface_data = Rc::new(smithay_wrapper::SurfaceData::new(
		&egl_data, size_w, size_h,
	)?);

	let smithay_surface = Rc::new(smithay_wrapper::create_egl_surface(
		&egl_data,
		&smithay_display,
		pixel_format,
		surface_data.clone(),
	)?);

	gles_renderer.bind(smithay_surface)?;

	let mut ticks = 0;

	loop {
		ticks += 1;
		let size = Size::from((size_w, size_h));
		let damage: Rectangle<i32, smithay::utils::Physical> =
			Rectangle::from_loc_and_size((0, 0), size);

		let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = state
			.xdg_shell
			.toplevel_surfaces()
			.iter()
			.flat_map(|surface| {
				render_elements_from_surface_tree(
					&mut gles_renderer,
					surface.wl_surface(),
					(0, 0),
					1.0,
					1.0,
					Kind::Unspecified,
				)
			})
			.collect();

		let mut frame = gles_renderer.render(size, Transform::Flipped180)?;
		frame.clear(Color32F::new(0.3, 0.3, 0.3, 1.0), &[damage])?;

		draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;

		let _sync_point = frame.finish()?;

		for surface in state.xdg_shell.toplevel_surfaces() {
			send_frames_surface_tree(surface.wl_surface(), (get_millis() - time_start) as u32);
		}

		if let Some(stream) = listener.accept()? {
			log::debug!("Stream accepted: {:?}", stream);

			let client = display
				.handle()
				.insert_client(stream, Arc::new(ClientState::default()))
				.unwrap();
			clients.push(client);
		}

		display.dispatch_clients(&mut state)?;
		display.flush_clients()?;

		// TODO: use epoll fd in the future
		std::thread::sleep(std::time::Duration::from_millis(10));

		smithay_wrapper::debug_save_pixmap(
			&egl_data,
			&surface_data,
			format!("debug/out_{}.png", ticks % 5).as_str(),
		)?;
	}

	Ok(())
}
