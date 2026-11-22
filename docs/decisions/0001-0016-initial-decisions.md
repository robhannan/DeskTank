# D1–D16: Initial Design Decisions (2026-11-17)

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
| D15 | **Web is a product target**: the same tank in a browser tab on desktop and mobile, via wgpu WebGPU/WebGL2 (§3.5). Native mobile apps remain a non-goal (revised by D23 for Android). |
| D16 | **Memory budget is raw RSS ≤ 100 MB** (`ps` RSS, which counts shared GPU-driver pages), raised from 80 MB after T2 measured 90 MB RSS / 53 MB physical footprint. |
| — | Platform order macOS → Windows → Linux, with no Linux desktop preference. Lock-screen limits accepted. **Open source.** Tray + config only to start. §6.1 budgets accepted. Name to be confirmed. |
