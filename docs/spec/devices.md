# Devices and prefabs

### 2.8 Devices and prefabs (D27)

Decision: [0027](../decisions/0027-prefab-devices.md). Work packages:
the `MD — Devices` milestone (DV1–DV7).

Equipment in the tank is a **prefab device**: a hand-drawn object with
its own state, placed and removed whole, rather than a blob of device
cells. Filters come first (four kinds); the bubbler follows (DV7).

- **Model.** The world keeps a **device table** (id → kind, anchor,
  facing, parameters, state), built like the rigid-body and plant tables.
  The device's **footprint** is every cell its art covers; those cells
  are solid cells of the `device` material holding the device's id in
  their `state`, so powders, bodies and creatures collide with it.
  Scooping (or overwriting) **any** footprint cell removes the **whole**
  device: its other cells become water under the water line and air
  above it. Kinds are data (`assets/devices/<kind>.toml`): name, label,
  mount, the nozzle (and later intake) cells, and the art. At most
  `MAX_DEVICES` (16) devices per tank. The state hash includes the
  table.
- **Art.** Each kind is a sprite in the house style (D19, [creatures](creatures.md) §2.4): its
  silhouette traced from a reference photo of the real device, then
  shaded (an outline in a dark shade of the local colour, never black,
  dithered light and shade bands, texel-level detail), with coarse frames
  at two texels per cell and fine frames at four. It is drawn facing
  right (mounted on the left glass) and mirrored on the right. The
  renderer draws footprint cells as the water behind them and the sprite
  on top, in the creature sprite pass: depth-sorted with creatures at
  `DEVICE_DEPTH` (just behind the front band), with plant occlusion as
  for creature sprites. Devices pack into the creature atlas, which stays
  within WebGL2's 2048-texel limit.
- **Mounts.** Where a kind may go:
  - *side glass:* against the nearer side glass at any height wholly
    under the water line (internal filter);
  - *rim:* hanging on the top edge, partly above the water line (HOB);
  - *bed:* dropped onto what is below, like the bubbler (sponge filter,
    prefab bubbler);
  - *floor:* a plate under the substrate with tubes at the back
    (under-gravel filter).

  Placement is whole or nothing: a placement whose footprint isn't all
  fluid cells (in the grid, under the water line for side-glass mounts)
  tries the nearest heights within the device's own height, then
  refuses.
- **The four filters** (reference photos supplied by the owner; not
  committed, as one is a copyrighted product advert):

  | Filter | Mount | Flow | Distinctive behaviour |
  |---|---|---|---|
  | Internal | Suction-cupped to the side glass at any height | Intake slots at the bottom, nozzle jet at the top | Closest to the old outlet |
  | HOB (hang-on-back) | Hangs on the rim, partly above the water line | Intake tube down into the tank, waterfall outlet at the surface | Ripples the surface |
  | Sponge | Sits on the bed and drops like the bubbler | Air-driven uplift: bubbles rise from its tube, gentle flow | Fry-safe; shrimp graze the sponge |
  | Under-gravel | Plate under the substrate, uplift tubes at the back | Pulls water down through the gravel and up the tubes | Interacts with substrate and roots (UGFs hurt rooted plants) |

  The references, by appearance:
  - *Internal:* a tall black vertical box, about three to four times as
    tall as wide, with two suction-cup knobs on its back, a short
    horizontal nozzle near the top jetting sideways, diagonal blue-tinted
    slot vents on the lower body and a small +/− flow slider.
  - *HOB:* a black boxy body hanging outside the glass on the rim of a
    rimless tank, a ribbed spillway lip pouring into the tank, and a
    black intake tube dropping into the water to a slotted, ribbed
    strainer.
  - *Sponge:* a black ribbed cylindrical sponge on a round weighted base,
    with a clear uplift tube on top and bubbles rising out of it.
  - *Under-gravel:* slotted grey plates covering the floor under the
    gravel, and a vertical uplift tube in a back corner rising to the
    surface, bending over into a horizontal spray bar held by suction
    cups.
- **Flow.** Until DV2 the current is the static, horizontal-only field
  at 1/8 resolution ([world](world.md) §2.1 *Devices*): each internal filter's nozzle tip is
  a jet source pointing away from its glass. DV2 makes it a 2D vector
  field with directed outlets and intake sinks.
- **Filtration** (DV3). Intakes capture suspended particles (food,
  waste, detritus, algae in the water) into the device's **media load**.
  Load clogs the filter and flow drops as it clogs, but **never below
  about 30%** at full clog: the screensaver and lock screen can't rinse,
  so an unrinsed filter must not kill the tank. A **Rinse** action
  clears the load. The media **matures** as bacteria colonise it; in
  simple mode (D3) a mature filter breaks waste down faster and raises
  the water-quality score. Rinsing hard or replacing the media sets
  maturity back. The real ammonia/nitrite/nitrate chemistry stays in E1 (#136),
  which takes over maturity as its bacteria colony.
- **Fry and shrimp.** Intakes suck in fry and small shrimp unless the
  filter's **intake guard** is on; the sponge filter is always fry-safe.
- **Depth and collision** (D21). A HOB's intake tube is in the back
  depth band, so fish pass in front of it. Internal, sponge and
  under-gravel bodies and tubes are solid in every band.
- **Interaction** ([interaction](interaction-and-saves.md) §2.5). The palette's Build › Devices sub-tab holds the filters
  and the bubbler as each lands (DV1: the internal filter, beside the
  legacy bubbler). Facing is chosen automatically from the nearer wall;
  the placement preview is a see-through **ghost** of the footprint at
  the pointer (tinted red where it doesn't fit), and a **flip** key (X)
  or button mounts it on the other glass instead. The inspect card (the
  one creatures use) shows a device's status: media load, maturity and
  flow (DV1 shows the flow only). Rinse and the intake-guard toggle join
  the card with DV3. Drag-to-move (keeping state) and flow/air dials come
  later. Non-interactive hosts (screensaver, lock screen) just run the
  devices.
- **Migration** (save format v11). Version 11 saves the device table.
  Loading a version 10 or older save turns each connected group of
  legacy `filter_outlet` cells into an internal filter at the group's
  centre on the nearer glass; a group with no room for one (boxed in by
  rock, say) is left as legacy outlet cells, which still drive the jet.
  Legacy `bubbler` cells stay as they are, and keep working, until DV7
  converts them into prefab bubblers. Nothing places outlet cells any
  more; scripts naming `filter_outlet` place an internal filter.
- **Resources** ([architecture](architecture.md) §6). An idle device adds no per-tick work of its own:
  the current is rebuilt only when a device is added or removed, intake
  work (DV3) touches only intake cells, and a device's state changes only
  when it works. The jet's current pass is the old outlet's: it scans the
  jet's blocks each tick for cells to carry (a settled tank still changes
  no cells and wakes no chunks); DV2 should skip blocks with nothing to
  carry. Sprites stay within the WebGL2 limits (one atlas, the creature
  instance buffer).
