// Shapes shared by the store, selectors and components (UI1 derives these from real results;
// in UI0 they come from src/lib/mock/default-example.ts).
import type { EngineId } from "./engines";

export type Operation = "full" | "search" | "start";
export type Flavour = "js" | "python" | "go" | "java";

/** Lane states (spec: Bench → Lane states). Every lane is in exactly one. */
export type LaneState = "idle" | "pending" | "same" | "differs" | "error" | "slow" | "not-confirmed";

/** Evidence levels (spec: Results → Evidence levels). One per verdict. */
export type EvidenceLevel = "proved" | "confirmed" | "recorded" | "bounded" | "tested";

export interface Evidence {
  level: EvidenceLevel;
  /** E.g. "Recorded on Go 1.27.4", "Checked up to length 8". */
  label: string;
}

export interface TestResult {
  /** The test string's index in the input list. */
  index: number;
  matched: boolean;
  /** Match span in code points, when matched. */
  span?: [number, number];
}

export interface Lane {
  engine: EngineId;
  state: LaneState;
  evidence?: Evidence;
  /** One-line summary, e.g. "matches 1 of 1 test string". */
  summary: string;
  results: TestResult[];
  /** Error lanes: what, where, what to try. */
  reason?: string;
}

export interface EnginePane {
  engine: EngineId;
  matched: boolean;
  span?: [number, number];
  /** The call and the engine's own output. */
  raw: string;
}

export interface Witness {
  /** Plain sentence naming the construct. */
  title: string;
  text: string;
  why: string;
  specHref: string;
  /** The engines on each side of the bend. */
  panes: EnginePane[];
}

export interface BenchView {
  flavour: Flavour;
  regex: string;
  operation: Operation;
  flags: string;
  tests: string[];
  engines: EngineId[];
  baseline: EngineId;
  lanes: Lane[];
  summary: string;
  witness: Witness | null;
}
