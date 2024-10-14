mod client;
mod comp;
pub mod display;
pub mod egl_data;
mod egl_ex;
mod event_queue;
mod id;
mod smithay_wrapper;
mod time;
pub mod wayvr;
mod window;

pub use khronos_egl;

#[cfg(test)]
mod tests {
	use crate::wayvr;

	fn run() -> Result<(), Box<dyn std::error::Error>> {
		let mut wayvr = wayvr::WayVR::new()?;

		let disp1 = wayvr.create_display(1024, 1024)?;
		let disp2 = wayvr.create_display(1024, 512)?;

		wayvr.spawn_process(disp1, "konsole", &[], &[])?;
		wayvr.spawn_process(disp1, "weston-terminal", &[], &[])?;

		for _ in 0..30 {
			wayvr.tick_events()?;

			wayvr.tick_display(disp1)?;
			wayvr.tick_display(disp2)?;

			wayvr.tick_events()?;

			std::thread::sleep(std::time::Duration::from_millis(50))
		}

		Ok(())
	}

	#[test]
	fn test() -> std::result::Result<(), Box<dyn std::error::Error>> {
		flexi_logger::Logger::try_with_env_or_str("info, wayvr=trace")?.start()?;
		run()
	}
}
