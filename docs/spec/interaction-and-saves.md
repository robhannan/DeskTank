# Interaction, Time & Persistence

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
  - a small pixel-art palette appears in a screen corner. It is a
    two-level tree: a row of top tabs, a row of the open tab's
    sub-tabs (none where a tab has only one sub-category) and the open
    sub-category's items:
    - **Tools**: scoop, net, inspect, glue, magnify;
    - **Build**: Substrate (sand, gravel, aquasoil, clown vomit gravel),
      Hardscape (rock, driftwood), Devices (bubbler, internal filter: a see-through
      placement ghost at the pointer and a flip key (X) or button for the
      other glass; see [devices](devices.md));
    - **Water**: water, air bubbles, CO₂ bubbles;
    - **Food**: Fish food (flakes, pellets, frozen bloodworms, algae
      wafers, frozen brine shrimp), Live (live bloodworms, live brine shrimp);
    - **Care**: root tabs, liquid fertiliser, waste, detritus;
    - **Plants**: Stem, Rosette, Carpet, Floating, Moss & epiphytes
      (one item per plant species);
    - **Fish**, by stocking role: Schooling (neon tetra, harlequin
      rasbora, guppy), Centrepiece (angelfish, betta, flowerhorn, honey
      gourami, pea puffer), Coldwater (common and fancy goldfish),
      Clean-up crew (corydoras, kuhli loach, bristlenose pleco,
      otocinclus);
    - **Invertebrates**: Shrimp & crabs, Snails;
    - **Amphibians**: the frogs and the axolotl (one sub-category, so no
      sub-tab row).

    The stocking tabs have one item per stocked species; cheat-only
    species aren't offered.

    The open tab opens into a lighter panel holding its sub-tabs and
    items, and sub-tabs are framed in their own colour, so the two
    levels never read as one row of the same kind. Every tab, sub-tab
    and chip is square-cornered pixel art; on the web the sub-tabs and
    propagule chips are smaller, lighter bevelled buttons, the open one
    drawn pressed in.
    Chaos materials (not built yet, §2.2) will name categories too and
    be offered only while their setting is on.
    `G` / `Shift+G` step the top tabs, `H` / `Shift+H` the sub-tabs, `1`–`0`
    pick within the open sub-category, `P` picks how the selected plant
    starts, and scrolling over the palette steps through the
    sub-categories on into the next tab. Where an asset is offered is
    data: `palette = "top/sub"` (or `"top"` for a tab with one
    sub-category) in its material, plant or species TOML, checked at
    load against the tree in `desktank_sim::palette` (an unknown path, or
    one for another kind of asset, is a load error naming the valid
    ones); assets without one aren't offered.
    `desktank_sim::tools::Tools::palette_tree` gives every host the same
    tree.

    Selecting a creature or plant shows its **species card** below the
    palette (top-left on the web, whose palette is DOM): its name, Latin
    name, sprite (male and female side by side where they are drawn
    differently; a sprout in a plant's colours), adult size (both sexes'
    where they differ), diet, behaviour, needs (or a plant's form and
    needs) and description, all from the species data. It is drawn in
    the canvas on every host from a second region of the HUD texture, in
    a small CPU-built pixel font (`desktank_render::font`, shared with
    the in-tank inspect card), so it needs no new pipeline and is
    WebGL2-safe. It is built only when
    the selection changes and costs nothing while nothing is selected.
    Above pixel scale 1 it covers at most half the canvas each way, so a
    phone keeps most of the tank clear. Clicks on it never reach the
    tank; a click closes it until another species is selected.
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

    Inspecting pins the **inspect card** beside what it finds (a
    creature, plant, device or cell), drawn in the canvas: to its right
    (or left, nearer the right glass), centred on it, following it as it
    swims and clamped inside the tank. Page 1 is the live state: a
    creature's name, stage and sex, age, health, energy, colour morph,
    days left carrying and any unmet need; a plant's age, health, size
    by part and what holds it back; a device's mount and, for a filter,
    flow, media load, maturity and intake guard (an air diffuser or a
    floor panel's connection otherwise); a cell's material and what kind
    of material it is. Creatures and plants have a second page, their
    species card, and then both pages are numbered in the top-right
    corner and a click on the card turns the page. Devices and materials
    have no encyclopedia entry yet, so theirs is one page. The next
    inspection moves the card, picking another tool removes it (so it
    can't catch that tool's clicks), and it goes by itself when its
    creature, plant or device does. It is drawn from a third HUD-texture region at
    half the cell scale (at least 2, at most half the canvas each way),
    and the live page is rebuilt only when its text changes, never per
    frame; nothing is built or drawn while nothing is inspected. It
    needs only the tank (`Tank::set_inspected`), so the screensaver and
    wallpaper can draw it too once they have a way to open it. Nothing
    else repeats what it shows: the window's title bar keeps only the
    keys that act on it (N names a creature; R, Shift+R, M and I for a
    filter), and the web's readout line is left for messages (refusals,
    cheat results), with the inspection read out to screen readers only.

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
    naming); the web uses a text input and shows the result in the
    readout line. A web viewer tab can't
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
  A gap under a minute or a clock that went backwards is skipped. Only
  the host that owns the saved tank catches up. The window measures closed
  time from the save's modification time and the web uses its IndexedDB
  save timestamp (`savedAt`).
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
      version 6, T9 part 3, migrating version 5 creatures unnamed; each
      creature's quarry, pursuit counter and fright in version 9, hunting
      and territory, migrating version 8 creatures calm; whether each
      creature is asleep for the night and how long it stays up after
      being woken in version 15, migrating version 14 creatures awake;
      the pane each creature crawls in version 16, #371, migrating
      version 15 creatures off the panes);
    - each floater's raft, its motion as a buoyant body (offset,
      velocity, rest ticks, awake; version 7, T6c part 2, D22; a version 6
      tank had no floaters and migrates with none);
    - each plant's colouration (version 8, T6d; a version 7 tank's
      plants migrate uncoloured);
    - the sub-cell outline of rigid cells (rock, driftwood) and of the
      rigid bodies' reference shapes: a 4×4 coverage mask per partial cell,
      stored sparsely (version 10; a version 9 tank's rigid cells migrate
      whole, drawn square as before);
    - per-cell driftwood waterlogging stored in the upper seven bits of
      `Cell::variant` (version 11; pre-v11 driftwood cells migrate to dry
      while preserving glue);
    - the device table (version 12, D27; loading version 11 or older
      turns legacy filter outlet cells into internal filters, see
      [devices](devices.md) *Migration*);
    - the surface and glass algae planes, each byte a variety and a
      density (version 17, D28, #369). Older saves replace algae material
      cells with water and start with bare surfaces;
    - each creature's swim pitch (version 18, #98). Older saves start
      with level swim poses;
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
