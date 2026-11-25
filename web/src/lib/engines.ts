// Engine identity (spec: Visual system → Engine identity). The one file that holds engine
// colours, patterns and shapes. Colour is never the only signal: every engine also has a hatch
// pattern, a shape and its name. Fill is for pattern backgrounds; ink is for lines and labels
// (≥ 4.5:1 on its own fill, checked).

export type EngineId = "js" | "python" | "go" | "java" | "pcre2" | "rust" | "ruby" | "dotnet";
export type PatternKind = "diagonal" | "dots" | "horizontal" | "crosshatch" | "vertical" | "waves" | "zigzag" | "checker";
export type ShapeKind = "circle" | "square" | "triangle" | "diamond" | "pentagon" | "hexagon" | "cross" | "star";

export interface Engine {
  id: EngineId;
  name: string;
  /** Pinned engine and version the models are built from. */
  engine: string;
  fill: string;
  ink: string;
  pattern: PatternKind;
  shape: ShapeKind;
  /** Release in which the engine joins the Bench. */
  ships: "v0.5" | "v0.8" | "v1.0";
  /** Keyboard shortcut that toggles the lane (1–8). */
  key: number;
}

export const ENGINES: readonly Engine[] = [
  { id: "js", name: "JavaScript", engine: "V8 14.6", fill: "#F6E7A8", ink: "#7A5A00", pattern: "diagonal", shape: "circle", ships: "v0.5", key: 1 },
  { id: "python", name: "Python re", engine: "CPython 3.15.2", fill: "#CBE2F6", ink: "#1F5A8F", pattern: "dots", shape: "square", ships: "v0.5", key: 2 },
  { id: "go", name: "Go (RE2)", engine: "Go 1.27.4", fill: "#C2EAE4", ink: "#0F6B63", pattern: "horizontal", shape: "triangle", ships: "v0.5", key: 3 },
  { id: "java", name: "Java", engine: "JDK 25.0.5.1", fill: "#F3D5B5", ink: "#8A4B08", pattern: "crosshatch", shape: "diamond", ships: "v0.5", key: 4 },
  { id: "pcre2", name: "PCRE2", engine: "PCRE2", fill: "#DAD3F1", ink: "#50409A", pattern: "vertical", shape: "pentagon", ships: "v0.8", key: 5 },
  { id: "rust", name: "Rust", engine: "regex 1.13", fill: "#D8E8BE", ink: "#4C6B1A", pattern: "waves", shape: "hexagon", ships: "v0.8", key: 6 },
  { id: "ruby", name: "Ruby", engine: "Onigmo", fill: "#F2CFDD", ink: "#9B2F5A", pattern: "zigzag", shape: "cross", ships: "v1.0", key: 7 },
  { id: "dotnet", name: ".NET", engine: ".NET", fill: "#D6DCE3", ink: "#3E4C5E", pattern: "checker", shape: "star", ships: "v1.0", key: 8 },
];

/** The engines on the Bench in this release (v0.5). Layouts keep room for all eight. */
export const SHIPPED: readonly Engine[] = ENGINES.filter((e) => e.ships === "v0.5");

export function engine(id: EngineId): Engine {
  const e = ENGINES.find((x) => x.id === id);
  if (!e) throw new Error(`unknown engine ${id}`);
  return e;
}

/** Inline style that hands an engine's colours to a component as CSS variables. */
export function engineVars(e: Engine): string {
  return `--eng-fill:${e.fill};--eng-ink:${e.ink}`;
}
