# Archived repo

# WayVR has been integrated directly into [wlx-overlay-s](https://github.com/galister/wlx-overlay-s).

<p align="center">
	<img src="./contrib/logo_traced.svg" height="180"/>
</p>

**WayVR acts as a bridge between Wayland applications and wlx-overlay-s panels, allowing you to display your applications within a VR environment. Internally, WayVR utilizes Smithay to run a Wayland compositor.**

![logo](./contrib/screenshot.webp)
_Chromium browser in WayVR_

# Features

- Display Wayland applications without GPU overhead (zero-copy via dma-buf)
- Mouse input
- Precision scrolling support
- XWayland "support" via `cage`

# Installation

1. Clone [this fork of wlx-xoverlay-s](https://github.com/olekolek1000/wlx-overlay-s) repository and go to `wayvr` branch:

```
git clone --branch wayvr https://github.com/olekolek1000/wlx-overlay-s wlx
cd wlx
```

2. Change your startup application list in `src/backend/common.rs` (search for `WayVRProcess` and modify the code accordingly).

3. Start wlx-overlay-s: `cargo run`

# Roadmap

✅ - Done | 🚧 - WIP | 📌 - Planned

- [✅] Basic Wayland compositor renderer
- [✅] Mouse input support
- [✅] Window focus support
- [🚧] Import mouse behaviour settings from wlx-overlay-s config (click freeze-time)
- [🚧] Basic cursor pointer rendering
- [🚧] Change window geometry
- [📌] Spawn processes via config and customizable ui buttons directly from wlx
- [📌] CPU fallback in case if dma-buf is not available
- [📌] Show/hide support
- [📌] Keyboard input (and keyboard focus control in wlx)
- [📌] Change compositor resolution on the fly
- [👀] Dedicated dashboard?
- [👀] Direct Gamescope support?

# Requirements

- Fork of wlx-overlay-s with WayVR support ([https://github.com/olekolek1000/wlx-overlay-s](https://github.com/olekolek1000/wlx-overlay-s))
- AMD graphics card (NVIDIA is not yet tested; feedback is welcome)
- `cage` is recommended for running XWayland applications

# Supported hardware

### Confirmed working GPUs

- Navi 32 family: AMD Radeon RX 7800 XT **\***
- Navi 23 family: AMD Radeon RX 6600 XT
- Navi 21 family: AMD Radeon Pro W6800, AMD Radeon RX 6800 XT
- Nvidia GTX 16 Series
- _Your GPU here? (Let me know!)_

**\*** - With dmabuf modifier mitigation (probably Mesa bug)

# Supported software

- Basically all Qt applications (they work out of the box)
- Most XWayland applications via `cage`

# Known issues

- Context menus are not functional in most cases yet

- Due to unknown circumstances, dma-buf textures may display various graphical glitches due to invalid dma-buf tiling modifier. Please report your GPU model when filing an issue. Alternatively, you can run wlx-overlay-s with `LIBGL_ALWAYS_SOFTWARE=1` to mitigate that (Smithay compositor will be running in software renderer mode).

- Potential data race in the rendering pipeline - A texture could be displayed during the clear-and-blit process in the compositor, causing minor artifacts (no fence sync support yet).

- Even though some applications support Wayland, some still check for the `DISPLAY` environment variable and an available X11 server (looking at you, Chromium).

- GNOME still insists on rendering client-side decorations in 2024 instead of server-side ones. This results in all GTK applications looking odd due to additional window shadows. [Fix here, "Client-side decorations"](https://wiki.archlinux.org/title/GTK)
