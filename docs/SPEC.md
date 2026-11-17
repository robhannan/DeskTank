# DeskTank — Plan and Spec

Status: **draft v0.2** (2026-11-17). Items marked ❓ are waiting on owner
decisions (see [Open questions](#11-open-questions)); the defaults shown
are what we build if nobody objects. Items marked 🔍 are technical claims
that must be verified on real hardware, or against primary docs, before
the work package that relies on them starts.

## 1. Vision

DeskTank is a calm, living aquarium for the desktop. You leave it running
all day, so it must feel cheap: it should never appear on a "top energy
users" list. It can run as any combination of four **display modes**:

| Mode | What the user sees |
|---|---|
| **Wallpaper** | The tank replaces the desktop background, behind the icons. |
| **Overlay** | Fish swim over open windows on a transparent, click-through layer. |
| **Screensaver** | A full-screen tank when the machine is idle. |
| **Lock screen** | The tank on the lock screen, as far as each OS allows (§3.4, ❓ Q5). |

### Principles

1. **Resources are a feature.** We have hard budgets (§6) and measure
   them. When nobody can see the tank, it does no work.
2. **One tank core, thin platform shells.** The simulation and renderer
   know nothing about the OS or any event loop. Each mode/OS pairing is a
   small adapter that hands the core a surface and calls it once per
   frame.
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
   animation. Anti-aliasing comes from the SDF, so there is no MSAA.
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
| Wallpaper | ✅ desktop-level window | ✅ WorkerW child window 🔍 | ✅ `_NET_WM_WINDOW_TYPE_DESKTOP` | ✅ layer-shell `background` on wlroots/KDE; ⚠️ GNOME experimental/none 🔍 |
| Overlay | ✅ | ✅ (compositing path 🔍) | ✅ needs a running compositor | ✅ layer-shell `overlay` (same GNOME caveat) |
| Screensaver | ✅ `.saver` bundle ⚠️ host bugs 🔍 | ✅ `.scr` | ✅ built-in idle mode; ⚠️ XScreenSaver hack only where XScreenSaver is the saver | ✅ built-in idle mode |
| Lock screen | ⚠️ saver shown while locked | ⚠️ static snapshot image only 🔍 | ⚠️ via saver | ⚠️ `ext-session-lock-v1` (stretch) |

### 3.2 Wallpaper

- **macOS**: one borderless `NSWindow` per display, at
  `CGWindowLevelForKey(kCGDesktopWindowLevelKey)` so it sits below the
  icons. It uses the collection behaviours `canJoinAllSpaces | stationary
  | ignoresCycle` and sets `ignoresMouseEvents`. It reacts to display
  add, remove and resize events, and to Space changes.
- **Windows**: send `0x052C` to `Progman` so it creates a `WorkerW`
  behind the icons, then parent our window into it.
  - 🔍 Windows 11 24H2 reportedly changed this hierarchy: users reported
    broken wallpaper apps and Microsoft placed a compatibility hold on
    some wallpaper customisation apps
    ([The Register](https://www.theregister.com/2025/04/14/windows_11_24h2_hold/)).
    We test on 23H2, 24H2 and 25H2. Lively Wallpaper is the reference
    for how others solved it. It is GPL-licensed, so we read it for
    behaviour only and never copy its code.
  - The WorkerW covers the whole virtual desktop. We create one child
    window per monitor, positioned in virtual-screen coordinates, each
    with its own per-monitor DPI scale.
  - Fallback if no WorkerW can be found: no live wallpaper. We set a
    rendered tank snapshot as the static wallpaper (`IDesktopWallpaper`)
    and refresh it at most every 30 minutes. The original wallpaper is
    restored on exit or when the mode is turned off.
- **Linux X11**: a window with `_NET_WM_WINDOW_TYPE_DESKTOP`, sized to
  each monitor.
- **Linux Wayland**: a `zwlr_layer_shell_v1` surface on the `background`
  layer through `smithay-client-toolkit`. On GNOME, where mutter's
  layer-shell support is at best experimental, the mode reports
  "unsupported on this desktop".

### 3.3 Overlay

The overlay is transparent, click-through, always on top, takes no focus
and has no taskbar or Dock entry. By default it is excluded from screen
capture, using `NSWindow.sharingType = .none` on macOS and
`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on Windows. This is
configurable, because some people want to show their fish off.

- **macOS**: transparent `NSWindow` at the floating window level, with
  `ignoresMouseEvents` and `canJoinAllSpaces | stationary`. It does
  **not** use `fullScreenAuxiliary`, so it never appears in full-screen
  Spaces. The app runs as an accessory app with no Dock icon.
- **Windows**: the primary path is a window with
  `WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT |
  WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`.
  - Per-pixel alpha comes from a DirectComposition swap chain with
    premultiplied alpha. 🔍 It's unconfirmed whether wgpu's DX12
    backend exposes a composition swap chain with premultiplied alpha
    for an HWND.
  - Click-through relies on `WS_EX_LAYERED | WS_EX_TRANSPARENT`
    (hit-testing passes through regardless of how the window is drawn).
  - Fallback path: render offscreen with wgpu, read back, and present
    with `UpdateLayeredWindow` at a lower frame rate. Click-through
    works the same way.
  - The O1-Win spike picks one path, with pass/fail criteria (§9).
- **Linux X11**: an ARGB visual plus an empty XShape input region. This
  needs a running compositing manager (`_NET_WM_CM_S<n>` owned).
  Without one the mode reports "needs a compositor" and does not start.
- **Linux Wayland**: a layer-shell `overlay` layer with an empty input
  region.
- **Full-screen apps**: the overlay and wallpaper pause and hide while a
  full-screen app is in front (§6.2 has detection details). ❓ Q3 decides
  whether the overlay is free-roaming fish or a small floating "bowl"
  window.

### 3.4 Screensaver and lock screen

- **Windows `.scr`**: the `desktank-saver` binary, renamed to `.scr`.
  - It handles `/s` (run), `/p <HWND>` (preview in the settings dialog)
    and `/c` (configure, which opens the config). It exits on input.
  - It is one process: with `/s` it creates one window per monitor.
  - Install options (🔍 to confirm in P1):
    - per-machine copy into `System32` (needs elevation, done by the
      installer), so it appears in the Screen Saver list;
    - per-user, by pointing `HKCU\Control Panel\Desktop\SCRNSAVE.EXE` at
      the user-local copy (no elevation, but it is not listed in the
      dialog).

    `desktank saver install` does the per-user variant.
- **macOS `.saver`**: a bundle whose principal class is a
  `ScreenSaverView` subclass defined with `objc2`.
  - The host drives it: `startAnimation`, `animateOneFrame` and
    `stopAnimation` come from `legacyScreenSaver`'s run loop, not ours,
    so the core must be steppable from a foreign loop (§4.2). wgpu
    renders into the view's `CAMetalLayer`.
  - 🔍 We need to check whether a Rust `cdylib` (`MH_DYLIB`) loads as a
    bundle executable or must be linked with `-bundle`.
  - 🔍 `legacyScreenSaver` on Sonoma, Sequoia and Tahoe reportedly never
    deallocates saver views, so it grows across activations
    ([cocoa-dev, Sep 2024](https://lists.apple.com/archives/cocoa-dev/2024/Sep/msg00001.html);
    [Apple forums](https://developer.apple.com/forums/thread/787444)).
    Mitigations:
    - free all GPU resources in `stopAnimation`;
    - keep one renderer per host process;
    - exit the host on the `com.apple.screensaver.willstop` distributed
      notification, a workaround saver authors report but which isn't
      reliable.

    Acceptance includes a leak-soak test.
  - Until signing exists (D2), the saver is ad-hoc signed and for
    developers only. Users must clear quarantine by hand.
- **Built-in idle mode** works on every OS. The running app detects
  idleness itself and shows its own full-screen tank windows, then
  dismisses them on input:
  - **macOS**: `CGEventSourceSecondsSinceLastEventType`, polled at most
    every 10 s.
  - **Windows**: `GetLastInputInfo`, polled the same way.
  - **Wayland**: `ext-idle-notify-v1`, which is event-driven.
  - **X11**: the XScreenSaver extension's idle query.

  It is the screensaver on Wayland, the default on X11 (GNOME and KDE do
  not use XScreenSaver), and the fallback where a native saver
  misbehaves.
- **Linux X11 XScreenSaver hack** (optional): renders into the window
  given by `XSCREENSAVER_WINDOW` / `-window-id`, for desktops that
  actually run XScreenSaver.
- **Lock screen** (❓ Q5):
  - **macOS** allows no third-party lock screen. When "Require password
    after screen saver" is on, the saver keeps running over the lock
    until input. That is our lock-screen mode, and we document it.
  - **Windows** allows only a static image or slideshow. We render a tank
    snapshot periodically and set it with
    `Windows.System.UserProfile.LockScreen.SetImageFileAsync`.
    - 🔍 It isn't confirmed whether this works for an unpackaged app or
      needs MSIX package identity (resolved in P1).
    - It only shows when the user's lock-screen background is set to
      "Picture". We detect and report that case rather than changing the
      user's setting.
    - The user's previous image is saved and restored when the mode is
      turned off.
  - **Linux Wayland** could have a real live lock through
    `ext-session-lock-v1` on sway, river, labwc and Wayfire. That means
    authenticating through PAM, which is security-critical, and a crashed
    lock client must leave the session locked and recoverable (the
    protocol keeps the session locked; recovery runs through the
    compositor or a TTY). It is deferred, and is an escalation item.

## 4. Architecture

### 4.1 Crates

```
crates/
  desktank-sim       # pure simulation: world, species, steering, RNG, time. No GPU, no OS.
  desktank-render    # wgpu renderer: takes a sim snapshot + a surface/target, draws a frame.
  desktank-platform  # per-OS surface hosts (wallpaper/overlay/idle/saver windows),
                     # power, visibility, full-screen and idle signals.
  desktank           # main binary: CLI, config, tray, event loop, mode orchestration.
  desktank-saver     # screensaver host binary (.scr on Windows, XScreenSaver hack on X11).
  desktank-saver-mac # macOS .saver bundle (ScreenSaverView subclass).
```

Dependency direction: `sim ← render ← platform ← {desktank,
desktank-saver}`, and `sim ← render ← desktank-saver-mac`. The saver
binary reuses `platform` for its windows and input detection.

`desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. If
wgpu surface creation from raw handles needs `unsafe`, it moves into
`platform`. All other `unsafe` lives in `platform` and
`desktank-saver-mac`, at FFI boundaries, with `// SAFETY:` comments.

### 4.2 Runtime model

- **Loop-agnostic core.** `desktank-render` exposes a `Tank` with
  `tick(now) -> FrameRequest` and `draw(surface)`. Whatever loop owns the
  surface calls them:
  - the winit loop (macOS and Windows windows, plus X11);
  - an SCTK/`calloop` loop (Wayland layer-shell);
  - `legacyScreenSaver`'s `animateOneFrame` (macOS saver);
  - the `.scr` message loop.

  Each loop host implements the same pacing rules.
- **One long-running app process**, `desktank run`. It owns one wgpu
  `Device` shared by every surface, plus the tray icon and config. It
  runs one tank per display per mode: wallpaper and overlay on the same
  display share the simulation but draw different layers (❓ Q3).
  - **macOS, Windows and X11**: the main thread runs the winit loop
    (needed for macOS and the tray).
  - **Wayland**: the main thread runs a `calloop` loop with the SCTK
    layer-shell surfaces, and winit is not used.
- The simulation steps on the loop thread at a fixed `dt`, with render
  interpolation. There are no worker threads in v1, because about 20
  fish × N displays costs microseconds.
- **Frame pacing**: no continuous redraw. The loop sleeps until the next
  frame deadline at the target fps (`ControlFlow::WaitUntil`, or a calloop
  timer), and a frame is only requested when a surface is visible. When
  every surface is hidden, the loop waits on OS events only, with no
  frame timer (§6.2 lists the event sources).
- **Screensaver hosts** are separate processes started by the OS, each
  with its own wgpu device and its own budget (§6.1). They read the same
  config. They optionally load a read-only tank state snapshot so the
  saver shows *your* fish (❓ Q2).

### 4.3 Configuration and control

- **Config**: TOML at the platform config dir (`directories`), for
  example `~/Library/Application Support/DeskTank/config.toml`. It is
  hot-reloaded with `notify`. Every field has a default and a missing
  file means defaults. The schema is versioned with a `version` key.
- **Tray / menu-bar icon** offers:
  - toggling each mode;
  - Pause;
  - Feed;
  - Open config;
  - Start at login;
  - Quit.

  On macOS and Windows it uses `tray-icon`. On Linux it uses `ksni`
  (StatusNotifierItem over D-Bus), because `tray-icon` needs a GTK main
  loop and libappindicator there, which costs resources (🔍 confirm
  before O2). ❓ Q7 decides whether to add a small settings window.
- **CLI** (`clap`):
  - `desktank run [--wallpaper] [--overlay] [--idle]`;
  - `desktank window` (a normal resizable window, for development and
    for users who want a desk-toy window);
  - `desktank capture --seed N --time HH:MM --size WxH --frames K --out DIR`
    (headless offscreen renders for tests, PRs and docs);
  - `desktank bench`;
  - `desktank saver install|uninstall`.
- **Start at login**: `SMAppService` on macOS, the `HKCU\…\Run` key on
  Windows, and an XDG autostart `.desktop` file on Linux.
- **Logging**: `tracing`, with `tracing-appender` writing a rotating file
  in the platform cache dir. The default level is `info`, overridable
  with `DESKTANK_LOG`. Nothing leaves the machine.

## 5. Tech stack

Versions current at the time of writing; Dependabot keeps them fresh.

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust 2024 edition, toolchain pinned in `rust-toolchain.toml` | MSRV = pinned stable |
| GPU | `wgpu` 30 | Metal, DX12 and Vulkan; offscreen rendering for capture and tests |
| Windowing | `winit` 0.30 (stable) | 0.31 is in beta, so Dependabot ignores winit majors until we choose to upgrade |
| Wayland | `smithay-client-toolkit` 0.21 + `calloop` | Layer-shell and idle-notify |
| macOS FFI | `objc2` 0.6 + `objc2-app-kit` / `objc2-screen-saver` | |
| Windows FFI | `windows` 0.62 | |
| Tray | `tray-icon` 0.26 (macOS/Windows), `ksni` (Linux) | |
| Math | `glam` 0.34 | |
| RNG | `rand_pcg` (seeded, reproducible) | |
| CLI / config | `clap` 4, `serde` + `toml` 1, `directories` 6, `notify` | |
| Errors / logs | `thiserror` 2 (libraries), `anyhow` (binaries only), `tracing` + `tracing-appender` | |
| Supply chain | `cargo deny` (licences, advisories, duplicates) | |
| Release | `cargo-dist` for archives, MSI and shell installers; zx scripts (pnpm) build `.app`, `.saver` and `.dmg` | |

## 6. Resource budget and power policy

### 6.1 Budgets

Reference machine: M1 MacBook Air, one 2560×1440 display (❓ Q10).
Windows and Linux numbers are recorded and tracked, but not gated in v1.
Each additional display may add the per-display increment.

| Metric | Main app (1 display) | Per extra display | Each saver host process |
|---|---|---|---|
| CPU while visible at 30 fps | ≤ 3% of one core, aiming for ≤ 1.5% | +1% | ≤ 3% |
| CPU while hidden or paused | ≤ 0.1% | — | n/a (exits) |
| Wake-ups while hidden | ≤ 1/s (OS events only, plus polls ≤ every 10 s) | — | n/a |
| GPU frame time at 1440p | ≤ 2 ms | +2 ms | ≤ 2 ms per display |
| Resident memory | ≤ 80 MB, aiming for ≤ 50 MB | +10 MB | ≤ 80 MB, with no growth over 8 h |
| Release binary size (stripped) | ≤ 15 MB | — | ≤ 15 MB |
| Cold start to first frame | ≤ 500 ms | — | ≤ 500 ms |
| Energy Impact (Activity Monitor) | "Low" while visible | — | "Low" |

`desktank bench` (from T2) prints CPU per frame, GPU frame time and RSS.
Any PR that touches the render loop, pacing or assets includes
before-and-after numbers.

### 6.2 Power policy and event sources

Defaults, all configurable:

- 30 fps on AC power and 15 fps on battery. Rendering pauses in Low Power
  Mode or battery saver, and below 20% battery.
- Rendering pauses when the surface is occluded or minimised, the display
  is asleep, or the session is locked (except for the saver).
- Wallpaper and overlay pause while a full-screen app is in front.

| Signal | macOS | Windows | Linux |
|---|---|---|---|
| Occluded / hidden | `NSWindow` occlusion-state notification | `WM_SIZE` minimise; DWM cloaked; per-frame present status | Wayland frame callbacks stop; X11 `VisibilityNotify` |
| AC / battery / level | `IOPSNotificationCreateRunLoopSource` | `RegisterPowerSettingNotification` (`GUID_ACDC_POWER_SOURCE`, battery %) | UPower over D-Bus |
| Low power / saver | `NSProcessInfoPowerStateDidChange` | Energy saver status power setting 🔍 | UPower / power-profiles-daemon |
| Display sleep / lock | `NSWorkspace` screens sleep/wake; `com.apple.screenIsLocked` | `WTSRegisterSessionNotification`; display-state power setting | logind `Lock`/`Unlock`, `PrepareForSleep` |
| Full-screen app in front | Active-Space change + `CGWindowListCopyWindowInfo` bounds check (no window titles, so no Screen Recording permission) 🔍 | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` → `SHQueryUserNotificationState` | X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_STATE_FULLSCREEN`; Wayland: `wlr-foreign-toplevel-management` where available, otherwise not detected (the overlay layer stays above full-screen windows) |

All of these are event-driven except the idle polls (§3.4), which run at
most every 10 s and only while a mode that needs them is enabled.

## 7. Quality, testing and CI

- **Lints**: workspace `clippy::pedantic`, `unsafe_code = "deny"` except
  in FFI crates, `missing_docs` on public library APIs, and
  `rustfmt.toml`.
- **Unit tests** in `sim`: steering maths, bounds, schooling cohesion
  over N steps, and determinism (same seed, same state hash).
- **Render tests**: offscreen captures compared with reference images.
  They pass when ≤ 0.5% of pixels differ by more than ΔE 2. They run in
  Linux CI on Mesa lavapipe (software Vulkan), so CI needs no GPU.
  Reference images are updated deliberately with a `--bless` flag.
- **Platform smoke tests** on each OS runner: start each available mode
  for a few seconds where the runner allows, then exit cleanly.
- **CI** runs on macOS, Windows and Ubuntu: fmt, clippy, test, doc and
  `cargo deny`. Dependabot covers cargo and actions.
- **Manual acceptance** for the visual and OS modes uses captures
  attached to PRs on the `pr-assets` branch, per the Developer brief.

## 8. Distribution

- GitHub Releases:
  - **`cargo-dist`** builds the archives, the Windows MSI (which also
    installs the `.scr`, per-machine) and the shell and PowerShell
    installers.
  - **zx scripts** build `DeskTank.app` (universal binary), assemble
    `DeskTank.saver`, and package both in a `.dmg`.
  - Linux ships a tarball now, with AppImage and AUR later.
- Signing: Developer ID signing and notarization on macOS, and
  Authenticode on Windows. Both cost money and need owner accounts.
  ❓ Q6
- Package managers later:
  - a Homebrew **cask** in our own tap, maintained separately (cargo-dist
    produces formulae, not casks);
  - winget;
  - Flathub. 🔍 Flatpak sandboxing may block layer-shell and desktop
    windows.
- No auto-updater in v1. The tray may show an "Update available" item
  that checks GitHub Releases, and only if the user opts in.

## 9. Milestones and work packages

Each package is one PR (or a few), built by the Developer agent. A
package is done when its acceptance criteria are met; tick it off here
in that PR. "Within budget" means §6.1, measured with `desktank bench` on
the reference machine and reported in the PR.

### M0 — Foundations

- [ ] **F1 Workspace scaffold + CI.** Crates from §4.1 as stubs, a
  toolchain file, lints, `deny.toml` and Dependabot (ignoring winit
  majors), with CI on three OSes. *Accept:* CI is green on all three OSes
  and `cargo run -p desktank -- --help` works.
- [ ] **P1 Packaging and identity spike** (findings go in PROGRESS, no
  product code). Answer:
  - whether `LockScreen.SetImageFileAsync` works unpackaged;
  - MSI vs MSIX;
  - per-user vs per-machine `.scr` install;
  - whether a Rust `cdylib` loads as a `.saver`;
  - whether wgpu supports premultiplied-alpha composition swap chains on
    DX12.

  *Accept:* each question answered yes or no, with evidence (a doc link
  or a minimal test), and the affected 🔍 marks resolved in this spec.

### M1 — A tank in a window

- [ ] **T1 Simulation core.** World, species loading from TOML, wander
  plus bounds steering, fixed timestep and seeded RNG. *Accept:* the
  same seed gives the same state hash after 10⁴ steps, and no fish
  leaves the bounds in 10⁵ steps.
- [ ] **T2 Renderer, `desktank window` and minimal `desktank bench`.**
  wgpu setup, water gradient and instanced SDF fish with tail animation.
  The loop-agnostic `Tank` API (§4.2) paces frames with an fps cap and
  pauses when occluded or minimised. *Accept:* within the main-app budget
  in a window; hidden-state CPU ≤ 0.1%.
- [ ] **T3 Capture.** `desktank capture` renders offscreen to PNG or a
  frame sequence, with render tests on lavapipe in CI at the §7
  tolerance. *Accept:* at least 2 reference images are tested in CI, and
  a deliberate shader change fails the test.
- [ ] **T4 Schooling and species.** Boids weighting, 5 species and depth
  bands. *Accept:*
  - schoolers' mean nearest-neighbour distance is ≤ 3 body lengths for
    ≥ 80% of 10⁴ steps;
  - each species spends ≥ 70% of its time in its depth band;
  - a GIF is in the PR.
- [ ] **T5 Scenery and light.** Rocks, sand, swaying plants, bubbles and
  caustics. *Accept:* within budget, with GPU frame time ≤ T2 + 1 ms and
  CPU ≤ T2 + 0.5%.
- [ ] **T6 Day/night and feeding.** Lighting tied to time, night
  behaviour, and food flakes (triggered by `desktank capture --feed` and
  a key in window mode). *Accept:* captures at 4 times of day differ as
  specified, and ≥ 80% of fish reach food within 10 s of simulated time.
- [ ] **T7 Cross-OS measurement.** `desktank bench` on Windows and
  Linux, with measurement recipes for all three OSes documented in
  PROGRESS. *Accept:* numbers recorded for all three OSes.

### M2 — Power, overlay and control

- [ ] **O3 Power policy and event sources** (§6.2), before any
  always-on mode ships. *Accept:*
  - on battery the frame rate drops to 15 fps;
  - Low Power Mode, lock and display sleep pause rendering;
  - hidden-state wake-ups stay within budget;
  - all verified on macOS, with Windows and Linux in their O1 PRs.
- [ ] **O1-Mac / O1-Win / O1-Linux Overlay mode**, one PR per OS. Each
  is accepted when:
  - clicks reach the windows underneath;
  - the overlay never takes focus;
  - it has no Dock or taskbar entry;
  - it is visible on every Space or virtual desktop;
  - it is hidden while a full-screen app is in front;
  - it is excluded from screen capture by default;
  - it is within budget.

  O1-Win starts with a spike choosing between the composition swap chain
  and `UpdateLayeredWindow` (§3.3). The spike passes when the window
  shows per-pixel alpha correctly, is click-through, and stays within
  budget. O1-Linux covers X11 with a compositor and Wayland layer-shell,
  and reports clearly when neither is available.
- [ ] **O2 Tray + config.** The tray menu (§4.3), TOML config with hot
  reload, and start at login. *Accept:* every menu item works on all
  three OSes, a config edit applies within 1 s without a restart, an
  invalid config logs an error and keeps the last good config, and start
  at login survives a reboot.
- [ ] **O4 Multi-monitor.** One tank per display, with hot-plug and
  per-display scale factors. *Accept:* plugging or unplugging a display,
  or changing its scale, adds, removes or rescales its tank within 2 s
  with no restart or leak.

### M3 — Wallpaper

- [ ] **W1 macOS wallpaper.** *Accept:* sits below the desktop icons on
  every display and Space, survives display changes and sleep/wake,
  pauses when fully covered, and stays within budget.
- [ ] **W2 Windows wallpaper.** WorkerW with the 24H2 hierarchy handled,
  per-monitor children and the snapshot fallback (§3.2). *Accept:* the
  same checks as W1 on 23H2, 24H2 and 25H2, and the original wallpaper is
  restored on exit.
- [ ] **W3 Linux wallpaper.** X11 desktop window and Wayland layer-shell
  background. *Accept:* the W1 checks on sway or Hyprland, KDE, and X11,
  plus an "unsupported" message on GNOME Wayland.

### M4 — Screensaver

- [ ] **S3 Built-in idle mode** on every OS (§3.4). It comes first
  because it is the fallback for S1 and S2. *Accept:* starts within 2 s
  of the configured idle time, dismisses on any input, covers every
  display, and the saver windows stay within budget.
- [ ] **S1 Windows `.scr`** with `/s`, `/p` and `/c`, and `desktank saver
  install` (per-user). The installer hook comes with D1. *Accept:* each
  argument works from the Screen Saver dialog or the registry entry, the
  preview renders inside the dialog, and the saver exits on input.
- [ ] **S2 macOS `.saver`.** objc2 `ScreenSaverView`, a Metal layer, the
  leak mitigations, and `desktank saver install` (developer-only until
  D2). *Accept:*
  - the saver runs and previews in System Settings;
  - an 8-hour soak with 50 or more activations shows `legacyScreenSaver`
    RSS growth of 20 MB or less;
  - it is within the saver-host budget.
- [ ] **S4 XScreenSaver hack** (optional). *Accept:* runs in
  `xscreensaver-demo` preview and full-screen.

### M5 — Lock screen

- [ ] **L1 macOS.** *Accept:* with "Require password after screen saver"
  on, the S2 saver keeps animating over the lock until input on the
  current macOS. The steps are in the README, and the tray item opens
  the right System Settings pane.
- [ ] **L2 Windows lock-screen snapshot** (only if P1 says the API is
  usable). *Accept:*
  - the lock screen shows a tank snapshot no more than 30 minutes old;
  - the user's previous image is restored when the mode is turned off;
  - the app reports clearly when the lock background is not "Picture".
- [ ] **L3 Linux `ext-session-lock-v1`** (stretch, escalation). *Accept:*
  - a security review is done before merging;
  - the PAM unlock works;
  - killing the client leaves the session locked and recoverable.

### M6 — Ship it

- [ ] **D1 Release pipeline.** *Accept:* a tag produces, through CI, a
  GitHub Release with the `.dmg` (`.app` + `.saver`), the MSI (with
  `.scr`) and a Linux tarball, and each installs and runs on a clean
  machine.
- [ ] **D2 Signing and notarization** (needs owner accounts, ❓ Q6).
  *Accept:* the `.dmg` passes `spctl --assess`, and the MSI and `.scr`
  are Authenticode-signed with no SmartScreen "unknown publisher"
  warning, given the certificate type.
- [ ] **D3 Package managers.** *Accept:* `brew install --cask`, `winget
  install` and an AUR package each install a working release.

Suggested order:

1. F1, then P1 and T1 in parallel.
2. T2, T3.
3. O3, then O1-Mac, then O2.
4. T4, T5, T6.
5. W1, then S3, then S2, then the rest.

Within each mode macOS comes first, then Windows, then Linux (❓ Q4).

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Windows 24H2+ WorkerW changes | Wallpaper broken on new Windows | Test matrix and snapshot fallback; study Lively's behaviour (GPL, so no code copying) |
| macOS `legacyScreenSaver` leaks and regressions | Saver eats RAM; Apple keeps breaking it | Mitigations, soak test, and built-in idle mode as a fallback |
| GNOME lacks layer-shell | No wallpaper or overlay on GNOME Wayland | Document it; X11 session works; a GNOME Shell extension is a possible later option |
| Transparent overlays on Windows with wgpu | Alpha may not compose | P1 question plus the O1-Win spike; `UpdateLayeredWindow` fallback |
| GPU driver baseline memory | RSS budget exceeded | Measure in T2; low-power adapter, small swapchains, no MSAA |
| Lock-screen API needs package identity | L2 not possible unpackaged | Decided in P1: MSIX packaging, or drop L2 |
| Unsigned builds | Gatekeeper and SmartScreen block users | Developer-only until D2; ❓ Q6 |

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
