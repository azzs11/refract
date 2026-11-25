// Live JavaScript check: replay a witness on this browser's own RegExp engine, exactly as
// oracle/node/runner.mjs does (full match: sticky start + look-ahead end).
import type { Op } from "./api";

export function jsLiteral(p: string): [string, string] {
  const m = /^\/(.*)\/([a-z]*)$/s.exec(p);
  return m ? [m[1]!, m[2]!] : [p, ""];
}

export function jsMatches(pattern: string, op: Op, input: string): boolean | string {
  const [src, flags] = jsLiteral(pattern);
  try {
    const rx = op === "full"
      ? new RegExp(`(?:${src})(?![\\s\\S])`, flags.replace("y", "") + "y")
      : new RegExp(src, flags);
    rx.lastIndex = 0;
    return rx.test(input);
  } catch (e) {
    return String((e as Error).message);
  }
}

export function browserEngine(): string {
  const ua = navigator.userAgent;
  if (/Firefox\//.test(ua)) return "SpiderMonkey (Firefox)";
  if (/Chrome\/|Chromium\/|Edg\//.test(ua)) return "V8 (this browser)";
  if (/Safari\//.test(ua)) return "JavaScriptCore (Safari)";
  return "this browser's engine";
}

/** Times `RegExp.test` on growing attacks in a throwaway worker (killed on timeout). */
export function timeAttack(pattern: string, op: Op, build: (n: number) => string, ns: number[],
  onPoint: (n: number, ms: number | null) => void, timeoutMs = 2000): Promise<void> {
  const [src, flags] = jsLiteral(pattern);
  const code = `onmessage = (e) => { const {src, flags, full, s} = e.data;
    const rx = full ? new RegExp("^(?:" + src + ")$", flags) : new RegExp(src, flags);
    const t = performance.now(); rx.test(s); postMessage(performance.now() - t); };`;
  const url = URL.createObjectURL(new Blob([code], { type: "text/javascript" }));
  return (async () => {
    for (const n of ns) {
      const w = new Worker(url);
      const ms = await new Promise<number | null>((res) => {
        const timer = setTimeout(() => { w.terminate(); res(null); }, timeoutMs);
        w.onmessage = (e) => { clearTimeout(timer); w.terminate(); res(e.data as number); };
        w.postMessage({ src, flags: flags.replace("y", ""), full: op === "full", s: build(n) });
      });
      onPoint(n, ms);
      if (ms === null || ms > 250) break;
    }
    URL.revokeObjectURL(url);
  })();
}
