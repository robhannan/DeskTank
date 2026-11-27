# Assets

Data and art embedded in the DeskTank binaries with `include_bytes!` (SPEC §4.1):

- `materials/*.toml` — material definitions
- `reactions.toml` — reaction rules
- `species/*.toml` — creature species
- `devices/*.toml` — prefab device kinds and their art ([`docs/spec/devices.md`](../docs/spec/devices.md))
- sprites — PNG atlases plus their source files

`materials/` holds the palette (SPEC §2.2), `reactions.toml` the reactions, decay, plant growth and wilting rules, and `fields.toml` the field, surface-film, device, water-change and water-quality settings; see SPEC §2.1 for units. A material, plant species or creature species with `palette = "top/sub"` (such as `"build/substrate"`, `"plants/stem"` or `"fish/schooling"`; fish are split by stocking role into `fish/schooling`, `fish/centrepiece`, `fish/coldwater` and `fish/cleanup`, invertebrates into `invertebrates/shrimp` and `invertebrates/snails`; just `"water"`, `"care"` or `"amphibians"` for a top category with one sub-category) is offered in that palette category (SPEC §2.5); one without isn't. A material's optional `palette_order` (default 0) sorts it within its sub-category, lower first and equal orders by ID, so a newer material can sit before an older one (the stones before `driftwood` in `build/hardscape`). The tree and its names live in `crates/desktank-sim/src/palette.rs`, and a path that names no category, or one holding another kind of asset (a material under `fish/…`), fails to load with a message listing the valid paths. The values are our own. Placeholder assets are licensed MIT OR Apache-2.0, like the code.
