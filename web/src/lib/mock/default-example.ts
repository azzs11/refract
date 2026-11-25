// The Bench's default example (spec: Bench → Default example), hard-coded for UI0. Every value
// was checked on the real engines (CPython 3.15.2, V8 14.6, Go 1.27.4, JDK 25.0.5.1):
//   re.search(r'^\d+$', '123\n')  → match (0, 3)      /^\d+$/.test('123\n')   → false
//   Go MatchString                → false             Java find()             → true (0, 3)
import type { BenchView } from "../types";

export const DEFAULT_EXAMPLE: BenchView = {
  flavour: "python",
  regex: "^\\d+$",
  operation: "search",
  flags: "",
  tests: ["123\n"],
  engines: ["js", "python", "go", "java"],
  baseline: "js",
  lanes: [
    {
      engine: "js",
      state: "same",
      evidence: { level: "confirmed", label: "Confirmed live" },
      summary: "Baseline. No match on 1 of 1 test string",
      results: [{ index: 0, matched: false }],
    },
    {
      engine: "python",
      state: "differs",
      evidence: { level: "confirmed", label: "Confirmed live" },
      summary: "Matches 1 of 1 test string",
      results: [{ index: 0, matched: true, span: [0, 3] }],
    },
    {
      engine: "go",
      state: "same",
      evidence: { level: "recorded", label: "Recorded on Go 1.27.4" },
      summary: "No match on 1 of 1 test string",
      results: [{ index: 0, matched: false }],
    },
    {
      engine: "java",
      state: "differs",
      evidence: { level: "recorded", label: "Recorded on JDK 25.0.5.1" },
      summary: "Matches 1 of 1 test string",
      results: [{ index: 0, matched: true, span: [0, 3] }],
    },
  ],
  summary: "Python and Java match; JavaScript and Go do not.",
  witness: {
    title: "$ before a final newline",
    text: "123\n",
    why:
      "Without the multiline flag, $ in Python and Java also matches just before a newline at the very end of the input, so ^\\d+$ finds 123 in this string. In JavaScript and Go, $ matches only at the very end.",
    specHref: "https://github.com/azzs11/refract/blob/main/spec/flavours/python.md",
    panes: [
      { engine: "python", matched: true, span: [0, 3], raw: "re.search(r'^\\d+$', '123\\n')\n→ <re.Match object; span=(0, 3), match='123'>" },
      { engine: "js", matched: false, raw: "/^\\d+$/.test('123\\n')\n→ false" },
    ],
  },
};
