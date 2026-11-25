// Promise wrapper around the analysis worker.
export type Flavor = "js" | "python" | "go" | "java" | "rust";
export type Op = "full" | "search";

export interface EngineRow {
  flavor: Flavor;
  verdict: "same" | "same-bounded" | "differs" | "rejects" | "not-analysed" | "source";
  witness?: { text: string; escaped: string };
  source_matches?: boolean;
  note?: string;
  max_len?: number;
  error?: { message: string; rendered?: string };
}

export interface Refraction {
  ok: boolean;
  error?: { message: string; rendered?: string; span?: [number, number] };
  engines?: EngineRow[];
  operation?: string;
}

export interface DfaView {
  ok: boolean;
  error?: { message: string };
  start: number;
  accepting: boolean[];
  dead: boolean[];
  path: number[];
  edges: { from: number; to: number; label: string }[];
}

export interface Redos {
  ok: boolean;
  error?: { message: string };
  growth: string;
  reason: string;
  span: [number, number] | null;
  attack: { prefix: string; pumps: string[]; suffix: string } | null;
}

export interface TranslationRow {
  flavor: Flavor;
  status: "verified" | "not-verified" | "untranslatable" | "printer-bug" | "not-analysed";
  scope?: string;
  text?: string;
  construct?: string;
  span?: [number, number];
  note?: string;
  checks?: {
    op: string;
    verdict: string;
    witness: { text: string; escaped: string } | null;
    cases: { text: string; matches: boolean }[];
  }[];
}

export interface Translation {
  ok: boolean;
  error?: { message: string };
  targets?: TranslationRow[];
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
let next = 1;
const pending = new Map<number, { resolve: (v: { value: unknown; ms: number }) => void; reject: (e: Error) => void }>();
worker.onmessage = (e) => {
  const p = pending.get(e.data.id);
  if (!p) return;
  pending.delete(e.data.id);
  if (e.data.ok) p.resolve({ value: e.data.value, ms: e.data.ms });
  else p.reject(new Error(e.data.error));
};

function call<T>(fn: string, ...args: string[]): Promise<{ value: T; ms: number }> {
  const id = next++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve: resolve as (v: { value: unknown; ms: number }) => void, reject });
    worker.postMessage({ id, fn, args });
  });
}

export const ALL: Flavor[] = ["js", "python", "go", "java", "rust"];
export const refract = (from: Flavor, p: string, op: Op) =>
  call<Refraction>("refract", from, p, ALL.join(","), op);
export const dfa = (f: Flavor, p: string, op: Op, input: string) => call<DfaView>("dfa", f, p, op, input);
export const redos = (f: Flavor, p: string, op: Op) => call<Redos>("redos", f, p, op);
export const translate = (f: Flavor, p: string) => call<Translation>("translate", f, p, ALL.join(","));
