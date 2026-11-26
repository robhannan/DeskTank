# Creatures

### 2.4 Creatures

- **Species are data** (D8). A TOML entry plus a sprite sheet defines:
  - scientific name (`scientific_name`) and a short species card description
    (`description`);
  - size;
  - zone: surface, mid-water, bottom or surfaces;
  - temperament: schooling, shoaling, peaceful or territorial;
  - diet: which food it eats and surfaces or materials it grazes, such as algae film and
    detritus, and which plant species (by their palatability, §2.7). A
    species that grazes algae can set how well it takes each algae variety
    (`grazing`, a percentage of a full bite, [world](world.md) *Algae
    varieties*): otocinclus rasp diatoms off at once, shrimp take soft
    films but not hard spots or dust, and only a nerite takes some green spot
    algae;
  - lifespan and maturity age;
  - breeding: egg layer (and which egg material) or livebearer;
  - tolerances;
  - colour variants.

  v1 ships about 8 real freshwater species across fish, shrimp, snails
  and a frog. Examples of the kind we mean: a schooling tetra, a
  livebearer such as a guppy, a betta, a corydoras, cherry and amano-type
  shrimp, a nerite-type and a ramshorn-type snail, and a dwarf frog that
  surfaces to breathe. Fictional species are a stretch goal.
  *Stocked species (23):* neon tetra, betta, corydoras, nerite, cherry
  shrimp (*Neocaridina davidi*), amano shrimp (*Caridina multidentata*),
  Thai micro crab (*Limnopilos naiyanetri*, a surface crawler beyond
  the examples above), honey gourami (*Trichogaster chuna*, an
  air-breather like the betta), guppy (*Poecilia reticulata*, the
  livebearer, with female frames since T9 part 2), harlequin rasbora
  (*Trigonostigma heteromorpha*, a second schooler) and otocinclus (a
  sucker-mouth catfish that crawls surfaces like the nerite). Common goldfish, fancy goldfish, angelfish, pea puffer and flowerhorn are also available. Bettas have heritable red, royal blue and copper colours. Bristlenose plecos and both mystery and ramshorn snails graze submerged surfaces. Kuhli loaches and axolotls forage on the bed; axolotls have heritable wild, leucistic and golden albino colours. African dwarf frogs cross the water column to breathe, while common frogs stay near the surface. Ten species keys extend the list above: `reach`
  (cells past the body it eats from, default 1; the crab's filter-feeding
  fans reach 2), `hop_every` (seconds between a bottom walker's short
  hops off the substrate, for shrimp), `swim_every` (seconds between a
  bottom dweller's chances of a short swim: shrimp, plecos, loaches and
  corydoras can swim but would rather not; snails and crabs, without it,
  can't), `burrow_every` (seconds between a bottom walker's chances to
  burrow into loose substrate, never rock: the kuhli loach; digging
  moves the grains it displaces onto spoil heaps beside it and draws them
  back over its body, conserving the substrate), `top_frames` and
  `under_frames` (with their fine frames: a crawler's look from above on
  the back pane and from below on the front pane, #371), `bands` (the depth bands it
  keeps to, front and back, D21; every band by default), `rests` (where
  it rests at night, *Night* below), `shy` (seeks cover and startles
  easily, false by default), `curious` (investigates the cursor and
  nearby pours, false by default), and `skittish` (tail-flips away from
  larger swimmers, false by default, allowed only for bottom walkers
  with `hop_every`). A species can enable only one of
  `shy` and `curious`. Every species also gives its
  real-world body length, `length_cm` (nose to tail without antennae or
  an outsized fin tail such as a male guppy's; a crab's carapace), and is drawn at a shared 2 cells per cm, so species
  sit at true relative sizes: Thai micro crab 0.5 cm (legs spanning
  1.5 cm), cherry shrimp 2.5 cm (a female 3 cm), guppy 3 cm (a female 4.5
  cm, a male 4.5 cm with its tail), neon tetra and otocinclus 3.5 cm, amano
  shrimp 4 cm, harlequin rasbora 4.5 cm, honey gourami 4.5 cm (a female 5
  cm) and betta 6 cm (a female 5 cm).
- **Behaviour** is steering-based:
  - wander;
  - schooling for schoolers: loose and irregular, speeding up and slowing
    with the lead's pace, splitting now and then into groups that travel
    apart (a sub-lead each) and rejoining, with followers always aiming
    inside the zone and the side walls;
  - shoaling for shoalers: each wanders on its own and often picks a
    target near a mate of its own stage (young with young, adults with
    adults). A straggler aims near its closest such mate. Surface
    crawlers head towards their mates on short swims when far away;
  - zone preference;
  - bottom-walking for corydoras and shrimp, with short swims now and
    then for bottom dwellers that can swim, and burrowing into loose
    substrate for kuhli loaches. During the rising half of a swim, a kuhli
    loach blocked by rock swims upward along its face and resumes its
    forward arc once its body clears the lip;
  - face-climbing for hungry shrimp: food out of reach above the bed (a
    film on a rock face, flakes by the glass) sends a shrimp to the face
    that holds it, in that face's band. It takes hold turned like a
    crawler, steps along the face towards the food (only where it gets
    closer, giving the food up where it can't), and grazes as it goes.
    Fed, frightened, asleep or out of food, it lets go and drifts back
    down to the bed;
  - surface-grazing for snails on glass, rock and plants, turned in
    quarter turns so the foot rests on the face they hold (walls,
    overhangs), with the collision box turned too;
  - pane-crawling for crawlers with pane art (#371; the nerite first): a
    crawler also spends some of its time on the front or back glass.
    For a minute in every two and a half (staggered by creature),
    upright on the bed or a top face, it shifts to the nearer pane's band
    (the front band or the back band) and takes hold of the pane. There it
    wanders the water in four headings (quarter turns of its pane sprite,
    which faces right like every sprite; diagonals are #194), grazing as it
    goes. Its trip over, or frightened, it heads down and steps off onto
    whatever stops it. On the front pane it is drawn from below (its
    `under_frames`), on the back pane from above (its `top_frames`), at its
    pane size (both views share one, head right). The net lifts it off;
    asleep it holds the pane;
  - surfacing for frogs;
  - finding and eating food cells, and grazing algae and detritus (the
    clean-up crew);
  - avoiding the cursor (optional, D8);
  - resting at night (*Night* below);
  - hunting for predators, fleeing to plant cover for their prey, and
    holding a territory for territorial species (*Hunting and
    territory* below).

  Creatures collide with solid cells and with the substrate surface.
  While swimming they point along their velocity in eighth turns
  (level, diagonal or vertical), with separate entry and exit thresholds
  to steady the pose and a low-speed dead zone that holds the last pitch.
  Bottom dwellers that take short swims pitch during those bouts and
  return to their ground pose when they settle. Pitch rotates the sprite
  about its centre after mirroring and leaves its collision box upright.
  Mirroring needs the same sideways speed as before, so a fish slowing on
  arrival keeps its face. Each tick its art is levelled as far as needed
  for the rotated box to lie in open water (clear of the bed, hardscape,
  plants in its band and the air). Sleepers ease level an eighth turn a
  tick where they settle, and on the way if they drift too slowly to pitch.
  Save format v18 stores this pitch and older saves start level.

  The tank is 2.5D (D21): it is eight **depth bands** deep, front glass
  to back. Plant cells have a band (a plant's stem keeps one, its leaves
  are up to a band off it, a rosette's blades fan up to two bands either
  way); a creature has a depth that eases between bands and collides with
  a plant cell only within one band of it. Rigid bodies (currently rock
  and driftwood) occupy a contiguous range of bands. A swimmer
  can pass in front of or behind a body wherever its band is free.
  Substrate, glass and air span every band, and so do food and other loose particles, so a
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
    (nerite, amano shrimp, Thai micro crab). `prey_cm` makes a predator
    and must be shorter than both sexes' body lengths. `[genetics]` lists
    `[[genetics.morph]]` entries in
    allele order (append only), each recolouring the sprite by key, and
    names the `wildtype` and the `sprite`'s own morph (the one the palette
    adds); without it a species has one morph, `wild`. `[sprite]` may give
    `female_frames` and `female` colours for a species drawn differently
    by sex. Female frames match the male frame count and can have their
    own even width and height, up to 64 texels per side. They set her
    body size in cells. The top-level `female_length_cm` defaults to
    `length_cm` and must fit the female sprite at the same scale. A
    livebearer with a female look (a shrimp carrying her eggs breeds as
    one) can also give `carrying_frames` to draw her while she carries.
    They match the female size and frame count.
    Both frame sets require their corresponding `female_fine_frames` or
    `carrying_fine_frames` at twice the width and height.
    Real figures, broods scaled down for
    a desktop tank: guppy livebearer 28 days, 3–8; cherry shrimp carries
    her eggs 30 days, 3–8; neon tetra and harlequin eggs 1 day, corydoras
    4, betta 2, honey gourami 1, otocinclus 3. Cherry shrimp morphs red
    (palette), wild (brown, the wildtype), blue and yellow; guppy strains
    wild (palette and wildtype), red and blue, and a larger, plainer,
    deeper, short-tailed female; betta morphs wild (red; palette and
    wildtype), royal blue, copper, koi (red and black patches on white),
    marble (blue and red patches), butterfly (white-edged fins), yellow
    and lavender. Cherry shrimp are drawn by sex too: a small, slim, paler
    male, a larger female with a rounder belly and a berried female whose
    yellow-orange eggs (keys no morph recolours) show under her abdomen.
    Predators: betta 2.5 cm (fry, male and young cherry shrimp, crabs: an
    adult female shrimp, at 3 cm, outgrows it), honey gourami
    1.2 cm (fry, shrimplets, crabs) and guppy 1.6 cm (so newborn fry of
    either sex, a female's a third of her 4.5 cm).
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
    predator is fed (an hour of energy). Neither may be in the net, and
    prey in plant cover is safe. Predators chase their prey and prey flees
    (*Hunting and territory* below).
  - **Drawing.** Each creature and corpse is drawn in its look (morph,
    sex and carrying state), two atlas strips per look: its frames, then its
    recoloured fine frames at four texels per cell. Strips pack side by side
    on shared rows, so the many looks stay inside WebGL2's 2048-texel atlas
    limit.
  - **Save format v3** adds each creature's sex, alleles, pregnancy and
    rest, each corpse's morph and sex, the clutches and every species'
    morph names (checked on load: a species' morphs may only be appended
    to). v2 saves migrate: every creature shows its palette morph, sexes
    alternate within a species in id order, none is carrying, and each
    settles for a day.
  - **Scripts** take `sex`, `morph` and `carrying_days` (a mature female
    livebearer, at most her species' gestation) in `[[creature]]`; `World` has
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
- *Hunting and territory* (`desktank_sim::pursuit`; the strike itself is
  T9 part 2's predation, unchanged):
  - **Sensing.** Every 10 ticks (offset by its id) a creature looks round
    a start-of-tick snapshot of the tank's creatures, built only while the
    tank holds a predator (outside easy mode), a territorial, shy or skittish creature,
    or live food and one of its eaters. The cost is at most n²/10 distance
    checks a tick.
  - **Hunting.** A hungry adult predator, outside easy mode, not in the
    net, not heading for food and rested picks the nearest creature it
    could eat (no longer than its `prey_cm`) within 40 cells (20 cm) that
    isn't in cover and chases it: it stalks halfway between cruising and
    top speed, dashes at top speed within 12 cells and shifts to its
    prey's band. It gives up when the prey is gone, more than 56 cells
    away, in cover, out of reach (its way is blocked), after 12 s or once
    fed, then rests 5 s before another chase.
  - **Fleeing.** A creature that a hungry adult predator within 14 cells
    could eat takes fright for 3 s, renewed while the threat lasts. It
    heads for the nearest cover within 16 cells (the middle of five plant
    cells in a column in one band: a stem or blade), taking the band two
    behind it (or two in front at the back of its range), or else runs 24
    cells straight away; already in cover it holds still. It flees at its
    top speed scaled by its size, at least its cruising speed, so fry are
    slow.
  - **Startle.** A calm schooler within 1.5 of its species' lengths of a
    frightened schoolmate catches the fright, 4 ticks shorter than the
    mate's, and runs the way that mate is heading. Each looks every 3–8
    ticks (its own reaction time), so the fright ripples outward through
    the school, fading as it goes, rather than reaching every fish at once.
    A loose shoal doesn't pass a fright on.
  - **Shyness.** Honey gouramis and kuhli loaches keep to the back bands
    and draw up to three wander targets, taking the first in plant cover
    or the last draw if none is sheltered. A calm, awake shy creature
    startles at an approaching swimmer at least 1.5 times its grown
    length, within three of its own grown lengths (at least 8 cells before
    its boldness is applied). An awake shy creature also startles when
    the keeper pours something it does not eat within 12 cells. It holds
    still if already in cover, seeks cover, or runs away as when hunted.
    It avoids an enabled cursor within five body lengths.
  - **Curiosity.** Pea puffers, bettas and fancy goldfish swim towards an
    enabled cursor within six grown body lengths. They cruise to about
    one and a half lengths away and hover facing it, holding station
    even against the current but still eating what is under its mouth.
    Fleeing, surfacing for air, heading for food and a chase (a betta
    driving off a rival) take priority. Each fish is interested for 20
    world seconds in every 50, at a phase of its own worked out from the
    tick and its id (nothing is saved, so a reloaded tank behaves the
    same). An awake, calm curious fish not in the net or the bed also
    heads toward a pour within 24 cells if it cannot eat the material,
    using its usual wander timer.
  - **Skittishness.** A calm, awake cherry shrimp tail-flips when a
    larger swimmer heads toward it within three of its grown lengths
    (at least 6 cells before its boldness is applied). For 0.5 s it
    moves away and up at twice its top speed, keeping its facing. It
    then sinks to the bed and ignores threats for 2 s. Amano shrimp
    stay bold, and a skittish shrimp ignores a hunter too during
    those 2 s.
  - **Cover.** A creature is in cover with 4 plant cells within 2 bands
    of its own inside 2 cells of its body. A predator neither sees nor
    strikes prey in cover.
  - **Territory.** A species with `temperament = "territorial"` may give
    `territory_cm` (2–50; betta 15 cm, 30 cells). An adult claims a home
    when it first senses (its column, at the nearest row of its zone) and
    wanders within that radius of it. It drives out intruders inside its
    territory and within 40 cells of it: surface and mid-water swimmers
    too big to eat, and adults of its own species and sex. It chases at
    top speed until the intruder is 6 cells past the edge or for 3 s,
    then turns for home and rests 5 s. An intruder chased within 14 cells
    flees straight away. Blocked outside its territory (driven out of
    reach of home, behind a stand of plants say), it settles a new one
    where it is. Territory is kept in easy mode too, as no one is hurt;
    there is no flaring or fin nipping yet.
  - **State.** Each creature's quarry, pursuit counter (chase ticks, or
    rest ticks without a quarry) and fright are saved (format version 9,
    migrating version 8 creatures calm) and hashed only when set; the
    territory is the creature's `home`.

- *Night* (`desktank_sim::night`, #195): creatures rest by the local
  time of day, which the host injects with the tank's lighting
  (`World::set_time_of_day`; noon by default, not saved).
  - **Bedtime.** Each creature settles at a bedtime of its own between
    21:00 and 22:00 and wakes at its own time between 06:00 and 07:00,
    both fixed for life by a hash of the seed and its id, so a tank
    drops off and wakes over an hour rather than as one.
  - **Where.** A species key `rests` says where: `bottom` (it settles on
    the bed below it: goldfish, the African dwarf frog), `plants` (it
    drifts to the nearest stem or leaf cell within 48 cells, or the lower
    part of its zone with none: the default for mid-water fish),
    `surface` (its body's top in the water's top row, where it can
    breathe: the default for surface fish), `in_place` (a walker sinks
    back to the bed, a crawler stops on the face it holds: the default
    for bottom and surfaces zones) or `never` (nocturnal: the kuhli loach
    and bristlenose pleco). Live food never rests. A swimmer drifts to
    its spot at a third of its cruising speed and holds still where it
    arrives or something stops it.
  - **Asleep** it skips steering, eating, habits and hunting, so once
    still it neither moves nor redraws: resting costs less than swimming.
    It burns energy at a quarter of the rate, so a well-fed creature lasts
    the night, and an air-breather needs a breath four times less often; a
    surface fish breathes where it rests, any other wakes, goes up and
    settles again once it has breathed.
  - **Waking.** It wakes at its wake time, or when disturbed: the net
    holds it, it takes fright, the avoided cursor comes within three body
    lengths (five for shy creatures), or the keeper pours something within
    12 cells of it. Pouring food it eats anywhere also wakes it. Woken
    at night it stays up for two minutes, long enough to eat, then settles
    again.
  - **State.** Asleep and the woken counter are saved (format version 15,
    migrating version 14 creatures awake) and hashed only when set, so a
    tank by day hashes as before.

- *Live food* (D25; `desktank_sim::world::creatures::live`): some foods
  are alive. A species with `live_food = "<material>"` is live food: a
  small creature moved by the usual systems, offered from **Food › Live**
  in the palette (a click adds a pinch of `PINCH` = 5, scattered within 3
  cells of the pointer) and never from Fish, Invertebrates or
  Amphibians, `spawn` or `scene::stock`. It never breeds (no `[breeding]`, `[genetics]`,
  `prey_cm`, territory or diet is allowed), and it doesn't count against
  the population cap: a tank holds at most `MAX_LIVE_FOOD` = 32 live
  foods beside the cap (the renderer draws up to cap + 32 + corpses).
  - **Eaters.** A species eats a live food when its diet holds the live
    food's material and it is at least as long, as grown: fish that take
    frozen bloodworms take live ones, with no diet change. A hungry eater
    (any age, in easy mode too, not in the net, not heading for a food
    cell) hunts the nearest one within 80 cells (40 cm, lost past 96) as
    a predator hunts prey, cover or not, and once a second swallows one
    its reach touches, whole, worth two bites of food (`LIVE_FOOD_BITES`).
    `prey_cm` doesn't apply to live food, and predation never takes it.
  - **Behaviour.** A live food takes fright only at a creature chasing
    it, and squirms straight away at its own slow top speed, seeking no
    cover.
  - **Death.** It ages and dies like any creature (old age, foul water
    outside easy mode) but leaves no corpse: it becomes one cell of its
    material in the lowest water cell its body covered, which rots like
    any food.
  - **Live bloodworms** (`live_bloodworm`, id 27, counting as `bloodworm`):
    midge larvae 1.5 cm long, an 8×4-texel (4×2-cell) sprite with a
    four-frame travelling-wave wriggle (16×8 fine frames). Bottom walkers
    that sink at 6 cells a second and crawl the bed at 0.8 (top 2) cells
    a second; 6 life days (about an hour of world time) from arriving
    grown. The frozen material keeps its id (`bloodworm`) and is labelled
    "Frozen bloodworms" on the web.
  - **Live brine shrimp** (`live_brine_shrimp`, id 28, counting as
    `brine_shrimp`): adult *Artemia* 1 cm long, with four poses of the
    tail and swimming limbs (6×4 coarse texels, 12×8 fine). They stay in
    midwater and alternate a half-second kick (top speed 3 cells/s)
    with a slow drift (0.5 cells/s). They live 2 life days and arrive
    at 1 day old. Frozen brine shrimp sinks and rots into detritus after five
    world minutes. Species that take bloodworms also eat brine shrimp.
  - **State.** The two species share the live-food limit. A save holds
    ordinary creature state for each live food and stores its frozen
    material by ID.
