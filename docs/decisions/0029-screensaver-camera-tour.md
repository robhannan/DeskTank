# D29 · Screensaver camera tour

A screensaver protects OLED and plasma screens from burn-in by never
holding a picture still. The tank's bed, glass edges and letterbox are
static, and a shift of a few pixels every few minutes (the old §3.6 rule)
only spreads their edges. Instead, the screensaver runs a **camera tour**.
The view slowly zooms in on one creature, follows it as it swims, glides on
to another, and every few creatures pulls back to show the whole tank. A
moving view also makes a better screensaver than a fixed one.

**Camera.** `desktank-render` gets an OS-agnostic camera: a view rectangle
in grid coordinates (fractional, so zoom and pan are smooth). The tank and
its sprites draw as they do today into an offscreen target, at the integer
layout the target would have without a camera. A post-pass then resamples
the view rectangle to the screen with a sharp-bilinear (pixel-art) filter,
which keeps texels square without shimmer at fractional zoom. Cards (the
inspect card, the species card) and other HUD draw in screen space after
that pass, so they stay crisp at their usual `card_scale`. With no camera
set, nothing changes: there's no offscreen pass and the output is
byte-identical. The full-tank view is the usual fit, letterboxed. Zoomed
views cover the screen, so the letterbox and the glass edges leave the
picture. The zoom runs from the fit (1×) to about 3×.

**Tour.** A deterministic director (`Tour`, seeded, driven by world tick
time and not wall time, so tests can assert on it) chooses the view:

- **Targets** are live creatures only, picked at random without repeating
  the last few. Plants and devices aren't targets.
- **Focus**: the view centres on the creature at a zoom chosen from its
  sprite size (about 2–3×). It follows the creature through a critically
  damped spring with a time constant of about 2 s, so darting doesn't jerk
  the view, and is clamped inside the tank. A focus lasts 20–40 s.
  It ends early if the creature dies or is netted out.
- **Glides** between views take 6–10 s and ease in and out (smootherstep)
  on both pan and zoom.
- **Pull-back**: after every 3–5 focuses, the tour glides out to the full
  tank and holds there for about 20 s.
- **Empty tank**: with no creatures, the view drifts slowly around a
  1.0–1.15× ellipse, so the picture still moves.

**Tour inspect card.** As an option (**off** by default), the tour shows
the inspect card for the creature in focus, pinned to it as usual. The
card appears once a glide has settled, steps through its pages (live,
then species) every 12 s, and goes at the start of the next glide. In this
mode the card never shows a name prompt or name entry, since a
screensaver can't take text: the **Name** row appears only for a creature
that already has a name, read-only.
This amends §3.6 ("no tools, inspect card or cheats"): the screensaver
still takes no input, and the card shows only when the tour's card setting
is on.

**Hosts and settings.** Every screensaver host runs the tour by default
with a setting to turn it off: the Android Dream now, then the macOS
`.saver`, the Windows `.scr`, the XScreenSaver hack and the built-in idle
mode as they land (the lock screen on macOS is the saver, so it runs the
tour too). The tour card is a second setting, off by default. The web
turns the tour on only by opt-in. Wallpaper and the overlay never tour,
since a camera that moves under a desktop would be distracting.

**Resources.** While the tour moves, it asks for frames at the host's fps
cap and no faster (30 on AC power, §6.2). The offscreen pass costs one
extra full-screen copy per frame, only while touring. While hidden it
asks for nothing, as with sway. Dirty-rect uploads of the grid are
unchanged, but every touring frame redraws the whole target.
