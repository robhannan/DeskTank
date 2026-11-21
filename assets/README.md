# Assets

Data and art embedded in the DeskTank binaries with `include_bytes!` (SPEC §4.1):

- `materials/*.toml` — material definitions
- `reactions.toml` — reaction rules
- `species/*.toml` — creature species
- sprites — PNG atlases plus their source files

`materials/` holds the palette (SPEC §2.2), `reactions.toml` the reactions, decay, plant growth, wilting and algae rules, and `fields.toml` the field, device, water-change and water-quality settings; see SPEC §2.1 for units. A material, plant species or creature species with `palette = "top/sub"` (such as `"build/substrate"`, `"plants/stem"` or `"animals/fish"`; just `"water"` or `"care"` for a top category with one sub-category) is offered in that palette category (SPEC §2.5); one without isn't. The tree and its names live in `crates/desktank-sim/src/palette.rs`, and a path that names no category, or one holding another kind of asset (a material under `animals/…`), fails to load with a message listing the valid paths. The values are our own. Placeholder assets are licensed MIT OR Apache-2.0, like the code.
