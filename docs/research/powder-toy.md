# Research: The Powder Toy (design inspiration)

Inspected 2026-11-17 with REA + Hopper on the locally installed macOS build
(`The Powder Toy.app/Contents/MacOS/powder`, x86_64, SHA-256
`379d43c6…7f05f`). The Powder Toy is open source under the GPL.

**DeskTank uses it for ideas only.** This note deliberately records
architecture and mechanism categories. It doesn't record identifiers,
element names, property values, in-game text or code. Nothing here is to
be copied into DeskTank; we design our own materials, names and values.

Each finding is labelled:

- **Observed**: seen in the binary through symbol names or decompiled
  structure.
- **Inferred**: our interpretation of what was observed.

## Architecture

- **Grid of cells, one particle per cell.**
  - Observed: per-material update callbacks take a fixed-size 2D
    particle map in their signatures, and the colour-decoration layer is
    a fixed-size plane.
  - Inferred: the playfield is a fixed grid of roughly 600×400 cells.
- **Data-driven material table with optional callbacks.**
  - Observed: each material has a constructor that fills one property
    record: an identifier, a colour, numeric coefficients, flags and a
    description.
  - Observed: some constructors also install update, create and graphics
    callbacks, and others install none.
  - Inferred: a generic engine moves materials from their properties
    alone (powder, liquid or gas behaviour), and callbacks exist only for
    special reactions such as growth, burning or decay.
- **Field layers on coarser grids.**
  - Observed: separate air-pressure/velocity and air-heat update
    routines, plus a block-approximation routine.
  - Observed: a separate wall layer.
  - Inferred: particles read from and write to these fields.
- **Optional gravity field on a worker thread.**
  - Observed: the gravity implementation spawns a thread and exchanges
    double-buffered input and output with the simulation.
- **Core operations.**
  - Observed: routines to plan and perform particle moves, kill and
    delete particles, flood-fill, recalculate free particle slots, run a
    pre-simulation step, and snapshot/restore for undo.
  - Observed: save and load of a game-save object.
  - Inferred: particles are stored in a slot array with a free list.
- **Painting.**
  - Observed: create-particles, line and box operations take a brush.
  - Observed: tools implement click, line, rectangle and fill.
  - Observed: a separate colour-decoration layer has its own point, line
    and fill operations.
- **Scripting.**
  - Observed: an embedded LuaJIT with bindings for simulation settings.
  - Inferred: users can script custom materials.

## Scope (counts only)

- Observed: about 220 material constructors.
- Inferred from their description text: they span powders, liquids,
  gases, solids, living or growing materials (plants, climbing plants,
  seeds, spreading organisms), reactive materials (fire, explosives,
  acids) and electronics.
- Observed: 11 simulation tools that act on the fields or particles:
  heating, cooling, air push and pull, gravity, mixing, ambient
  temperature, and wind.

## Ideas for DeskTank (our design, not theirs)

| Mechanism category | DeskTank use |
|---|---|
| Fixed grid, one material per cell | The tank is a pixel grid at a fraction of screen resolution, which fits the pixel-art style |
| Generic motion from properties | Substrates fall and pile up; bubbles are a buoyant class that rises ("reverse falling sand") |
| Optional per-material behaviour | Plants grow, fertiliser feeds them, eggs hatch, detritus decays |
| Decay transitions | Dead creatures and plants sink, become detritus, then nutrients |
| Brush + click tools | Pour substrate, fertiliser, eggs or food in interact mode |
| Coarse field layers | Nutrients, CO₂, light and water current at 1/8 resolution |
| Off-thread field solve | Not needed at our scale. DeskTank defaults to one thread; a parallel chunk pass is an option only if profiling needs it (SPEC §4.2) |

**Resource caution** (inferred, not measured): the engine appears to
process every live particle each frame. That suits a foreground toy, but
not an always-on desktop app. DeskTank instead uses sleeping chunks with
dirty rectangles, random ticks for slow processes, and a simulation tick
below the render rate (SPEC §2.1).
