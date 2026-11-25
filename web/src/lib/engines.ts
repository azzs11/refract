// Engine identity: Okabe–Ito colours (colour-blind safe), always with a name and a shape.
import type { Flavor } from "./api";

export const ENGINE: Record<Flavor, { name: string; engine: string; color: string; shape: string }> = {
  js: { name: "JavaScript", engine: "V8 14.6 model", color: "#E69F00", shape: "circle" },
  python: { name: "Python", engine: "CPython 3.15 re", color: "#0072B2", shape: "square" },
  go: { name: "Go", engine: "regexp, Go 1.27", color: "#009E73", shape: "triangle" },
  java: { name: "Java", engine: "java.util.regex, JDK 25", color: "#D55E00", shape: "diamond" },
  rust: { name: "Rust", engine: "regex 1.13", color: "#CC79A7", shape: "hexagon" },
};

/** Visible invisibles: every character a reader could miss gets a name. */
export function visible(s: string): string {
  if (s === "") return "(empty string)";
  const names: Record<string, string> = {
    "\n": "(newline)", "\r": "(return)", "\t": "(tab)", " ": "·", " ": "(nbsp)",
    " ": "(line sep)", " ": "(para sep)", "\u0000": "(NUL)", "\u000b": "(vtab)", "\u000c": "(formfeed)",
    "\u0085": "(NEL)", "﻿": "(BOM)", "​": "(zwsp)",
  };
  return Array.from(s)
    .map((c) => names[c] ?? (c.charCodeAt(0) < 32 ? `(U+${c.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")})` : c))
    .join("");
}
