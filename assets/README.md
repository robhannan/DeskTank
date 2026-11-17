# Assets

Data and art embedded in the DeskTank binaries with `include_bytes!` (SPEC §4.1):

- `materials/*.toml` — material definitions
- `reactions.toml` — reaction rules
- `species/*.toml` — creature species
- sprites — PNG atlases plus their source files

`materials/` holds the T1 test palette (water, air, sand, gravel, rock, air bubble, flake food); see SPEC §2.1 for units. The values are our own. Placeholder assets are licensed MIT OR Apache-2.0, like the code.
