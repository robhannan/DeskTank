# DeskTank

A lightweight virtual aquarium for your desktop, written in Rust.

DeskTank renders a calm, living fish tank that can run as any combination of:

- **Wallpaper** — drawn behind your desktop icons
- **Screensaver** — takes over when the machine is idle
- **Lock screen** — shown while the session is locked (where the OS allows it)
- **Overlay** — a transparent, click-through layer floating above open windows

## Goals

- **Minimal resource use** — low idle CPU/GPU, small memory footprint, frame rate that throttles when hidden or on battery.
- **Rust first** — idiomatic, safe Rust with platform-specific code isolated behind small, well-defined boundaries.
- **Cross-platform** — macOS, Windows and Linux.

**[Try the sand tank in your browser](https://robhannan.github.io/DeskTank/)**: the simulation and GPU renderer compiled to WebAssembly (WebGPU, or WebGL2 as the fallback). Pour sand, gravel, rock, bubbles and food into the water, glue rocks in place and place bubblers, with the mouse or touch (drag pours, two-finger drag scoops, long-press inspects). Add creatures from the palette. To make fish flee the pointer, enable **Fish avoid the pointer** in web Settings or press **A** in `desktank window`. Cursor avoidance starts off.

## Status

Early days: the falling-sand simulation core works (see the browser demo above) and renders in a window. Working commands:

```sh
cargo run --release -p desktank -- window   # the tank in a window: click a group tab in the palette (top left; G / Shift+G or scrolling over it also switch group), then an item (or keys 1–9 and 0 within the group) for a material, device or tool, left drag uses it, right drag scoops, scroll sets the brush size, F tints the nutrient, CO₂ and light fields, W changes the water, Q shows the water quality, P picks the plant species the plants slot pours, O cycles how it starts (seeds, cuttings, fragments or runners, as the species allows); --time HH:MM lights it for a time of day. The tank is saved (to `~/Library/Application Support/DeskTank/tanks`, `%APPDATA%\DeskTank\tanks` or `~/.local/share/desktank/tanks`; `DESKTANK_DATA_DIR` overrides) every 5 minutes, when hidden and on close, and reopened next time; --fresh starts a new tank, --no-save neither loads nor saves
cargo run --release -p desktank -- bench    # 60 s scripted pour + settle, prints CPU, tick, GPU and memory numbers
cargo run --release -p desktank -- capture --script crates/desktank/tests/scenes/starter.toml --out shots   # headless PNG capture
```

The other commands are still stubs that print "not yet implemented".

Capture scripts and the render tests are described in [docs/capture.md](docs/capture.md).

## Building

Install [rustup](https://rustup.rs); the pinned toolchain in `rust-toolchain.toml` is fetched automatically. On Linux, install the system packages the windowing, tray and render crates need (CI installs them already):

```sh
sudo apt-get install libxkbcommon-dev libwayland-dev libdbus-1-dev mesa-vulkan-drivers
```

Then:

```sh
cargo build --workspace            # debug build of every crate
cargo run -p desktank -- --help    # list the commands
cargo build --release --workspace  # small, stripped release binaries
cargo test --workspace
```

Before a PR, also run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo deny check` (`cargo install --locked cargo-deny`).

## Web

The [web version](https://robhannan.github.io/DeskTank/) (SPEC §3.5, D15) is `crates/desktank-web`, a wasm-bindgen host that drives `desktank-render`'s `Tank` through wgpu (WebGPU first, WebGL2 as the fallback; `?backend=webgpu|webgl2` forces one), plus the static page in `web/`. To build and preview it locally you need [Node](https://nodejs.org) 22+, [pnpm](https://pnpm.io) and the wasm-bindgen CLI at the version in `Cargo.lock`; [`wasm-opt`](https://github.com/WebAssembly/binaryen) is used if it is on the `PATH`:

```sh
cargo install --locked wasm-bindgen-cli --version 0.2.130
pnpm install
pnpm build:web           # builds target/site/ and reports the gzip transfer size (budget 3 MB)
pnpm serve:web           # then open http://127.0.0.1:8000
pnpm exec playwright install chromium
pnpm test:web            # Playwright: smoke test, context-loss recovery, render match with the native reference, tools with mouse and touch, settings
pnpm bench:web chrome 60 # p50/p95 loop and drawn-frame intervals over 60 s of pouring at DPR 2 (also chromium, firefox, webkit)
```

Pushes to `main` deploy it to GitHub Pages (`.github/workflows/pages.yml`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
