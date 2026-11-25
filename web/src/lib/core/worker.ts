// Hosts refract-core (WASM) off the main thread: the UI never blocks on an analysis.
import init, * as core from "../../wasm/refract_wasm.js";

type Call = { id: number; fn: "refract" | "dfa" | "redos" | "parse" | "translate"; args: string[] };

const ready = init();

self.onmessage = async (e: MessageEvent<Call>) => {
  await ready;
  const { id, fn, args } = e.data;
  let out: string;
  try {
    const t0 = performance.now();
    out = (core[fn] as (...a: string[]) => string)(...args);
    const ms = performance.now() - t0;
    self.postMessage({ id, ok: true, value: JSON.parse(out), ms });
  } catch (err) {
    self.postMessage({ id, ok: false, error: String(err) });
  }
};
