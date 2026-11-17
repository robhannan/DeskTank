# DeskTank — Plan and Spec

Status: **draft v0.3** (2026-11-17).

- ❓ marks items still waiting on owner decisions (see
  [Open questions](#12-open-questions)). The defaults shown are what we
  build if nobody objects.
- 🔍 marks technical claims to verify on real hardware, or against
  primary docs, before the work package that relies on them starts.
- Owner decisions made so far are logged in
  [§11](#11-decision-log).

## 1. Vision

DeskTank is a pixel-art aquarium that lives on your desktop. It combines
a falling-sand material simulation with a pet simulation.

- **Materials:** you pour substrate, food, fertiliser, seeds and eggs into
  the tank. Grains settle, bubbles rise, plants grow, and dead plants and
  creatures sink and break down.
- **Creatures:** real freshwater fish, shrimp, snails and frogs live,
  breed and die.

The tank's edges are the edges of the screen. You leave it running all
day, so it must feel cheap: it should never appear on a "top energy
users" list.

It can run as any combination of four **display modes**:

| Mode | What the user sees |
|---|---|
| **Wallpaper** | The tank replaces the desktop background, behind the icons. |
| **Overlay** | The same tank drawn over open windows on a transparent, click-through layer. The water is clear (or lightly tinted) and the substrate rests along the bottom of the screen. |
| **Screensaver** | A full-screen tank when the machine is idle. |
| **Lock screen** | The tank on the lock screen, as far as each OS allows (§3.4). |

### Principles

1. **Resources are a feature.** We have hard budgets (§6) and measure
   them. When nobody can see the tank, it does no work. Settled material
   costs nothing, because chunks of the grid sleep.
2. **One tank core, thin platform shells.** The simulation and renderer
   know nothing about the OS or any event loop. Each mode/OS pairing is a
   small adapter that hands the core a surface and calls it once per
   frame.
3. **A pixel-art cellular world.** Water, substrate, plants, bubbles,
   food and detritus are cells on a grid. Creatures are pixel-art sprites
   that move over the grid and interact with it. Everything is rendered
   at integer scale with nearest-neighbour sampling.
4. **Data-driven.** Materials, reactions and species are TOML data, not
   code. Code exists only for behaviour that data can't express. This is
   the falling-sand pattern; see
   [`research/powder-toy.md`](research/powder-toy.md).
5. **Deterministic.** Given a seed and a sequence of inputs, the
   simulation is reproducible, so it is testable and screenshots are
   repeatable.
6. **Honest about platform limits.** Where an OS does not allow a mode,
   we document it and offer the closest alternative.
7. **Inspired by, never copied from.** The Powder Toy (GPL) and other
   falling-sand games are references for ideas only. No code, element
   tables, names or art are taken from them.

### Non-goals (v1)

- Mobile and web builds.
- 3D.
- Online features, accounts or telemetry.
- Progression, unlocks or currency: it is a sandbox (D9).
- Plugin or scripting support.

## 2. The tank

### 2.1 World model

- **Grid.** The tank is a grid of cells covering the display. Each cell
  is a configurable number of screen pixels: 4×4 by default (2–8
  allowed, D1), which gives 640×360 cells at 2560×1440.
  - The screen edges are the tank walls, and the bottom edge is the
    floor.
  - Near the top there is a water surface, with a thin band of air above
    it where bubbles pop and flakes float. ❓ Q2
- **Cell.** A material ID (u8), a variant or colour-noise byte (u8), and
  per-cell state such as age, a timer or moisture (u16). That is about
  4 bytes per cell, or about 1 MB per display.
- **Fields.** Coarse grids at 1/8 resolution hold the values that
  diffuse:
  - nutrients;
  - CO₂;
  - light;
  - water current from the filter outlet;
  - in full ecosystem mode, also ammonia, nitrate, O₂ and temperature.
- **Chunks.** The grid is split into 64×64-cell chunks. Each chunk is
  awake (it has moving cells or pending reactions) or asleep. Awake
  chunks keep a dirty rectangle, in the style of Noita. Only awake
  chunks are simulated and only dirty rectangles are uploaded to the
  GPU.
- **Ticks.** The simulation runs at a fixed 20 Hz (configurable).
  Rendering runs at 30 fps, and creature motion is interpolated between
  ticks.

### 2.2 Materials

Every material has a **movement class**:

| Class | Behaves like |
|---|---|
| solid | doesn't move |
| powder | falls and piles up, with an angle of repose |
| liquid | falls and spreads sideways |
| buoyant | rises; "reverse falling sand" (bubbles) |
| floating | stays at the surface for a while, then sinks |

Materials also carry these properties:

- **density**, which decides who displaces whom: a bubble rises because
  it is lighter than water;
- **friction and dispersion**;
- **decay** into another material after some time;
- **interactions with fields**: a material can consume from or emit into
  them.

A pairwise **reaction table** sets what happens when two materials
touch: A next to B becomes C with probability p per tick, optionally
needing a field threshold.

The v1 palette (D2):

| Group | Materials |
|---|---|
| Water and air | water (the medium, the "empty" cell), air (above the surface) |
| Substrates | sand, gravel, aquasoil (holds nutrients), rock (solid), driftwood (solid) |
| Food | flakes (float, then sink), sinking pellets, bloodworms (sink fast), algae wafers (for bottom feeders) |
| Fertilisers | root tabs (sit in the substrate and feed rooted plants), liquid fertiliser (diffuses into the nutrient field), CO₂ (bubbles that dissolve into the CO₂ field and boost plant growth) |
| Life | seeds and cuttings (become plants), moss (grows on rock and wood), algae (nuisance; grows with excess light and nutrients), eggs (one material per breeding species, which hatches into fry) |
| Gases | air bubbles (from the bubbler, plants and creatures), CO₂ bubbles |
| Decay | waste, dead plant matter and corpses, which sink and become **detritus**; detritus slowly disintegrates into nutrients |
| Chaos *(hidden behind a setting, off by default)* | acid (dissolves substrate and plants), magma (boils water into steam bubbles and cools into rock), dye (tints the water), salt (harms freshwater life), blight (spreads through plants) |

Plants are structures of cells: roots, stems, leaves. A plant grows by
converting nearby water cells while it has nutrients, light and CO₂.
Without them it wilts and drops dead plant matter.

### 2.3 Ecosystem (D3)

- **Simple mode** (the default):
  - Waste and detritus become nutrients.
  - Plants consume nutrients and light to grow.
  - Excess nutrients plus light grow algae.
  - A single **water quality** score falls with waste and rises with
    plants and water changes. "Water change" is a tray action.
- **Full mode** (optional):
  - a nitrogen cycle: waste becomes ammonia, bacteria colonies on the
    substrate and filter turn it into nitrite and then nitrate, and
    plants absorb ammonia and nitrate;
  - O₂ from plants by day, the bubbler and the surface;
  - CO₂ from respiration and injection;
  - a heater temperature setting.

  Each species has tolerances for these values.

### 2.4 Creatures

- **Species are data** (D8). A TOML entry plus a sprite sheet defines:
  - size;
  - zone: surface, mid-water, bottom or surfaces;
  - temperament: schooling, peaceful or territorial;
  - diet: which food and which materials it grazes, such as algae and
    detritus;
  - lifespan and maturity age;
  - breeding: egg layer (and which egg material) or livebearer;
  - tolerances;
  - colour variants.

  v1 ships about 8 real freshwater species across fish, shrimp, snails
  and a frog. Examples of the kind we mean: a schooling tetra, a
  livebearer such as a guppy, a betta, a corydoras, cherry and amano-type
  shrimp, a nerite-type and a ramshorn-type snail, and a dwarf frog that
  surfaces to breathe. Fictional species are a stretch goal.
- **Behaviour** is steering-based:
  - wander;
  - schooling for schoolers;
  - zone preference;
  - bottom-walking for corydoras and shrimp;
  - surface-grazing for snails on glass, rock and plants;
  - surfacing for frogs;
  - finding and eating food cells, and grazing algae and detritus (the
    clean-up crew);
  - avoiding the cursor (optional, D8);
  - resting at night.

  Creatures collide with solid cells and with the substrate surface.
- **Life cycle.** Egg, then fry, juvenile, adult and elder. Growth
  depends on feeding.
  - **Breeding** needs mature, well-fed creatures and suitable
    conditions. It slows as the tank gets crowded.
  - **Offspring** take a parent's colour variant, with a small chance of
    mutation. ❓ Q3
  - A **population cap** per display (default 60 creatures,
    configurable) protects the budget.
- **Death** (D4) can come from old age, starvation, poor water quality,
  wrong temperature (full mode), predation (larger fish eat fry and
  shrimp) or chaos materials.
  - **Easy mode** limits deaths to old age.
  - A dead creature becomes a sinking corpse sprite, which turns into
    detritus cells and disintegrates.
- **Names** (D4): none by default. In interact mode, click a creature to
  see its card (species, age, health) and give it a name.

### 2.5 Interaction (D6)

- **Interact mode** turns on while a modifier key is held (default ⌥ on
  macOS and Alt elsewhere, configurable), or with a tray toggle. While
  it is on:
  - the tank accepts clicks over its whole area, in wallpaper and
    overlay modes alike;
  - a small pixel-art palette appears in a screen corner;
  - Esc or releasing the key (for the modifier variant) returns to
    click-through.
- **Tools:**
  - **pour** the selected material: left-drag, with the scroll wheel
    setting brush size;
  - **scoop**: erase cells with right-drag;
  - **net**: pick up and move a creature;
  - **inspect**: show a creature's card and name it.

  The tray menu also has quick actions: Feed (default food), Water
  change, and Pause.
- On Wayland, global key state isn't readable. There, the CLI
  `desktank interact toggle` talks to the running app over IPC, so you
  can bind it to a key in the compositor (§4.3).

### 2.6 Time and persistence

- **World time advances only while a tank is being simulated**: while it
  is visible, or while the saver is running. Hidden or closed time
  doesn't count.
- **Optional catch-up** (D5, off by default): on resume, hidden or
  offline time up to 24 hours is applied as a summarised step. Creatures
  age, eat from what's available and may breed or die. Plants grow, and
  detritus decays. This is a coarse, cheap model, not a per-cell replay.
- **Day and night** follow local time.
- **Persistence.** Each tank (one per display, or one spanning tank) is
  saved to the platform data dir every 5 minutes and on exit, and the
  format is versioned. The save holds:
  - the grid, run-length encoded and compressed;
  - the fields;
  - the creatures and their names.

  Wallpaper, overlay and screensaver show the **same tank** (D10). While
  the app runs they share one simulation. Saver host processes load the
  latest save, simulate locally and write nothing back, so there is only
  ever one writer.
- **Multi-monitor** (configurable): separate tanks per display (the
  default), or one spanning tank where creatures and materials cross
  between adjacent screen edges (stretch goal).

## 3. Display modes per platform

### 3.1 Summary of feasibility

| | macOS | Windows | Linux (X11) | Linux (Wayland) |
|---|---|---|---|---|
| Wallpaper | ✅ desktop-level window | ✅ WorkerW child window 🔍 | ✅ `_NET_WM_WINDOW_TYPE_DESKTOP` | ✅ layer-shell `background` on wlroots/KDE; ⚠️ GNOME experimental/none 🔍 |
| Overlay | ✅ | ✅ (compositing path 🔍) | ✅ needs a running compositor | ✅ layer-shell `overlay` (same GNOME caveat) |
| Screensaver | ✅ `.saver` bundle ⚠️ host bugs 🔍 | ✅ `.scr` | ✅ built-in idle mode; ⚠️ XScreenSaver hack only where XScreenSaver is the saver | ✅ built-in idle mode |
| Lock screen | ⚠️ saver shown while locked | ⚠️ static snapshot image only 🔍 | ⚠️ via saver | ⚠️ `ext-session-lock-v1` (stretch) |
| Interact (modifier key) | ✅ modifier polling 🔍 | ✅ | ✅ | ⚠️ tray toggle or compositor key binding to the CLI |

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

### 3.3 Overlay and interact input

The overlay is transparent, click-through, always on top, takes no focus
and has no taskbar or Dock entry. By default it is excluded from screen
capture, using `NSWindow.sharingType = .none` on macOS and
`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on Windows. This is
configurable, because some people want to show their tank off.

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
  - Click-through relies on `WS_EX_LAYERED | WS_EX_TRANSPARENT`.
  - Fallback path: render offscreen, read back, and present with
    `UpdateLayeredWindow` at a lower frame rate.
  - The O1-Win spike picks one path (§9).
- **Linux X11**: an ARGB visual plus an empty XShape input region. This
  needs a running compositing manager (`_NET_WM_CM_S<n>` owned).
  Without one the mode reports "needs a compositor" and does not start.
- **Linux Wayland**: a layer-shell `overlay` layer with an empty input
  region.
- **Full-screen apps**: the overlay and wallpaper pause and hide while a
  full-screen app is in front (§6.2 has detection details).
- **Interact input** (§2.5). Entering interact mode makes the
  overlay-type window accept mouse input. It shows that window over the
  desktop if only wallpaper mode is on. Leaving interact mode makes it
  click-through again.
  - **macOS**: toggle `ignoresMouseEvents`. The modifier is detected by
    polling `CGEventSourceFlagsState` at 10 Hz, only while a tank is
    visible. 🔍 We need to confirm this needs neither Accessibility nor
    Input Monitoring permission.
  - **Windows**: toggle `WS_EX_TRANSPARENT`; poll the modifier with
    `GetAsyncKeyState`.
  - **X11**: set or clear the XShape input region; poll with
    `XQueryKeymap`.
  - **Wayland**: set the input region. No modifier detection; the tray
    toggle or CLI is used instead.

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
  - Until signing exists (R2), the saver is ad-hoc signed and for
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
  given by `XSCREENSAVER_WINDOW` / `-window-id`.
- **Lock screen** (accepted, §11):
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
    lock client must leave the session locked and recoverable. It is
    deferred, and is an escalation item.

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
assets/              # materials/*.toml, reactions.toml, species/*.toml, sprites (PNG atlases + source files)
```

Dependency direction: `sim ← render ← platform ← {desktank,
desktank-saver}`, and `sim ← render ← desktank-saver-mac`. The saver
binary reuses `platform` for its windows and input detection.

`desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. If
wgpu surface creation from raw handles needs `unsafe`, it moves into
`platform`. All other `unsafe` lives in `platform` and
`desktank-saver-mac`, at FFI boundaries, with `// SAFETY:` comments.

Assets are embedded in the binary (`include_bytes!`). A user override
directory for custom materials and species is a stretch goal.

### 4.2 Runtime model

- **Loop-agnostic core.** `desktank-render` exposes a `Tank` with
  `tick(now) -> FrameRequest` and `draw(surface)`. Whatever loop owns the
  surface calls them:
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
  checkerboard pattern on a small rayon pool, but only in that case.
- **Rendering**: each tick, the dirty rectangles of the cell grid are
  copied into an `R8G8` texture (material, variant). A fragment shader
  maps material and variant to a palette colour, then applies lighting:
  quantised pixel-art caustics, day/night, and plant shadow. Creatures
  are instanced sprites from an atlas, snapped to the cell grid. Water
  is opaque in wallpaper mode and transparent or tinted in overlay mode.
- **Frame pacing**: no continuous redraw. The loop sleeps until the next
  frame deadline at the target fps (`ControlFlow::WaitUntil`, or a
  calloop timer), and a frame is only requested when a surface is
  visible. When every surface is hidden, the loop waits on OS events
  only (§6.2).
- **Screensaver hosts** are separate processes started by the OS, each
  with its own wgpu device and its own budget (§6.1). They read the
  latest save and write nothing back (§2.6).

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
  before O2). There is no settings window in v1 (§11).
- **CLI** (`clap`):
  - `desktank run [--wallpaper] [--overlay] [--idle]`;
  - `desktank window` (a normal resizable window, for development and
    for users who want a desk-toy window);
  - `desktank interact on|off|toggle`, `desktank feed [FOOD]` and
    `desktank pause` (sent over IPC to the running app);
  - `desktank capture --seed N --time HH:MM --size WxH --frames K --script FILE --out DIR`
    (headless offscreen renders; scripted pours make the output
    reproducible);
  - `desktank bench`;
  - `desktank saver install|uninstall`.
- **IPC**: a local socket (a Unix domain socket, or a named pipe on
  Windows), user-only permissions, and a tiny line-based command
  protocol.
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
| GPU | `wgpu` 30 | Metal, DX12 and Vulkan; offscreen rendering for capture and tests |
| Windowing | `winit` 0.30 (stable) | 0.31 is in beta, so Dependabot ignores winit majors until we choose to upgrade |
| Wayland | `smithay-client-toolkit` 0.21 + `calloop` | Layer-shell and idle-notify |
| macOS FFI | `objc2` 0.6 + `objc2-app-kit` / `objc2-screen-saver` | |
| Windows FFI | `windows` 0.62 | |
| Tray | `tray-icon` 0.26 (macOS/Windows), `ksni` (Linux) | |
| Math / RNG | `glam` 0.34, `rand_pcg` (seeded) | |
| Images | `png` (decoding the sprite atlas) | Atlases are built from source art; see ❓ Q1 |
| Save format | `serde` + `postcard`, compressed with `lz4_flex` (pure Rust) | Versioned |
| IPC | `interprocess` (local sockets) | |
| CLI / config | `clap` 4, `serde` + `toml` 1, `directories` 6, `notify` | |
| Errors / logs | `thiserror` 2 (libraries), `anyhow` (binaries only), `tracing` + `tracing-appender` | |
| Supply chain | `cargo deny` (licences, advisories, duplicates) | |
| Release | `cargo-dist` for archives, MSI and shell installers; zx scripts (pnpm) build `.app`, `.saver` and `.dmg` | |

## 6. Resource budget and power policy

### 6.1 Budgets

Reference machine: M1 MacBook Air, one 2560×1440 display at the default
4× cell scale (640×360 cells), with about 30 creatures. Windows and
Linux numbers are recorded and tracked, but not gated in v1.

| Metric | Main app (1 display) | Per extra display | Each saver host process |
|---|---|---|---|
| CPU while visible, settled tank (no pouring) | ≤ 3% of one core, aiming for ≤ 1.5% | +1.5% | ≤ 3% |
| CPU while interacting (pouring) | ≤ 15% of one core | — | n/a |
| Simulation tick time: settled / heavy pour | ≤ 0.5 ms / ≤ 5 ms | — | same |
| CPU while hidden or paused | ≤ 0.1% | — | n/a (exits) |
| Wake-ups while hidden | ≤ 1/s (OS events only, plus polls ≤ every 10 s) | — | n/a |
| GPU frame time at 1440p | ≤ 2 ms | +2 ms | ≤ 2 ms per display |
| Resident memory | ≤ 80 MB, aiming for ≤ 50 MB | +10 MB | ≤ 80 MB, with no growth over 8 h |
| Release binary size (stripped, assets embedded) | ≤ 20 MB | — | ≤ 20 MB |
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

- 30 fps and a 20 Hz tick on AC power. On battery, 15 fps and a 10 Hz
  tick. Rendering pauses in Low Power Mode or battery saver, and below
  20% battery.
- Rendering and simulation pause when the surface is occluded or
  minimised, the display is asleep, or the session is locked (except for
  the saver). World time stops too (§2.6).
- Wallpaper and overlay pause while a full-screen app is in front.

| Signal | macOS | Windows | Linux |
|---|---|---|---|
| Occluded / hidden | `NSWindow` occlusion-state notification | `WM_SIZE` minimise; DWM cloaked; per-frame present status | Wayland frame callbacks stop; X11 `VisibilityNotify` |
| AC / battery / level | `IOPSNotificationCreateRunLoopSource` | `RegisterPowerSettingNotification` (`GUID_ACDC_POWER_SOURCE`, battery %) | UPower over D-Bus |
| Low power / saver | `NSProcessInfoPowerStateDidChange` | Energy saver status power setting 🔍 | UPower / power-profiles-daemon |
| Display sleep / lock | `NSWorkspace` screens sleep/wake; `com.apple.screenIsLocked` | `WTSRegisterSessionNotification`; display-state power setting | logind `Lock`/`Unlock`, `PrepareForSleep` |
| Full-screen app in front | Active-Space change + `CGWindowListCopyWindowInfo` bounds check (no window titles, so no Screen Recording permission) 🔍 | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` → `SHQueryUserNotificationState` | X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_STATE_FULLSCREEN`; Wayland: `wlr-foreign-toplevel-management` where available, otherwise not detected |
| Interact modifier | `CGEventSourceFlagsState` polled at 10 Hz while visible 🔍 | `GetAsyncKeyState` polled at 10 Hz while visible | X11 `XQueryKeymap` polled at 10 Hz while visible; Wayland: none (IPC/tray) |

Everything is event-driven except the idle polls (at most every 10 s)
and the modifier poll (10 Hz, only while a tank is visible and interact
by modifier is enabled).

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
- **Render tests**: offscreen captures compared with reference images.
  They pass when ≤ 0.5% of pixels differ by more than ΔE 2. They run in
  Linux CI on Mesa lavapipe (software Vulkan), so CI needs no GPU.
  Reference images are updated deliberately with a `--bless` flag.
- **Benchmarks**: `criterion` benches for the simulation tick (settled,
  heavy pour, 60 creatures). CI records the numbers; it doesn't gate on
  them.
- **Platform smoke tests** on each OS runner: start each available mode
  for a few seconds where the runner allows, then exit cleanly.
- **CI** runs on macOS, Windows and Ubuntu: fmt, clippy, test, doc and
  `cargo deny`. Dependabot covers cargo and actions.
- **Manual acceptance** for the visual and OS modes uses captures
  attached to PRs on the `pr-assets` branch, per the Developer brief.

## 8. Distribution

- **Open source** (§11): the repository becomes public before the first
  release. The code is MIT OR Apache-2.0. Art and data assets carry a
  licence chosen in ❓ Q1.
- **GitHub Releases**:
  - **`cargo-dist`** builds the archives, the Windows MSI (which also
    installs the `.scr` per-machine) and the shell and PowerShell
    installers.
  - **zx scripts** build `DeskTank.app` (universal binary), assemble
    `DeskTank.saver`, and package both in a `.dmg`.
  - Linux ships a tarball now, with AppImage and AUR later.
- **Signing**: Developer ID signing and notarization on macOS, and
  Authenticode on Windows. Both cost money and need owner accounts.
  ❓ Q4. Until then, builds are unsigned and for developers.
- **Package managers later**:
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
in that PR. "Within budget" means §6.1, measured with `desktank bench`
on the reference machine and reported in the PR.

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
    DX12;
  - whether `CGEventSourceFlagsState` needs any permission.

  *Accept:* each question answered yes or no, with evidence (a doc link
  or a minimal test), and the affected 🔍 marks resolved in this spec.

### M1 — A living tank in a window

- [ ] **T1 Grid, chunks and movement.** A cell grid with 64×64 chunks,
  sleep and wake, and dirty rectangles. All five movement classes and
  density displacement. Materials load from TOML (water, air, sand,
  gravel, rock, air bubble). Fixed tick and seeded RNG. *Accept:*
  - same seed and script give the same hash after 10⁴ ticks;
  - non-reacting mass is conserved;
  - a settled scene has 0 awake chunks within 5 s;
  - bubbles reach the surface;
  - sand piles at its angle of repose;
  - tick time on a heavy-pour bench ≤ 5 ms and on a settled scene
    ≤ 0.5 ms.
- [ ] **T2 Renderer, `desktank window` and minimal `desktank bench`.**
  Grid texture with dirty-rectangle uploads, a palette shader with
  integer scaling, frame pacing, and a pause when occluded. *Accept:*
  within the main-app budget in a window; hidden-state CPU ≤ 0.1%.
- [ ] **T3 Capture.** `desktank capture` with input scripts, and render
  tests on lavapipe in CI at the §7 tolerance. *Accept:* at least 2
  reference images in CI, and a deliberate shader change fails the test.
- [ ] **T4 Window-mode interaction.** The palette, and the pour, scoop,
  net and inspect tools, with brush size and a cursor preview. *Accept:*
  every tool works with mouse and trackpad, and pouring stays within the
  interacting budget.
- [ ] **T5 Creatures.** Species TOML, the sprite atlas pipeline with
  placeholder art (❓ Q1), steering (wander, schooling, zones,
  bottom-walking, surface-grazing, surfacing), collision with cells,
  eating food cells, and optional cursor avoidance. *Accept:*
  - no creature enters a solid cell in 10⁵ ticks;
  - schoolers' mean nearest-neighbour distance stays ≤ 3 body lengths
    for ≥ 80% of the time;
  - each species spends ≥ 70% of its time in its zone;
  - food is found and eaten within 10 s of simulated time;
  - a GIF is in the PR.
- [ ] **T6 Plants, food, fertiliser and reactions.** The reaction table;
  plant growth (seeds and cuttings); moss and algae; the food types;
  fertilisers (root tabs, liquid, CO₂ bubbles); and the decay chain
  (waste and dead matter become detritus, then nutrients). *Accept:* a
  test for each reaction; plants grow when fed and lit and wilt without;
  detritus fully disintegrates within its configured time.
- [ ] **T7 Simple ecosystem and day/night.** Nutrient and light fields, a
  water-quality score, the water-change action, and lighting tied to
  local time. *Accept:* a scripted scenario showing overfeeding leading
  to an algae bloom and a water change bringing it back, as tests plus a
  GIF.
- [ ] **T8 Life cycle.** Eggs and livebearing, growth stages, breeding
  rules, colour inheritance, every death cause, easy mode, corpses that
  turn into detritus, the population cap, and names on the inspect card.
  *Accept:* a test for each cause of death; easy mode allows only
  old-age deaths; the population never exceeds the cap; a named creature
  keeps its name across save and load.
- [ ] **T9 Persistence and catch-up.** A versioned save format, autosave
  and save on exit, and optional catch-up (off by default, capped at
  24 h). *Accept:*
  - save and load round-trip;
  - a test with a v1-format save fixture;
  - catch-up for 24 h runs in ≤ 200 ms and keeps the population and
    nutrients within plausible bounds;
  - a corrupt save is moved aside, and the app starts with a fresh tank
    plus a logged error.
- [ ] **T10 Cross-OS measurement.** `desktank bench` on Windows and
  Linux, with measurement recipes for all three OSes in PROGRESS.
  *Accept:* numbers recorded for all three OSes.

### M2 — Power, overlay and control

- [ ] **O3 Power policy and event sources** (§6.2), before any
  always-on mode ships. *Accept:*
  - on battery the frame rate drops to 15 fps and the tick to 10 Hz;
  - Low Power Mode, lock and display sleep pause rendering and world
    time;
  - hidden-state wake-ups stay within budget;
  - all verified on macOS, with Windows and Linux in their O1 PRs.
- [ ] **O1-Mac / O1-Win / O1-Linux Overlay mode**, one PR per OS. The
  water is transparent or tinted and the substrate rests on the bottom
  edge. Each is accepted when:
  - clicks reach the windows underneath;
  - the overlay never takes focus;
  - it has no Dock or taskbar entry;
  - it is visible on every Space or virtual desktop;
  - it is hidden while a full-screen app is in front;
  - it is excluded from screen capture by default;
  - it is within budget.

  O1-Win starts with the composition-path spike (§3.3). O1-Linux covers
  X11 with a compositor and Wayland layer-shell.
- [ ] **O5 Global interact mode.** The modifier key, the tray toggle, and
  the IPC CLI (`interact`, `feed`, `pause`), with switching between
  click-through and accepting input on each OS (§3.3). *Accept:*
  - holding the modifier enables pouring over the overlay and the
    wallpaper;
  - releasing it or pressing Esc restores click-through within 100 ms;
  - Wayland works through the tray and CLI;
  - no OS permission prompt appears (per P1).
- [ ] **O2 Tray + config.** The tray menu (§4.3), TOML config with hot
  reload, and start at login. *Accept:* every menu item works on all
  three OSes, a config edit applies within 1 s without a restart, an
  invalid config logs an error and keeps the last good config, and start
  at login survives a reboot.
- [ ] **O4 Multi-monitor.** Separate tanks per display (each persisted),
  with hot-plug and per-display scale factors. *Accept:* plugging or
  unplugging a display, or changing its scale, adds, removes or rescales
  its tank within 2 s, with no restart, leak or data loss.

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
  install` (per-user). It loads the latest save read-only. *Accept:* each
  argument works from the Screen Saver dialog or the registry entry, the
  preview renders inside the dialog, and the saver exits on input.
- [ ] **S2 macOS `.saver`.** objc2 `ScreenSaverView`, a Metal layer, the
  leak mitigations, read-only save loading, and `desktank saver install`
  (developer-only until R2). *Accept:*
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

### M6 — Depth

- [ ] **E1 Full ecosystem mode.** Nitrogen cycle, bacteria colonies, O₂,
  CO₂ and heater temperature; species tolerances; a config switch.
  *Accept:*
  - a cycling scenario test: a new tank shows ammonia, then nitrite,
    then nitrate peaking in order;
  - species outside their tolerances decline;
  - simple mode is unchanged.
- [ ] **E2 Chaos materials** behind a setting that is off by default
  (§2.2). *Accept:* a test for each chaos reaction; nothing chaotic is
  reachable from the palette or saves while the setting is off; turning
  it off removes chaos materials from the palette.
- [ ] **E3 Optional sound.** Bubbles, pouring and the filter hum, off by
  default. Audio is only active while the tank is visible. *Accept:*
  within budget with sound on; audio is fully released when hidden.

### M7 — Ship it

- [ ] **R1 Release pipeline.** *Accept:* a tag produces, through CI, a
  GitHub Release with the `.dmg` (`.app` + `.saver`), the MSI (with
  `.scr`) and a Linux tarball, and each installs and runs on a clean
  machine.
- [ ] **R2 Signing and notarization** (needs owner accounts, ❓ Q4).
  *Accept:* the `.dmg` passes `spctl --assess`, and the MSI and `.scr`
  are Authenticode-signed with no SmartScreen "unknown publisher"
  warning, given the certificate type.
- [ ] **R3 Package managers.** *Accept:* `brew install --cask`, `winget
  install` and an AUR package each install a working release.
- [ ] **R4 Go public.** README, CONTRIBUTING, issue templates, and
  licence headers for assets. Make the repo public. *Accept:* the owner
  approves, and the repo is public.

### Stretch goals (unscheduled)

- Open windows act as solid obstacles, so sand piles on title bars and
  creatures swim around windows (D7).
- One spanning tank across monitors (D9).
- Fictional species (D8).
- A user directory for custom materials and species.
- A settings window.

### Suggested order

1. F1, then P1 and T1 in parallel.
2. T2, T3.
3. T4, T5, T6.
4. T7, T8, T9.
5. O3, then O1-Mac, then O5, then O2.
6. W1, then S3, then S2, then the rest.

Within each mode macOS comes first, then Windows, then Linux (§11). M6 can run alongside M3 and M4.

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Falling-sand cost on an always-on app | Budget blown during activity or by a "busy" tank | Sleeping chunks, a 20/10 Hz tick, the population cap, profiling in T1; parallel chunks only if needed |
| Pixel art supply | No art, or inconsistent art | ❓ Q1; placeholder generator from day one; a consistent palette and sprite-size spec |
| Pet sim feels punishing | Users come back to dead tanks | Easy mode; world time only while visible; catch-up off by default; slow, gentle death curves |
| Global modifier detection needs permissions | Permission prompts, or the feature fails | P1 verification; the tray toggle as the always-available path |
| Windows 24H2+ WorkerW changes | Wallpaper broken on new Windows | Test matrix and snapshot fallback; study Lively's behaviour (GPL, so no code copying) |
| macOS `legacyScreenSaver` leaks and regressions | Saver eats RAM | Mitigations, soak test, and built-in idle mode as a fallback |
| GNOME lacks layer-shell | No wallpaper or overlay on GNOME Wayland | Document it; X11 session works; a GNOME Shell extension is a possible later option |
| Transparent overlays on Windows with wgpu | Alpha may not compose | P1 question plus the O1-Win spike; `UpdateLayeredWindow` fallback |
| Lock-screen API needs package identity | L2 not possible unpackaged | Decided in P1: MSIX packaging, or drop L2 |
| Unsigned builds | Gatekeeper and SmartScreen block users | Developer-only until R2; ❓ Q4 |
| Save corruption or schema drift | A lost pet tank | Versioned format, atomic writes, the last 3 saves kept, fixture tests |

## 11. Decision log

| # | Decision (2026-11-17) |
|---|---|
| D1 | Art: **pixel art**. Cell scale configurable, default 4×. |
| D2 | Materials: several kinds of food and fertiliser, plus CO₂. **Chaos materials** are hidden behind a setting, off by default. |
| D3 | Ecosystem: **simple by default**, full as an option. |
| D4 | **Pet sim.** Death for any reasonable cause; **easy mode** limits deaths to old age. Names are optional and user-given. |
| D5 | Offline or hidden **catch-up**: optional, **off by default**. |
| D6 | **Interact** via a modifier key **and** a tray toggle. |
| D7 | **Screen edges are tank edges.** Windows as solid objects is a stretch goal. |
| D8 | **Real freshwater** fish, shrimp, snails and frogs. Fictional species are a stretch goal. Cursor reaction yes; sound optional. |
| D9 | **Sandbox**, no progression for now. Multi-monitor layout configurable (separate by default; spanning is a stretch goal). |
| D10 | **One shared tank** across modes. |
| — | Platform order macOS → Windows → Linux, with no Linux desktop preference. Lock-screen limits accepted. **Open source.** Tray + config only to start. §6.1 budgets accepted. Name to be confirmed. |

## 12. Open questions

1. **Art source and licence.**
   - Who makes the pixel art: you, a commissioned artist, or
     programmer-art placeholders from me first?
   - Which asset licence: CC BY 4.0, CC BY-SA 4.0, or the code's
     licence?
   - Any reference style, such as sprite size or palette?
2. **Water surface.** A visible surface line with an air gap at the top
   (default: about 4% of the screen height), or water all the way to the
   top edge?
3. **Colour inheritance and mutation.** Is it wanted, or should all
   offspring look like their species' default?
4. **Signing.** Will you pay for an Apple Developer ID ($99/yr) and a
   Windows code-signing certificate before the first public release?
5. **Name and identity.** "DeskTank" is TBC. Do you want an icon or logo
   direction?
