# DeskTank — Plan and Spec

Status: **draft v0.1** (2026-11-17). Items marked ❓ are waiting on owner
decisions (see [Open questions](#11-open-questions)); the defaults shown
are what we build if nobody objects. Items marked 🔍 are technical facts
still to verify on real hardware.

## 1. Vision

DeskTank is a calm, living aquarium for the desktop. You leave it running
all day, so it must feel cheap: it should never appear on a "top energy
users" list. It can run as any combination of four **display modes**:

| Mode | What the user sees |
|---|---|
| **Wallpaper** | The tank replaces the desktop background, behind the icons. |
| **Overlay** | Fish swim over open windows on a transparent, click-through layer. |
| **Screensaver** | A full-screen tank when the machine is idle. |
| **Lock screen** | The tank on the lock screen, as far as each OS allows (see §3.4). |

### Principles

1. **Resources are a feature.** We have hard budgets (§6) and measure
   them. When nobody can see the tank, it does no work.
2. **One tank core, thin platform shells.** The simulation and renderer
   know nothing about the OS. Each mode/OS pairing is a small adapter
   that hands the core a surface to draw on.
3. **Procedural first.** Fish, plants and light are generated from
   parameters and shaders, not bitmaps, so the app stays small and sharp
   on any DPI. ❓ Q1
4. **Deterministic.** Given a seed and time step, the simulation is
   reproducible, so it is testable and screenshots are repeatable.
5. **Honest about platform limits.** Where an OS does not allow a mode,
   we document it and offer the closest alternative, rather than
   shipping fragile hacks.

### Non-goals (v1)

Mobile and web builds; 3D tanks; online features, accounts or telemetry;
a plugin system; sound (❓ Q8).

## 2. The tank (v1 content)

### 2.1 Scene layers (back to front)

1. **Water**: vertical gradient with depth fog, tinted by time of day.
2. **Light**: animated caustics and god-rays, all in a single
   full-screen shader pass at a reduced internal resolution.
3. **Back scenery**: rocks, sand and silhouettes of distant plants, plus
   parallax.
4. **Fish**: instanced quads, each fish drawn by an SDF fragment shader
   from its species parameters, with body bend and tail and fin
   animation.
5. **Front scenery**: swaying plants made of chains of segments with a
   simple spring sway.
6. **Particles**: bubbles from a bubbler and drifting specks; food flakes
   when the fish are fed.

In overlay mode only the fish and particles are drawn, with no water
(❓ Q3).

### 2.2 Fish

- **Species are data, not code**: a TOML table per species covering body
  shape (length, height profile, tail and fin type), palette and pattern
  (stripes, spots, gradient), size range, cruise and burst speed,
  preferred depth band, and a schooling weight. v1 ships **5 species**
  covering three behaviours: schooling (tetra-like), solitary cruiser
  (angelfish-like) and bottom-dweller (catfish-like).
- **Behaviour** is steering-based:
  - wander (smoothed noise);
  - boids separation, alignment and cohesion within a species, weighted
    by species;
  - soft tank-bound and obstacle avoidance;
  - depth preference;
  - turning with a natural body flip;
  - occasional bursts.
- **Day/night**: lighting follows local time. At night the fish slow
  down and some rest near the bottom.
- **Interaction** (v1):
  - **Feed** from the tray menu: flakes sink and fish converge on them.
  - In overlay and wallpaper modes, fish shy away from the cursor when it
    moves fast. We only read the cursor position and never capture input.
    ❓ Q8
- **Population**: 8–20 fish per screen by default, configurable. ❓ Q2
  decides whether fish persist, have needs or breed.

### 2.3 Multi-monitor

The default is one independent tank per display, each with its own seed
and the same species mix. A single tank spanning all displays, where fish
swim from one screen to the next, is a stretch goal. ❓ Q9

## 3. Display modes per platform

### 3.1 Summary of feasibility

| | macOS | Windows | Linux (X11) | Linux (Wayland) |
|---|---|---|---|---|
| Wallpaper | ✅ desktop-level window | ✅ WorkerW child window 🔍 | ✅ `_NET_WM_WINDOW_TYPE_DESKTOP` | ✅ layer-shell `background` on wlroots/KDE; ⚠️ GNOME experimental/none |
| Overlay | ✅ | ✅ | ✅ | ✅ layer-shell `overlay` (same caveat) |
| Screensaver | ✅ `.saver` bundle ⚠️ host bugs | ✅ `.scr` | ✅ XScreenSaver hack | ⚠️ no standard; our own idle mode |
| Lock screen | ⚠️ saver shown while locked | ⚠️ static snapshot image only 🔍 | ⚠️ via saver | ⚠️ `ext-session-lock-v1` (stretch) |

### 3.2 Wallpaper

- **macOS**: one borderless `NSWindow` per display, at
  `CGWindowLevelForKey(kCGDesktopWindowLevelKey)` so it sits below the
  icons. It uses the collection behaviours `canJoinAllSpaces | stationary
  | ignoresCycle` and sets `ignoresMouseEvents`. It reacts to display
  add, remove and resize events, and to Space changes.
- **Windows**: send `0x052C` to `Progman` so it spawns a `WorkerW` behind
  the icons, then parent our window into it. 🔍 Windows 11 24H2 changed
  this hierarchy and broke several wallpaper apps (Microsoft placed a
  compatibility hold), so this needs testing on 23H2, 24H2 and 25H2. If
  it fails, we fall back to a bottom-most, non-activating window behind
  all others but above the icons, with a warning.
- **Linux X11**: a window with `_NET_WM_WINDOW_TYPE_DESKTOP`, sized to
  each monitor.
- **Linux Wayland**: a `zwlr_layer_shell_v1` surface on the `background`
  layer through `smithay-client-toolkit`. On GNOME, where mutter's
  layer-shell support is experimental or missing, the mode reports
  "unsupported on this desktop".

### 3.3 Overlay

The overlay is transparent, click-through, always on top, takes no focus
and has no taskbar or Dock entry. It is not captured as a regular window
where the OS lets us opt out.

- **macOS**: transparent `NSWindow` at a floating or status level with
  `ignoresMouseEvents` and `canJoinAllSpaces | fullScreenAuxiliary`. The
  app runs as an accessory app with no Dock icon.
- **Windows**: `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST |
  WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`, with per-pixel alpha through a
  DirectComposition swap chain (wgpu DX12 or Vulkan with premultiplied
  alpha 🔍).
- **Linux**: on X11, an ARGB visual plus an empty XShape input region. On
  Wayland, a layer-shell `overlay` layer with an empty input region.
- The overlay hides automatically when a full-screen app or game is in
  front (§6.2). ❓ Q3 decides whether this is free-roaming fish or a small
  floating "bowl" window.

### 3.4 Screensaver and lock screen

- **Windows `.scr`**: a second binary target (`desktank-saver.exe`,
  renamed to `.scr`) that handles `/s` (run), `/p <HWND>` (preview in the
  settings dialog) and `/c` (configure, which opens the config). It exits
  on input.
- **macOS `.saver`**: a `cdylib` bundle whose principal class is a
  `ScreenSaverView` subclass defined with `objc2`. wgpu renders into the
  view's `CAMetalLayer`. The `legacyScreenSaver` host on Sonoma, Sequoia
  and Tahoe is known to leak by never deallocating views. Mitigations:
  - free all GPU resources in `stopAnimation`;
  - keep a single global renderer per process;
  - exit the host process on `com.apple.screensaver.willstop`, the
    approach other saver authors use.

  A leak-soak test is part of acceptance.
- **Built-in idle mode** works on every OS. The running app watches for
  idleness (via `CGEventSourceSecondsSinceLastEventType`,
  `GetLastInputInfo` or Wayland `ext-idle-notify-v1`) and shows its own
  full-screen tank. This is the Wayland screensaver, and a fallback where
  the native saver misbehaves.
- **Linux X11**: an XScreenSaver hack that renders into the window from
  `XSCREENSAVER_WINDOW` / `-window-id`.
- **Lock screen**:
  - **macOS** allows no third-party lock screen. When "Require password
    after screen saver" is on, the saver keeps running over the lock
    until input. That is our lock-screen mode, and we document it.
  - **Windows** allows only a static image or slideshow. We render a tank
    snapshot periodically and set it with
    `Windows.System.UserProfile.LockScreen`. 🔍 We need to check whether
    that API works for an unpackaged app or needs MSIX package identity.
  - **Linux Wayland** could have a real live lock through
    `ext-session-lock-v1` on sway, river, labwc and Wayfire, but that
    means authenticating through PAM, which is security-critical. It is
    deferred, and is an escalation item.

## 4. Architecture

### 4.1 Crates

```
crates/
  desktank-sim       # pure simulation: world, species, steering, RNG, time. No GPU, no OS.
  desktank-render    # wgpu renderer: takes a sim snapshot + a surface/target, draws a frame.
  desktank-platform  # per-OS surface hosts (wallpaper/overlay/idle), power & visibility signals.
  desktank           # main binary: CLI, config, tray, event loop, mode orchestration.
  desktank-saver     # screensaver host binary (.scr on Windows, XScreenSaver hack on X11).
  desktank-saver-mac # cdylib → DeskTank.saver bundle (macOS only).
```

Dependency direction: `sim ← render ← platform ← desktank`, with both
saver crates depending on `sim` and `render`. `desktank-sim` uses
`#![forbid(unsafe_code)]`, as does `render` if possible. All `unsafe`
lives in `platform` and `saver-mac`, at FFI boundaries.

### 4.2 Runtime model

- **One long-running app process**, `desktank run`. It owns one wgpu
  `Device` shared by every surface, plus the tray icon and config. It
  runs one tank per display per mode: wallpaper and overlay on the same
  display share the simulation but draw different layers (❓ Q3).
- The main thread runs the winit event loop (needed for macOS and the
  tray). The simulation steps on the same thread at a fixed `dt`, with
  render interpolation. There are no worker threads in v1, because the
  simulation for about 20 fish × N displays costs microseconds.
- **Frame pacing**: no continuous redraw.
  `ControlFlow::WaitUntil(next_frame)` is used at the target fps, and a
  frame is only requested when a surface is visible. When every surface
  is hidden, the loop waits on events with no timers.
- **Screensaver hosts** are separate processes started by the OS. They
  share the core crates and read the same config. They optionally load a
  read-only tank state snapshot so the saver shows *your* fish (❓ Q2).

### 4.3 Configuration and control

- **Config**: TOML at the platform config dir (`directories`), for
  example `~/Library/Application Support/DeskTank/config.toml`. It is
  hot-reloaded. Every field has a default and a missing file means
  defaults. The schema is versioned with a `version` key.
- **Tray / menu-bar icon** (`tray-icon`) offers:
  - toggling each mode;
  - Pause;
  - Feed;
  - Open config;
  - Start at login;
  - Quit.

  ❓ Q7 decides whether to add a small settings window.
- **CLI** (`clap`):
  - `desktank run [--wallpaper] [--overlay] [--idle]`;
  - `desktank window` (a normal resizable window, for development and
    for users who want a desk-toy window);
  - `desktank capture --seed N --time HH:MM --size WxH --frames K --out DIR`
    (headless offscreen renders for tests, PRs and docs);
  - `desktank saver install|uninstall`.
- **Start at login**: `SMAppService` on macOS, the `HKCU\…\Run` key on
  Windows, and an XDG autostart `.desktop` file on Linux.
- **Logging**: `tracing` to a rotating file in the platform cache dir. It
  is off by default above `info`, and nothing leaves the machine.

## 5. Tech stack

Versions current at the time of writing; Dependabot keeps them fresh.

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust 2024 edition, toolchain pinned in `rust-toolchain.toml` | MSRV = pinned stable |
| GPU | `wgpu` 30 | Metal, DX12 and Vulkan; offscreen rendering for capture and tests |
| Windowing | `winit` 0.30 (stable) | 0.31 is in beta, so we upgrade when it ships. Wayland layer-shell bypasses winit and uses `smithay-client-toolkit` 0.21 |
| macOS FFI | `objc2` 0.6 + `objc2-app-kit` / `objc2-screen-saver` | |
| Windows FFI | `windows` 0.62 | |
| Tray | `tray-icon` 0.26 | |
| Math | `glam` 0.34 | |
| RNG | `rand_pcg` (seeded, reproducible) | |
| CLI / config | `clap` 4, `serde` + `toml` 1, `directories` 6 | |
| Errors / logs | `thiserror` 2 (libraries), `anyhow` (binaries only), `tracing` | |
| Supply chain | `cargo deny` (licences, advisories, duplicates) | |
| Release | `cargo-dist` for GitHub Releases, plus zx scripts (pnpm) for bundle glue | |

## 6. Resource budget and power policy

### 6.1 Budgets

Reference machine: M1 MacBook Air, one 2560×1440 display. Windows and
Linux numbers are recorded and tracked, but not gated in v1.

| Metric | Budget |
|---|---|
| CPU while visible at 30 fps (process total) | ≤ 3% of one core, aiming for ≤ 1.5% |
| CPU while hidden or paused | ≤ 0.1%, with ≤ 1 wake-up per second |
| GPU frame time | ≤ 2 ms at 1440p |
| Resident memory | ≤ 80 MB, aiming for ≤ 50 MB |
| Release binary size (stripped) | ≤ 15 MB |
| Cold start to first frame | ≤ 500 ms |
| Energy Impact (Activity Monitor) | "Low" while visible |

`desktank bench` prints CPU per frame, frame time and RSS. Any PR that
touches the render loop, pacing or assets includes before-and-after
numbers.

### 6.2 Power policy (defaults, all configurable)

- 30 fps on AC power and 15 fps on battery. Rendering pauses in Low Power
  Mode or battery saver, and below 20% battery.
- Rendering pauses when the surface is occluded or minimised, the display
  is asleep, or the session is locked (except for the saver).
- Wallpaper and overlay pause while a full-screen app is in front.
- The renderer may draw caustics at 0.5× resolution and update the
  background at a lower rate than the fish.

## 7. Quality, testing and CI

- **Lints**: workspace `clippy::pedantic`, `unsafe_code = "deny"` except
  in FFI crates, `missing_docs` on public library APIs, and
  `rustfmt.toml`.
- **Unit tests** in `sim`: steering maths, bounds, schooling cohesion
  over N steps, and determinism (same seed, same state hash).
- **Render tests**: offscreen captures compared with reference images
  using a tolerance (perceptual diff). These run in Linux CI on Mesa
  lavapipe, which is software Vulkan, so CI needs no GPU.
- **Platform smoke tests** on each OS runner: start each available mode
  for a few seconds headlessly where possible, then exit cleanly.
- **CI** runs on macOS, Windows and Ubuntu: fmt, clippy, test, doc and
  `cargo deny`. Dependabot covers cargo and actions.
- **Manual acceptance** for the visual and OS modes uses captures
  attached to PRs on the `pr-assets` branch, per the Developer brief.

## 8. Distribution

- GitHub Releases through `cargo-dist`:
  - **macOS**: `DeskTank.app` (universal binary) plus `DeskTank.saver`,
    in a `.dmg`.
  - **Windows**: an installer (MSI) that installs the `.scr` too.
  - **Linux**: a tarball, plus AppImage and AUR later.
- Signing: Developer ID signing and notarization on macOS, and
  Authenticode on Windows. Both cost money and need owner accounts.
  ❓ Q6
- Package managers later: Homebrew cask, winget and Flathub. Flatpak
  sandboxing may block layer-shell and desktop windows 🔍.
- No auto-updater in v1. The tray may show an "Update available" item
  that checks GitHub Releases, and only if the user opts in.

## 9. Milestones and work packages

Each package is one PR (or a few), built by the Developer agent. A
package is done when its acceptance criteria are met; tick it off here
in that PR.

### M0 — Foundations

- [ ] **F1 Workspace scaffold + CI.** Crates from §4.1 as stubs, a
  toolchain file, lints, `deny.toml` and Dependabot, with CI on three
  OSes. *Accept:* CI is green on all three OSes and `cargo run -p
  desktank -- --help` works.

### M1 — A tank in a window

- [ ] **T1 Simulation core.** World, species loading from TOML, wander
  plus bounds steering, fixed timestep and seeded RNG. *Accept:* the
  determinism test passes and fish stay inside the bounds for 10⁵ steps.
- [ ] **T2 Renderer + `desktank window`.** wgpu setup, water gradient and
  instanced SDF fish with tail animation, frame pacing with an fps cap,
  and a pause when occluded or minimised. *Accept:* within the §6.1
  budgets in a window on the reference machine.
- [ ] **T3 Capture.** `desktank capture` renders offscreen to PNG or a
  frame sequence, with render tests on lavapipe in CI. *Accept:* a
  reference-image test is in CI.
- [ ] **T4 Schooling and species.** Boids weighting, 5 species and depth
  bands. *Accept:* schoolers stay cohesive (a test on mean
  nearest-neighbour distance) and the GIF looks natural.
- [ ] **T5 Scenery and light.** Rocks, sand, swaying plants, bubbles and
  caustics. *Accept:* still within budget and within 10% of T2's CPU.
- [ ] **T6 Day/night and feeding.** Lighting tied to time, night
  behaviour, and food flakes (triggered from the CLI or a key in window
  mode).
- [ ] **T7 `desktank bench`.** Prints frame CPU, frame time and RSS, with
  measurement recipes for all three OSes documented in PROGRESS.

### M2 — Overlay and control

- [ ] **O1 Overlay mode** on macOS, Windows and Linux (X11 + Wayland
  layer-shell). *Accept:* click-through verified, no focus stealing, no
  Dock or taskbar entry, and works across Spaces and virtual desktops.
- [ ] **O2 Tray + config.** Tray menu (§4.3), TOML config with hot
  reload, and start at login.
- [ ] **O3 Power policy** (§6.2). *Accept:* the hidden-state CPU budget
  is met, and on battery the frame rate drops to 15 fps.
- [ ] **O4 Multi-monitor.** One tank per display, with hot-plug and
  per-display scale factors.

### M3 — Wallpaper

- [ ] **W1 macOS wallpaper.** Below the icons on every display and
  Space.
- [ ] **W2 Windows wallpaper.** WorkerW with the 24H2 hierarchy handled
  and a fallback, tested on 23H2, 24H2 and 25H2.
- [ ] **W3 Linux wallpaper.** X11 desktop window and Wayland
  layer-shell background, with a clear message on unsupported desktops.

### M4 — Screensaver

- [ ] **S1 Windows `.scr`** with `/s`, `/p` and `/c`, plus an installer
  hook.
- [ ] **S2 macOS `.saver`.** objc2 `ScreenSaverView`, a Metal layer, the
  leak mitigations, and `desktank saver install`. *Accept:* an 8-hour
  soak shows no growth in `legacyScreenSaver` RSS beyond 20 MB.
- [ ] **S3 Built-in idle mode** on every OS (the Wayland saver).
- [ ] **S4 XScreenSaver hack** for X11.

### M5 — Lock screen

- [ ] **L1 macOS.** Verify and document "saver while locked", with a
  tray shortcut to the relevant System Settings pane.
- [ ] **L2 Windows lock-screen snapshot.** Periodic tank render set as
  the lock image, pending the 🔍 API check.
- [ ] **L3 Linux `ext-session-lock-v1`** (stretch, escalation). Live lock
  screen with PAM authentication; needs a security review.

### M6 — Ship it

- [ ] **D1 Release pipeline** (`cargo-dist`), with bundles for the `.app`,
  `.saver`, MSI and tarball.
- [ ] **D2 Signing and notarization** (needs owner accounts, ❓ Q6).
- [ ] **D3 Package managers**: Homebrew cask, winget, AUR and Flathub.

Suggested order: F1 → T1 → T2 → T3 → O1 (macOS) → O2 → T4/T5 → W1 → S2
→ the rest. Within each mode macOS comes first, then Windows, then Linux
(❓ Q4).

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Windows 24H2+ WorkerW changes | Wallpaper broken on new Windows | Test matrix and a fallback window; follow Lively Wallpaper's fixes |
| macOS `legacyScreenSaver` leaks and regressions | Saver eats RAM; Apple keeps breaking it | Mitigations, soak test, and built-in idle mode as a fallback |
| GNOME lacks layer-shell | No wallpaper or overlay on GNOME Wayland | Document it; X11 session works; a GNOME Shell extension is a possible later option |
| Transparent overlays on Windows with wgpu | Alpha may not compose | Spike in O1; fall back to `UpdateLayeredWindow` blit if needed 🔍 |
| GPU driver baseline memory | RSS budget exceeded | Measure in T2; low-power adapter, small swapchains, no MSAA (analytic SDF anti-aliasing) |
| Lock-screen APIs need package identity | L2 not possible unpackaged | Consider MSIX packaging, or drop L2 |

## 11. Open questions

1. **Art direction.** Procedural stylised vector fish (default: small,
   crisp, no assets), pixel art, or realistic hand-made sprites?
2. **Tank personality.** Purely ambient (default), or light pet
   mechanics: persistent fish, names, hunger, growth, breeding,
   unlockable species?
3. **Overlay content.** Free-roaming fish over all windows (default), a
   movable floating bowl window, or both?
4. **Platform priority.** macOS → Windows → Linux (default)? Which Linux
   desktops matter to you?
5. **Lock screen.** Do the platform limits in §3.4 match your
   expectations? Is a static snapshot on Windows worth doing?
6. **Distribution.** Public open source? Will you pay for an Apple
   Developer ID ($99/yr) and Windows code signing? Is this free, or will
   it be sold?
7. **Settings.** Tray menu plus config file only (default), or a small
   settings window?
8. **Interaction and sound.** Fish react to the cursor (default: yes,
   gently)? Optional ambient sound (default: none)?
9. **Multi-monitor.** One tank per display (default) or one spanning
   tank?
10. **Budgets.** Are the §6.1 numbers and reference machine right?
11. **Name and identity.** Is "DeskTank" final? Do you want an icon or
    logo direction?
