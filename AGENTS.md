# DeskTank

Lightweight virtual aquarium desktop toy in Rust: wallpaper, screensaver,
lock screen and click-through overlay on macOS, Windows and Linux. It also
runs in the browser (the web) and on Android, led by the Android TV
screensaver (`docs/spec/platforms.md` §3.6, D23). Read
`docs/spec/README.md` for the spec index (`docs/spec/` by domain,
`docs/decisions/` for ADRs, `docs/notes/` for platform/technical learnings)
and GitHub Issues & Milestones for the roadmap, work packages, follow-ups
and open decisions.

- Build/test: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Keep the simulation/rendering core OS-agnostic; platform code lives behind `cfg(target_os = …)` boundaries.
- Minimal resource use is a core requirement: no uncapped redraws; pause when hidden.
- For features and fixes, follow the shared developer workflow (`.agents/skills/desktank-developer/SKILL.md`): one PR per increment, never commit to `main`. Prefer stacking PRs when multiple PRs are in flight or related (branch each upper PR from the lower PR's branch and target that branch as its PR base, rather than leaving overlapping PRs unstacked against `main`). The orchestrator assigns bounded implementation parcels to Sol (Codex), Sonnet (Claude), or Flash (Gemini) and owns integration, review and CI. In T3 Code use `delegate_task`; elsewhere use the named developer and reviewer agents as described in the skill.
- Backlog grooming follows `.agents/skills/desktank-product-owner/SKILL.md`, run weekly (Mondays after the 08:00 GMT usage reset) or on request. Issues carry a native Status (Backlog → Todo → In progress → In review → Done). The coordinator delegates drafting to `desktank-refiner` models, applies every GitHub change itself, and posts a summary to the pinned "Backlog grooming log" discussion.
- Toolchain: pinned exactly in `rust-toolchain.toml`; that version is the MSRV (`rust-version` in the root `Cargo.toml`). Bump both together.
- `Cargo.lock` is committed (binary project); CI builds with `--locked`.
- `desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. The workspace denies `unsafe_code`; only `desktank-platform`, `desktank-saver-mac` and `desktank-android` allow it, at FFI (JNI/NDK) boundaries with `// SAFETY:` comments.
- Lints live in the root `Cargo.toml` (`clippy::pedantic` warn, `-D warnings` in CI); every crate sets `[lints] workspace = true`.
- Web (`crates/desktank-web`, `web/`): `pnpm install && pnpm build:web` builds `target/site/` (needs the wasm-bindgen CLI at the `Cargo.lock` version) and fails over the 3 MB gzip budget; `pnpm test:web` runs the Playwright tests in `tests/web/` (CI runs them), `pnpm bench:web` measures frame intervals, `pnpm typecheck` runs `tsc` over the page, scripts and tests (CI runs it; after one `pnpm build:web`, which writes wasm-bindgen's declarations). The web is a product target (SPEC §3.5, D15): `desktank-sim` and `desktank-render` must build for `wasm32-unknown-unknown` (CI checks this from T2) and `desktank-render` must stay within wgpu's WebGL2 limits.
- Non-Rust code is TypeScript (`.ts`) wherever possible; JS only where a tool genuinely cannot take TS. The one other exception is the Android shim in `android/`, which is Kotlin (Rust can't subclass `DreamService` and friends; keep it thin, with no tank state). Strict `tsconfig.json`, erasable syntax only: Node runs `scripts/*.ts` by type stripping (relative imports keep their `.ts` extension), Playwright loads the tests directly, and `scripts/build-web.ts` emits `web/*.ts` to readable `.js` with `tsc` (`web/tsconfig.json`).
- Android (`crates/desktank-android`, `android/`): `cd android && ./gradlew assembleDebug` builds `libdesktank_android.so` with `cargo-ndk` and the debug APK (needs JDK 17+, the Android SDK with the NDK pinned in `android/gradle/libs.versions.toml`, `cargo install --locked cargo-ndk` and the `rustup` targets `aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`; `-Pdesktank.abis=arm64-v8a` builds one ABI). The JNI glue compiles only for Android: run `cargo clippy -p desktank-android --target aarch64-linux-android` (CI does). Android tests use Tapsmith (`pnpm test:android`, `tests/android/*.tapsmith.ts`, `tapsmith.config.ts`): build the debug APK first; it installs it on the attached device or emulator (set `ANDROID_SERIAL` if several are attached) and starts the Dream with adb. CI runs it on the API 36 Android TV emulator.
- Work in small increments; link/close the relevant GitHub Issue in your PR description, file follow-up issues for out-of-scope review leftovers, and only update `docs/spec/`, `docs/decisions/` (new numbered file), or `docs/notes/` when durable design or technical constraints change.
