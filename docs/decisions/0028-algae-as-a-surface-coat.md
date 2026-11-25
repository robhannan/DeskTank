# D28 · Algae as a surface coat

Algae grows as a translucent film on submerged substrate, rock, driftwood
and all four glass surfaces. The film leaves the water volume available for
creatures, plants and the current. It follows each surface cell as a rigid
body moves or rotates. D24 epiphytes occupy grid cells and can detach when
their support moves.

Each world cell has a coat byte with a four-bit variety and a four-bit
density. Green film uses variety 1. Varieties 2–15 are reserved for
[#369](https://github.com/robhannan/DeskTank/issues/369). Glass densities
use the fields' 1/8 resolution, with front and back planes and one strip
per side wall. The front pane is capped at 5 of 15 density levels so the
tank contents stay visible. See the [ecosystem rules](../spec/world.md#23-ecosystem-d3)
for growth, competition and decay.

Plants compete with film for nutrient and CO₂. The film does not model
either resource directly: plant competition (healthy plant cells against
the water-quality threshold) is scaled up per field block by the block's CO₂
level (`co2_boost` in `assets/fields.toml`), so plants that have CO₂
to grow with hold film back sooner. Film growth itself needs only
nutrient and light. Catch-up updates the field blocks in bounded, disjoint
groups and credits each group for the intervals it skips.

Format version 17 saves the four density layers (version 16 went to the snails' panes, #380). Earlier algae cells
become water on load, since a cell of the old material could touch several
different surfaces. Legacy material ID 76 stays reserved for those saves.
