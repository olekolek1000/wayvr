mod comp;
pub mod egl_data;
mod smithay_wrapper;
mod time;
pub mod wayvr;

pub use khronos_egl;

#[cfg(test)]
mod tests {
	use crate::wayvr;

	fn run() -> Result<(), Box<dyn std::error::Error>> {
		let mut wayvr = wayvr::WayVR::new(1280, 720)?;
		wayvr.spawn_process("konsole", Vec::new())?;

		for _ in 0..100 {
			wayvr.tick()?;
			std::thread::sleep(std::time::Duration::from_millis(10))
		}

		Ok(())
	}

	#[test]
	fn test() {
		std::env::set_var("RUST_LOG", "trace");
		pretty_env_logger::init();

		if let Err(e) = run() {
			log::error!("Unhandled error: {}", e);
		}
	}
}