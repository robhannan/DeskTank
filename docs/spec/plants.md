# Plants

### 2.7 Plants (D18)

Plants are **individual organisms** of **data-defined species**, not just
materials. Every plant is distinct from every other, even one of the same
species growing into it: it has its own age, health and size, competes
for light and nutrients, and the inspect card shows it. T6 built plants
as materials and rules (§2.2 *T6*, §2.3 *T6*); T6b turns them into this
model, T6c adds the growth forms and T6d the appearance effects. Algae
grows as a surface coat on hardscape, substrate and glass (§2.3, D28).

- **Species are data**, as creatures are (§2.4, D8). One TOML file per
  species in `assets/plants/` defines:
  - name, scientific name (Latin binomial, with cultivar or variety if it
    has one) and a one-line description (the inspect and species cards);
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
    optional lifespan (`lifespan_days`, in life days: see *Health and
    death* below);
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
    bits (capped at 63) and colour noise in its top 2. The low byte of a
    seed or cutting's `state` holds the species id it grows into (0 for
    the default, the lowest id). The high byte holds rot progress for
    cuttings and stays zero for seeds. The generic parts so far are root,
    stem and leaf (a mat's cells are stems); T6c adds the rest.
    *T6c (parts 1–3):* a rosette's, carpet's or epiphyte's blades are leaf
    cells (the species' `[look] leaf` picks a blade shape), so blades cost
    no material id; `runner` (id 77, counted with roots), `floating_leaf`
    (id 79, counted with leaves) and `rhizome` (id 80, counted with stems)
    are the remaining part materials, so the table and saves keep three
    count slots.
  - The table holds age and health, which the grid can't rebuild, so
    saves store it (§2.6). Ids, part counts and bounds are checked
    against the grid in tests.
  - **Cutting and splitting.** Scooping through a plant splits it as it
    splits a body: at the next tick, the piece still joined to the base
    keeps the id, and each other 4-connected piece becomes a new plant of
    the same species (a fragment, parent id set) if the species
    propagates by fragments and the piece holds a stem or rhizome (or a
    floater's floating leaf, which floats on as a buoyant body, D22);
    otherwise it becomes dead plant matter. A cut or split fragment
    drifts or sinks as a `Raft` (below) like a placed cutting until it
    roots.
  - **Discrete propagules (#469, #289).** Seeds pour as powder (the
    `seed` material, stateful by species). Every other propagule
    (cuttings, fragments, runners, buds) is a real, discrete multi-cell
    plant placed one per click (`World::place_propagule`) with its anchor
    cell at the pointer, built of the species' own part cells: a stem
    cutting is three stem cells with two pairs of leaves (a fragment two
    with one pair); a runner's plantlet a crown with a few short blades;
    an epiphyte's fragment a rhizome with leaves; a moss fragment a small
    clump of stem cells; a marimo fragment a plus-shaped ball; a bud one
    floating leaf (a floater's fragment two with a root). Every cell must
    land in open water, and the piece becomes a plant (table entry,
    species, parent unset) at once, so it appears in Inspect and counts
    toward the plant cap (a full tank, or no room there, places nothing).
    There is no brush, radius or preview for a placed piece. A piece that
    roots or clings starts as a `Raft` (the same buoyant body as a
    floater, D22) that drifts with the current and sinks until it rests on
    a substrate or host that suits it, then roots as a plant of the same
    species (at once, if it is placed resting on one). An unrooted piece,
    placed or cut, ages while its rooting site or conditions are
    unsuitable and becomes dead plant matter after about 7.5 minutes
    (within one in-tank day, 10 minutes), scaling with the random-tick
    interval. Seeds wait indefinitely for suitable conditions. Scripts
    (`script.rs`) check at parse time that the named species grows from
    the propagule they use.
  - **Legacy `cutting` powder.** Older saves and worlds may still hold
    `cutting` material cells (a poured cutting, fragment, runner or bud
    from before #469). They keep working as before: such a cell roots on
    suitable substrate, accumulates rot progress in the high byte of its
    state and rots after the same 7.5 minutes. Nothing pours the `cutting`
    material any more, and the save format is unchanged (a raft may
    belong to any plant).
  - **Health and death.** Health falls while the plant's conditions are
    outside its ranges and recovers inside them. Low health drops the
    oldest leaves first, then the whole plant becomes dead plant matter;
    old age only if the species has a lifespan. Plant ages and lifespans
    are on the **life clock** like creatures' (§2.4; `creatures.md`, *Life
    clock*): one day per `LIFE_DAY_SECONDS` = 600 s (10 minutes) of world
    time. A species gives `[growth] lifespan_days` (finite, above 0; a
    fraction of a day is fine), and the inspect cards (and the web inspect
    line) show a plant's age in whole life days, worded as a creature's
    ("under a day", "1 day", "N days"). The growth `rate` stays in new
    cells per world minute. Saves keep each plant's birth tick, so they
    are unchanged. Grazing (T7) removes
    cells and costs health. Hungry guppies seek palatable floating leaves (duckweed).
    Each bite removes one floating leaf and costs its plant 0.05 health.
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
    loosely or floats (hornwort type). *T6c part 3:* `[stem] branching`
    (0..=1; rotala 0.2, ludwigia 0.15) sets the deterministic per-node
    chance of a side shoot, which leaves its node on the diagonal and leans
    out (below), running beside the main stem for its first two cells
    (which it must not stop growing: the main stem's growth ignores shoot
    cells beside it; leaves stop short of a rising shoot below them); any stem reaching the top water row trails sideways
    along it up to `height`. *Stem leaves in the tank look like the info
    pane's sprite:* leaf length tapers to the tip of the plant's own stem
    or side shoot (`StemForm::leaf_taper`): the top `internode` cells are
    a bare stalk ending in a bud (drawn from the leaf ramp's brightest
    shade), then a one-cell pair, lengthening in whole cells to
    `leaf_length` at half the stem's `height` below the tip, and never
    longer than the tier below. A leaf's growth is capped by that depth,
    so a young stem is tapered too and its lower leaves lengthen as the
    stem grows past them; the shader draws each blade at the length it
    has reached. At every other node the broad shapes (lanceolate, round)
    grow an *upper cell* over their middle cells (`StemForm::leaf_upper`;
    position `LEAF_UPPER` (32) plus its row cell's), so the blade is two
    cells thick where widest. A plant's pieces don't join through an upper
    cell and the leaf above it, so cutting the stem still frees the top.
    Feather leaves (cabomba) grow upper cells too, so each fan of filaments,
    on its short petiole, stands taller; needle leaves stay a row thick. Side shoots lean:
    the shoot leaves its node on the diagonal (no flat stub) and steps out
    a column every two rows, up to `SHOOT_LEAN` columns from the main stem,
    then rises straight on; their cells join diagonally, in growth (`stem_sites`) and
    in placement, and in splitting a plant into pieces. The shader draws
    that stalk on a slope and each leaf's outer cell a texel short where the
    taper falls between two lengths. Stem plants' upper leaves and buds lean
    a little towards the colouration colour even before the light colours
    the plant (`TIP_TINT`, a quarter of the full amount at the very tips:
    a hint of pink on rotala's crown in fertilised water, where the plant
    otherwise stays green; the light and lean water colour it fully).
    *Leaves round the stem* (render only, `tank.wgsl`): opposite-leaved
    species are decussate. At alternate nodes (the narrow ones; the broad
    ones carry the upper cells) the pair points at and away from the viewer:
    a front leaf is drawn over the stem at the node and the internode cell
    above it, and the side pair is a texel shorter at its tip, seen at an
    angle (its cells still grow, so nothing invisible blocks a swimmer). For
    cabomba the front leaf is a fan seen face-on. `[look] arrangement`
    (`opposite`, the default, or `whorl`) is a data option; `whorl` draws
    every node's feather leaves as a lacy disc round the stem and no shipped
    species uses it yet.
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
    `spacing` (at most 62 cells, the position bits' limit; `spacing` is
    2–48). With `runner_depth` (1–8, rosettes only; tiger lotus type) a
    runner instead dives to that many cells below the bed's top layer,
    creeps on there (rising where the bed rises, going under rock), and
    from `spacing` on climbs back up to root its child where water is
    above, then stops. A plant at its mature size still grows its runners.
    `[growth] root_feeder` makes only roots and runners sample nutrients.
  - **Carpet:** low tufts; runners creep through the top substrate layer
    and sprout a new tuft every `spacing` cells. *T6c:* `[carpet]` is a
    rosette with a one-cell crown and blades of at most 3 cells; each tuft
    is its own plant.
  - **Floater:** leaves in the top water row under the air band, with
    roots hanging `root_length` cells; buds new plants alongside, drifts
    with the current and shades what is below it through the light field.
    *T6c part 2 (D22):* a floater is a **buoyant body**, not a cell rule.
    `[floater]` sets `fronds` (floating leaf cells side by side, at most
    8), `roots` (hanging strands) and `root_length` (at most 32 cells).
    Its leaves are the `floating_leaf` part material (density 0.75,
    opacity 0.95), grown only into water open to the air above, so they
    keep to the top water row; roots hang straight down through water.
    The plant table owns the plant (species, health, part counts, growth,
    cutting, death); a **raft** beside the table, by plant id, holds only
    its motion (sub-cell offset and velocity, as a rigid body's, D17,
    T4c). Each tick, with the rigid bodies, an awake raft feels its
    leaves' buoyancy against the fluid beside each row, is pulled towards
    the current's mean speed over its cells, and moves a cell at a time
    into fluid or floating cells (which take the cells it leaves), stopped
    by grains, solids, bodies, other plants and the glass; it sleeps at
    rest and wakes when it grows, is cut, work reaches its chunks or the
    current changes. Because the raft holds no reference shape, growth on
    the moving plant needs no re-anchoring: the next step works on
    whatever cells it has. Once it has its fronds (or its mature size) it
    **buds** a new plant (one floating leaf beside an end frond; parent id
    set, generation + 1, moving as its parent does), never past the plant
    cap. A floater lies in one depth band (D21), from its plant id, so
    fish pass under it in the other bands. Tools: a scoop or cut splits
    it as any plant (a piece holding a floating leaf lives on as a
    fragment with its own raft, moving as its parent did, and a floater
    left with no floating leaf dies); the net takes
    only creatures. Duckweed (*Lemna minor*) and Amazon frogbit
    (*Limnobium laevigatum*) ship (ids 13, 14). Rafts are saved (format
    version 7) and hashed with the plant table, so a saved tank drifts on
    exactly.
  - **Epiphyte and mat:** attaches to rock, driftwood or glued bodies by
    a creeping rhizome with leaves (anubias type) or as a mat (moss), and
    falls off when nothing holds it. *T6c part 3 (D24):* `[epiphyte]` sets
    `on`, `rhizome_length` (1–32), `spacing` (rhizome cells between
    leaves), `leaf_length` and `lean`. A fragment touching `on` roots as a
    `rhizome` cell (id 80, counted with stems); the rhizome creeps as a
    single strand over cells touching `on` and puts up upright blade leaves
    (`pad` for anubias, id 15; `sword` for java fern, id 16). `[mat]` gains
    `layers` (1–4; java moss 1, christmas moss 3), storing each cell's
    layer in its variant position; outer layers stay while touching a
    nearer layer. Both epiphytes and mats stay on the grid and are **torn
    off** when a moving body leaves them behind (D24): an unsupported
    rhizome or mat cell falls off as dead plant matter, cascading through
    outer mat layers, and an epiphyte piece left without a rhizome dies.
  - **Ball** (issue #45, marimo type): `[ball] radius` (1–8). A fragment
    roots as one `stem` cell where it rests on anything not fluid, and
    the ball grows into the water beside it within `radius` + ½ of its
    cells' mean and a `2 · radius + 1` box, nearest the middle first
    (sites touching more of its cells count as a little nearer), so it
    stays round; each cell's position is how far out it grew. Like a
    floater it is one body with a **raft**, but its cells weigh their
    density less the fluid's everywhere, so it sinks to rest on the bed;
    it grips the current a third as hard and rolls at most at a quarter
    of its speed, so a current nudges it along the bottom. A cut piece
    lives on as a ball of its own. A poured disc of fragments coalesces
    into one ball (or a few) rather than rooting separately. A ball
    **rolls down a slope** of the bed, tracking a roll angle (a clockwise
    turn, saved with the raft) that advances as it moves. Drawn with the
    `velvet` stem shape: a sphere lit from above by how far the ball runs
    on each way from a cell, its rim fuzzed and corners rounded; the
    velvet texture (pile grain, rim tufts and cell mottling) is fixed to
    the ball and turns with its roll angle (carried in the plant texture's
    blue channel), clockwise as it rolls right, while the lighting stays
    fixed from above and to the left. Marimo (*Aegagropila linnaei*, id
    17) ships.
  - **Leaf pads** (issue #45, tiger lotus type): a rosette with `[rosette]
    pad` (1–4) has petioles in place of blades (`lotus` leaf shape). Each
    leans out by the column it rose from (every `lean` cells from the
    crown, twice as often from beside it, …) and stops at `blade_length`
    (the one from the crown's first cell) or 55–100% of it (by plant and
    column), or where it is blocked, such as under the air. There it
    spreads a leaf pad sideways along its tip's row, `pad` cells each way
    (positions past `blade_length`), with a narrower upper row under
    water; at the surface the pad is `floating_leaf` cells, which shade
    the water below. Pads die back first. Tiger lotus (*Nymphaea
    zenkeri*, id 18) ships.
  - **Propagation:** runners and buds make child plants (parent id set);
    mature plants of seeding species drop seeds now and then; the palette
    pours a species' seeds or places one of its cuttings, fragments,
    runners or buds (one it lists; seeds are powder, the rest discrete
    plants, see *Discrete propagules*); fragments also come from
    cuts. *T6c:* a species listing `runners` (rosettes and carpets) spreads
    by them, and the palette's runners place a plantlet; healthy mature (or topped-out) plants of seeding species
    drop seeds into adjacent water now and then while below the plant cap.
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
    *T6d part 2:* the plant texel is (health, colouration, spare,
    species), each 0–1 value at 32 steps, so the plants version changes
    only at a step. The shader tints from health (pale and yellowing to
    0.5, browning to 0, keeping each shade's lightness; below 0.3 the
    plant turns translucent, down to 35% opaque, over what is behind it)
    and shifts a species' leaves (stems half as much, roots not at all,
    lighter and newer shades most) towards its colouration colour, a
    texel after the sizes in the species texture. Colouration is sim
    state: an optional `[colouration]` in the species TOML (`colour`,
    `light`, `nutrient`) sets a target from the mean light and nutrient
    at each plant update (rising over 0.25 above `light`, and from
    `nutrient` down to half of it), and the plant's colouration moves
    towards it at 1/120 per second (holding within a tint step of a target inside (0, 1)), so sampling noise doesn't flicker it.
    It is saved (format version 8) and hashed when non-zero. Rotala
    (light 0.65, nutrient 0.8: an aquasoil tank's 0.4 is lean for it)
    turns pink-red near the surface and stays green when fertilised;
    ludwigia (0.7, 0.6) deepens to crimson.
    *T6d part 3:* night folding is render-only in `tank.wgsl`, driven by
    the existing darkness uniform (`view.daylight.w`, 0 by day to 1 at
    midnight): stem leaves tuck inward toward their stem and fold upward
    as darkness rises, broad rosette/epiphyte blades and pads narrow, and
    leaves dim slightly, with zero daytime cost or chunk wakes. Pearling
    runs in `visit_plant_cell` on submerged leaf cells with water above
    when `health >= 0.8` (`PEARL_HEALTH`), `light >= 0.65` (`PEARL_LIGHT`)
    and `co2 >= 0.6` (`PEARL_CO2`), releasing `air_bubble` cells at a
    hard tank-wide cap of 4 bubbles/s (`PEARL_MAX_PER_SECOND`: at most 1
    bubble every 5 ticks, needing no extra saved state). Optional sway
    is render-only (`view.target_size.zw` holds the world-time phase and
    enable flag, keeping the 160-byte `View` uniform unchanged), gently
    displacing leaves and upper stems (`pos >= 4`, keeping roots, runners,
    rhizomes, floaters, crowns and mats anchored) in a smooth wave across
    water cells and matching sprite occlusion (`hidden_by_plant`). The
    horizontal current leans swaying parts downstream by up to 1¼ cells
    past the wave, and stronger lean halves the wave. The lean is built up
    from the floor through each field row's current and fades slowly above
    it, so a plant's top above a jet stays bent (`sway_lean`, uploaded
    with the flow texture when the current is rebuilt). Still water keeps
    the plain wave. It is
    off by default (`Tank::set_sway`, `--sway` / `S` in `desktank window`,
    `--sway` in `desktank capture` and `desktank bench`, and **Plant sway**
    in web Settings) and capped at 10 fps (`SWAY_MAX_FPS`). When the host
    detects battery power, sway also requires a separate opt-in, off by
    default (`--sway-on-battery` / `Shift+S` in `desktank window`, and
    **Allow plant sway on battery** in web Settings). The web stores both
    preferences in `localStorage`. Sway remains disabled in screensaver
    mode, while hidden, or when the tank has no plants. The host's power
    and visibility policies still govern the tank's pacing and pauses.
- **Interaction.** Each plant species is a palette item under Plants
  (§2.5), in `desktank window` and on the web. A click pours seeds or
  places one piece of the species' default propagule; `P` (and, on the web, a row of chips under the
  items listing that species' methods: seeds, cuttings, fragments,
  runners, buds, then **juvenile** and **mature**) picks another,
  remembered per species. Juvenile and mature place a whole plant of the
  species in one click, grown to about 38% or 95% of its mature size with
  its roots, stems, leaves, runners or crown the way growth would shape
  them. Like a device, a whole plant shows a see-through **placement
  ghost** at the pointer: its cells in the species' own part colours,
  snapped to where the species can stand (a rooted plant's base to open
  water above a substrate it roots in; an epiphyte or moss mat to open
  water touching what it clings to; a floater to the surface; a marimo to
  rest on what is below it) and tinted red where it can't be placed (no
  suitable spot, or the tank is at its plant cap). `X` (on the web, a
  **Flip plant** button) mirrors it left to right, as it does a device
  for the other glass. The ghost is drawn only while one is shown, so it
  costs no frame when nobody is placing a plant. Inspect shows species,
  age, health, size and what is limiting growth (for example, "light too
  low"). The water-quality score counts plant cells from the table.
