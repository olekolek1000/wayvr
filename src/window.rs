use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::gen_id;

pub struct Window {
	pub pos_x: i32,
	pub pos_y: i32,
	pub size_x: u32,
	pub size_y: u32,
	pub toplevel: ToplevelSurface,
}

impl Window {
	pub fn new(toplevel: &ToplevelSurface) -> Self {
		Self {
			pos_x: 0,
			pos_y: 0,
			size_x: 0,
			size_y: 0,
			toplevel: toplevel.clone(),
		}
	}

	pub fn set_pos(&mut self, pos_x: i32, pos_y: i32) {
		self.pos_x = pos_x;
		self.pos_y = pos_y;
	}

	pub fn set_size(&mut self, size_x: u32, size_y: u32) {
		self.toplevel.with_pending_state(|state| {
			//state.bounds = Some((size_x as i32, size_y as i32).into());
			state.size = Some((size_x as i32, size_y as i32).into());
		});
		self.toplevel.send_configure();

		self.size_x = size_x;
		self.size_y = size_y;
	}
}

gen_id!(WindowVec, Window, WindowCell, WindowHandle);

pub struct WindowManager {
	pub disp_width: u32,
	pub disp_height: u32,

	pub windows: WindowVec,
}

impl WindowManager {
	pub fn new(disp_width: u32, disp_height: u32) -> Self {
		Self {
			windows: WindowVec::new(),
			disp_width,
			disp_height,
		}
	}

	fn reposition_windows(&mut self) {
		let window_count = self.windows.vec.iter().flatten().count();

		for (i, cell) in self.windows.vec.iter_mut().flatten().enumerate() {
			let window = &mut cell.obj;

			let d_cur = i as f32 / window_count as f32;
			let d_next = (i + 1) as f32 / window_count as f32;

			let left = (d_cur * self.disp_width as f32) as i32;
			let right = (d_next * self.disp_width as f32) as i32;

			window.set_pos(left, 0);
			window.set_size((right - left) as u32, self.disp_height);
		}
	}

	fn find_window_handle(&self, toplevel: &ToplevelSurface) -> Option<WindowHandle> {
		for (idx, cell) in self.windows.vec.iter().enumerate() {
			if let Some(cell) = cell {
				let window = &cell.obj;
				if window.toplevel == *toplevel {
					return Some(WindowVec::get_handle(cell, idx));
				}
			}
		}
		None
	}

	fn create_window_handle(&mut self, toplevel: &ToplevelSurface) -> WindowHandle {
		self.windows.add(Window::new(toplevel))
	}

	pub fn get_window_handle(&mut self, toplevel: &ToplevelSurface) -> WindowHandle {
		// Check for existing window handle
		if let Some(handle) = self.find_window_handle(toplevel) {
			return handle;
		}

		let handle = self.create_window_handle(toplevel);
		self.reposition_windows();

		handle
	}

	pub fn get_window(&mut self, toplevel: &ToplevelSurface) -> &Window {
		let handle = self.get_window_handle(toplevel);
		self.windows.get(&handle).unwrap() // never fails
	}
}
