# DeskTank — Plan and Spec

- **Roadmap, work packages, follow-ups and open questions** live in [GitHub Issues & Milestones](https://github.com/robhannan/DeskTank/issues) (filtered by `work-package`, `follow-up`, and `decision` labels).
- **Decisions (ADRs)** live in [`docs/decisions/`](../decisions/) (one file per decision; add a new numbered file rather than editing an existing one).
  - [D28 · Algae as a surface coat](../decisions/0028-algae-as-a-surface-coat.md)
  - [D29 · Screensaver camera tour](../decisions/0029-screensaver-camera-tour.md)
  - [D30 · Directional lighting and 2D lightmap](../decisions/0030-directional-lighting-and-lightmap.md)
- **Technical notes & platform learnings** live in [`docs/notes/`](../notes/).
- 🔍 marks technical claims to verify on real hardware, or against primary docs, before the work package that relies on them starts.

## Spec sections

- [World, materials & ecosystem](world.md) (§2.1–2.3)
- [Creatures](creatures.md) (§2.4)
- [Interaction, time & persistence](interaction-and-saves.md) (§2.5–2.6)
- [Plants](plants.md) (§2.7)
- [Devices and prefabs](devices.md) (§2.8: filters, the bubbler, D27)
- [Display modes per platform](platforms.md) (§3: desktop, web, Android)
- [Architecture, budgets, testing, distribution & risks](architecture.md) (§4–8, §10)

---

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

It can run as any combination of four desktop **display modes**, in a browser tab (web), or on Android TV and phones (Android):

| Mode | What the user sees |
|---|---|
| **Wallpaper** | The tank replaces the desktop background, behind the icons. |
| **Overlay** | The same tank drawn over open windows on a transparent, click-through layer. The water is clear (or lightly tinted) and the substrate rests along the bottom of the screen. |
| **Screensaver** | A full-screen tank when the machine is idle. |
| **Lock screen** | The tank on the lock screen, as far as each OS allows (§3.4). |
| **Web** | The same tank, full-page in a browser tab on desktop or mobile, hosted on GitHub Pages (§3.5, D15). |
| **Android** | The same tank as the screensaver (a Dream) on Android TV and phones, then as an app and a live wallpaper (§3.6, D23). |

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
   [`research/powder-toy.md`](../research/powder-toy.md).
5. **Deterministic.** Given a seed and a sequence of inputs, the
   simulation is reproducible, so it is testable and screenshots are
   repeatable.
6. **Honest about platform limits.** Where an OS does not allow a mode,
   we document it and offer the closest alternative.
7. **Inspired by, never copied from.** The Powder Toy (GPL) and other
   falling-sand games are references for ideas only. No code, element
   tables, names or art are taken from them.

### Non-goals (v1)

- A native iOS app. iPhones and iPads are covered by the web version
  (D15). Android has a native host, led by its screensaver (D23).
- 3D rendering. The tank is drawn as a 2D picture; discrete depth bands
  for movement, collision and occlusion are in scope (D21).
- Online features, accounts or telemetry.
- Progression, unlocks or currency: it is a sandbox (D9).
- Plugin or scripting support.
