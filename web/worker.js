// Runs the WebAssembly compression engine off the page's main thread, one
// job at a time. Messages:
//   in : { id, level, name, bytes: ArrayBuffer }
//   out: { type: "ready", build: { version, commit, dirty, jpegEncoder }, differs: { level: bool } }
//        { type: "result", id, level, output: ArrayBuffer, inputSize, outputSize, fidelity }
//        { type: "error", id, level, message, crashed }
// `crashed` means the module trapped (a Rust panic aborts in WebAssembly):
// its memory can't be trusted any more, the page replaces this worker.

import init, { buildInfo, compress, levelDiffersFromDesktop } from "./pkg/pdfshrink_wasm.js";

const LEVELS = ["lossless", "low", "medium", "high", "extreme", "extreme-max"];

const ready = init().then(() => {
  const differs = Object.fromEntries(LEVELS.map((l) => [l, levelDiffersFromDesktop(l)]));
  const info = buildInfo();
  const build = {
    version: info.version,
    commit: info.commit,
    dirty: info.dirty,
    jpegEncoder: info.jpegEncoder,
  };
  info.free();
  postMessage({ type: "ready", build, differs });
});
ready.catch((e) => postMessage({ type: "failed", message: String(e) }));

self.onmessage = async ({ data: { id, level, name, bytes } }) => {
  await ready;
  let result;
  try {
    result = compress(new Uint8Array(bytes), level, name);
  } catch (e) {
    postMessage({
      type: "error",
      id,
      level,
      message: e instanceof Error ? e.message : String(e),
      crashed: e instanceof WebAssembly.RuntimeError,
    });
    return;
  }
  const output = result.output;
  const fidelity =
    result.fidelityMean === undefined
      ? null
      : { mean: result.fidelityMean, min: result.fidelityMin, images: result.fidelityImages };
  const msg = {
    type: "result",
    id,
    level,
    output: output.buffer,
    inputSize: result.inputSize,
    outputSize: result.outputSize,
    fidelity,
  };
  result.free();
  postMessage(msg, [output.buffer]);
};
