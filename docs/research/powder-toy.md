# Research: The Powder Toy (design inspiration)

Inspected 2026-11-17 with REA + Hopper on the locally installed macOS build
(`The Powder Toy.app/Contents/MacOS/powder`, x86_64, SHA-256
`379d43c6…7f05f`). The Powder Toy is open source under the GPL. **DeskTank
uses it for ideas only: no code, element tables, names or art are copied.**
We reimplement everything from scratch under our own licence.

Each finding is labelled:

- **Observed**: seen in the binary (symbol names, decompiled constructors).
- **Inferred**: our reading of what that means.

## Architecture

- **One particle per cell on a fixed grid** (observed). Element `update`
  callbacks are typed against a particle map `int pmap[384][612]` in their
  signatures, and the decoration layer is a `PlaneAdapter<array<u32,
  241536>, 629, 384>`. Inferred: the playfield is roughly 612×384 cells
  plus a border.
- **Data-driven element table with optional behaviour callbacks**
  (observed).
  - Each `Element_XXXX()` constructor fills a property struct: an
    identifier string such as `DEFAULT_PT_SAND`, a colour (SAND
    `0xFFD090`, SEED `0x88E788`), numeric physics coefficients, flags and
    a description string.
  - Most elements stop there: SAND sets no callbacks.
  - Elements with special behaviour set `update`, `create` and
    `graphics` function pointers. SEED sets all three.
  - Inferred: the generic engine handles powder, liquid and gas motion
    from the coefficients, and callbacks handle reactions such as
    growing, burning, dissolving and decaying.
- **Separate field layers on coarser grids** (observed: `Air::update_air`,
  `Air::update_airh`, `Air::ApproximateBlockAirMaps`). There is an air
  pressure and velocity field, and an air-heat field, that particles
  interact with. Walls are a separate layer (`CreateWalls`,
  `CreateWallLine`).
- **Newtonian gravity on a worker thread** (observed: `GravityImpl::Init`
  spawns a `std::thread`, `Gravity::Exchange(GravityOutput&,
  GravityInput&)`). It is a double-buffered field computed off the main
  thread, toggled by `EnableNewtonianGravity`.
- **Core particle operations** (observed): `PlanMove` and `do_move`
  (movement and displacement), `kill_part`, `delete_part`, `flood_water`,
  `RecalcFreeParticles` (a free-list of particle slots), `BeforeSim`, and
  `Snapshot`/`Restore` (undo). `Save` and `Load` work on a `GameSave`, and
  the `.cps`/`.stm` document types are registered in Info.plist.
- **Painting** (observed): `CreateParts`, `CreateLine` and `CreateBox`
  take a `Brush`. Tools implement `DrawLine`, `DrawRect`, `DrawFill` and
  `Click`. A separate **decoration** colour layer has point, line and
  fill operations.
- **Scripting** (observed): LuaJIT is embedded, with Lua bindings for
  gravity, air mode, ambient temperature and more. Inferred: custom
  elements can be scripted.

## Inventory

- **About 220 elements** (observed constructors). They fall into roughly
  these families, grouped from their names and description strings:
  - **powders**: sand, dust, salt, sawdust, snow, clay dust, seeds;
  - **liquids**: water and its variants (salt, distilled, carbonated),
    oil, acid, lava, mercury, soap;
  - **gases**: steam, smoke, CO₂, O₂, H₂, fog;
  - **solids**: stone, glass, metals, wood, plant, vine, sponge, ice;
  - **life**: plant ("drinks water and grows"), vine ("grows along
    wood"), seed ("put on sand and add water to grow a tree"), yeast
    ("grows when warm"), virus ("turns everything it touches into
    virus"), soap ("creates bubbles… cures virus");
  - **reactions**: fire, explosives, electricity, sensors, radiation.
- **11 simulation tools** (observed `SimTool::Tool_*`): heat, cool, air
  (push), vacuum, positive and negative gravity, mix, cycle, ambient heat
  ±, and wind.

## Ideas for DeskTank

| TPT mechanism | DeskTank use |
|---|---|
| Grid of cells, one material per cell | The tank is a pixel grid at a fraction of screen resolution. Fits the pixel-art style. |
| Generic powder/liquid/gas motion from properties | Substrates (gravel, sand, soil) fall and pile up. Bubbles are a light "gas" that rises ("reverse falling sand"). |
| Optional per-material callbacks | Plants grow, fertiliser feeds them, eggs hatch, detritus decays. |
| Decay transitions | Dead fish and plants sink, then break down into detritus, then nutrients. |
| Brush + click tools | Click to pour substrate, fertiliser, eggs or food. |
| Coarse field layers | Water currents (filter outflow) and a nutrient/oxygen field on a coarse grid, not per cell. |
| Off-thread field solve | Not needed at our scale; keep everything on one thread for low resource use. |

Resource caution: TPT updates every particle every frame, which is fine
for a foreground toy but not for an always-on desktop app. DeskTank needs
sleeping/dirty-chunk updates, so settled substrate costs nothing, and a
simulation tick below the render rate.
