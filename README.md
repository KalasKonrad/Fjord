# Fjord

A Jellyfin media frontend built in Rust with [Slint](https://slint.dev/) and libmpv. Designed for HTPC use — keyboard navigable, fast to start, and smooth on NVIDIA legacy hardware.

## Why

Every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit to drive mpv. That integration never calls `mpv_render_context_report_swap()`, so mpv gets no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland.

Fjord uses the mpv render API directly: mpv renders into an OpenGL FBO that Slint composites, with `report_swap()` called after every frame. No Flutter layer, no texture roundtrip.

## Features

- Home screen with Continue Watching, Next Up, and Recently Added rows
- Movies and TV library with search
- Keyboard navigation throughout — designed to be used with a remote or keyboard without a mouse
- Fullscreen player with controls and stats overlay
- Video-behind-UI mode for background playback
- Hardware decode with automatic format selection (NVIDIA NVDEC, AMD Vulkan, VA-API)
- Playback stats overlay: codec, resolution, pixel format, HDR info, audio passthrough status, A/V sync
- Warm start: window opens with content visible on the first frame from disk cache

## Requirements

- [Jellyfin](https://jellyfin.org/) server
- `mpv` / `libmpv`
- Wayland compositor (X11 may work but is untested)

## Build

```bash
# Dependencies (Arch Linux)
sudo pacman -S mpv fontconfig freetype2 libxkbcommon rust

cargo build --release
```

The binary is `target/release/fjord-app`. On Arch Linux the included PKGBUILD installs it to `/usr/bin/fjord`.

```bash
# Install via PKGBUILD (builds natively from source)
makepkg -si
```

## NVIDIA legacy hardware

NVIDIA legacy drivers on Wayland/EGL silently ignore `GL_UNPACK_ROW_LENGTH`. NVDEC outputs frames with a 256-byte-aligned stride (e.g. 2048 bytes for a 1920-pixel-wide video), and without the stride hint the GL upload corrupts every row.

**Fix:** In Settings → Video, set **Video filter** to `auto`. Fjord will detect the active decoder and bit depth after playback starts and apply the right tight-packed format filter:

| Decoder | 8-bit | 10-bit |
|---|---|---|
| nvdec-copy | `format=yuv420p` | `format=yuv420p10le` |
| nvdec | `format=nv12` | `format=p010` |

Recommended settings for NVIDIA legacy:
- HW decode: `nvdec-copy`
- Video filter: `auto`
- GPU API: `opengl`
- Enable: Video latency hacks, OpenGL early flush

## Keyboard shortcuts

| Key | Action |
|---|---|
| `↑` `↓` | Navigate sidebar / rows |
| `←` `→` | Navigate cards |
| `Enter` | Play |
| `1` `2` `3` | Home / Movies / TV |
| `S` | Settings |
| `B` | Browse / search |
| `F` `F11` | Toggle fullscreen |
| `Space` | Pause / resume |
| `←` `→` (during playback) | Seek ±30s |
| `I` | Stats overlay |
| `Q` | Quit |

## Project structure

```
crates/
  fjord-api/     Jellyfin REST API client (no UI, no mpv)
  fjord-player/  libmpv wrapper + render context (no UI, no HTTP)
  fjord-app/     Slint UI + main binary
```

## License

GPL v3 — see [LICENSE](LICENSE)
