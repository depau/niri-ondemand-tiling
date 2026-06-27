# niri-ondemand-tiling

On-demand tiling for [niri](https://github.com/YaLTeR/niri). Resizes all columns
on a workspace to fit the screen in one shot, then gets out of the way.

## Usage

```
niri-ondemand-tiling tile [-m equal|proportional] [-w WORKSPACE]
```

`tile` resizes all tiling columns on the focused workspace (or a specified one)
to fill the screen, then snaps the viewport.

### Modes (`-m` / `--mode`)

- **`proportional`** *(default)* — columns keep their relative sizes, scaled to
  fit the screen
- **`equal`** — all columns are resized to equal width (`screen / N`)

### Usage

```bash
# Proportional fit on the focused workspace (default)
niri-ondemand-tiling tile

# Equal-width fit
niri-ondemand-tiling tile -m equal

# Target workspace by index or name
niri-ondemand-tiling tile -w 3
niri-ondemand-tiling tile -w my-workspace

# Target workspace by ID, useful in scripts, i.e.:
niri-ondemand-tiling tile \
  -w "#$(niri msg --json workspaces | jq '[.[] | select(.output == "HDMI-A-1")][0].id')"
```

Bind it to a key in your niri config:

```kdl
binds {
    Mod+T { spawn "niri-ondemand-tiling" "tile"; }
}
```

## Building

Requires Rust and a running niri instance (the `niri-ipc` crate is taken from a
local niri checkout).

```bash
cargo build --release
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
