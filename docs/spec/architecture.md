# Architecture, Budgets, Testing, Distribution & Risks

## 4. Architecture

### 4.1 Crates

```
crates/
  desktank-sim       # pure simulation: grid, chunks, materials, reactions, fields,
                     # creatures, ecosystem, time, save format. No GPU, no OS.
  desktank-render    # wgpu renderer: grid texture + palette shader, sprite atlas,
                     # lighting; draws a sim snapshot to a surface/target.
  desktank-platform  # per-OS surface hosts (wallpaper/overlay/idle/saver windows),
                     # power, visibility, full-screen, idle and modifier-key signals,
                     # interact input switching.
  desktank           # main binary: CLI, config, tray, IPC, event loop, mode orchestration.
  desktank-saver     # screensaver host binary (.scr on Windows, XScreenSaver hack on X11).
  desktank-saver-mac # macOS .saver bundle (ScreenSaverView subclass).
  desktank-web       # web host (wasm-bindgen): page lifecycle, rAF loop, input, IndexedDB,
                     # in-page menu. Today a canvas-2D demo of the sim (WB0).
  desktank-android   # Android host (cdylib over JNI): surfaces, render thread, lifecycle,
                     # saves for the Kotlin Dream, Activity and live wallpaper (§3.6).
android/             # Gradle project: the Kotlin shim (DreamService, Activity,
                     # WallpaperService, settings) that packages libdesktank_android.so
assets/              # materials/*.toml, reactions.toml, plants/*.toml, species/*.toml, sprites (PNG atlases + source files)
```

Dependency direction: `sim ← render ← platform ← {desktank,
desktank-saver}`, `sim ← render ← desktank-saver-mac`, and
`sim ← render ← desktank-web`, and
`sim ← render ← platform ← desktank-android` (for `SaveStore`; the
platform crate's desktop hosts stay behind `cfg`). The saver binary reuses `platform` for its
windows and input detection.

`desktank-sim` and `desktank-render` must build for `wasm32-unknown-unknown`,
and CI checks this from T2. Platform-specific code never goes into them.

`desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. If
wgpu surface creation from raw handles needs `unsafe`, it moves into
`platform`. All other `unsafe` lives in `platform`,
`desktank-saver-mac` and `desktank-android` (JNI and the NDK), at FFI
boundaries, with `// SAFETY:` comments.

Assets are embedded in the binary (`include_bytes!`). A user override
directory for custom materials and species is a stretch goal.

### 4.2 Runtime model

- **Loop-agnostic core.** `desktank-render` exposes a `Tank` with
  `tick(now) -> FrameRequest` and `draw(surface)`. Whatever loop owns the
  surface calls them. *(T2: `now` is a `Duration` on the host's monotonic
  clock, since `std::time::Instant` panics on wasm32; `FrameRequest` carries
  `redraw` and `wake_at`. The host creates the `wgpu::Instance` and
  `Surface` and wraps it in a `TankSurface`; `Tank::render_to` draws to any
  texture view for offscreen use.)* The hosts are:
  - the winit loop (macOS and Windows windows, plus X11);
  - an SCTK/`calloop` loop (Wayland layer-shell);
  - `legacyScreenSaver`'s `animateOneFrame` (macOS saver);
  - the `.scr` message loop.
- **One long-running app process**, `desktank run`. It is a
  single-instance app: a lock file plus an IPC endpoint. It owns one wgpu
  `Device` shared by every surface, plus the tray icon and config.
  Wallpaper and overlay on the same display share one simulation and
  differ only in how the water layer is drawn.
  - **macOS, Windows and X11**: the main thread runs the winit loop
    (needed for macOS and the tray).
  - **Wayland**: the main thread runs a `calloop` loop with the SCTK
    layer-shell surfaces, and winit is not used.
- **Simulation**: a fixed 20 Hz tick on the loop thread, covering only
  awake chunks. Creatures and the ecosystem are updated every tick, and
  the fields every 4th tick. If profiling shows a heavy pour exceeds the
  tick budget, awake chunks can be updated in parallel in a
  checkerboard pattern on a small rayon pool, but only in that case. The
  RNG streams (§2.1) keep the parallel pass deterministic. One thread is
  the default.
- **Rendering**: each tick, the dirty rectangles of the cell grid are
  copied into an `R8G8` texture (material, variant); from T6b an `RGBA8`
  texture (material, variant, state) plus a per-plant texture (§2.7
  [Plant appearance](#plant-appearance)). A fragment shader
  maps material and variant to a palette colour, then applies lighting:
  quantised pixel-art caustics, day/night, and plant shadow. Creatures
  are instanced sprites from an atlas. Their position is interpolated
  between ticks at screen-pixel precision, and sprites have two texels
  per cell side (D19), drawn at half the cell scale, so motion is smooth
  while the art stays crisp. Sprites are sorted back to front by depth
  and carry it; an `R8` texture beside the cell texture holds each plant
  cell's depth band, and a sprite fragment over a covered texel of a
  nearer plant cell is discarded, so the plant the grid drew stays in
  front (D21). A versioned `R8` texture holds the per-cell algae coat
  (D28), each byte a variety and a density. The grid shader blends it into
  the substrate and hardscape shading in the variety's style. A
  field-resolution `RGBA8` texture holds the four glass planes in the same
  packed form; the shader reads it by texel and blends densities
  bilinearly only between blocks of the same variety. The look of each
  variety (style, two shades and a strength; the styles are film, slime,
  spots and dust) comes from `assets/fields.toml`. The one-pass grid reads
  it as two `vec4`s per variety id in the view uniform and works out the
  slime and spot patterns per texel. The split grid (webOS, DX12) can't
  afford that, so it reads the shades and the same patterns from a baked
  `RGBA8` atlas (`algae.rs`, 482×240, uploaded once) that the one-pass grid
  gets as a 1×1 placeholder (twelve textures). Both patterns repeat every
  240 texels and round to the atlas's bytes, so the two grids agree. The back and sides tint the water behind the
  contents, and the front blends over the submerged scene with at most
  25% opacity (spots and dust are sparse and keep to the same cap in
  proportion). On opaque sRGB targets the grid and sprites apply the same
  pane tint before conversion to linear colour, so they match the separate
  pane draw on encoded targets. Clear panes skip the blend.
  Water is opaque in wallpaper mode and transparent or tinted in overlay
  mode.
- **Frame pacing**: no continuous redraw. The loop sleeps until the next
  frame deadline at the target fps (`ControlFlow::WaitUntil`, or a
  calloop timer), and a frame is only requested when a surface is
  visible. When every surface is hidden, the loop waits on OS events
  only (§6.2).
- **Screensaver hosts** are separate processes started by the OS, each
  with its own wgpu device and its own budget (§6.1). They take tank
  ownership through the handover in §2.6.

### 4.3 Configuration and control

- **Config**: TOML at the platform config dir (`directories`), for
  example `~/Library/Application Support/DeskTank/config.toml`. It is
  hot-reloaded with `notify`. Every field has a default and a missing
  file means defaults. The schema is versioned with a `version` key. It
  covers:
  - modes;
  - cell scale;
  - fps and tick rate;
  - power policy;
  - multi-monitor layout;
  - ecosystem mode;
  - easy mode;
  - chaos materials;
  - catch-up;
  - interact modifier;
  - cursor avoidance;
  - sound;
  - overlay tint;
  - capture exclusion;
  - population cap.

  *(O2: `desktank window` reads `version` and the `[power]`, `[plants]`
  and `[life]` tables from `config.toml` in the per-user config
  directory or `DESKTANK_CONFIG_DIR`. Set `catch_up` in `[life]` to
  enable catch-up on reopen and after power or visibility pauses (§2.6).
  See the [config paths and example](../config.md).)*
- **Tray / menu-bar icon** offers:
  - toggling each mode;
  - Interact;
  - Feed;
  - Water change;
  - Pause;
  - Open config;
  - Start at login;
  - Quit.

  On macOS and Windows it uses `tray-icon`. On Linux it uses `ksni`
  (StatusNotifierItem over D-Bus) to avoid needing a GTK loop (🔍 confirm
  before O2). There is no native settings window in v1 (§11); the web's in-page settings panel is covered by D15 (§3.5).
- **CLI** (`clap`):
  - `desktank run [--wallpaper] [--overlay] [--idle]`;
  - `desktank window` (a normal resizable window, for development and
    for users who want a desk-toy window);
  - `desktank interact on|off|toggle`, `desktank feed [FOOD]`,
    `desktank pause` and `desktank status` (sent over IPC to the running app);
  - `desktank capture --seed N --time HH:MM --size WxH --frames K --script FILE --out DIR`
    (headless offscreen renders; scripted pours make the output
    reproducible);
  - `desktank bench`;
  - `desktank import FILE` and `desktank export FILE` (move a save to
    and from the web version or another machine; WB3);
  - `desktank saver install|uninstall`.
- **Single instance and IPC.**
  - An advisory lock (`File::try_lock`) in the runtime dir is released
    automatically if the process crashes, so a stale lock can't block
    startup.
  - The local socket lives at `$XDG_RUNTIME_DIR/desktank/desktank.sock` on
    Linux and in the app's cache dir on macOS, both mode `0600`. On Windows it
    is a named pipe with a DACL for the current user only.
  - The protocol is line-based. Commands are `interact`, `feed`,
    `pause`, `saver-handover` and `status`. Arguments are validated
    against the material table and config, unknown input is rejected,
    and each line is capped at 1 KB.
  - *(O2: `desktank window` hosts IPC, including `desktank status`, until
    `desktank run` is implemented. The runtime directory is
    `$XDG_RUNTIME_DIR/desktank` on Linux (with an XDG cache fallback),
    `~/Library/Caches/DeskTank` on macOS and `%LOCALAPPDATA%\DeskTank`
    on Windows. `DESKTANK_RUNTIME_DIR` overrides it. On Unix the runtime
    directory is private (mode `0700`). Windows uses a per-user local
    pipe name with the default DACL pending a
    current-user-only DACL. The blocking Windows pipe API has no read
    timeout, so an idle connection can stall IPC on Windows.)*
- **Start at login**: `SMAppService` on macOS, the `HKCU\…\Run` key on
  Windows, and an XDG autostart `.desktop` file on Linux.
- **Logging**: `tracing`, with `tracing-appender` writing a rotating file
  in the platform cache dir. The default level is `info`, overridable
  with `DESKTANK_LOG`. Nothing leaves the machine.

## 5. Tech stack

Versions current at the time of writing; Dependabot keeps them fresh.

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust 2024 edition, toolchain pinned in `rust-toolchain.toml` | MSRV = pinned stable; `Cargo.lock` committed |
| GPU | `wgpu` 30 | Metal, DX12 and Vulkan natively; WebGPU with a WebGL2 fallback on the web; offscreen rendering for capture and tests |
| Web | `wasm-bindgen`, `web-sys`, `js-sys` | wasm-bindgen CLI pinned to the crate version; built by `scripts/build-web.ts` |
| Android | `jni`, `ndk` (`NativeWindow`), `cargo-ndk`; Gradle with the Android Gradle Plugin and Kotlin | Kotlin only for the platform classes Rust can't provide (D23); versions pinned in the Gradle version catalogue |
| Windowing | `winit` 0.30 (stable) | 0.31 is in beta, so Dependabot ignores winit minor and major updates (0.x minors are breaking) until we choose to upgrade |
| Wayland | `smithay-client-toolkit` 0.21 + `calloop` | Layer-shell and idle-notify |
| macOS FFI | `objc2` 0.6 + `objc2-app-kit` / `objc2-screen-saver` | |
| Windows FFI | `windows` 0.62 | |
| Tray | `tray-icon` 0.26 (macOS/Windows), `ksni` (Linux) | |
| Math / RNG | `glam` 0.34, `rand_pcg` (seeded) | |
| Images | `png` (decoding the sprite atlas) | Atlases are built from source art; placeholder art first (D11) |
| Save format | `serde` + `postcard` inside a versioned envelope (§2.6), compressed with `lz4_flex` (pure Rust); `fd-lock` for ownership | Migrations per version |
| Time | `jiff` | Local time of day and time zones for day/night |
| Parallelism | `rayon` (optional, behind a feature) | Only if T1 profiling needs it |
| Benchmarks | `criterion` (dev-dependency) | |
| IPC | `interprocess` (local sockets) | |
| CLI / config | `clap` 4, `serde` + `toml` 1, `directories` 6, `notify` | |
| Errors / logs | `thiserror` 2 (libraries), `anyhow` (binaries only), `tracing` + `tracing-appender` | |
| Supply chain | `cargo deny` (licences, advisories, duplicates) | |
| Release | `cargo-dist` for archives, MSI and shell installers; zx scripts (TypeScript, pnpm) build `.app`, `.saver` and `.dmg` | |

## 6. Resource budget and power policy

### 6.1 Budgets

Reference machine: M1 MacBook Air, one 2560×1440 display at the default
4× cell scale (640×360 cells), with about 30 creatures. Windows and
Linux numbers are recorded and tracked, but not gated in v1.

| Metric | Main app (1 display) | Per extra display | Each saver host process |
|---|---|---|---|
| CPU while visible, settled tank (no pouring) | ≤ 3% of one core, aiming for ≤ 1.5% | +1.5% | ≤ 3%, +1.5% per extra display |
| CPU while interacting (pouring) | ≤ 15% of one core | — | n/a |
| Simulation tick time: settled / heavy pour | ≤ 0.5 ms / ≤ 5 ms | — | same |
| CPU while hidden or paused | ≤ 0.1% | — | n/a (exits) |
| Wake-ups while hidden | ≤ 1/s (OS events only, plus polls ≤ every 10 s) | — | n/a |
| GPU frame time at 1440p | ≤ 2 ms | +2 ms | ≤ 2 ms per display |
| Resident memory (raw RSS, as `ps` reports it, including shared GPU-driver pages; D16) | ≤ 100 MB, aiming for ≤ 60 MB | +10 MB | ≤ 100 MB, with growth ≤ 20 MB over 8 h |
| Release binary size (stripped, assets embedded) | ≤ 20 MB | — | ≤ 20 MB |
| Web transfer size (wasm + JS + assets, gzip, as reported by `scripts/build-web.ts`) | ≤ 3 MB | — | — |
| Web on the reference phone (🔍 to be chosen in WB2: a 2021-era mid-range iPhone or Android) | p95 frame interval ≤ 50 ms, no thermal throttling within 10 min | — | — |
| Android TV reference device (Chromecast with Google TV 4K: 2 GB RAM, Mali-G31, 32-bit userspace, Android 14; chosen in A0) and Android reference phone | Screensaver: p95 frame interval ≤ 40 ms at the 30 fps cap, no thermal throttling within 1 h, memory (PSS from `dumpsys meminfo`) ≤ 120 MB (A0's empty Dream on the reference device: 39–48 MB on Vulkan, 55 MB with GLES forced), start to first frame ≤ 1 s (A0: 208–257 ms cold from `surfaceCreated`, not counting process and Dream start); APK ≤ 25 MB per ABI | — | — |
| Cold start to first frame (including loading the save) | ≤ 500 ms | — | ≤ 500 ms |
| Energy Impact (Activity Monitor) | "Low" while visible | — | "Low" |

`desktank bench` (from T2) prints, for a scripted scenario:
- CPU per frame;
- simulation tick time;
- the number of awake chunks;
- GPU frame time;
- RSS.

Any PR that touches the simulation, render loop, pacing or assets
includes before-and-after numbers.

### 6.2 Power policy and event sources

Defaults, all configurable:

- 30 fps on AC power and 15 fps on battery. The simulation tick stays
  1/20 s of world time, so the power state never changes behaviour. Rendering pauses in Low Power Mode or battery saver, and below
  20% battery.
- Rendering and simulation pause when the surface is occluded or
  minimised, the display is asleep, or the session is locked. World time
  stops too (§2.6). The exception is a saver host running over the lock
  (L1).
- Wallpaper and overlay pause while a full-screen app is in front.

| Signal | macOS | Windows | Linux |
|---|---|---|---|
| Occluded / hidden | `NSWindow` occlusion-state notification | `WM_SIZE` minimise; DWM cloaked; per-frame present status | Wayland frame callbacks stop; X11 `VisibilityNotify` |
| AC / battery / level | `IOPSNotificationCreateRunLoopSource` | `RegisterPowerSettingNotification` (`GUID_ACDC_POWER_SOURCE`, battery %) | UPower over D-Bus |
| Low power / saver | `NSProcessInfoPowerStateDidChange` | `GUID_ENERGY_SAVER_STATUS` (build 26100+), `GUID_POWER_SAVING_STATUS` on older builds | UPower / power-profiles-daemon |
| Display sleep / lock | `NSWorkspace` screens sleep/wake; `com.apple.screenIsLocked` | `WTSRegisterSessionNotification`; display-state power setting | logind `Lock`/`Unlock`, `PrepareForSleep` |
| Full-screen app in front | Active-Space change + `CGWindowListCopyWindowInfo` bounds check (no window titles, so no Screen Recording permission) | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` → `SHQueryUserNotificationState` | X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_STATE_FULLSCREEN`; Wayland: `wlr-foreign-toplevel-management` where available, otherwise not detected |
| Interact modifier | `CGEventSourceFlagsState` polled at 10 Hz while visible | `GetAsyncKeyState` polled at 10 Hz while visible | X11 `XQueryKeymap` polled at 10 Hz while visible; Wayland: none (IPC/tray) |

Everything is event-driven except the idle polls (at most every 10 s)
and the modifier poll (10 Hz, only while a tank is visible and interact
by modifier is enabled).

On Android the platform reports these as lifecycle callbacks instead
(§3.6): a hidden surface is `onDreamingStopped`, `onPause` or a wallpaper
`onVisibilityChanged(false)`; power comes from `BatteryManager` and
`PowerManager` broadcasts; display sleep and lock end the Dream or pause
the app; there is no full-screen check or interact modifier.

*O3 (`desktank_platform`, `desktank::window`):* `PowerPolicy::evaluate`
turns `PowerSignals` plus surface visibility into `Run { max_fps }` (30
on AC, 15 on battery) or `Pause(PauseReason)` (`Hidden`,
`DisplayAsleep`, `SessionLocked` unless `allow_over_lock` for a saver
host, `LowPowerMode`, or `LowBattery` below 20%). On macOS,
`PowerMonitor` monitors `IOPSNotificationCreateRunLoopSource`,
`NSProcessInfoPowerStateDidChangeNotification`, `NSWorkspace`
screen sleep/wake notifications, and `NSDistributedNotificationCenter`
`com.apple.screenIsLocked`/`Unlocked` with zero polling timers, waking
the event loop via `EventLoopProxy`; Windows and Linux keep default `Ac`
stubs until O1-Win and O1-Linux.

## 7. Quality, testing and CI

- **Lints**: workspace `clippy::pedantic`, `unsafe_code = "deny"` except
  in FFI crates, `missing_docs` on public library APIs, and
  `rustfmt.toml`.
- **Simulation tests**:
  - determinism (same seed and input script give the same state hash);
  - mass conservation for non-reacting materials;
  - settled scenes put every chunk to sleep;
  - powder angle of repose;
  - bubbles reach the surface;
  - each reaction rule fires;
  - plants grow when fed and wilt when not;
  - the life cycle (hatch, grow, breed, die for each cause, corpse
    turns to detritus);
  - easy mode prevents non-age deaths;
  - save and load round-trips, plus a test that loads saves from earlier
    schema versions;
  - catch-up stays within bounds.
- **Web smoke test**: from WB1 on, a Playwright test in CI loads the
  built site in headless Chromium twice, once with `?backend=webgl2` and
  once with the default backend. Each run pours, checks that cells
  changed, and fails on console errors, so both the WebGL2 path and the
  fallback are exercised.
- **Render tests**: offscreen captures compared with reference images.
  They pass when ≤ 0.5% of pixels differ by more than CIEDE2000 ΔE 2. They run in
  Linux CI on Mesa lavapipe (software Vulkan), so CI needs no GPU.
  Reference images are updated deliberately with a `--bless` flag.
- **Benchmarks**: `criterion` benches for the simulation tick (settled,
  heavy pour, 60 creatures). CI records the numbers; it doesn't gate on
  them.
- **Android** (from A0): CI builds the `cdylib` for each ABI and the
  APK on Linux; from A2 it starts the Dream on an Android TV emulator,
  takes a `screencap` frame and checks the tank is drawn, failing on a
  Rust panic or a `FATAL` line in logcat.
- **Platform smoke tests** on each OS runner: start each available mode
  for a few seconds where the runner allows, then exit cleanly.
- **CI** runs on macOS, Windows and Ubuntu: fmt, clippy, test, doc and
  `cargo deny`. Dependabot covers cargo and actions.
- **Manual acceptance** for the visual and OS modes uses captures
  attached to PRs on the `pr-assets` branch, per the Developer brief.

## 8. Distribution

- **Open source** (§11): the repository becomes public before the first
  release. The code is MIT OR Apache-2.0. Placeholder art and data assets
  use the same licence. The licence for final art is decided when final
  art is sourced (❓ Q1).
- **GitHub Releases**:
  - **`cargo-dist`** builds the archives, the Windows MSI (which also
    installs the `.scr` per-machine) and the shell and PowerShell
    installers.
  - **zx scripts** (TypeScript) build `DeskTank.app` (universal binary), assemble
    `DeskTank.saver`, and package both in a `.dmg`.
  - Linux ships a tarball now, with AppImage and AUR later.
- **Signing: none for now** (D14).
  - macOS: builds are ad-hoc signed. Gatekeeper blocks first launch, so
    the README documents right-click → Open, or removing quarantine for
    the `.saver`.
  - Windows: SmartScreen shows "unknown publisher".
  - Homebrew (where the cask can strip quarantine) and winget soften
    this.
  - Signing can be added later without changing the pipeline shape.
  - Android (D23): every APK must be signed to install, so releases are
    signed with a self-generated release key held as a CI secret (PR
    builds use a debug key). Sideloading needs "install unknown apps"
    for the browser or file manager; the README covers it, and
    `adb install` for TVs.
- **Android**: a signed APK (per ABI, plus a universal one) on GitHub
  Releases, sideloaded. Google Play (and its TV review) is a later
  decision (❓ Q3).
- **Package managers later**:
  - a Homebrew **cask** in our own tap, maintained separately (cargo-dist
    produces formulae, not casks);
  - winget;
  - Flathub. 🔍 Flatpak sandboxing may block layer-shell and desktop
    windows.
- No auto-updater in v1. The tray may show an "Update available" item
  that checks GitHub Releases, and only if the user opts in.

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Falling-sand cost on an always-on app | Budget blown during activity or by a "busy" tank | Sleeping chunks, a 20/10 Hz tick, the population cap, profiling in T1; parallel chunks only if needed |
| Pixel art supply | No art, or inconsistent art | Placeholder art first (D11), behind a consistent palette and sprite-size spec, so real art can be swapped in later |
| Pet sim feels punishing | Users come back to dead tanks | Easy mode; world time only while visible; catch-up off by default; slow, gentle death curves |
| Modifier polling varies by platform | Chord unavailable | macOS aggregate flags worked without a grant in the [P1 probe](../notes/packaging-and-identity.md); the tray toggle is always available |
| Windows 24H2+ WorkerW changes | Wallpaper broken on new Windows | Test matrix and snapshot fallback; study Lively's behaviour (GPL, so no code copying) |
| macOS `legacyScreenSaver` leaks and regressions | Saver eats RAM | Mitigations, soak test, and built-in idle mode as a fallback |
| GNOME lacks layer-shell | No wallpaper or overlay on GNOME Wayland | Document it; X11 session works; a GNOME Shell extension is a possible later option |
| Transparent overlays on Windows with wgpu | Alpha may not compose | O1-Win checks visible alpha and mouse delivery with the DX12 visual swap chain; `UpdateLayeredWindow` fallback |
| Lock-screen policy or background mode prevents display | L2 snapshot invisible | Use the unpackaged API with Picture selected and report restrictions |
| Unsigned builds (D14) | Gatekeeper and SmartScreen warn or block users | Clear README steps (R2); Homebrew and winget channels; signing can be added later |
| Save corruption or schema drift | A lost pet tank | Versioned format, atomic writes, the last 3 saves kept, fixture tests |
| WebGL2 limits constrain the renderer | Native features blocked, or web broken | Renderer kept within WebGL2 downlevel limits from T2; CI builds render for wasm32; the WB1 smoke test |
| Browser storage eviction | A lost web tank | `navigator.storage.persist()`, export/import, a visible "last saved" indicator |
| Mobile browser performance | Janky or hot phones | Cell scale adapts to screen size; the population cap; profile in WB1 and WB2 |
| Google TV hides third-party screensavers | Users can't choose DeskTank as the screensaver | One-time ADB grant so the app sets itself; launcher overrides; plain Android TV and phones unaffected; checked in A0 |
| Weak TV GPUs and Vulkan drivers | Slow or broken drawing on cheap TV devices | GLES 3.0 fallback within the WebGL2 limits; a low-end reference TV device; the A2 emulator test |
| Burn-in from a static screensaver | Damaged OLED or plasma TVs | A periodic shift of a few pixels (§3.6) |
