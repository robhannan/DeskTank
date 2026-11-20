# DeskTank — Plan and Spec

Status: **draft v0.5** (2026-11-18).

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

It can run as any combination of four desktop **display modes**, or in a browser tab (web):

| Mode | What the user sees |
|---|---|
| **Wallpaper** | The tank replaces the desktop background, behind the icons. |
| **Overlay** | The same tank drawn over open windows on a transparent, click-through layer. The water is clear (or lightly tinted) and the substrate rests along the bottom of the screen. |
| **Screensaver** | A full-screen tank when the machine is idle. |
| **Lock screen** | The tank on the lock screen, as far as each OS allows (§3.4). |
| **Web** | The same tank, full-page in a browser tab on desktop or mobile, hosted on GitHub Pages (§3.5, D15). |

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

- Native mobile apps. Mobile browsers are covered by the web version
  (D15).
- 3D rendering. The tank is drawn as a 2D picture; discrete depth bands
  for movement, collision and occlusion are in scope (D21).
- Online features, accounts or telemetry.
- Progression, unlocks or currency: it is a sandbox (D9).
- Plugin or scripting support.

## 2. The tank

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
  - water current from the filter outlet;
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
  bubbler. The **filter outlet** makes a jet in a static current field
  (1/8 resolution) pointing away from the nearer side wall, fading over
  `reach` cells and widening as it goes; the current swaps floating cells
  and non-water dense liquids one cell downstream with probability
  speed / 20 per tick. Water is the medium and never starts a swap, so a
  jet through clear water changes no cells and wakes no chunks.
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
Placeables are a bubbler and a filter outlet: a palette slot places one
fixed footprint per click (the bubbler drops onto what is below it), made of
solid device cells that scoop removes like any other cell. All powders fall
one cell per tick, so "sinks fast" bloodworms differ by density and repose,
not speed.

The v1 palette (D2):

| Group | Materials |
|---|---|
| Water and air | water (the medium, the "empty" cell), air (above the surface) |
| Substrates | sand, gravel, aquasoil (holds nutrients), rock (rigid), driftwood (rigid, lighter than water) |
| Food | flakes (float, then sink), sinking pellets, bloodworms (sink fast), algae wafers (for bottom feeders) |
| Fertilisers | root tabs (sit in the substrate and feed rooted plants), liquid fertiliser (diffuses into the nutrient field), CO₂ (bubbles that dissolve into the CO₂ field and boost plant growth) |
| Life | seeds and cuttings of each plant species (become plants, §2.7), moss (grows on rock and wood; a plant species from T6b), algae (nuisance; grows with excess light and nutrients), eggs (one material per breeding species, which hatches into fry) |
| Gases | air bubbles (from the bubbler, plants and creatures), CO₂ bubbles |
| Decay | waste, dead plant matter and corpses, which sink and become **detritus**; detritus slowly disintegrates into nutrients |
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
there (algae spores). `when`/`needs`/`unless` take one condition or a
list of up to 4. Materials gained `opacity` (0–1, how much light a cell
blocks; plant tissue 0.3–0.5) for the light field. Seeds and cuttings
are powders that root when resting on substrate; stems, leaves, roots,
moss and algae are solid; dead plant matter is a powder that becomes
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
    sand, gravel or aquasoil roots into it and becomes a stem base while
    nutrient > 0.1 and light > 0.1. Stems grow up to 48 cells, leaves a
    row apart up to 3 cells sideways, roots up to 10 cells through the
    substrate; each new cell takes 0.01 nutrient. Stems and leaves wilt
    into dead plant matter at 1% per second while nutrient ≤ 0.02 or
    light ≤ 0.05. *Measured N:* a fed (aquasoil bed) and lit cutting grows
    185–235 cells in 10 simulated minutes (seeds 0–5, 160×120 tank); the
    acceptance test requires N = 150.
  - **Moss** grows over rock and driftwood only (each new cell must touch
    one), needs little (nutrient > 0.05, light > 0.05), and falls off as
    dead plant matter when it touches neither.
  - **Algae** spores settle on substrate, rock and wood where the water
    above has nutrient > 0.6 and light > 0.15 (0.2 attempts per second
    per 64 columns); the film creeps over those surfaces under the same
    conditions and dies back (to water) once nutrient ≤ 0.6. Aquasoil
    alone buffers to 0.4, so only excess nutrients grow algae. *Coverage*
    is the fraction of columns whose top non-fluid cell is algae.
  - **Water quality** (`World::ecosystem`, 0–1, computed on demand): 1,
    minus up to 0.6 as the mean nutrient over water cells rises from 0.5
    to 1.5, minus up to 0.4 as waste, detritus and dead plant matter reach
    20 cells per 1000 water cells, plus up to 0.2 for plants (20 cells per
    1000); constants in `assets/fields.toml` `[quality]`.
  - **Water change** (`World::water_change`, a tray action; `W` in
    `desktank window`, `[[water_change]]` in scripts): the nutrient and
    CO₂ fields lose 50% and waste, detritus, dead plant matter and uneaten
    sinking food are siphoned out.
  - *Overfeeding, measured:* a 320×180 sand tank given 40 pinches of
    pellets (about 1500) over 10 minutes reaches a mean nutrient of 0.8
    as the food rots; algae covers 28–35% of the substrate at 20 minutes
    and all of it at 30. A water change halves the nutrients to 0.4 and
    the algae is gone (< 1%) 10 minutes later.
  - **Day and night** (`desktank_sim::daylight`): the host injects the
    local time of day; it tints the drawn tank (untinted 07:30–19:30, warm
    dawn and dusk ramps, blue moonlight overnight) and nothing else, so
    plants grow at any hour and the hour never wakes a chunk.

### 2.4 Creatures

- **Species are data** (D8). A TOML entry plus a sprite sheet defines:
  - size;
  - zone: surface, mid-water, bottom or surfaces;
  - temperament: schooling, peaceful or territorial;
  - diet: which food and which materials it grazes, such as algae and
    detritus, and which plant species (by their palatability, §2.7);
  - lifespan and maturity age;
  - breeding: egg layer (and which egg material) or livebearer;
  - tolerances;
  - colour variants.

  v1 ships about 8 real freshwater species across fish, shrimp, snails
  and a frog. Examples of the kind we mean: a schooling tetra, a
  livebearer such as a guppy, a betta, a corydoras, cherry and amano-type
  shrimp, a nerite-type and a ramshorn-type snail, and a dwarf frog that
  surfaces to breathe. Fictional species are a stretch goal.
  *Shipped so far (11):* neon tetra, betta, corydoras, nerite, cherry
  shrimp (*Neocaridina davidi*), amano shrimp (*Caridina multidentata*),
  Thai micro crab (*Limnopilos naiyanetri*, a surface crawler beyond
  the examples above), honey gourami (*Trichogaster chuna*, an
  air-breather like the betta), guppy (*Poecilia reticulata*, the
  livebearer, with female frames since T9 part 2), harlequin rasbora
  (*Trigonostigma heteromorpha*, a second schooler) and otocinclus (a
  sucker-mouth catfish that crawls surfaces like the nerite). Three species keys extend the list above: `reach`
  (cells past the body it eats from, default 1; the crab's filter-feeding
  fans reach 2), `hop_every` (seconds between a bottom walker's short
  hops off the substrate, for shrimp) and `bands` (the depth bands it
  keeps to, front and back, D21; every band by default). Every species also gives its
  real-world body length, `length_cm` (nose to tail without antennae or
  an outsized fin tail such as a male guppy's; a crab's carapace), and is drawn at a shared 2 cells per cm, so species
  sit at true relative sizes: Thai micro crab 0.5 cm (legs spanning
  1.5 cm), cherry shrimp 2.5 cm, guppy 3 cm (4.5 cm with its tail), neon
  tetra and otocinclus 3.5 cm, amano shrimp 4 cm, harlequin rasbora and
  honey gourami 4.5 cm.
- **Behaviour** is steering-based:
  - wander;
  - schooling for schoolers;
  - zone preference;
  - bottom-walking for corydoras and shrimp;
  - surface-grazing for snails on glass, rock and plants, turned in
    quarter turns so the foot rests on the face they hold (walls,
    overhangs), with the collision box turned too;
  - surfacing for frogs;
  - finding and eating food cells, and grazing algae and detritus (the
    clean-up crew);
  - avoiding the cursor (optional, D8);
  - resting at night.

  Creatures collide with solid cells and with the substrate surface.
  The tank is 2.5D (D21): it is eight **depth bands** deep, front glass
  to back. Plant cells have a band (a plant's stem keeps one, its leaves
  are up to a band off it, a rosette's blades fan up to two bands either
  way); a creature has a depth that eases between bands and collides with
  a plant cell only within one band of it. Rock, substrate, glass, bodies
  and air span every band, and so do food and other loose particles, so a
  creature eats from any band. Blocked by a plant, a swimmer shifts band
  to slip past before it turns away. Species may keep to a range of
  bands (`bands`, every band by default). Creatures and corpses are drawn
  back to front by depth; a plant cell in a band nearer the glass than a
  creature hides it where the plant's shape covers the cell (the
  creature shows through the gaps between leaves), and a plant cell
  level with or behind it is drawn under it. Creatures and plant cells
  fade slightly towards the water's blue and darken with depth, up to
  about a fifth at the back band.
- **Life cycle.** Egg, then fry, juvenile, adult and elder. Growth
  depends on feeding.
  - **Breeding** needs mature, well-fed creatures and suitable
    conditions. It slows as the tank gets crowded.
  - **Colour genetics** (D13). Each species has a wildtype colour and
    optional colour morphs.
    - Offspring inherit their parents' morphs.
    - Crossing different morphs, or breeding without selection, drifts
      the line back towards wildtype over generations. Example: mixed
      neocaridina colours revert to brown.
    - Rare mutations can produce a morph.

    The model is a simple per-species allele table in TOML, not real
    genetics.
  - A **population cap** per display (default 60 creatures,
    configurable) protects the budget.
- **Death** (D4) can come from old age, starvation, poor water quality,
  wrong temperature (full mode), predation (larger fish eat fry and
  shrimp whole: no corpse and no recorded cause) or chaos materials.
  - **Easy mode** limits deaths to old age.
  - A dead creature becomes a sinking corpse sprite, which turns into
    detritus cells and disintegrates.
- *T9 part 1 (age, stages, growth, deaths, corpses, cap):*
  - **Life clock.** Species give `lifespan_days` and `maturity_days` in
    real days, from aquarium figures (neon tetra 1825 and 180, cherry
    shrimp 550 and 75, guppy 730 and 90, …); a day of a creature's life
    passes every `LIFE_DAY_SECONDS` = 600 s of world time, so a neon lives
    about 300 hours of world time (which only passes while the tank is
    shown, §2.6) and a cherry shrimp about 90. Each creature's own lifespan
    is within ±10% of its species', picked from its id and the seed.
  - **Energy and growth.** A creature stores up to 8 hours of energy
    (`ENERGY_RESERVE`, in ticks); it burns a tick's worth each tick, each
    bite of food or grazed cell adds 20 minutes, and at zero it starves.
    It grows towards maturity at full rate while the reserve is at least
    half full, at half rate down to an eighth, and not below that.
  - **Stages.** Fry until a quarter of the way to maturity, juvenile until
    maturity, adult, and elder past 80% of its own lifespan. Fry and
    juveniles are drawn and collide at 35% of the adult size at hatching
    rising to 100% at maturity (each side at least a cell, odd when the
    adult's is); elders are drawn a little greyer. A creature added from
    the palette is an adult just mature and well fed; scripts take
    `age_days`.
  - **Water quality** is sampled once a second (`World::water_quality`):
    the §2.3 score from water and pollutant counts per field cell,
    recounted only where cells changed, not a grid scan. Below 0.5 a
    creature builds stress at up to a second a second (in the foulest
    water) and sheds half a second a second above it; it dies at 300 s,
    so foul water kills in 5 minutes at the earliest.
  - **Corpses** lie belly up (the first frame turned twice), sink at a
    cell a second through anything a creature swims in, and from 60 s
    after death release a detritus cell every 2 s into the water they
    cover, losing texels as they go, until a third of their cells have
    become detritus (a neon tetra makes 7). At most 128 corpses.
  - **Settings** (saved with the tank): easy mode (off by default) and
    the population cap (60 by default, 1–128, never below the creatures
    already there). `desktank window --easy-mode[=false]` and
    `--creature-cap N`; the web has an easy-mode setting.
- *T9 part 2 (sexes, breeding, eggs, colour genetics, predation):*
  - **Species data.** `[breeding]` gives `mode` (`livebearer` or `eggs`),
    `days` (carried, or until a clutch hatches), `brood = [fewest, most]`
    (1–16) and `rest_days`; species without it don't breed in fresh water
    (nerite, amano shrimp, Thai micro crab). `prey_cm` (below `length_cm`)
    makes a predator. `[genetics]` lists `[[genetics.morph]]` entries in
    allele order (append only), each recolouring the sprite by key, and
    names the `wildtype` and the `sprite`'s own morph (the one the palette
    adds); without it a species has one morph, `wild`. `[sprite]` may give
    `female_frames` (same size and count) and `female` colours for a
    species drawn differently by sex. Real figures, broods scaled down for
    a desktop tank: guppy livebearer 28 days, 3–8; cherry shrimp carries
    her eggs 30 days, 3–8; neon tetra and harlequin eggs 1 day, corydoras
    4, betta 2, honey gourami 1, otocinclus 3. Cherry shrimp morphs red
    (palette), wild (brown, the wildtype), blue and yellow; guppy strains
    wild (palette and wildtype), red and blue, and a plainer, deeper,
    short-tailed female. Predators: betta 2.5 cm (fry, cherry shrimp,
    crabs), honey gourami and guppy 1.2 cm (fry, shrimplets, crabs).
  - **Sexes.** Every creature is male or female. One added from the
    palette (or a script or host without a sex) takes whichever sex its
    species has fewer of in the tank, a male on a tie, so adding two makes
    a pair; young are either at even odds.
  - **Breeding** is checked once a world minute. A female conceives when
    she is an adult (mature, not an elder), her energy reserve at least
    half full, unstressed, rested, not in the net, with a mature male of
    her species in the tank and the tank under its cap, with a chance of a
    quarter times the share of the cap still free, so crowding slows it
    and a full tank stops it. A creature added to the tank settles for a
    day of its life first. A livebearer then carries her young for `days`
    and gives birth to a brood of fry around her, cut to the room left
    under the cap; an egg layer lays a clutch at once. Either then rests
    `rest_days`: after the birth, or once a clutch is down (none goes
    down at the clutch limit or with nowhere to lay, and she stays
    ready).
  - **Eggs** are cells of the `egg` material (solid, amber), each holding
    its clutch's id in its state; the clutch (species, both parents'
    alleles, hatch tick, egg cells) is in `World::clutches` (at most 64).
    Eggs go on water cells around the mother that touch glass, rock,
    plants or the bed (not the surface air: no eggs on the waterline), then on whatever lies below her, never under a
    creature. When due, each egg still in place hatches into a fry there,
    or rots into detritus if the tank is at its cap.
  - **Colour genetics** (D13): two morph alleles per creature; it shows a
    morph when both match and the wildtype otherwise. A young takes one
    allele from each parent at random; each mutates into a random
    non-wildtype morph (possibly its own) 5 times in 1000 and reverts to
    the wildtype 10 times in 1000.
    Same-morph lines breed true, two morphs crossed give wildtype young
    (red × blue cherry shrimp give brown), and mixed lines stay mostly
    wildtype, drifting further towards it.
  - **Predation**, once a second, not in easy mode: a hungry adult
    predator whose reach touches a creature no longer than its `prey_cm`
    (its species' length at its size, own young included) eats it with a
    chance of 1 in 4. The prey leaves no corpse (and no death cause); the
    predator is fed (an hour of energy). Neither may be in the net.
  - **Drawing.** Each creature and corpse is drawn in its look (morph and
    sex), two atlas shelves per look: its frames, then its recoloured
    fine frames at four texels per cell.
  - **Save format v3** adds each creature's sex, alleles, pregnancy and
    rest, each corpse's morph and sex, the clutches and every species'
    morph names (checked on load: a species' morphs may only be appended
    to). v2 saves migrate: every creature shows its palette morph, sexes
    alternate within a species in id order, none is carrying, and each
    settles for a day.
  - **Scripts** take `sex` and `morph` in `[[creature]]`; `World` has
    `add_creature_with_traits` (age, sex, alleles) and the debug hook
    `skip_breeding_time`.
- **Names** (D4): none by default. In interact mode, click a creature to
  see its card (species, age, health) and give it a name.
- *T9 part 3 (names and the inspect card):* a name is optional and
  user-given; young are born without one. Names are cleaned the same way
  everywhere (`desktank_sim::clean_name`): control characters dropped,
  whitespace runs collapsed to one space, trimmed, at most 24 characters
  (`MAX_NAME_CHARS`); a blank name removes it. Names are saved (format
  version 6, migrating version 5 creatures unnamed) and hashed only when
  set, and never change behaviour. The card (`World::creature_card`) shows
  the name, species, stage, sex, age in days of its life, health, energy,
  the morph it shows (for species with more than one) and the days a
  carrying female has to go. The window shows the card in its title bar
  and `N` opens a name field there for the creature last inspected; the
  web shows it in its readout with a name field under it (`N` or a click
  focuses it; owner tab only). A drawn in-tank card comes with the
  overlay's interact mode (O5), which has no title bar.

### 2.5 Interaction (D6)

- **Interact mode** has two triggers:
  - **Hold** a modifier chord: by default ⌃⌥ on macOS and Ctrl+Alt on
    Windows and Linux, configurable. A chord avoids stealing ⌥-clicks
    from other apps and Windows' Alt-key menu activation. 🔍 We need to
    check it doesn't clash with AltGr layouts; if it does, the Windows
    default changes. Releasing the chord ends interact mode.
  - **Toggle** with the tray item or `desktank interact toggle`. While
    toggled on, the interact window takes keyboard focus, so **Esc**
    ends it, as does toggling again. Focus returns to the previous app
    on exit.
- While interact mode is on:
  - an input-catching window covers the display. In overlay mode it is
    the overlay. In wallpaper-only mode the tank is temporarily drawn in
    that window, with opaque water, so you can see what you are pouring
    through your other windows;
  - a small pixel-art palette appears in a screen corner. It is grouped
    (Tools, Substrate, Water, Food, Plants, Waste, Devices, Creatures):
    a row of group tabs over the open group's items, with number keys
    picking within the group and a key (`G` / `Shift+G`) or scrolling
    over the palette switching group. A material's group is data
    (`palette = "…"` in its TOML; materials without one aren't
    offered); `desktank_sim::tools::Tools::palette_groups` gives every
    host the same groups.
- **Tools:**
  - **pour** the selected material: left-drag, with the scroll wheel
    setting brush size;
  - **scoop**: erase cells with right-drag;
  - **glue** (D17): click or drag over rigid bodies to glue them in place
    (static) or free them. The first body a stroke touches is toggled and
    sets the way for the rest of the stroke. Glued cells are drawn with a
    subtle amber cast;
  - **net**: pick up and move a creature;
  - **inspect**: show a creature's card and name it.

  The tray menu also has quick actions: Feed (default food), Water
  change, and Pause.
- **Cheats** (D20): the Konami code (↑ ↑ ↓ ↓ ← → ← → B A) opens a small
  text field over the tank. Enter runs the typed line as a cheat and
  closes the field; an unknown or malformed cheat shows a short message
  and leaves the field open; Esc closes it. Cheats are harmless shortcuts
  to what the tools already do: `help`, `feed`, `spawn SPECIES [N]` (N up
  to 10, within the population cap) and `clean` (a water change), plus
  two easter-egg phrases (the whole line, any case): `why not?` adds a
  medical crab and `home owner!` a home owning crab, each within the
  cap. These two crabs are cheat-only: not in the palette, `spawn` or
  stocking, and they never breed. Peaceful bottom walkers, they eat
  everything edible that isn't alive (every food, algae, detritus, dead
  plant matter and corpses, but not live plants, propagules or waste),
  leave a dropping of waste every 4 s (species key `waste_every`; no
  stocked species has one), and the home owner picks a home on the
  substrate soon after it arrives, plants three dwarf hairgrass
  plantlets there once (species key `home_carpet`) and keeps within 12
  cells of it. The home is saved (format version 5). A third phrase,
  `radiation leak`, adds a blinker: a cheat-only 15 cm three-eyed mutant
  fish (30×22 cells) that lives like a goldfish (mid water, peaceful,
  eats every food, algae, detritus, dead plant matter and corpses,
  swallows creatures up to 1.5 cm when hungry, a dropping every 3 s, the
  heaviest bioload) and glows a faint pulsing green at night (species
  key `[glow]`, drawn emissive after the daylight tint). A tank too
  shallow or narrow for its body turns the cheat down. The
  detector and the registry are platform-free
  (`desktank_sim::cheats`); hosts feed key presses (no auto-repeats) and
  draw the field. Where it works:
  - **window** and **web**: the host has keyboard focus, so the code is
    typed as is. The window shows the field in its title bar (as it does
    inspection and naming); the web uses a text input and
    shows the result in the inspect readout. A web viewer tab can't
    cheat.
  - **overlay and wallpaper** (O5): only while interact mode is on, and
    in chord mode only while the chord is held (`Konami::with_chord`);
    letting go starts over. The overlay sees the arrow and letter keys
    only once interact mode gives it key focus, since chord detection
    reads modifier flags only (§3.3); checking that ⌃⌥ + arrows aren't
    taken by the OS (macOS Spaces uses ⌃ + arrows) is part of O5.
  - **screensaver**: no, any key ends it.
  - **lock screen**: never; it must not take typed input beyond the OS's
    own password field.
- On Wayland, global key state isn't readable. There, the CLI
  `desktank interact toggle` talks to the running app over IPC, so you
  can bind it to a key in the compositor (§4.3).

### 2.6 Time and persistence

- **World time advances only while a tank is being simulated**: while it
  is visible, or while the saver is running. Hidden or closed time
  doesn't advance world time.
- **Consequence:** a tank that stays covered by windows all workday
  stands still. That is the price of zero CPU while hidden, and the
  catch-up option exists for it.
- **Optional catch-up** (D5, off by default): on resume, the wall-clock
  time spent hidden or closed, up to 24 hours, is applied as one
  summarised step. Creatures
  age, eat from what's available and may breed or die. Plants grow, and
  detritus decays. This is a coarse, cheap model, not a per-cell replay.
- **Day and night** follow local time.
- **Persistence.** Each tank (one per display, or one spanning tank) is
  saved to the platform data dir every 5 minutes and on exit.
  - **Envelope:** magic bytes, format version, payload length, CRC32,
    then the payload. Each format version has a migration to the next.
  - **Payload:**
    - the world clock and tick count;
    - the RNG seed (streams are derived, §2.1);
    - the material table version;
    - the grid, run-length encoded and then compressed;
    - the fields;
    - the plant table (§2.7: species by name, age, health, stored energy,
      lineage), which the grid alone can't rebuild;
    - the creatures (genetics, age and names included; T9 part 1 adds
      age, growth, energy and stress, the corpses, easy mode and the
      population cap in format version 2, migrating version 1 creatures
      to well-fed adults just mature; T9 part 2 adds sex, morph alleles,
      pregnancy and rest, the egg clutches and the species' morph names in
      version 3; 2.5D depth adds each creature's depth and each corpse's
      band in version 4, D21; a home owning crab's home in version 5,
      migrating version 4 creatures without one; each creature's name in
      version 6, T9 part 3, migrating version 5 creatures unnamed);
    - the ecosystem state.
  - **Writes are atomic**: write to a temporary file, fsync, then rename.
    The last 3 saves are kept.

  Wallpaper, overlay and screensaver show the **same tank** (D10). While
  the app runs, wallpaper and overlay share one simulation.
- **Tank ownership.** (On the web, the same rule applies with tabs instead of processes; see §3.5.) Exactly one process owns a tank at a time: it
  simulates the tank and is the only one that writes its save. Ownership
  is an OS advisory lock on the save file, which is released
  automatically if the process crashes.
  - A **saver host** starting up asks the running app over IPC to hand
    over. The app saves, pauses that tank and releases the lock. The
    saver then takes the lock, simulates, and saves on exit. The app
    takes the lock back and reloads.
  - If the app isn't running, the saver takes the lock directly.
  - If the handover times out (1 s), the saver shows the latest save
    read-only and discards its changes.

  So time and events during the saver are kept, and there is never more
  than one writer.
- **Multi-monitor** (configurable): separate tanks per display (the
  default), or one spanning tank where creatures and materials cross
  between adjacent screen edges (stretch goal).

### 2.7 Plants (D18)

Plants are **individual organisms** of **data-defined species**, not just
materials. Every plant is distinct from every other, even one of the same
species growing into it: it has its own age, health and size, competes
for light and nutrients, and the inspect card shows it. T6 built plants
as materials and rules (§2.2 *T6*, §2.3 *T6*); T6b turns them into this
model, T6c adds the growth forms and T6d the appearance effects. Algae
stays a material film, not individuals.

- **Species are data**, as creatures are (§2.4, D8). One TOML file per
  species in `assets/plants/` defines:
  - name and a one-line description (the inspect card);
  - the **growth form** and its parameters ([below](#growth-forms));
  - **environment preferences**: for light, nutrient and CO₂ a range of
    `min`, `optimum` and `max`. Each factor's response is 0 outside
    min..max, 1 at the optimum and linear in between; the growth rate
    scales with the product of the responses. Also where it can live:
    the substrates it roots in or the surfaces it attaches to, and a
    depth range (for example, must reach the surface, or stays in the
    bottom third). With E1, temperature and nitrate join the ranges;
  - **growth**: base rate (new cells per minute at full response),
    mature size (cells) and height, nutrient cost per cell, and an
    optional lifespan;
  - **hardiness**: how long it tolerates conditions outside its range
    before it drops leaves, and how fast it then declines;
  - **propagation**: any of seeds, cuttings, fragments, runners, plantlets
    and budding (floaters);
  - **appearance**: colour ramps per part and its effects
    ([below](#plant-appearance));
  - **palatability** to grazers (T7) and whether creatures shelter in it.

  v1 ships about 8 real freshwater plants, in the spirit of D8: a fast
  stem plant (hornwort type, can float rootless), a stem plant that
  reddens in strong light (rotala type), a bushy branching stem plant, a
  rosette that spreads by runners (vallisneria or sword type), a carpeting
  plant (dwarf hairgrass type), a floater (duckweed or frogbit type), a
  slow low-light epiphyte on wood or rock (anubias or java fern type) and
  a moss (java moss type, replacing T6's moss material).
- **Plant table.** As for rigid bodies (§2.1 [Rigid bodies](#rigid-bodies)),
  each plant cell's `state` holds its **plant id** (1–65535), and the
  world keeps a plant table in a `BTreeMap` (id → species, age, health,
  stored energy, cell counts per part, base cell, bounds, parent id,
  generation), so passes run in id order.
  - Plant cells use a small set of **generic part materials** shared by
    every species (root, stem, leaf, blade, rhizome, runner, floating
    leaf), so species cost no material ids (u8, §2.1). The species comes
    from the table.
  - Growth only extends a plant from its own cells, and spacing checks
    (T6's `alone`) compare plant ids, so two plants of one species
    growing into each other stay separate.
  - The part-local position T6 keeps in `state` (distance from where a
    part began) moves to the variant byte or is derived from the plant's
    form; T6b fixes the layout.
    *T6b:* the variant byte holds the part-local position in its low 6
    bits (capped at 63) and colour noise in its top 2; a seed or
    cutting's `state` holds the species id it grows into (0 for the
    default, the lowest id). The generic parts so far are root, stem and
    leaf (a mat's cells are stems); T6c adds the rest.
    *T6c (part 1):* a rosette's or carpet's blades are leaf cells (the
    species' `[look] leaf` picks a blade shape), so blades cost no
    material id; `runner` is a new part material, counted with roots in a
    plant's part counts (so the table and saves keep three count slots).
    Rhizomes and floating leaves come with T6c's later parts.
  - The table holds age and health, which the grid can't rebuild, so
    saves store it (§2.6). Ids, part counts and bounds are checked
    against the grid in tests.
  - **Cutting and splitting.** Scooping through a plant splits it as it
    splits a body: at the next tick, the piece still joined to the base
    keeps the id, and each other 4-connected piece becomes a new plant of
    the same species (a fragment, parent id set) if the species
    propagates by fragments and the piece holds a stem or rhizome;
    otherwise it becomes dead plant matter. A fragment drifts or sinks
    like a cutting until it roots.
  - **Health and death.** Health falls while the plant's conditions are
    outside its ranges and recovers inside them. Low health drops the
    oldest leaves first, then the whole plant becomes dead plant matter;
    old age only if the species has a lifespan. Grazing (T7) removes
    cells and costs health.
  - **Cap.** A plant population cap per tank, set from measurement in
    T6b, alongside the creature cap (T9). *T6b:* 2048 plants
    (`MAX_PLANTS`, `World::set_plant_cap`); at the cap seeds and
    cuttings stay unrooted and cut pieces become dead plant matter. A
    settled tank with 528 plants (21,745 cells) ticks in 11.3 µs (M3
    Max), and the plant update visits at most 16 plants a tick, so the
    cap guards the table and the tint texture, not the tick.
- **Cost.** Plant cells are still visited by random ticks within the
  budget (§2.1), and a visit looks up its plant's species. A slow **plant
  update** (each plant about every 5 s of world time, a bounded number of
  plants per tick) turns what its cells sampled into health and stored
  energy and decides growth events: new nodes, leaves, branches,
  runners, seeds. Neither wakes a chunk unless a cell changes. A settled
  tank with 500 plants across every v1 species stays within the settled
  tick budget (0.5 ms, §9 T1).
- <a id="growth-forms"></a>**Growth forms.** A form is a small growth
  program in Rust, parametrised by the species TOML: new species are
  data, new forms are code. Forms are cheap and deterministic, not a
  general L-system.
  - **Stem:** an upright stem with leaves at nodes every `internode`
    cells (opposite or alternate, with length and angle), side shoots
    from nodes with probability `branching` (bushy species), topping out
    at the surface and trailing along it. A rootless variant anchors
    loosely or floats (hornwort type).
  - **Rosette:** a crown at the substrate grows `blades` blades of a
    given length, width (1–2 cells) and lean; spreads by runners.
    *T6c:* `[rosette]` gives `crown` (1–3 cells, growing rightwards from
    where it rooted), `blades` (at most `crown + 2`: above each crown cell
    and beside its ends), `blade_length`, `lean`, `root_depth`, `spacing`
    and `substrates`. Blades are one cell wide; those off the crown's
    centre step a cell outwards at their second cell and every `lean`
    cells after, more often further out (down to every third cell), so
    outer blades arch away. Once every blade has started, a runner leaves
    the root under the crown on each side, creeps through the top two
    substrate layers (following the bed up and down) and, `spacing` or
    more cells out where water is above, roots a child (crown above, root
    at the tip; parent id set, generation + 1); it gives up at twice
    `spacing`. A plant at its mature size still grows its runners.
    `[growth] root_feeder` makes only roots and runners sample nutrients.
  - **Carpet:** low tufts; runners creep through the top substrate layer
    and sprout a new tuft every `spacing` cells. *T6c:* `[carpet]` is a
    rosette with a one-cell crown and blades of at most 3 cells; each tuft
    is its own plant.
  - **Floater:** leaves in the top water row under the air band, with
    roots hanging `root_length` cells; buds new plants alongside, drifts
    with the current and shades what is below it through the light field.
    Whether floaters hold to the surface by a rule or as buoyant bodies is
    settled in T6c.
  - **Epiphyte and mat:** attaches to rock, driftwood or glued bodies by
    a creeping rhizome with leaves (anubias type) or as a mat (moss), and
    falls off when nothing holds it. Whether a plant on a moving body
    moves with it or is torn off is settled in T6c.
  - **Propagation:** runners and buds make child plants (parent id set);
    mature plants of seeding species drop seeds now and then; the palette
    pours a species' seeds, cuttings or fragments (one it lists; a
    poured fragment is the `cutting` material); fragments also come from
    cuts. *T6c:* a species listing `runners` (rosettes and carpets) spreads
    by them, and the palette's runners pour a plantlet (the `cutting`
    material).
- <a id="plant-appearance"></a>**Appearance.** Effects are computed at
  render time where possible, so a change in a plant's look costs no
  chunk wakes and no cell rewrites.
  - Each species has a **colour ramp per part** (about 6 shades) in a
    species palette texture.
  - **Per cell** (the variant byte): a shade from position and age, so
    new growth is lighter, older leaves darker and tips differ from
    bases, plus noise.
  - **Per plant** (a plant texture with one texel per plant id, written
    by the plant update): health tints the whole plant, from vigorous to
    pale (nutrient shortage), yellowing, then brown and translucent as it
    dies; and species colouration, such as reddening in strong light
    with low nutrients (rotala type).
  - **Night:** leaves can fold or dim by time of day (render only, like
    the day/night tint).
  - **Pearling:** under strong light and high CO₂ a healthy plant
    releases small oxygen bubbles from leaf cells (a sim effect using the
    air-bubble material, at a capped rate).
  - **Sway:** leaves and blades shift by a cell in the current or a
    gentle wave. It is render-only but needs frames when nothing else
    changes, which §6 otherwise avoids. ❓ Default: off, a setting,
    capped at 10 fps, and off on battery, in the saver and while hidden.
  - **Mechanism:** the cell texture widens from `Rg8Unorm` (material,
    variant) to `Rgba8Unorm` (material, variant, state), so the shader
    reads a plant cell's id and looks up its plant texel (a 256×256
    texture covers every id) and its species ramp. This stays within the
    WebGL2 limits (§3.5). It doubles the bytes of each dirty-rectangle
    upload; 🔍 T6b measures that against the §6.1 budgets. *T6b:*
    `desktank bench` (2560×1440, M3 Max) spends 2.94 ms of CPU per
    frame while pouring against 2.86 ms with `Rg8Unorm` (+3%), GPU time
    unchanged, RSS +0.7 MB; within §6.1. The plant texture uploads only
    the rows whose tint changed. (The species ramps and the
    position/age shades are T6d; T6b draws the material palette tinted
    by health.)
    *T6d:* the plant texel's alpha holds the plant's species id, and a
    species texture (32×256, one row per species id) holds each part's
    ramp resampled to 8 shades, the part shapes and the sizes (stem
    height, root depth, leaf length). A plant cell is drawn at two
    texels per cell side (as sprites, D19): the shader reads its
    same-plant neighbours (which side a leaf grows from, its tip, the
    stem node it hangs from, a stem's nodes and tip, a root's links, a
    mat's water-facing sides) and the species' shape decides which
    texels are covered and their ramp shade; uncovered texels show the
    neighbouring non-plant cell. Shades rise with height on the stem
    (stem position / species height), towards leaf tips and on leaf
    tops, plus the noise bits. Per-cell age isn't stored, so height
    stands in for it (new growth is at the top).
- **Interaction.** The palette gets one plants slot with a species picker
  (the palette strip is short, §9 T4) in `desktank window` and on the
  web, and a propagule picker listing only that species' methods (seeds,
  cuttings, fragments), pouring the species as the picked propagule. Inspect shows species,
  age, health, size and what is limiting growth (for example, "light too
  low"). The water-quality score counts plant cells from the table.

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
assets/              # materials/*.toml, reactions.toml, plants/*.toml, species/*.toml, sprites (PNG atlases + source files)
```

Dependency direction: `sim ← render ← platform ← {desktank,
desktank-saver}`, `sim ← render ← desktank-saver-mac`, and
`sim ← render ← desktank-web`. The saver binary reuses `platform` for its
windows and input detection.

`desktank-sim` and `desktank-render` must build for `wasm32-unknown-unknown`,
and CI checks this from T2. Platform-specific code never goes into them.

`desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. If
wgpu surface creation from raw handles needs `unsafe`, it moves into
`platform`. All other `unsafe` lives in `platform` and
`desktank-saver-mac`, at FFI boundaries, with `// SAFETY:` comments.

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
  front (D21). Water
  is opaque in wallpaper mode and transparent or tinted in overlay mode.
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
  - `desktank interact on|off|toggle`, `desktank feed [FOOD]` and
    `desktank pause` (sent over IPC to the running app);
  - `desktank capture --seed N --time HH:MM --size WxH --frames K --script FILE --out DIR`
    (headless offscreen renders; scripted pours make the output
    reproducible);
  - `desktank bench`;
  - `desktank import FILE` and `desktank export FILE` (move a save to
    and from the web version or another machine; WB3);
  - `desktank saver install|uninstall`.
- **Single instance and IPC.**
  - An advisory lock (`fd-lock`) in the runtime dir is released
    automatically if the process crashes, so a stale lock can't block
    startup.
  - The local socket lives at `$XDG_RUNTIME_DIR/desktank.sock` on Linux
    and in the app's cache dir on macOS, both mode `0600`. On Windows it
    is a named pipe with a DACL for the current user only.
  - The protocol is line-based. Commands are `interact`, `feed`,
    `pause`, `saver-handover` and `status`. Arguments are validated
    against the material table and config, unknown input is rejected,
    and each line is capped at 1 KB.
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
| Web | `wasm-bindgen`, `web-sys`, `js-sys` | wasm-bindgen CLI pinned to the crate version; built by `scripts/build-web.mjs` |
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
| Release | `cargo-dist` for archives, MSI and shell installers; zx scripts (pnpm) build `.app`, `.saver` and `.dmg` | |

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
| Web transfer size (wasm + JS + assets, gzip, as reported by `scripts/build-web.mjs`) | ≤ 3 MB | — | — |
| Web on the reference phone (🔍 to be chosen in WB2: a 2021-era mid-range iPhone or Android) | p95 frame interval ≤ 50 ms, no thermal throttling within 10 min | — | — |
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
  - **zx scripts** build `DeskTank.app` (universal binary), assemble
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
in that PR.

"Within budget" means §6.1, measured with `desktank bench` on the
reference machine and reported in the PR. In Windows- and Linux-specific
PRs it means the numbers are recorded, and are no worse than 2× the
reference targets, until those platforms get their own reference
machines.

### M0 — Foundations

- [x] **F1 Workspace scaffold + CI.** Crates from §4.1 as stubs, a
  toolchain file, lints, `deny.toml` and Dependabot (ignoring winit
  minor and major updates), with CI on three OSes.
  - Platform dependencies are gated with `[target.'cfg(...)'.dependencies]`.
  - The Rust version is pinned exactly.
  - Linux CI installs the system packages it needs: libxkbcommon,
    wayland, libdbus and mesa-vulkan-drivers (lavapipe).

  *Accept:* CI is green on all three OSes, `cargo run -p desktank --
  --help` works, and each platform crate builds only on its own OS
  (elsewhere it compiles to an empty crate, so `cargo build --workspace`
  works on every OS).
- [ ] **P1 Packaging and identity spike** (findings go in PROGRESS, no
  product code). Answer:
  - whether `LockScreen.SetImageFileAsync` works unpackaged;
  - MSI vs MSIX;
  - per-user vs per-machine `.scr` install;
  - whether a Rust `cdylib` loads as a `.saver`;
  - whether wgpu supports premultiplied-alpha composition swap chains on
    DX12;
  - whether `CGEventSourceFlagsState` needs any permission;
  - whether `WS_EX_LAYERED | WS_EX_TRANSPARENT` stays click-through with
    `WS_EX_NOREDIRECTIONBITMAP` and DirectComposition;
  - whether the `CGWindowListCopyWindowInfo` bounds check works without
    Screen Recording permission;
  - which Windows power setting reports energy saver status.

  These 🔍 items stay with their packages instead: `ksni` (O2),
  `sharingType` (O1-Mac), the Ctrl+Alt/AltGr clash (O5), GNOME
  layer-shell (W3) and Flatpak (R3).

  *Accept:* each question answered yes or no, with evidence (a doc link
  or a minimal test), and the affected 🔍 marks resolved in this spec.

### M1 — A living tank in a window

Each package depends only on the packages before it.

- [x] **T1 Grid, chunks and movement.** The cell grid, chunk sleep and
  wake, dirty rectangles and the movement rules (§2.1). All five movement
  classes are covered, with test materials water, air, sand, gravel,
  rock, air bubble and one floating flake. Material TOML with units, a
  fixed tick, and RNG streams. `criterion` benches. *Accept:*
  - same seed and script give the same state hash after 10⁴ ticks,
    including with the parallel pass if it is enabled;
  - non-reacting mass is conserved;
  - a scene with no living cells and no creatures has 0 awake chunks
    within 1 s of world time after the last change;
  - bubbles released at the bottom reach the configured water level;
  - a sand pile's slope measures 35° ± 5°;
  - the flake floats for its `float_time`, then sinks;
  - the criterion bench gives a tick time ≤ 5 ms for a heavy pour (a
    20%-of-grid sand column falling into water) and ≤ 0.5 ms for a
    settled scene.
- [x] **T2 Renderer, `desktank window` and minimal `desktank bench`.**
  Grid texture with dirty-rectangle uploads, a palette shader with
  integer scaling, frame pacing, and a pause when occluded. Window mode
  uses winit on every OS, Wayland included. `desktank-render` stays
  within the WebGL2 limits (§3.5), and a CI job builds and clippies
  `desktank-sim` and `desktank-render` for `wasm32-unknown-unknown`.
  *Accept:* bench numbers for a 60 s settled scenario are within the
  main-app budget; CPU while minimised for 60 s is ≤ 0.1%; the wasm32 CI
  job is green.
- [x] **T3 Capture.** `desktank capture` with input scripts, and render
  tests on lavapipe in CI at the §7 tolerance. *Accept:* at least 2
  reference images in CI, and a deliberate shader change fails the test.
- [x] **T4 Window-mode interaction.** The palette, and the pour, scoop,
  net and inspect tools, with brush size and a cursor preview. Creature
  inspection becomes usable once T7 lands. *Where it lives* (for WB2 and
  the overlay): the tool state machine, brush, stroke interpolation and
  pour/scoop edits are `desktank_sim::tools` (pointer events in cell
  coordinates, no OS or GPU); pixel→cell mapping is
  `desktank_render::Layout::cell_at`, palette hit tests
  `desktank_render::PaletteLayout` (via `Tank::palette_layout`), and the
  palette and brush outline are drawn by the tank shader
  (`Tank::set_palette`, `Tank::set_cursor`). A host only maps its pointer,
  buttons and wheel onto these. Net's creature hit test is a stub returning
  nothing until T7, and inspect reports the cell's material. *Accept:*
  - scripted-input tests for each tool, checking the cell changes they
    make;
  - a manual checklist (mouse and trackpad) in the PR;
  - CPU stays ≤ 15% during a scripted 10 s continuous pour.
- [x] **T4b Rigid bodies and the glue tool** (D17). The `rigid` movement
  class (rock), the body table, body motion, splitting and holding
  (§2.1 [Rigid bodies](#rigid-bodies)); the glue tool in
  `desktank_sim::tools` with the palette slot and key 0 in
  `desktank window`; the glued shade in the palette (no new texture or
  channel, WebGL2-safe); `glued` in input scripts; one body per stroke on
  the web. *Accept:*
  - a rock body sinks through water as one piece, keeping its shape and
    moving at most a row per tick, and comes to rest on sand or the
    floor;
  - mass is conserved while bodies sink;
  - a glued body stays put while the water and sand under it are
    scooped, and drops once unglued;
  - scooping a rock in half gives two bodies that move independently;
  - same seed and input give the same state hash, and a settled scene
    with bodies reaches 0 awake chunks;
  - the T1 bench budgets still hold, with a resting-bodies case in the
    criterion bench;
  - a GIF in the PR of a rock sinking as one piece and a glued one
    hanging in place.
- [x] **T4c Rigid-body physics** (D17 revised). Bodies accelerate, feel
  buoyancy and drag, tip about their contacts and rotate, slide off steps,
  push grains aside on impact and pass momentum to other bodies
  (§2.1 [Rigid bodies](#rigid-bodies)). *Accept:*
  - a rock balanced on a point topples onto a wider side, and a block
    overhanging a ledge tips off it, keeping every cell;
  - a flat slab lands level and sleeps without jitter;
  - a rock accelerates in air and holds a terminal speed in water;
    driftwood rises at under half the speed rock sinks;
  - a rock dropped into sand embeds deeper than one set down gently, and
    no grain is lost;
  - glued bodies never move, and a body rests on a glued one;
  - same seed and input give the same state hash; the settled benches
    stay within noise of `main`;
  - a GIF in the PR of a rock toppling and one embedding in sand.
- [x] **T5 Materials, reactions and fields.** The reaction table; the
  nutrient, CO₂ and light fields; water current from a filter-outlet
  placeable and a bubbler placeable (which emits bubbles); food types;
  fertilisers (root tabs, liquid, CO₂ bubbles that dissolve into the
  field); waste and detritus decay into nutrients; and the random-tick
  scheduler (§2.1). *Accept:*
  - a test for each reaction rule;
  - liquid fertiliser raises the nutrient field around it, spreading at
    a rate within 10% of the configured diffusion constant;
  - detritus disintegrates within ±10% of its configured time;
  - the current moves floating and liquid cells in the expected
    direction;
  - random ticks stay within their per-tick budget.
- [x] **T6 Plants and the simple ecosystem.** Plant growth from seeds
  and cuttings, moss, algae, the water-quality score, the water-change
  action, and day/night lighting from the injected time of day.
  *Accept:*
  - a plant grows ≥ N cells in 10 simulated minutes when fed and lit,
    and wilts when not (N is set from measurement and recorded);
  - a scripted overfeeding scenario raises algae coverage above 5% of
    the substrate surface within 30 simulated minutes, and a water
    change brings it below 1% within 30 more minutes;
  - captures at 4 times of day each match a reference image.

  *Done:* N = 150 (measured 185–235); mechanisms and numbers in §2.2 and
  §2.3 (*T6*).
- [x] **T6b Plant individuals and species** (D18, §2.7). The plant table
  and plant ids in cell `state`; generic part materials; species TOML in
  `assets/plants/` with preference ranges, growth rate, size, hardiness,
  propagation and colour ramps; T6's plant and moss become the first two
  species (stem form only, today's behaviour), plus a second stem species
  with different preferences; health and death; splitting by scoop; the
  `Rgba8Unorm` cell texture and the per-plant texture with the health
  tint; the plants palette slot with a species picker (window and web);
  inspect shows species, age, health, size and the limiting factor; the
  water-quality score counts from the table; the population cap.
  *Accept:*
  - two touching plants of one species keep separate ids, sizes and
    health, and scooping one never changes the other;
  - in one tank, species A outgrows species B under strong light and B
    outgrows A under weak light, following their ranges;
  - scooping through a stem gives two plants, or a plant and dead plant
    matter, as the species says, and the table always matches the grid;
  - a starving plant's tint changes without waking a chunk or rewriting
    its cells;
  - the T6 acceptance tests still pass;
  - same seed and script give the same state hash; the T1 and T5 bench
    budgets hold, with a 500-plant settled case in the criterion bench;
    the cost of the wider texture upload is measured and recorded.

  *Done:* species hygrophila (T6's plant), java moss (T6's moss) and
  rotala; layout, cap and upload cost in §2.7 (*T6b*).
- [ ] **T6c Growth forms and propagation** (§2.7
  [Growth forms](#growth-forms)). Stems with branching and surface
  trailing, rosettes, carpets, floaters, epiphytes and mats; runners,
  buds, fragments and seeds; the v1 species set (about 8). *Accept:*
  - for each form, a seeded grow captures to a reference image;
  - runners and buds make child plants with the parent id set;
  - floaters stay in the top water row, drift with the current and
    measurably reduce the light below them;
  - epiphytes and mats grow only on what holds them, and fall off when
    it is gone;
  - a GIF per form in the PR;
  - the settled bench budget holds with every v1 species growing.

  Split into three PRs: (1) rosettes with runners, and carpets; (2)
  floaters and budding; (3) epiphytes, mat fall-off and layered mats,
  stem branching and surface trailing.
  *Part 1 done:* the rosette and carpet forms with runners (§2.7
  [Growth forms](#growth-forms)), the `runner` part, root feeders, and
  amazon sword, vallisneria, cryptocoryne, dwarf hairgrass and Monte
  Carlo; reference images `plant_rosettes` and `plant_carpets`; runners
  root children with the parent id set; `settled_500_plants` grows every
  form so far in 11.5 µs.
- [ ] **T6d Plant appearance effects** (§2.7
  [Appearance](#plant-appearance)). Per-part colour ramps with age and
  position shades, health tints, colouration in strong light, night
  folding, pearling and the optional sway. *Accept:*
  - reference images for a healthy, a starving and a strongly lit red
    plant;
  - pearling appears only above its light and CO₂ thresholds, at no more
    than its capped rate;
  - sway is off by default; with it on, CPU and GPU stay within budget at
    its frame cap, and it draws nothing on battery or while hidden;
  - with sway off, a settled planted tank draws no extra frames (a
    frame-count test).

  *Partly done (visual subset):* per-part species colour ramps with
  position shades and the health tint, drawn at two texels per cell side
  with per-species part shapes (`[look]` in the species TOML: leaf
  lanceolate, needle, round or feather; stem stalk or thick, fuzz or
  frond for mats; root strand or thick), and the `plant_species`
  reference image of healthy plants. Colouration in strong light, dying
  translucency, night folding, pearling and sway are open.
- [x] **T7 Creatures.** Species TOML (including depth and surface
  needs), the sprite atlas pipeline with placeholder art (D11), steering
  (wander, schooling, zones, bottom-walking, surface-grazing,
  surfacing), collision with cells, eating food cells, grazing algae and
  detritus, and optional cursor avoidance. *Accept:*
  - no creature enters a solid cell in 10⁵ ticks;
  - schoolers' mean nearest-neighbour distance stays ≤ 3 body lengths
    for ≥ 80% of the time;
  - each species spends ≥ 70% of its time in its zone;
  - food is found and eaten within 10 s of simulated time;
  - a GIF is in the PR.

  *Done:* Species TOML selects the movement and diet. Tetras follow the oldest
  unheld schoolmate and separate from close neighbours. Corydoras follow
  one-cell substrate steps. Nerites trace submerged contours, including the
  underside of rock, turning in quarter turns so the foot rests on the face
  they hold (at outside corners, held only diagonally, they keep their turn
  until a face is found); the body box turns with the sprite, and the turn
  is sim state. Air-breathing bettas return to the air interface every
  20 simulated seconds and skim there for 3 seconds. Grazing a cell gives
  the same satiety as one food cell. Cursor avoidance starts off in both hosts.
  In seeded tests the school met the spacing limit on 97.0% of ticks,
  minimum zone occupancy was 96.4%, food was eaten within 7.85 s, and all
  four species cleared the 100,000-tick collision run. On an M3 Max the
  30-fish settled and active-grazing cases took 7.37 and 28.04 µs/tick.
  Rendering the settled 30-fish tank at 2560×1440 used 0.72% of one core.
  *Invertebrates:* cherry and amano shrimp (the v1 set's shrimp, §2.4)
  are bottom walkers that hop a few cells off the substrate every 6 and
  10 s and graze algae and detritus; Thai micro crabs are surface
  crawlers that cling to plants, rock and glass, walk the bed and
  filter-feed flakes and detritus from two cells away.
  *More fish:* honey gouramis (upper half, air visits like the betta),
  male guppies (roaming the middle and upper water), harlequin rasboras
  (schooling with their own kind only) and otocinclus (surface crawlers
  grazing algae) use the existing behaviours with no new species key.

- [x] **T8 Persistence.** The save envelope and format, ownership lock,
  atomic writes keeping the last 3, autosave and save on exit (§2.6).
  *Accept:*
  - save and load round-trip, giving the same state hash;
  - a v1-format fixture loads;
  - killing the process mid-save leaves a loadable previous save;
  - a corrupt save is moved aside, and the app starts with a fresh tank
    plus a logged error.

  *T8:* `desktank_sim::save` (format, OS-agnostic) and
  `desktank_platform::SaveStore` (files, lock, atomic writes). The lock is
  on a `<tank>.lock` file beside the saves, because saves are replaced by
  renames. A damaged newest save falls back to the next older one before
  a fresh tank. The snapshot stores everything that feeds later ticks, so a
  loaded tank continues tick for tick, not only with the same hash.
- [x] **T9 Life cycle.** Eggs and livebearing, growth stages, breeding
  rules, colour genetics (inheritance, reversion to wildtype, mutation),
  predation, the death causes available in simple mode (old age,
  starvation, poor water quality; predation eats the prey whole, with no
  corpse or recorded cause), easy mode, corpses that
  turn into detritus, the population cap, and names on the inspect card.
  Temperature deaths come with E1 and chaos deaths with E2. *Accept:*
  - a test for each cause of death;
  - easy mode allows only old-age deaths;
  - a seeded multi-generation test shows same-morph lines breeding true
    and mixed lines reverting towards wildtype;
  - the population never exceeds the cap;
  - a named creature keeps its name across save and load.

  Split into three PRs: (1) age, life stages, growth with feeding, deaths
  from old age, starvation and poor water, easy mode, corpses, the
  configurable population cap and save format v2; (2) eggs and
  livebearing, breeding rules, colour genetics (with the multi-generation
  test) and predation; (3) names on the inspect card (window and web),
  kept across save and load.
  *Part 1 done:* §2.4 *T9 part 1*: species lifespans and maturity ages,
  fry, juvenile, adult and elder stages drawn to size, energy that feeds
  growth and starves, foul-water stress from a once-a-second quality
  sample, easy mode, sinking corpses that rot into detritus, the cap
  (`World::set_creature_cap`), save format v2 with a v1 migration; a test
  per cause so far, easy mode, the cap and the save round trip
  (`crates/desktank-sim/tests/lifecycle.rs`) and the `lifecycle`
  reference image; 30 settled fish tick in 7.37 µs (main 7.13 µs).
  *Part 2 done:* §2.4 *T9 part 2*: sexes, livebearers and egg layers,
  breeding rules (mature, well fed, rested, paired, slower when crowded,
  within the cap), eggs as cells that hatch or rot, colour morphs with
  inheritance, reversion and mutation, predation of fry and shrimp, female
  guppy frames, and save format v3 with a v2 migration; tests in
  `crates/desktank-sim/tests/breeding.rs` (including the seeded
  multi-generation test and a predation death), the `breeding` reference
  image; 30 settled fish tick in 7.75 µs (main 7.52 µs).
  *Part 3 done:* §2.4 *T9 part 3*: optional user-given names, cleaned and
  capped at 24 characters, saved in format v6 with a v5 migration; the
  inspect card (`World::creature_card`: name, species, stage, sex, age,
  health, energy, morph, days left carrying) in the window's title bar
  with an `N` name field, and in the web readout with a name field; tests
  in `crates/desktank-sim/tests/names.rs` (the name across save and load,
  cleaning, no effect on the tank, the card, young born unnamed), the v6
  fixture, and the web's Rust and Playwright tests (naming, and the name
  across a reload); 30 settled fish tick in 8.41–8.49 µs (main
  8.43–8.54 µs, unchanged).
- [ ] **T10 Catch-up and cross-OS measurement.** Optional catch-up (off
  by default, capped at 24 h), plus `desktank bench` on Windows and Linux
  with measurement recipes in PROGRESS. *Accept:*
  - 24 h of catch-up computes in ≤ 200 ms;
  - for a reference tank, catch-up results stay within ±25% of a full
    24 h simulation for population, plant mass and nutrient level;
  - numbers are recorded for all three OSes.

### M2 — Power, overlay and control

- [ ] **O3 Power policy and event sources** (§6.2), before any
  always-on mode ships. *Accept:*
  - on battery the frame rate drops to 15 fps, and the tick is
    unchanged;
  - Low Power Mode, lock and display sleep pause rendering and world
    time, except a saver host running over the lock;
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
  - holding the chord enables pouring over the overlay and the
    wallpaper;
  - releasing the chord, or pressing Esc in toggle mode, restores
    click-through within 150 ms (the poll interval plus one frame);
  - focus returns to the previous app;
  - typing a creature name works in toggle mode;
  - Wayland works through the tray and CLI;
  - the AltGr check is done, and the Windows default chord is adjusted if
    needed;
  - no OS permission prompt appears (per P1).
- [ ] **O2 App shell: tray, config and IPC.** Single instance, IPC
  (§4.3), the tray menu, TOML config with hot reload, and start at login. *Accept:* every menu item works on all
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
  because it is the fallback for S1 and S2. *Accept:* starts within 10 s
  of the configured idle time (the poll interval), dismisses on any input, covers every
  display, and the saver windows stay within budget.
- [ ] **S1 Windows `.scr`** with `/s`, `/p` and `/c`, and `desktank saver
  install` (per-user), with the tank ownership handover (§2.6). *Accept:* each
  argument works from the Screen Saver dialog or the registry entry, the
  preview renders inside the dialog, and the saver exits on input.
- [ ] **S2 macOS `.saver`.** objc2 `ScreenSaverView`, a Metal layer, the
  leak mitigations, the ownership handover (§2.6), and `desktank saver install`
  (unsigned, D14). *Accept:*
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
- [ ] **R2 Unsigned-install experience** (no signing, D14). README
  install steps for macOS (Gatekeeper, `.saver` quarantine) and Windows
  (SmartScreen), plus ad-hoc signing in the macOS build. *Accept:*
  following the README on a clean machine gets the app and saver
  running on each OS.
- [ ] **R3 Package managers.** *Accept:* `brew install --cask`, `winget
  install` and an AUR package each install a working release.
- [ ] **R4 Go public.** README, CONTRIBUTING, issue templates, and
  licence headers for assets. Make the repo public. *Accept:* the owner
  approves, and the repo is public.

### MW — Web

- [x] **WB0 Browser demo of the sim core.** `desktank-web` wraps
  `desktank-sim` with wasm-bindgen; a plain HTML/CSS/JS page in `web/`
  pours and erases materials at a fixed 20 Hz tick, built by
  `scripts/build-web.mjs` and deployed to GitHub Pages
  (https://robhannan.github.io/DeskTank/) from `main`. Not the product
  renderer. *Accept:* the demo builds in CI on every PR, and the page
  loads and pours on desktop and mobile.
- [x] **WB1 Web host on the real renderer** (after T3). `desktank-web`
  drives `desktank-render` through wgpu, with backend selection and
  device-loss recovery (§3.5), and replaces the canvas-2D demo. It adds
  the Playwright smoke test and a transfer-size report in the build
  script. *Accept:*
  - with `?backend=webgl2`, a seeded scripted scene matches the native
    `desktank capture` of the same scene within the §7 tolerance;
  - on the reference Mac at DPR 2, the p95 frame interval is ≤ 40 ms
    over 60 s in Chrome, Safari and Firefox (measured with the
    Performance API and recorded in the PR);
  - the smoke test is green in CI;
  - the transfer size reported by the build script is within budget;
  - a forced context loss (`WEBGL_lose_context`) recovers without a
    reload.

  *(WB1: the "frame interval" is the page's rAF cadence; drawn frames
  follow the 20 Hz tick until something moves between ticks, so they are
  50 ms apart. The renderer now shades in sRGB-encoded space on every
  target, so sRGB canvases and windows match the `Rgba8Unorm` captures.)*
- [x] **WB2 Web interaction and in-page controls** (after T4). The
  interact tools with pointer and touch, the in-page menu, the settings
  panel (`localStorage`) and per-device cell scale (§3.5). *Accept:*
  - Playwright scripts each tool with mouse and with emulated touch,
    asserting the cell changes;
  - pinch and scroll on the canvas don't zoom or scroll the page;
  - settings persist across reloads;
  - a manual check on the reference phone is in the PR, within the
    phone budget (§6.1).

  *(WB2: the palette is DOM buttons from `Tools::palette` rather than the
  shader strip, for touch-sized targets and a keyboard radio group; the
  shader draws the brush outline. Touch strokes start once a finger moves
  8 CSS px, so a long-press (500 ms) or a second finger never pours first;
  a tap uses the tool once. The reference-phone check is left to the
  owner, see PROGRESS. Later the palette was grouped (§2.5): group tabs
  over the open group's radio group, both wrapping, so nothing scrolls
  sideways on a phone.)*
- [x] **WB3 Web persistence and ownership** (after T8). IndexedDB saves
  using the §2.6 format, save on hide and `pagehide`, autosave,
  `navigator.storage.persist()`, export/import, Web Locks ownership with
  Take over, and the visibility pause. *Accept:*
  - after a `visibilitychange` to hidden, a reload restores the same
    state hash;
  - a second tab shows "open in another tab" and never writes, and Take
    over moves ownership within 1 s without losing state;
  - an exported native save imports on the web and vice versa,
    letterboxed if the sizes differ;
  - a hidden tab runs no simulation ticks (checked by tick count).

  *(WB3: saves are generations keyed `[tank, n]` in one IndexedDB object
  store, each written in one transaction that also prunes to the newest 3
  (the transaction stands in for write-fsync-rename). A viewer tab shows
  the newest save paused with its controls inert and queues for the lock,
  so it takes over by itself when the owner closes; an owner that doesn't
  answer Take over within 1 s (a frozen tab) has the lock stolen
  (`steal: true`), and an owner epoch checked inside each write keeps a
  stolen owner from writing. Native import and export are `desktank import FILE` and
  `desktank export FILE` (§4.3).)*
- [ ] **WB4 Installable PWA** (after WB3). Manifest, placeholder icons and
  a service worker, with paths relative to the `/DeskTank/` subpath
  (§3.5). *Accept:* a Playwright test checks the manifest (name, icons,
  `start_url` and `scope` of `./`) and an active service worker; after
  `context.setOffline(true)`, a reload loads the page and it runs.

### Stretch goals (unscheduled)

- Open windows act as solid obstacles, so sand piles on title bars and
  creatures swim around windows (D7).
- One spanning tank across monitors (D9).
- Fictional species (D8).
- A user directory for custom materials and species.
- A settings window.

### Suggested order

1. F1, then P1 and T1 in parallel.
2. T2, T3, then T4, T4b and T5 to T10 in order. WB1 slots in after T3, WB2 after
   T4, and WB3 and WB4 after T8. T6b, T6c and T6d follow T6: T6b must
   land before T8 (the save format includes the plant table and the cell
   meaning of `state`); T6c and T6d come after T6b and can run alongside
   T7.
3. O3, then O1-Mac, then O2, then O5.
4. W1, then S3, then S2, then the rest.

Within each mode macOS comes first, then Windows, then Linux (§11). M6 can run alongside M3 and M4.

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Falling-sand cost on an always-on app | Budget blown during activity or by a "busy" tank | Sleeping chunks, a 20/10 Hz tick, the population cap, profiling in T1; parallel chunks only if needed |
| Pixel art supply | No art, or inconsistent art | Placeholder art first (D11), behind a consistent palette and sprite-size spec, so real art can be swapped in later |
| Pet sim feels punishing | Users come back to dead tanks | Easy mode; world time only while visible; catch-up off by default; slow, gentle death curves |
| Global modifier detection needs permissions | Permission prompts, or the feature fails | P1 verification; the tray toggle as the always-available path |
| Windows 24H2+ WorkerW changes | Wallpaper broken on new Windows | Test matrix and snapshot fallback; study Lively's behaviour (GPL, so no code copying) |
| macOS `legacyScreenSaver` leaks and regressions | Saver eats RAM | Mitigations, soak test, and built-in idle mode as a fallback |
| GNOME lacks layer-shell | No wallpaper or overlay on GNOME Wayland | Document it; X11 session works; a GNOME Shell extension is a possible later option |
| Transparent overlays on Windows with wgpu | Alpha may not compose | P1 question plus the O1-Win spike; `UpdateLayeredWindow` fallback |
| Lock-screen API needs package identity | L2 not possible unpackaged | Decided in P1: MSIX packaging, or drop L2 |
| Unsigned builds (D14) | Gatekeeper and SmartScreen warn or block users | Clear README steps (R2); Homebrew and winget channels; signing can be added later |
| Save corruption or schema drift | A lost pet tank | Versioned format, atomic writes, the last 3 saves kept, fixture tests |
| WebGL2 limits constrain the renderer | Native features blocked, or web broken | Renderer kept within WebGL2 downlevel limits from T2; CI builds render for wasm32; the WB1 smoke test |
| Browser storage eviction | A lost web tank | `navigator.storage.persist()`, export/import, a visible "last saved" indicator |
| Mobile browser performance | Janky or hot phones | Cell scale adapts to screen size; the population cap; profile in WB1 and WB2 |

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
| D11 | Art: **placeholder pixel art** to start. |
| D12 | **Water level configurable**; species declare depth and surface needs. |
| D13 | **Colour genetics**: offspring inherit morphs; mixed lines revert to wildtype (e.g. neocaridina). |
| D14 | **No code signing** for now. |
| D15 | **Web is a product target**: the same tank in a browser tab on desktop and mobile, via wgpu WebGPU/WebGL2 (§3.5). Native mobile apps remain a non-goal. |
| D16 | **Memory budget is raw RSS ≤ 100 MB** (`ps` RSS, which counts shared GPU-driver pages), raised from 80 MB after T2 measured 90 MB RSS / 53 MB physical footprint. |
| D17 | **Solids are rigid bodies.** Rock (and driftwood) poured in one stroke is one object that sinks (or rises) as a whole, can topple and can embed in loose substrate; a **glue** tool makes bodies static. The `solid` class stays for truly static materials (§2.1 [Rigid bodies](#rigid-bodies), T4b). |
| D18 | **Plants are individuals of data-defined species** (2026-11-18). Each plant is distinct from every other, even of the same species, with its own age, health and size; species set environment preferences, growth speed, size, hardiness and appearance in TOML (§2.7). **Varied growth forms** (branching stems, rosettes and runners, carpets, floaters, epiphytes and mats) and **dynamic leaf appearance** (age and health shading, colouration in strong light, pearling, optional sway) are wanted, not stretch goals (T6b–T6d). |
| D19 | **Creature sprites have two texels per cell side** (2026-11-18). A creature's body (collision box) is half its sprite's size in texels, so sprite sides are even; palette icons are hand-drawn at one pixel per cell. Every target draws the grid at an **even integer scale** so each texel is a whole number of pixels; scale 1 is the fallback when 2 doesn't fit, and then shows one texel of each 2×2 block. |
| D20 | **Cheats behind the Konami code** (2026-11-19). The code opens a text field that runs small, harmless cheats from a registry (§2.5); interactive hosts only (window, web, and the overlay or wallpaper in interact mode with the chord held), never the screensaver or lock screen. |
| D21 | **2.5D depth** (2026-11-19). The picture stays 2D, but the tank has discrete depth bands for movement and collision, and later for occlusion (plant cells in front of a creature drawn over it): plants are no longer solid in every band, so fish slip past them and reach food lodged inside them (§2.4). Full 3D (perspective, a camera, 3D models) remains a non-goal. Occlusion (part 2, 2026-11-20) needs no second grid pass or depth buffer: the grid is drawn once, sprites follow back to front, and each sprite fragment checks the cell under it in a per-cell band texture, dropping itself where a plant cell in a nearer band covers that texel (its shape is evaluated as the grid shader does). One grid pass and one sprite draw per frame, whatever the number of creatures or bands; per-band plant layers would cost up to eight grid passes, and splitting the grid in two works for one depth only. A subtle depth cue fades creatures and plant cells towards the water colour with depth. |
| — | Platform order macOS → Windows → Linux, with no Linux desktop preference. Lock-screen limits accepted. **Open source.** Tray + config only to start. §6.1 budgets accepted. Name to be confirmed. |

## 12. Open questions

1. **Final art.** Who makes the final pixel art, and under which licence
   (CC BY 4.0, CC BY-SA 4.0, or the code's licence)? This is deferred
   until placeholder art is in place.
2. **Name and identity.** "DeskTank" is TBC, and there is no icon
   direction yet. A placeholder icon will be used.
