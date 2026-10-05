// Loads the built WebAssembly module (web/dist) in Node and compresses a PDF
// at every level, failing on any error or on a result that isn't a PDF.
// Used by .github/workflows/pages.yml before deploying.
//   node web/smoke-test.mjs web/dist file.pdf

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const [dist, pdf] = process.argv.slice(2);
if (!dist || !pdf) {
  console.error("usage: node web/smoke-test.mjs <dist dir> <file.pdf>");
  process.exit(2);
}
const pkg = join(resolve(dist), "pkg");
const { initSync, compress, levelDiffersFromDesktop, buildInfo } = await import(
  pathToFileURL(join(pkg, "pdfshrink_wasm.js")).href
);
initSync({ module: readFileSync(join(pkg, "pdfshrink_wasm_bg.wasm")) });

const info = buildInfo();
console.log(`pdfshrink web ${info.version} (${info.commit}), JPEG: ${info.jpegEncoder}`);
const input = readFileSync(pdf);
for (const level of ["lossless", "low", "medium", "high", "extreme", "extreme-max"]) {
  const t0 = performance.now();
  const r = compress(input, level, pdf);
  const head = new TextDecoder().decode(r.output.slice(0, 5));
  if (head !== "%PDF-") throw new Error(`${level}: output is not a PDF (${head})`);
  console.log(
    `${level.padEnd(12)} ${r.inputSize} -> ${r.outputSize} B` +
      `  ${Math.round(performance.now() - t0)} ms  differs from desktop: ${levelDiffersFromDesktop(level)}`
  );
  r.free();
}
