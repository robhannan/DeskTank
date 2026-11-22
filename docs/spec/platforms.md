# Display Modes per Platform

### 3.1 Summary of feasibility

| | macOS | Windows | Linux (X11) | Linux (Wayland) |
|---|---|---|---|---|
| Wallpaper | ✅ desktop-level window | ✅ WorkerW child window 🔍 | ✅ `_NET_WM_WINDOW_TYPE_DESKTOP` | ✅ layer-shell `background` on wlroots/KDE; ⚠️ GNOME experimental/none 🔍 |
| Overlay | ✅ | ✅ (compositing path 🔍) | ✅ needs a running compositor | ✅ layer-shell `overlay` (same GNOME caveat) |
| Screensaver | ✅ `.saver` bundle ⚠️ host bugs 🔍 | ✅ `.scr` | ✅ built-in idle mode; ⚠️ XScreenSaver hack only where XScreenSaver is the saver | ✅ built-in idle mode |
| Lock screen | ⚠️ saver shown while locked | ⚠️ static snapshot image only 🔍 | ⚠️ via saver | ⚠️ `ext-session-lock-v1` (stretch) |
| Interact (modifier key) | ✅ modifier polling 🔍 | ✅ | ✅ | ⚠️ tray toggle or compositor key binding to the CLI |

Android (§3.6, D23): screensaver ✅ as a Dream (⚠️ Google TV has no
screensaver picker: the Dream is chosen over ADB); wallpaper ✅ as a live
wallpaper on phones and tablets; window ✅ as an app; overlay and lock
screen ❌.

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
capture on a best-effort basis, using `NSWindow.sharingType = .none` on
macOS and `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on Windows.
🔍 Newer macOS capture paths may ignore `sharingType`; O1-Mac verifies
this and documents what is actually excluded. This is
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
  - Click-through relies on `WS_EX_LAYERED | WS_EX_TRANSPARENT`. 🔍 It's
    unverified whether this combination stays click-through with
    `WS_EX_NOREDIRECTIONBITMAP` and DirectComposition (P1, O1-Win).
  - Fallback path: render offscreen, read back, and present with
    `UpdateLayeredWindow` at a lower frame rate.
  - The O1-Win spike picks one path (O1-Win, §9).
- **Linux X11**: an ARGB visual plus an empty XShape input region. This
  needs a running compositing manager (`_NET_WM_CM_S<n>` owned).
  Without one the mode reports "needs a compositor" and does not start.
- **Linux Wayland**: a layer-shell `overlay` layer with an empty input
  region.
- **Full-screen apps**: the overlay and wallpaper pause and hide while a
  full-screen app is in front (§6.2 has detection details).
- **Interact input** (§2.5). Entering interact mode makes the
  overlay-type window accept mouse input. In toggle mode it also takes
  key focus for Esc and for typing creature names. It shows that window
  over the desktop, with the tank drawn in it, if only wallpaper mode is
  on. Leaving interact mode makes it click-through again.
  - **macOS**: toggle `ignoresMouseEvents`. The chord is detected by
    polling `CGEventSourceFlagsState` at 10 Hz, only while a tank is
    visible. It reads modifier flags only, never other keys. 🔍 We need
    to confirm this needs neither Accessibility nor Input Monitoring
    permission (P1). In toggle mode, Esc arrives as a normal key event,
    because the window is focused.
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
  - The saver is ad-hoc signed only, with no Developer ID (D14), so
    users clear quarantine by hand, following the README (R2).
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

### 3.5 Web (D15)

The web version is a full product target: the same simulation, renderer
and save format running in a browser tab. It is the natural home for
mobile too. It has no wallpaper, overlay, screensaver or lock-screen
modes; it is a "window mode" in a page.

- **Host**: `desktank-web` (wasm-bindgen) owns the page lifecycle and
  drives the loop-agnostic `Tank` (§4.2) from `requestAnimationFrame`,
  with a fixed 20 Hz simulation tick.
- **Backend selection**: request a WebGPU adapter; if there is none, or
  device creation fails, create a WebGL2 context instead. A `?backend=`
  URL parameter (`webgpu` or `webgl2`) forces one for testing.
- **Renderer limits**: from T2 on, `desktank-render` stays within wgpu's
  WebGL2 downlevel limits:
  - no compute shaders on the critical path;
  - no storage buffers in fragment shaders;
  - the grid texture is `Rg8Unorm`, read with `textureLoad`, never
    filtered;
  - textures stay within the downlevel maximum dimension (2048): at
    integer scale ≥ 2 a grid for a display up to 4096 px wide fits.

  T2 adds the wasm32 CI build that enforces this.
- **Device loss**: WebGL context loss or WebGPU device loss recreates the
  renderer from the simulation state, which is unaffected.
- **Grid size and scale**: the cell scale is an even whole number (2–8,
  as in §2.1; D19), or 1 when 2 doesn't fit. On the web it is chosen per
  device as the largest even scale that gives at least 320 cells across
  the viewport, and can be overridden in settings (1, 2, 4, 6 or 8). The
  grid size is fixed when a tank is created.
  - A tank opened later in a different-sized viewport, or imported from
    another device, keeps its own grid size. It is drawn at the largest
    even scale that fits (1 if 2 doesn't), letterboxed.
- **Input**: pointer and touch events. The page is always interactive,
  with no click-through, so the interact tools (§2.5) are simply
  available. Touch maps:
  - drag to pour;
  - two-finger drag to scoop;
  - long-press to inspect.

  The canvas sets `touch-action: none` and suppresses the context menu,
  so browser pinch, scroll and long-press defaults don't interfere. The
  rest of the page stays normally zoomable.
- **Controls**: an in-page menu replaces the tray, with the same actions
  as §4.3. The web has no config file, so a small in-page settings panel
  (saved to `localStorage`) is how config is edited there. This is part
  of D15. Native v1 still has no settings window (§4.3).
- **Persistence**: the same envelope and payload as §2.6, stored in
  IndexedDB.
  - A browser tab has no reliable exit hook. So the page saves on
    `visibilitychange` (hidden) and `pagehide` (best effort), and
    autosaves every 5 minutes as on native.
  - It calls `navigator.storage.persist()` and offers export/import of a
    save file.
  - 🔍 Safari may still evict script-written storage for sites not added
    to the Home Screen (the 7-day cap). Installing the PWA (WB4) avoids
    this, and the page shows a "last saved / export" reminder.
- **Ownership**: the Web Locks API (`navigator.locks`) ensures only one
  tab simulates and writes. Another tab shows "open in another tab" and
  offers **Take over**. That sends a `BroadcastChannel` message asking
  the owner to save and release the lock, mirroring the saver handover
  in §2.6.
- **Time and power**:
  - Hidden tabs pause through the Page Visibility API, so world time
    stops (§2.6). Optional catch-up works as on native.
  - Visible tabs get rAF at the display refresh rate. The host draws
    only when a frame is due, capped at 30 fps, and skips the rest. The
    simulation only runs when a tick is due.
  - Where the Battery Status API exists (Chromium) the host drops to
    15 fps on battery. Elsewhere it stays at 30 fps.
- **Sound** (E3): WebAudio, started only after a user gesture, as
  browsers require. On iOS the ring/silent switch can mute it.
- **Offline / install** (WB4): a web app manifest and service worker,
  with all paths relative (`start_url` and `scope` of `./`) so they work
  under the GitHub Pages project subpath `/DeskTank/`. The service
  worker cache is versioned by build hash and old caches are deleted on
  activate.
- **Legacy-Chromium (webOS) variant** (D26): the same page built for
  Chrome 68, the WebAppManager of LG webOS 5 TVs (OLED CX; Mali-G51, WebGL2,
  1920×1080 CSS px at devicePixelRatio 2), so DeskTank can later run as
  the TV's screensaver. `pnpm build:web:webos` writes `target/site-webos/`:
  - **wasm**: MVP only. Our code is compiled with `-C target-cpu=mvp`;
    binaryen lowers what the precompiled std still holds (sign-ext, bulk
    memory, saturating float-to-int) and strips the `target_features`
    section, so wasm-bindgen emits glue without an externref table; the
    final `wasm-opt -Oz` runs with `--mvp-features` (no multivalue).
  - **JS**: the page, the service worker and wasm-bindgen's glue lowered
    to Chrome 68 syntax by esbuild; the page's TypeScript is also
    type-checked against ES2018's library. DOM APIs newer than Chrome 68
    are avoided or feature-detected by hand.
  - **Pixels**: `<html data-build="webos">` caps the canvas at 1 device
    pixel per CSS pixel (a 1080p backing store, not 4K); `?dpr=` overrides
    it on any build.
  - **Diagnostics**: startup steps are named; a step stalled for 10 s is
    reported in the status line, and `?debug=1` lists the steps, visibility
    and errors on screen, for a browser with no developer tools at hand.
  - **Graphics**: Chrome 68's GPU stack can't compile the one-pass grid
    shader (its 2018 SwiftShader allows 4096 temporary registers per
    fragment shader, and the TV showed a black tank), so the webOS build
    draws with the *split grid*: the ground first, then leafy and other
    plant parts in two blended passes, and the sprites in two passes split
    by the plant part under each pixel, each shader reaching only its own
    code. It draws the same picture (the render tests check every scene
    both ways). Its canvas is non-sRGB, which WebGL2 presents with a blit
    rather than wgpu's sRGB present shader (which draws nothing on that
    SwiftShader). `?grid=split|single` and `?srgb=1|0` override both on
    any build, and a GPU error (such as a rejected shader) is shown on
    screen rather than leaving the tank black.
  - CI builds it, checks it (`scripts/check-webos.ts`: MVP wasm, Chrome 68
    syntax), loads it in the web tests and runs it in a real Chromium 68
    (`scripts/check-chrome68.ts`). It stays within the §6.1 budget.

### 3.6 Android (D23)

Android is a native target, led by the screensaver: Android TV first,
phones and tablets second. It uses the same simulation, renderer and save
format as the desktop and the web, through a thin Kotlin shim.

| Mode | Android host | Package |
|---|---|---|
| Screensaver | A `DreamService` ("Dream": the screensaver on Android TV, and on phones while charging or docked) | A1 |
| Window | An Activity: the tank with touch interaction and an in-app menu and settings | A3 |
| Wallpaper | A `WallpaperService` live wallpaper (phones and tablets) | A4 |

There is no overlay (it would need `SYSTEM_ALERT_WINDOW`, and an always-on
layer over other apps would break the budget) and no lock-screen mode
(Android has no third-party lock-screen surface; a Dream already shows
over the keyguard while charging).

- **Host**: a Gradle project in `android/`, written in Kotlin and kept as
  thin as possible: one class per mode, no UI framework beyond the
  platform's (the settings screen may use AndroidX Preference). Each class
  owns a `SurfaceView` (or the wallpaper's `SurfaceHolder`) and forwards
  its lifecycle over JNI to `desktank-android`, a Rust `cdylib`
  (`libdesktank_android.so`):
  - `surfaceCreated` / `surfaceChanged` / `surfaceDestroyed`, plus
    `onDreamingStarted` / `onDreamingStopped`, the Activity's
    `onResume` / `onPause` and the wallpaper's `onVisibilityChanged`;
  - touch events in the app (as §3.5's touch mapping).

  `desktank-android` turns the `Surface` into an `ANativeWindow`
  (`ndk::NativeWindow::from_surface`), makes a wgpu surface from its
  raw window handle, and drives the loop-agnostic `Tank` (§4.2) on a
  render thread it owns, paced by the frame pacer: no `Choreographer`
  callback runs while nothing is due. Kotlin holds no tank state.
- **Backend selection**: Vulkan where the device offers a conformant
  Vulkan 1.1 driver, otherwise OpenGL ES 3.0 through wgpu's GL backend
  (EGL). The renderer already stays within WebGL2 limits (§3.5), which
  GLES 3.0 meets. A setting forces one backend for testing, and a Vulkan
  device that is lost or fails to create falls back to GLES. Android
  requires Vulkan 1.1 only of 64-bit devices, and most Google TV devices
  run a 32-bit userspace (below), so Vulkan can't be assumed there.
  *A0:* on the Android TV and Google TV emulators wgpu gets Vulkan
  (SwiftShader) and, forced, GLES 3.0; both draw. On the reference TV
  device (Chromecast with Google TV 4K, 32-bit userspace, Mali-G31) wgpu
  gets Vulkan 1.3 and draws with it, so its Vulkan driver is usable;
  forced GLES 3.2 draws too. On an NVIDIA SHIELD (64-bit, Tegra X1+)
  Vulkan draws but forced GLES fails to open the device ("Parent device
  is lost"). *A1:* its EGL gives a desktop GL 3.3 context, on which
  wgpu-core's indirect-draw validation can't build its compute pipeline,
  so A1 must still make the fallback work there (PROGRESS).
- **Builds**: `cargo-ndk` builds the `cdylib` for `arm64-v8a`,
  `armeabi-v7a` (many TV devices run a 32-bit userspace on a 64-bit CPU,
  including Chromecast with Google TV and the Google TV Streamer; wgpu
  builds for it) and `x86_64` (the emulator); Gradle packages it, and
  `./gradlew assembleDebug` runs `cargo-ndk` itself. `minSdk` is API 26
  (Android 8): every Google TV device ships Android 10 or later, and
  older Android TV boxes are out of scope (A0). *A0:* the reference TV
  device runs only 32-bit code (`armeabi-v7a`) and the app runs there as
  `armeabi-v7a`, so that ABI stays in every build.
- **Grid and scale**: an even scale from 2 to 8 (§2.1, D19), chosen from
  the display's size: at 1080p the default 4× (D1) gives 480×270 cells,
  and at 4K the same tank is drawn at 8×, so a tank looks the same on any
  TV. Phones use the web's
  rule (§3.5). A tank keeps its grid size when the display changes; it is
  drawn at the largest even scale that fits, letterboxed. The grid and
  scale come from the Dream's surface, not the panel: *A0:* both TV
  devices tried have 4K panels but, with their UI at 1080p as found, give
  the Dream a 1920×1080 surface.
- **Screensaver** (the Dream):
  - full screen, non-interactive (`isInteractive = false`), so any key or
    touch ends it, as a screensaver should; no tools, inspect card or
    cheats (D20);
  - it starts drawing on `onDreamingStarted` and stops, saves and
    releases the surface on `onDreamingStopped`;
  - to protect OLED and plasma TVs from burn-in by the static bed and
    glass edges, the picture shifts by a few pixels every few minutes,
    within the letterbox margin or by a crop of at most one cell;
  - its `android:settingsActivity` meta-data opens the app's settings
    (A3).
- **Choosing the screensaver**: plain Android TV and phones list the
  Dream in Settings (Display → Screen saver). Google TV devices (Android
  TV 12 and later: Chromecast with Google TV, Google TV Streamer, and
  Google TV sets) hide third-party screensavers from that picker, or
  have none. There,
  the user grants `WRITE_SECURE_SETTINGS` once over ADB, after which the
  app's settings can set `screensaver_components` itself; the README and
  the settings screen give the one-line command. A launcher with a
  screensaver override is the other route, though a 2025 Google TV update
  blocked one such launcher (Projectivy). *A0:* on the Google TV emulator
  image (Android 16) `adb shell settings put secure screensaver_components`
  selects the Dream and it runs. On the reference TV device (Chromecast
  with Google TV 4K, Android 14, a 2025 build) there is no screensaver
  picker at all (`android.settings.DREAM_SETTINGS` doesn't resolve), and
  the same `settings put` selects the Dream: the screensaver timeout
  starts it, a remote key ends it, and the setting survives a reboot.
  Setting it from an app granted `WRITE_SECURE_SETTINGS` is untested
  (A3). Turning on network ADB there takes Wireless debugging (pairing
  with a code), and the "Stay awake" developer option stops the
  screensaver from starting; A1's README says both (PROGRESS).
- **Persistence**: one shared tank across the modes (D10): the same
  envelope and payload as §2.6, through `SaveStore` in the app's private
  files directory (`Context.getFilesDir()`, passed in over JNI). Every
  host runs in the one app process, so at most one surface owns the tank
  at a time; when another becomes visible it takes over as in §2.6's
  saver handover. Saves happen when a surface stops or is destroyed, and
  every 5 minutes. The app offers export and import of a save file
  (Storage Access Framework), so a tank moves between Android, the web
  and the desktop.
- **Time and power** (§6.2):
  - world time runs only while a surface is visible (§2.6); optional
    catch-up works as on native;
  - 30 fps cap; 15 fps on battery and paused in battery saver
    (`BatteryManager`, `PowerManager.isPowerSaveMode`); a TV is always
    on mains power, and a phone Dream runs only while charging;
  - nothing runs while no surface is visible: the render thread parks and
    no timers are left set.
- **Interaction** (app only): touch as on the web (§3.5). D-pad control
  of the tools on a TV (a cursor moved by the arrows) is open.
