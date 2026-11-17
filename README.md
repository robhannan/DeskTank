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

**[Try the sand tank in your browser](https://robhannan.github.io/DeskTank/)**: the simulation core compiled to WebAssembly. Pour sand, gravel, rock, bubbles and food into the water.

## Status

Early days: the falling-sand simulation core works (see the browser demo above) and renders in a window. Working commands:

```sh
cargo run --release -p desktank -- window   # the tank in a window; left drag pours (keys 1–5 pick the material), right drag erases
cargo run --release -p desktank -- bench    # 60 s scripted pour + settle, prints CPU, tick, GPU and memory numbers
```

The other commands are still stubs that print "not yet implemented".

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

## Web demo

The [browser demo](https://robhannan.github.io/DeskTank/) is `crates/desktank-web` (a wasm-bindgen wrapper around `desktank-sim`) plus the static page in `web/`. It currently runs the sim core with a canvas-2D renderer; the full web version (SPEC §3.5, D15) replaces it from WB1. To build and preview it locally you need [Node](https://nodejs.org) 22+, [pnpm](https://pnpm.io) and the wasm-bindgen CLI at the version in `Cargo.lock`; [`wasm-opt`](https://github.com/WebAssembly/binaryen) is used if it is on the `PATH`:

```sh
cargo install --locked wasm-bindgen-cli --version 0.2.130
pnpm install
pnpm build:web                              # builds target/site/
python3 -m http.server -d target/site 8000  # then open http://localhost:8000
```

Pushes to `main` deploy it to GitHub Pages (`.github/workflows/pages.yml`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
