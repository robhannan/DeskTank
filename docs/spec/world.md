# World, Materials & Ecosystem

### 2.1 World model

- **Grid.** The tank is a grid of cells covering the display. Each cell
  is a configurable number of screen pixels: 4×4 by default (2, 4, 6 or
  8 allowed, D1; even so sprite texels stay square, D19), which gives
  640×360 cells at 2560×1440.
  - The screen edges are the tank walls, and the bottom edge is the
    floor.
  - The **water level** is configurable (D12): default about 96% of the
    screen height, up to completely full. Above the surface is a band of
    air where bubbles pop, flakes float and surface-breathers breathe.
  - Species declare water-depth needs, for example a minimum depth or
    needing surface access to breathe. The tank warns, via the inspect
    card and the log, when the current level doesn't suit a species
    living in it.
- **Cell.** A material ID (u8), a variant or colour-noise byte (u8), and
  per-cell state such as age, a timer, moisture, a rigid cell's body id
  or a plant cell's plant id (u16, §2.7). That is about
  4 bytes per cell, or about 1 MB per display.
- **Fields.** Coarse grids at 1/8 resolution hold the values that
  diffuse:
  - nutrients;
  - CO₂;
  - light;
  - water current from the filters' nozzles ([devices](devices.md));
  - in full ecosystem mode, also ammonia, nitrate, O₂ and temperature.
- **Chunks.** The grid is split into 64×64-cell chunks, and each chunk
  is awake or asleep.
  - A chunk is **awake** if any cell in it moved or changed during the
    last tick, or something wakes it: a pour, a creature or reaction
    editing a cell, or a cell changing within reach of its border, which
    wakes the neighbour. A chunk falls asleep after 1 s of world time
    (20 ticks) with no changes.
  - *Implementation (T1):* each change at `(x, y)` adds its **reach**,
    the cells that read it next tick, to the next tick's work rectangle
    of every chunk the reach overlaps, and wakes them. The reach is ±1
    row and ±`reach_x` columns, where `reach_x` is the longest sideways
    look of any material: [`LEVEL_REACH`](#liquid-levelling) (32) when a
    liquid or buoyant material disperses, otherwise the longest powder
    slide. 32 ≤ half a chunk, so a change still reaches only the direct
    neighbours. A tick updates only the cells in each chunk's work
    rectangle; an awake chunk with an empty one costs nothing.
  - Awake chunks keep a dirty rectangle, in the style of Noita. Only
    awake chunks run the movement pass, and only dirty rectangles are
    uploaded to the GPU. *(T1: `World::dirty_rects()` gives the cells
    that changed in the last tick, separate from the work rectangle.)*
  - Movement per tick is bounded. A cell moves at most 1 cell
    vertically per tick, and a liquid spreads at most its `dispersion`
    (≤ 8 cells) sideways. So a change can only affect the chunk it is in
    and its direct neighbours.
    - *T1:* this bounds each cell's **own** move, and so which chunks a
      change wakes. A liquid displaced by
      a mover can be displaced again in the same tick (so a column of
      sand falls through water together, and the water it displaces
      rises the whole column), but it never acts itself twice. A
      per-cell "moved this tick" stamp (a separate `u32` per cell, about
      0.9 MB at 640×360) enforces that. The optional parallel pass must
      account for these chains, for example by keeping each chain in its
      own checkerboard phase.
- **Slow processes don't keep chunks awake.** Plant growth, decay, algae,
  moss and root tabs run as **random ticks**: each tick, a fixed budget
  of cells (default 256 per display) is sampled from the list of
  "living" cells and given a chance to act. A random tick wakes a chunk
  only if it actually changes a cell.
  - Fields update on their own schedule (§2.1 Fields), not the chunks'.
  - Day/night lighting happens only at render time.
  - Creatures are entities, not cells, and wake a chunk only when they
    edit a cell, for example by eating food or leaving waste.
  - A tank with plants and creatures therefore costs the movement pass
    only where something moves, plus the fixed random-tick, field and
    creature budgets.
  - *T5 (`desktank_sim::World`, `world/life.rs`):* living cells (any
    material with a reaction, decay, emit or buffer rule) and device cells
    are kept in per-chunk index lists. After each tick only the chunks'
    changed rectangles are rescanned, so a settled tank pays nothing to keep
    them. The "sample" is a window over the lists that rotates by the
    budget each tick (stratified, not with replacement), so each living cell
    is visited once every ⌈living / budget⌉ ticks and a visit acts for
    `living / budget` ticks of world time (rounded at random so the mean is
    exact). Decay ages and buffer levels live in the cell `state` and are
    written without waking anything; only a material change wakes a chunk.
    Rigid cells' state is their body id (D17), so rigid materials may take
    part in reactions (through the body bookkeeping) but not age or buffer.
    `World::set_random_tick_budget` changes the budget (default 256).
- <a id="fields"></a>**Fields** (*T5*, `desktank_sim::field`). Nutrient and
  CO₂ diffuse with the explicit five-point scheme and closed walls every
  4th tick (0.2 s), with a per-second decay (CO₂ outgassing) and upwind
  advection by the filter current; diffusion constants are in cells²/s
  (≤ 80 for stability), and a field whose largest change falls below 10⁻⁶
  stops stepping until something adds to it. Light is not diffused: each
  field cell gets the light left after the field cells above it (water
  absorbs a fraction per field cell, opaque cells block in proportion to
  how full the field cell is), recounted only where cells changed.
  Constants live in `assets/fields.toml`. The state hash includes the
  nutrient and CO₂ fields.
- **Devices** (*T5*, placeables). The **bubbler** releases a bubble from
  each top cell with water above at a configured rate; to keep the water
  level (a bubble pops into air), each release turns the first air cell
  above it back into water, which leaves a small mound over a running
  bubbler. The **internal filter** ([devices](devices.md), D27; the bare
  **filter outlet** before DV1) drives a static 2D vector current field
  (1/8 resolution, rebuilt only when devices change or an 8×8 block flips
  between open water and solid/air): each nozzle emits a directed jet and
  each intake draws water radially inwards, projected onto an
  incompressible MAC flow with zero normal velocity at tank walls and
  blocked cells so jets deflect around rocks and off the far wall to
  circulate back towards intakes. The current carries floating cells,
  non-water dense liquids and light loose powders (density < 1.5,
  excluding substrate) along both axes with probability `|v| / 20` per
  tick per axis, skipping field blocks with no carryable cells so a
  settled tank scans nothing. Water is the medium and never starts a
  swap. Rising bubbles deflect along the 2D current, rafts and marimo
  follow its mean vector, and creatures drift with it (swimmers more
  than bottom walkers).
- **Movement rules.**
  - The movement pass runs bottom-up for falling materials and top-down
    for buoyant ones. The horizontal sweep direction alternates every
    tick.
  - A cell swaps with the neighbour in its direction of travel if that
    neighbour is lighter (when falling) or heavier (when rising). Equal
    densities never swap.
    - *T1:* only **liquids** are ever displaced. Solids, powders,
      floaters and buoyant cells are not, so each liquid–bubble exchange
      is made once, by the bubble. **Air** above the surface is a very
      light liquid (density 0.0012), so everything falls through it and
      water falls into air trapped below the surface.
  - Powders that are blocked below try the two diagonals in random order.
    How readily they slide is set by the `repose` property; the default
    gives a pile angle of about 35°.
    - *T1 mechanism:* `repose` is an angle in degrees (15–45°). Its mean
      run `k = 1/tan(repose)` (in 1/256 cells) gives a sideways run per
      row, `⌊(y+1)k⌋ − ⌊yk⌋`, which alternates between ⌊k⌋ and ⌈k⌉
      (1 and 2 at 35°). A grain blocked below and on both diagonals steps
      one cell sideways if the row below has a free cell within that run
      along a free path. A pile settles when each row overhangs the one
      below by at least its run, so the mean slope is exactly the repose
      angle, deterministically and on both flanks (measured 34.8–34.9°).
  - Liquids that are blocked below spread sideways.
    - <a id="liquid-levelling"></a>*T1:* after down and the two
      diagonals, a liquid looks sideways along a free path up to
      `LEVEL_REACH` (32) cells for the nearest cell it can fall from, and
      moves up to its `dispersion` towards it. With nowhere to fall it
      stays put, so a surface settles (level to within one cell per 32
      columns) instead of jittering forever. Buoyant materials do the
      same upside down.
  - Floating materials stay at the surface for their `float_time`, then
    become falling.
    - *T1:* while floating, a cell moves at its `float_density` (between
      air and water), so it falls through air and rises through water.
      Each tick it rests, its state counts up; at `float_time` it moves
      as a powder at its `density` and `repose`.
  - Buoyant materials with `pops_into` turn into that material when they
    touch it from below, or the top edge: a bubble pops into air at the
    water surface, or under the lid of a full tank.
  - <a id="rigid-bodies"></a>**Rigid bodies** (D17, T4b). Rigid materials
    (rock, later driftwood) form **bodies**: objects that move as one piece
    and rotate about unstable supports.
    - A body is the set of rigid cells whose state holds its id (1–65535).
      The world keeps a body table (id → material, cells, bounds, glued
      flag) in a `BTreeMap`, so passes over it run in id order. The table
      can be rebuilt from the grid: the id is in each cell's state and the
      glued flag in the low bit of its variant byte, which the renderer
      reads to shade glued bodies.
    - **Making bodies.** Each edit call (`paint`, `fill_rect`, `set_cell`)
      puts the rigid cells it writes into one new body. A stroke (a tool
      drag, a script `[[pour]]` entry with `repeat`, a web drag) continues
      one body with `World::paint_into`. A body being poured into is
      **held still** until two ticks after the last stamp, so a stroke can
      draw a shape and it drops when released.
    - **Outlines.** A rigid cell draws only the quarters of its 4×4 mask
      (`rigid_mask`); a brush stamps its disc at quarter-cell resolution, so
      rock and driftwood come out round. A rigid material with a `shape`
      (`assets/materials/*.toml`) is weathered per dab instead: the disc is
      trimmed along its edge, keeping an untrimmed core of at least a third
      of the radius (so the stone stays about the size painted), and any
      quarter that doesn't reach the core through drawn quarters is
      dropped, so nothing floats free. A cell the trim empties isn't made
      rigid. Dabs of radius 2 or less stay round. `angular` (Seiryu stone) draws each
      dab as a convex polygon of five or six flat faces, taller than wide,
      chipped along its edge; the faces are seeded by the world seed, the
      material and the 16-cell block of the dab, so a drag sweeps one
      polygon (flat faces, sharp corners) and the faces step where it
      crosses into the next block. `layered` (dragon stone) takes a
      world-fixed field of broad teeth with straight flanks and shallow
      steps along the ridge direction (up and a little to the right, as
      the texture's slots and strata run). Where the slots reach the edge
      they open into notches. The trim is lighter beneath each dab, leaving
      a broad foot, and spurs too thin to contain a cell-sized square are
      clipped outside the core. Drawing over a spot again changes nothing.
      The same seed and strokes give the same stone, which saves keep as
      masks and bodies keep through every move.
    - **Depth.** A body occupies one band per four cells of its shorter
      reference dimension, rounded up and capped at eight. The world seed
      and body ID determine its placement. Motion and rotation preserve
      this range. Reshaping or splitting a body recalculates it.
    - **Motion.** Gravity accelerates released bodies. Displaced fluid supplies
      buoyancy and drag. Translation advances one cell at a time within a tick
      to detect obstacles. A body can tip around its support, transfer momentum
      to another free body, or push grains aside on impact. The rotated outline
      preserves the number of cells. Bodies at rest sleep until disturbed.
    - **Splitting.** Overwriting a body's cells (scoop, a replace brush)
      marks it; at the next tick its remaining cells are split into
      4-connected pieces. The piece holding the lowest cell index keeps
      the id; the others get new ids in order of their lowest cell and
      inherit the glued flag. Adding cells never splits a body, so a
      stroke stays one body even where it is only diagonally connected.
    - **Sleep.** A body is checked only if it moved, was created, split,
      released or unglued, or a chunk it overlaps has work this tick. A
      resting body costs a bounds check against its chunks per tick (13
      resting rocks: settled tick 3.5 µs vs 3.4 µs without) and never
      wakes them, so its chunks sleep as usual.
    - The old `solid` class stays for truly static materials (none built
      in yet; unknown IDs behave as solid).
  - **Units:** density is relative to water (1.0); times are in seconds
    of world time; rates are per world second.
  - **Water level** (T1): `WorldConfig::water_level` is a fraction of the
    height from the floor; a new world is water up to it and air above.
- **Ticks and determinism.**
  - The simulation advances in fixed ticks of **1/20 s of world time**.
    Power state never changes the tick length, only how often frames
    are drawn (§6.2).
  - Rendering runs at up to 30 fps, and creature motion is interpolated
    between ticks.
  - The simulation never reads the wall clock. The host passes in the
    world clock (start time + ticks) and the local time of day, and
    tests inject both.
  - Randomness comes from per-chunk and per-entity PCG streams derived
    from `(seed, tick, chunk or entity id)`. So results don't depend on
    update order, which also covers the optional parallel pass (§4.2).
  - Every collection iterated in the simulation has a defined order:
    vectors or `BTreeMap`, never `HashMap` iteration.
  - *T1:* each chunk's stream is a `rand_pcg::Pcg32` seeded from a
    SplitMix64 mix of `(seed, tick, chunk id)`, with the chunk id as the
    stream. The state hash is 64-bit FNV-1a over the grid size, tick and
    every cell (std's hasher is randomly seeded).

### 2.2 Materials

Every material has a **movement class**:

| Class | Behaves like |
|---|---|
| solid | doesn't move |
| rigid | moves as one body with the rest of its pour: sinks through lighter fluids, rises through heavier ones, rests on powders and bodies (§2.1 [Rigid bodies](#rigid-bodies)) |
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

*T5:* the table and the slow processes are `assets/reactions.toml`
(`desktank_sim::ecology`): `[[reaction]]` (A next to B, one of its 4
neighbours, becomes C, and B optionally D, with probability p per world
second, gated by `when = { field, below | above }`, releasing into a field),
`[[decay]]` (after t seconds becomes C, optionally releasing), `[[emit]]`
(adds to a field at a rate, up to a level) and `[[buffer]]` (stores up to a
capacity and trades with a field towards a level). All values are our own.
Placeables are a bubbler and the internal filter: a palette slot places one
per click. The bubbler is a fixed footprint (it drops onto what is below
it) of solid device cells that scoop removes like any other cell, until
DV7; the internal filter is a prefab device ([devices](devices.md), D27),
placed and removed whole. All powders fall
one cell per tick, so "sinks fast" bloodworms differ by density and repose,
not speed.

Build › Ornaments places a loose rigid prefab centred on the pointer. The
preview uses its exact starting footprint and turns red if an occupied cell
would overlap it. The placed ornament has the same collision, finite depth,
settling and sleep rules as hardscape. Scooping any occupied cell removes the
whole body. Ornaments use a rigid material definition in `assets/ornaments/`
with a tight `[ornament]` sprite, colour key and optional
`bubbler_outlets` in local cell coordinates. Opaque pixels carry their
colours through rigid-body rotation and save/load. The volcano's outlets
follow the drawn crater and use the DV7 bubbler's emission rate and rising
bubble behaviour. The stone and pastel castles share a traced sprite with
open arches and use separate colour palettes. Saves store ornaments in the
existing body and cell records.

The v1 palette (D2), by where the palette offers it (§2.5; materials
without a `palette` path, such as air, propagules, eggs and dead plant
matter, aren't offered):

| Palette category | Materials |
|---|---|
| Build › Substrate | sand, gravel, aquasoil (holds nutrients), clown vomit gravel (neon pebbles, same physics as gravel) |
| Build › Hardscape | rock (rigid), dragon stone (rigid, muted khaki clay rock with small ochre patches and dark gouges along one ridge direction, lighter than rock, stepped and notched outline; material `dragon_stone`) and Seiryu stone (rigid, dense grey limestone, roughly pitted and streaked, with softly pale worn faces and thin tan veins, angular faceted outline; material `seiryu_stone`), each behaving exactly like rock and with its own texture and outline (see *Outlines*), driftwood (rigid, starts lighter than water; waterlogs per cell, darkening and sinking over time, and dries in air) |
| Water | water (the medium, the "empty" cell), air bubbles (from the bubbler, plants and creatures), CO₂ bubbles (dissolve into the CO₂ field and boost plant growth); air (above the surface) isn't offered |
| Food › Fish food | flakes (float, then sink), sinking pellets, frozen bloodworms (sink fast; material id `bloodworm`), algae wafers (for bottom feeders), frozen brine shrimp (sinks and rots; material ID 34, `brine_shrimp`); live bloodworms and brine shrimp are offered from Food › Live (§2.4 *Live food*, D25) |
| Care | root tabs (sit in the substrate and feed rooted plants), liquid fertiliser (diffuses into the nutrient field), waste and **detritus** (waste, dead plant matter and corpses sink and become detritus, which slowly disintegrates into nutrients) |
| Plants | plant species (including moss): seeds pour as powder, while cuttings, fragments, runners and buds each place one discrete multi-cell plant per click (§2.7). Algae grows naturally as a surface film (§2.3, D28) |
| Chaos *(hidden behind a setting, off by default)* | acid (dissolves substrate and plants), magma (boils water into steam bubbles and cools into rock), dye (tints the water), salt (harms freshwater life), blight (spreads through plants) |

Plants are structures of cells: roots, stems, leaves. A plant grows by
converting nearby water cells while it has nutrients, light and CO₂.
Without them it wilts and drops dead plant matter. From T6b each plant is
an individual of a data-defined species (D18, §2.7); the T6 rules below
are the first step.

*T6:* `assets/reactions.toml` gained three rule kinds, run on random ticks
like the rest. `[[grow]]`: a cell grows a `makes` cell into a neighbour
that is one of `into`, along given directions, with probability `p` per
second while all its field `needs` hold (a `boost` multiplies `p`, e.g.
CO₂ doubles plant growth), optionally only where the new cell touches one
of `touching` or no other `makes` cell (`alone`), turning itself into
`becomes` and taking `consume` from a field. Each plant cell's `state`
counts its distance from where its part began (stem height, leaf length,
root depth), capped by `max`. `[[wilt]]`: a cell becomes `into` with
probability `p` per second while one of its `unless` needs fails or it
touches none of `attached`. `[[spawn]]`: a rate of attempts per second
per 64 columns, each at a random column's first non-fluid cell; if it is
one of `on` and the water cell above meets `when`, the material appears
there. `when`/`needs`/`unless` take one condition or a
list of up to 4. Materials gained `opacity` (0–1, how much light a cell
blocks; plant tissue 0.3–0.5) for the light field. Seeds and cuttings
are powders that root when resting on substrate; stems, leaves, roots,
and moss are solid; dead plant matter is a powder that becomes
detritus after 120 s. Keep-probabilities `(1 − p)^(1/20)` come from a
Newton-iteration root with plain arithmetic, not `powf`, so every
target computes the same bits.

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
- *T6 (simple mode):*
  - **Plants.** A seed (slow, mean 50 s) or cutting (2 s) resting on
    sand, gravel, aquasoil or clown vomit gravel roots into it and becomes a
    stem base while nutrient > 0.1 and light > 0.1. Stems grow up to 48 cells, leaves a
    row apart up to 3 cells sideways, roots up to 10 cells through the
    substrate; each new cell takes 0.01 nutrient. Stems and leaves wilt
    into dead plant matter at 1% per second while nutrient ≤ 0.02 or
    light ≤ 0.05. *Measured N:* a fed (aquasoil bed) and lit cutting grows
    185–235 cells in 10 simulated minutes (seeds 0–5, 160×120 tank); the
    acceptance test requires N = 150. Under unsuitable rooting conditions,
    propagules using the (legacy) cutting material quietly accumulate rot progress
    in the high byte of their cell state. They become dead plant matter
    after about 7.5 minutes (within one in-tank day, 600 s), then decay to
    detritus and release nutrients as the detritus breaks down. Seeds
    retain their species id in the low byte and do not rot.
  - **Moss** grows over rock and driftwood only (each new cell must touch
    one), needs little (nutrient > 0.05, light > 0.05), and falls off as
    dead plant matter when it touches neither.
  - **Algae** grows as a translucent coat on submerged sand, gravel,
    clown vomit gravel, aquasoil, rock and driftwood, and on the front
    pane, back wall and both side walls (D28). It comes in
    [varieties](#algae-varieties-369) that each have their own hosts,
    conditions, look and grazers. Establishment, creep and thickening
    need adjacent water with the variety's conditions (the generic green
    film: nutrient > 0.6 and light > 0.15). Existing film recedes when
    its variety's conditions fail (for the green film, nutrient ≤ 0.6) or
    its surface dries.
    Aquasoil alone buffers nutrient to 0.4. Healthy plant cells slow growth
    and exclude film at the water-quality plant threshold (20 healthy
    plant cells per 1000 water cells). At that threshold existing film
    recedes twice as fast. Plants with CO₂ to work with compete harder: in
    each field block the plant competition is multiplied by
    1 + `co2_boost` × the block's CO₂ level (capped at full exclusion), so
    a CO₂-rich tank needs fewer plants to hold film back. Growth rates,
    thresholds and `co2_boost` live in `assets/fields.toml` (`[algae]`).
    Film updates sample coarse field blocks and leave material chunks
    asleep. Catch-up visits at most 4096 blocks per five-minute interval,
    in disjoint groups taken in turn, and credits each visited block with
    the time of the groups it stands for, so long catch-ups grow film at the
    live rate (a catch-up shorter than one cycle of groups updates only
    some blocks). Coat on
    solid surfaces attenuates light below in proportion to its density.
    Grazers scrape density off reachable surfaces and nearby glass, as
    well as their diet says for each variety.
    *Coverage* is the fraction of columns with a coated top non-fluid cell.
  - **Water quality** (`World::ecosystem`, 0–1, computed on demand): 1,
    minus up to 0.6 as the mean nutrient over water cells rises from 0.5
    to 1.5, minus up to 0.4 as waste, detritus and dead plant matter reach
    20 cells per 1000 water cells, plus up to 0.2 for plants (20 cells per
    1000); constants in `assets/fields.toml` `[quality]`.
  - **Water change** (`World::water_change`, a tray action; `W` in
    `desktank window`, `[[water_change]]` in scripts): the nutrient and
    CO₂ fields lose 50% and waste, detritus, dead plant matter and uneaten
    sinking food are siphoned out.
  - *Overfeeding, measured:* a 320×180 sand tank (seed 0) given 40 pinches
    of pellets (about 1500) over 10 minutes reaches a mean nutrient of
    0.53 at 20 minutes and 0.80 at 30 minutes. Film covers 48.4% of the
    substrate at 20 minutes and 100% at 30 minutes. Ten minutes after a
    water change, mean nutrient is 0.40 and substrate coverage is 0%.
  - **Day and night** (`desktank_sim::daylight`): the host injects the
    local time of day; it tints the drawn tank (untinted 07:30–19:30, warm
    dawn and dusk ramps, blue moonlight overnight) and nothing else, so
    plants grow at any hour and the hour never wakes a chunk.

#### Algae varieties (#369)

Each coat byte and each glass block holds one variety (a four-bit id) and a
density. The varieties are data: `[[algae.variety]]` entries in
`assets/fields.toml` give each its id, `priority`, `hosts` (substrate,
hardscape, front glass, rear glass: the back wall and both side walls),
the conditions it grows in, its growth and die-back rates, how hard
grazers find it (`resistance`) and its look. The conditions are ranges on
fields the tank already has: the block's nutrient, CO₂ and light, and the
filters' biological maturity (`World::filter_bio_capacity`: 0 with no
filter or a new one, 1 once cycled; filters mature over seven life days).
No new field was added.

- Where a variety's conditions hold in a block it *grows* there; where
  they fail its coat *recedes*; where only its light is below the minimum
  it waits, neither growing nor receding.
- A bare cell takes the highest-priority variety that can grow on its
  host in the adjacent water. A neighbour's variety that can also grow
  there and ranks as high creeps in instead, at the creep rate.
- `hosts` gate where a variety starts. A coated cell or glass block keeps
  its variety and grows on its conditions alone, so a change to a
  variety's hosts applies to new coat only.
- A coated cell keeps its variety. Varieties never overwrite each other:
  a variety only gives way once its own coat has receded to bare.
- Algae is not in the palette; it only spawns naturally.

| Variety (id) | Hosts | Grows when | Real cause (Green Aqua guide) | Grazed by |
| --- | --- | --- | --- | --- |
| Green film (1) | everything | nutrient > 0.6, light > 0.15; lowest priority, so where no other variety fits | General algae from light and ammonia (nitrate and phosphate "not the main culprits" at ordinary levels) | every grazer: snails, pleco and crabs fully, amano shrimp 80%, otocinclus 70%, cherry shrimp 60% |
| Brown diatoms (2) | substrate, hardscape, both panes | nutrient > 0.9, light ≤ 0.68, maturity ≤ 0.5: a young tank with rich water in dim light | New tank (ammonia, silicates from sand, gravel, stone and tap water), or old non-CO₂ tank with weak light; disappears as nitrifying bacteria establish | otocinclus (fully), shrimp, snails, pleco |
| Green spot (3) | hardscape, both panes | light > 0.68, CO₂ ≤ 0.3, nutrient > 0.6: strong light with little CO₂ | Strong light with low phosphate and/or CO₂; "the algae of healthy aquariums"; slow, sticks hard | nerite a little (35%); no one else |
| Green dust (4) | hardscape, both panes (never substrate) | CO₂ ≤ 0.3, nutrient in (0.6, 0.9], light > 0.3, maturity ≤ 0.8: lean water and little CO₂ in a young tank | "Mainly low CO₂ and low nutrients", more often in new tanks; not on plants | almost no one: nerite 15%, pleco 20%, other grazers 10% |

How the guide's chemistry maps onto the existing fields, so the Full mode
nitrogen cycle (above) can replace these stand-ins:

| Guide cause | Simple-mode stand-in | Full-mode field to use |
| --- | --- | --- |
| Ammonia (new tank, disturbed substrate, decay) | filter maturity low, nutrient high (waste and food become nutrient) | ammonia |
| Nitrate | nutrient (the one pool; high enough means plants are not starved) | nitrate |
| Phosphate (deficiency drives green spot) | CO₂ low in strong light | phosphate |
| Silicate (diatoms) | young tank, via diatoms' hosts and maturity | silicate released by new sand, gravel and stone |
| CO₂ (low or uneven drives threads and spots) | the CO₂ field's level in the block | CO₂, with its fluctuation |
| Light | the light field in the block | unchanged |
| Flow (strong favours brush algae, weak favours hair and staghorn) | not used by the films | the 2D current, for the later threads and tufts |

Grazers take each variety at the percentage of a full bite their species
sets (`grazing` in `assets/species/*.toml`, see
[creatures](creatures.md)); a variety a species has no entry for is taken
at 100 − the variety's `resistance`. Zero means the creature ignores it
and does not look for it. A smaller bite feeds that percentage of a full
one, and scrapes that share of a full bite's four density levels rounded
to whole levels, at least one: up to 37% scrapes one, 38% to 62% two.

The brown, green and dusty looks are drawn in `tank.wgsl` from a baked
atlas; see [architecture](architecture.md). The threads, tufts and green-water
varieties of the issue follow in later increments.
