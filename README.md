# vram-map

GPU VRAM visualizer TUI for AMD UMA iGPUs -- shows VRAM, GTT, combined UMA usage, and per-process GPU memory in real time.

## Features

- Live gauges for VRAM, GTT, combined UMA pool, and GPU utilization
- Per-process GPU memory table (VRAM + GTT breakdown per PID)
- Color-coded bars: green under 70%, yellow 70-90%, red above 90%
- Header shows GPU name, driver version, temperature, shader/memory clocks
- Reads directly from sysfs (`/sys/class/drm/`) -- no ROCm dependency
- 500ms auto-refresh
- Phosphor-green on black aesthetic

## Install

```
cargo build --release
cp target/release/vram-map ~/.local/bin/
```

## Usage

```bash
vram-map    # launch the TUI
```

## Keybindings

| Key | Action |
|-----|--------|
| `q` / `Esc` | Quit |

Built with Rust + ratatui.
