#!/usr/bin/env zx
// Builds the web version into target/site/: the desktank-web crate for
// wasm32, wasm-bindgen's JS glue (--target web), wasm-opt if it is on the
// PATH, and the static files from web/. Then it reports the gzip transfer
// size of the site (wasm + JS + assets) and fails above the SPEC §6.1 budget.
//
//   pnpm install && pnpm build:web
//   python3 -m http.server -d target/site 8000
//
// Needs the wasm32-unknown-unknown target (added here if missing) and the
// wasm-bindgen CLI at exactly the version in Cargo.lock:
//   cargo install --locked wasm-bindgen-cli --version <version>
import { gzipSync } from "node:zlib";
import { $, cd, fs, glob, path, which, echo, chalk } from "zx";

$.verbose = true;

const root = path.resolve(import.meta.dirname, "..");
cd(root);

const TARGET = "wasm32-unknown-unknown";
const site = path.join(root, "target", "site");
const wasmIn = path.join(root, "target", TARGET, "release", "desktank_web.wasm");

// The CLI and the crate must agree exactly, or the glue won't load.
const lock = await fs.readFile("Cargo.lock", "utf8");
const want = lock.match(/name = "wasm-bindgen"\r?\nversion = "([^"]+)"/)?.[1];
if (!want) throw new Error("wasm-bindgen not found in Cargo.lock");
const cli = await which("wasm-bindgen", { nothrow: true });
const have = cli ? (await $`wasm-bindgen --version`.quiet()).stdout.trim().split(" ")[1] : null;
if (have !== want) {
  echo(chalk.red(`wasm-bindgen CLI ${have ?? "not found"}, crate needs ${want}.`));
  echo(`Install it with: cargo install --locked wasm-bindgen-cli --version ${want}`);
  process.exit(1);
}

const targets = (await $`rustup target list --installed`.quiet()).stdout;
if (!targets.split("\n").includes(TARGET)) {
  await $`rustup target add ${TARGET}`;
}

// The workspace release profile strips everything, which also drops the
// `target_features` section wasm-bindgen reads; strip only debug info.
await $`cargo build --locked --release -p desktank-web --target ${TARGET} --config ${'profile.release.strip="debuginfo"'}`;

await fs.remove(site);
await $`wasm-bindgen --target web --no-typescript --out-dir ${path.join(site, "pkg")} ${wasmIn}`;

const wasmOut = path.join(site, "pkg", "desktank_web_bg.wasm");
if (await which("wasm-opt", { nothrow: true })) {
  // Rust's wasm32 target enables these proposals by default; wasm-opt
  // must be told, or it rejects the module.
  await $`wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-multivalue --enable-reference-types ${wasmOut} -o ${wasmOut}`;
} else {
  echo(chalk.yellow("wasm-opt not found; skipping size optimisation."));
}

await fs.copy(path.join(root, "web"), site);

// Transfer size (SPEC §6.1): every file as a browser would fetch it, gzipped
// at the highest level (a static host's on-the-fly gzip is at most this good).
const BUDGET = 3 * 1024 * 1024;
const kib = (n) => `${(n / 1024).toFixed(1)} KiB`;
let total = 0;
for (const file of (await glob("**/*", { cwd: site, onlyFiles: true })).sort()) {
  const raw = await fs.readFile(path.join(site, file));
  const gz = gzipSync(raw, { level: 9 }).length;
  total += gz;
  echo(`  ${file.padEnd(28)} ${kib(raw.length).padStart(12)} raw ${kib(gz).padStart(12)} gzip`);
}
echo(`Transfer size (gzip): ${kib(total)} of a ${kib(BUDGET)} budget.`);
if (total > BUDGET) {
  echo(chalk.red("Over the web transfer-size budget (SPEC §6.1)."));
  process.exit(1);
}
echo(chalk.green(`Site built in ${path.relative(root, site)}/.`));
