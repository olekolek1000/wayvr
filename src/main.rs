mod comp;
mod egl_data;
mod smithay_wrapper;
mod time;

fn run() -> Result<(), Box<dyn std::error::Error>> {
	log::info!("Offscreen GL backend initialized");

	let display_addr = "wayland-42";

	comp::run(display_addr)?;
	Ok(())
}

fn main() {
	std::env::set_var("RUST_LOG", "trace");
	pretty_env_logger::init();

	if let Err(e) = run() {
		log::error!("Unhandled error: {}", e);
	}
}
