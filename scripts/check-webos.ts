#!/usr/bin/env node
// Checks the legacy-Chromium web build (target/site-webos/, SPEC D26) for
// what LG webOS 5's Chrome 68 can't load, so it can't regress unnoticed:
//
// - every .wasm validates with only the MVP features (binaryen's wasm-opt
//   --mvp-features): no sign-ext, bulk memory, saturating float-to-int,
//   reference types or multivalue. It must also have no target_features
//   section: wasm-opt enables whatever that section lists on top of
//   --mvp-features, so a modern module would pass;
// - every .js parses as an ES module at Chrome 68's level: ES2018 syntax
//   plus import.meta and dynamic import() (Chrome 64 and 63), but no
//   optional chaining, ??, BigInt literals or `export * as` (acorn at
//   ES2020, then a walk that rejects the ES2019+ syntax Chrome 68 lacks).
//
// Run by `pnpm build:web -- --legacy`; standalone: `node scripts/check-webos.ts [dir]`.
import { fileURLToPath } from "node:url";
import { parse, type Node } from "acorn";
import { $, fs, glob, path } from "zx";

const root = path.resolve(import.meta.dirname, "..");

// Syntax acorn accepts at ES2020 but Chrome 68 doesn't, by node.
function tooNew(node: Node & Record<string, unknown>): string | null {
  switch (node.type) {
    case "ChainExpression":
      return "optional chaining (?.)";
    case "LogicalExpression":
      return node.operator === "??" ? "nullish coalescing (??)" : null;
    case "Literal":
      return "bigint" in node && node.bigint != null ? "a BigInt literal" : null;
    case "ExportAllDeclaration":
      return node.exported ? "export * as" : null;
    default:
      return null;
  }
}

function* nodes(value: unknown): Generator<Node & Record<string, unknown>> {
  if (Array.isArray(value)) {
    for (const v of value) yield* nodes(v);
  } else if (value && typeof value === "object") {
    const obj = value as Record<string, unknown>;
    if (typeof obj.type === "string") yield obj as Node & Record<string, unknown>;
    for (const [key, v] of Object.entries(obj)) if (key !== "loc") yield* nodes(v);
  }
}

// The names of a wasm module's custom sections.
export function customSections(bytes: Uint8Array): string[] {
  let at = 8; // magic and version
  const leb = () => {
    let n = 0;
    for (let shift = 0; ; shift += 7) {
      const b = bytes[at++];
      if (b === undefined) throw new Error("truncated wasm");
      n += (b & 0x7f) * 2 ** shift;
      if (!(b & 0x80)) return n;
    }
  };
  const names: string[] = [];
  while (at < bytes.length) {
    const id = bytes[at++];
    const size = leb();
    const end = at + size;
    if (id === 0) {
      const len = leb();
      names.push(new TextDecoder().decode(bytes.subarray(at, at + len)));
    }
    at = end;
  }
  return names;
}

// Every problem in one JS module's source, as "line:column what".
export function checkJs(source: string): string[] {
  let ast: Node;
  try {
    ast = parse(source, { ecmaVersion: 2020, sourceType: "module", locations: true });
  } catch (err) {
    return [String((err as Error).message)];
  }
  const problems: string[] = [];
  for (const node of nodes(ast)) {
    const what = tooNew(node);
    if (what) problems.push(`${node.loc?.start.line}:${node.loc?.start.column} ${what}`);
  }
  return problems;
}

export async function checkWebos(site: string): Promise<void> {
  const failures: string[] = [];
  const files = (await glob(["**/*.js", "**/*.wasm"], { cwd: site, onlyFiles: true })).sort();
  if (!files.some((f) => f.endsWith(".wasm")) || !files.some((f) => f.endsWith(".js"))) {
    throw new Error(`${site}: no .wasm or .js to check; build it with pnpm build:web -- --legacy`);
  }
  for (const file of files) {
    const full = path.join(site, file);
    if (file.endsWith(".wasm")) {
      if (customSections(await fs.readFile(full)).includes("target_features")) {
        failures.push(`${file}: has a target_features section, which would hide post-MVP features from the check`);
      }
      const r = await $`${path.join(root, "node_modules", ".bin", "wasm-opt")} --mvp-features ${full}`.quiet().nothrow();
      if (r.exitCode !== 0) failures.push(`${file}: not MVP wasm:\n${r.stderr.trim().split("\n").slice(0, 10).join("\n")}`);
    } else {
      for (const p of checkJs(await fs.readFile(full, "utf8"))) failures.push(`${file}:${p}`);
    }
  }
  const html = await fs.readFile(path.join(site, "index.html"), "utf8");
  if (!html.includes('data-build="webos"')) failures.push('index.html: not marked data-build="webos"');
  if (failures.length) {
    throw new Error(`The webOS build won't load on Chrome 68:\n${failures.join("\n")}`);
  }
  console.log(`webOS build checked: ${files.length} files are MVP wasm and Chrome 68 JS.`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await checkWebos(path.resolve(process.argv[2] ?? path.join(root, "target", "site-webos")));
}
