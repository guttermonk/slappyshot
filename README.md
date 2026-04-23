# slappyshot

A fast screenshot annotation tool. Pipe in a screenshot, annotate it, copy or save.

Built on [egui](https://github.com/emilk/egui)/[eframe](https://github.com/emilk/egui/tree/master/crates/eframe) — no GTK required.

## Usage

```sh
# From a file
slappyshot --filename screenshot.png

# From stdin
grim - | slappyshot --filename -

# With output file
grim - | slappyshot --filename - --output-filename ~/Pictures/shot-%Y-%m-%d_%H:%M:%S.png
```

## Tools

| Icon | Tool | Shortcut | Notes |
|------|------|----------|-------|
| ↖ | Pointer | `p` | Drag canvas to pan; drag an annotation to move it |
| ✂ | Crop | `x` | |
| ✏ | Brush | `b` | |
| ∕ | Line | `l` | |
| ↗ | Arrow | `a` | |
| □ | Rectangle | `r` | |
| ○ | Ellipse | `e` | |
| T | Text | `t` | |
| # | Numbered Marker | `m` | Press `m` again to reset counter to 1 |
| ≈ | Blur | `w` | |
| H | Highlight | `h` | |
| 🗑 | Delete | `d` | Click an annotation to delete it |

Tool shortcuts are configurable via `[keybinds]` in the config file.

## Keyboard Shortcuts

### Tools
| Shortcut | Action |
|----------|--------|
| `p` | Pointer |
| `x` | Crop |
| `b` | Brush |
| `l` | Line |
| `a` | Arrow |
| `r` | Rectangle |
| `e` | Ellipse |
| `t` | Text |
| `m` | Numbered Marker (press again to reset counter) |
| `w` | Blur |
| `h` | Highlight |
| `d` | Delete |

### Style
| Shortcut | Action |
|----------|--------|
| `1` – `6` | Select palette color 1–6 |
| `7` | Small size |
| `8` | Medium size |
| `9` | Large size |
| `f` | Toggle fill |

### Actions
| Shortcut | Action |
|----------|--------|
| `z` / `Ctrl+Z` | Undo |
| `y` / `Ctrl+Y` | Redo |
| `c` / `Ctrl+C` | Copy to clipboard |
| `s` / `Ctrl+S` | Save to file |
| `Ctrl+T` | Toggle toolbars |
| `Enter` | Configurable (default: copy + exit) |
| `Escape` | Configurable (default: exit) |

### Zoom & Pan
| Shortcut | Action |
|----------|--------|
| `=` / `+` | Zoom in |
| `-` | Zoom out |
| `0` | Reset zoom |
| Scroll wheel | Zoom (centered on cursor) |
| Middle mouse drag | Pan |
| Pointer tool drag | Pan (when not over an annotation) |

### Tool Modifiers

- **Arrow / Line**: Hold `Shift` to snap to 15° increments
- **Rectangle / Ellipse**: Hold `Shift` for a square/circle
- **Highlight**: Hold `Ctrl` to switch between block and freehand mode

## Command Line

```
slappyshot [OPTIONS]

Options:
  -f, --filename <FILE>                   Input image, or '-' for stdin
      --output-filename <FILE>            Output filename template (supports chrono format specifiers)
      --fullscreen                        Start fullscreen
      --copy-command <CMD>                Command to pipe image to on copy (e.g. wl-copy)
      --config <FILE>                     Path to config file
      --initial-tool <TOOL>               Tool to select on startup
      --annotation-size-factor <FACTOR>   Scale annotation sizes up or down
      --early-exit                        Exit after copy/save
      --auto-copy                         Auto-copy to clipboard after each annotation change
  -h, --help                              Print help
  -V, --version                           Print version
```

## Configuration

Config file location: `~/.config/slappyshot/config.toml`

```toml
[general]
# Start fullscreen
fullscreen = false
# Exit after copy/save
early_exit = false
# Auto-copy after every annotation change
auto_copy = false
# Output filename template (chrono format specifiers supported)
output_filename = "/tmp/slappyshot-%Y-%m-%d_%H:%M:%S.png"
# Save to file after copying
save_after_copy = false
# Hide toolbars on startup
default_hide_toolbars = false
# Fill shapes by default
default_fill_shapes = false
# Corner roundness for rectangles (0 = sharp corners)
corner_roundness = 12
# Initial tool on startup
initial_tool = "arrow"
# Command to run on copy (e.g. "wl-copy"). Falls back to native clipboard if unset.
copy_command = "wl-copy"
# Scale factor for annotation sizes
annotation_size_factor = 1.0
# Primary highlighter mode [block, freehand]
primary_highlighter = "block"
# Brush smoothing history size (0 = disabled)
brush_smooth_history_size = 5
# Zoom step factor per scroll tick
zoom_factor = 1.1
# Pan step size in pixels (for keyboard panning)
pan_step_size = 50.0
# Disable window decorations
no_window_decoration = false
# Actions on Enter key [save_to_clipboard, save_to_file, copy_filepath_to_clipboard, exit]
actions_on_enter = ["save_to_clipboard", "exit"]
# Actions on Escape key
actions_on_escape = ["exit"]
# Actions on right click
actions_on_right_click = []

[color_palette]
# Hex colors shown in the toolbar
palette = ["#f0932b", "#eb4d4b", "#6ab04c", "#22a6b3", "#130f40", "#c825b8"]

[keybinds]
pointer   = "p"
crop      = "x"
brush     = "b"
line      = "l"
arrow     = "a"
rectangle = "r"
ellipse   = "e"
text      = "t"
marker    = "m"
blur      = "w"
highlight = "h"
delete    = "d"

[theme]
# Hex colors for UI theming (all optional)
panel_fill   = "#1e1e23"
text_color   = "#ffffff"
button_fill  = "#2a2a2f"
button_active = "#f0932b"
```

## Build from Source

```sh
cargo build --release
# binary at ./target/release/slappyshot
```

### Dependencies

- Rust toolchain
- On Wayland: wayland client libraries
- On X11: x11/xcb libraries

## Example: Sway / Hyprland

```sh
# Sway
grim -g "$(slurp)" -t ppm - | slappyshot --filename - \
  --output-filename ~/Pictures/Screenshots/shot-%Y%m%d-%H%M%S.png \
  --copy-command wl-copy --early-exit

# Hyprland (escape # in shell)
grim -g "$(slurp)" -t ppm - | slappyshot --filename - \
  --copy-command wl-copy --early-exit
```

## License

MPL-2.0
