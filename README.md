# Fjord

A Jellyfin media frontend built in Rust with [Slint](https://slint.dev/) and libmpv. Designed for HTPC use — keyboard navigable, fast to start, and smooth on NVIDIA legacy hardware.

## Why

Every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit to drive mpv. That integration never calls `mpv_render_context_report_swap()`, so mpv gets no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland.

Fjord uses the mpv render API directly: mpv renders into an OpenGL FBO that Slint composites, with `report_swap()` called after every frame. No Flutter layer, no texture roundtrip.

## Features

**Browsing**
- Home screen: Continue Watching, Next Up, Recently Added Shows, Recently Added Movies
- Movies library: Continue Watching, Recently Added, Not Watched (random, refreshes every 10 min)
- TV library: Continue Watching, Next Up, Recently Added Shows, Not Watched Shows (random, refreshes every 10 min)
- Episode cards in dashboard rows show the series poster, not the episode thumbnail
- TV series drill-down: season tabs → episode list with per-episode watched state
- Item detail page: overview, cast, runtime, rating, backdrop
- Flat search/browse list across the full library
- Watched markers on every card: ✓ badge for fully watched, progress bar for in-progress
- Warm start: window opens with content on the first frame from disk cache

**Playback**
- Fullscreen player with seek bar, elapsed/total time, and controls overlay
- Video-behind-UI mode and mini sidebar card for background playback
- Resume from saved Jellyfin position
- Episode auto-advance: 5-second countdown banner after each episode (cancellable)
- Intro skip prompt: integrates with the [Intro Skipper](https://github.com/intro-skipper/intro-skipper) plugin — "Skip Intro →" button appears at the right moment and seeks past the intro
- Subtitle, audio, and video track selection mid-playback
- Hardware decode with automatic format selection (NVIDIA NVDEC, AMD Vulkan, VA-API)
- Playback stats overlay: codec, resolution, pixel format, HDR info, audio passthrough, A/V sync

**General**
- Full keyboard navigation — usable entirely without a mouse
- Dynamic card sizing: card dimensions scale with window width

## Requirements

- [Jellyfin](https://jellyfin.org/) server
- `mpv` / `libmpv`
- Wayland compositor (X11 may work but is untested)
- Optional: [Intro Skipper plugin](https://github.com/intro-skipper/intro-skipper) on your Jellyfin server for intro detection

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

### Navigation

| Key | Action |
|---|---|
| `↑` `↓` | Navigate sidebar / card rows |
| `←` `→` | Navigate cards (hold to scroll, tap at edge to exit to sidebar) |
| `Enter` | Play / open |
| `Backspace` | Back |
| `1` `2` `3` | Home / Movies / TV |
| `S` | Settings |
| `B` | Browse / search list |
| `F` `F11` | Toggle fullscreen |
| `Q` | Quit |

### Player

| Key | Action |
|---|---|
| `Space` / `K` | Pause / resume |
| `←` `→` | Seek ±10 s |
| `Shift+←` `Shift+→` | Seek ±30 s |
| `↑` `↓` | Volume ±5 |
| `0`–`9` | Jump to 0%–90% |
| `M` | Mute |
| `S` | Subtitle track panel |
| `A` | Audio track panel |
| `V` | Video track panel |
| `I` | Stats overlay |
| `R` | Resume background player to fullscreen |

## Project structure

```
crates/
  fjord-api/     Jellyfin REST API client (no UI, no mpv)
  fjord-player/  libmpv wrapper + render context (no UI, no HTTP)
  fjord-app/     Slint UI + main binary
```

## License

GPL v3 — see [LICENSE](LICENSE)
