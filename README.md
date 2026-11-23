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

**[Try the sand tank in your browser](https://robhannan.github.io/DeskTank/)**: the simulation and GPU renderer compiled to WebAssembly (WebGPU, or WebGL2 as the fallback). Pour sand, gravel, rock, bubbles and food into the water, glue rocks in place and place bubblers, with the mouse or touch (drag pours, two-finger drag scoops, long-press inspects). Add creatures from the palette, inspect one to see its card and give it a name (N, or click the name field). To make fish flee the pointer, enable **Fish avoid the pointer** in web Settings or press **A** in `desktank window`. Cursor avoidance starts off. The tank is saved in your browser (when you leave the page, and every 5 minutes while it runs) and reopened next visit; **Export** and **Import** move it to and from the desktop app. After the first online visit, you can reopen the aquarium offline. Use your browser’s install command or **Add to Home Screen** to launch it as an app. Open it in a second tab and that tab shows "open in another tab" with **Take over**.

Plant sway is off by default. Enable **Plant sway** in web Settings or press **S** in `desktank window`. Where battery power is detected, also enable **Allow plant sway on battery** in web Settings or press **Shift+S** in the window. To start the desktop window with both enabled, run `desktank window --sway --sway-on-battery`.

## Status

Early days: the falling-sand simulation core works (see the browser demo above) and renders in a window. Working commands:

```sh
cargo run --release -p desktank -- window   # the tank in a window: click a tab in the palette (top left; G / Shift+G step the tabs), then a sub-tab where it has several (H / Shift+H; scrolling over the palette steps through every sub-tab), then an item (or keys 1–9 and 0 within the sub-tab) for a tool, material, device, plant or creature, left drag uses it, right drag scoops, scroll sets the brush size, F cycles nutrient, CO₂, light and water flow (the tint changes from teal to yellow as water speed increases), W changes the water, Q shows the water quality, X mounts the next device on the other side glass (the see-through ghost shows where it goes; scooping any part of a device removes it whole), each plant species has its own item under Plants and P cycles how the selected one starts (seeds, cuttings, fragments, runners or buds, as the species allows), the Konami code (↑↑↓↓←→←→BA) opens a cheat field in the title bar (type `help`, Enter runs, Esc closes); inspecting a creature shows its card in the title bar and N names it (Enter names, Esc cancels; names are saved); --time HH:MM lights it for a time of day. The tank is saved (to `~/Library/Application Support/DeskTank/tanks`, `%APPDATA%\DeskTank\tanks` or `~/.local/share/desktank/tanks`; `DESKTANK_DATA_DIR` overrides) every 5 minutes, when hidden and on close, and reopened next time; --fresh starts a new tank, --no-save neither loads nor saves; creatures age, grow while fed and die of old age, starvation or foul water (their corpses sink and rot into detritus): --easy-mode lets only old age kill, --creature-cap N sets the population cap (default 60), both saved with the tank; creatures added one after another make pairs, and well-fed adults breed (guppies and cherry shrimp give birth, other fish lay amber eggs that hatch), slower as the tank fills; colour morphs pass on (crossed cherry shrimp revert to brown), and bettas, gouramis and guppies eat fry and shrimp they catch (not in easy mode)
cargo run --release -p desktank -- export tank.dtank   # copy the newest save to a file (opens on the web with Import)
cargo run --release -p desktank -- import tank.dtank   # make a save file (e.g. the web's Export) the tank's newest save; quit the app first
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

The [web version](https://robhannan.github.io/DeskTank/) (SPEC §3.5, D15) is `crates/desktank-web`, a wasm-bindgen host that drives `desktank-render`'s `Tank` through wgpu (WebGPU first, WebGL2 as the fallback; `?backend=webgpu|webgl2` forces one), plus the static page in `web/` (TypeScript, emitted to JS by the build). To build and preview it locally you need [Node](https://nodejs.org) 22.18+ (it runs the TypeScript scripts directly), [pnpm](https://pnpm.io) and the wasm-bindgen CLI at the version in `Cargo.lock`; [`wasm-opt`](https://github.com/WebAssembly/binaryen) is used if it is on the `PATH`:

```sh
cargo install --locked wasm-bindgen-cli --version 0.2.130
pnpm install
pnpm build:web           # builds target/site/ and reports the gzip transfer size (budget 3 MB)
pnpm build:web:webos     # the Chrome 68 variant for LG webOS 5 TVs (SPEC D26) in target/site-webos/, served at /webos/
pnpm typecheck           # strict tsc over the page, scripts and tests (after a build)
pnpm serve:web           # then open http://127.0.0.1:8000
pnpm exec playwright install chromium
pnpm test:web            # Playwright: smoke test, context-loss recovery, render match with the native reference, tools with mouse and touch, settings
pnpm bench:web chrome 60 # p50/p95 loop and drawn-frame intervals over 60 s of pouring at DPR 2 (also chromium, firefox, webkit)
```

`?dpr=<n>` caps the canvas's device pixels per CSS pixel (1 by default on the webOS build) and `?debug=1` lists the startup steps and any error on screen.

Pushes to `main` deploy it to GitHub Pages (`.github/workflows/pages.yml`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
