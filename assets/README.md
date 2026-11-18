# Assets

Data and art embedded in the DeskTank binaries with `include_bytes!` (SPEC §4.1):

- `materials/*.toml` — material definitions
- `reactions.toml` — reaction rules
- `species/*.toml` — creature species
- sprites — PNG atlases plus their source files

`materials/` holds the palette (SPEC §2.2), `reactions.toml` the reactions, decay, plant growth, wilting and algae rules, and `fields.toml` the field, device, water-change and water-quality settings; see SPEC §2.1 for units. A material with `palette = "substrate"` (or `water`, `food`, `plants`, `waste`) is offered for pouring in that palette group (SPEC §2.5); one without isn't. The values are our own. Placeholder assets are licensed MIT OR Apache-2.0, like the code.
