// Run recheck (makenowjust-labs/recheck) on one JavaScript pattern; print one JSON line.
//   node bench/redos/recheck.mjs 'pattern'
import { check } from "recheck";

const pattern = process.argv[2];
const r = await check(pattern, "", { timeout: 20000 });
const a = r.attack;
console.log(JSON.stringify({
  status: r.status,
  complexity: r.complexity ? { type: r.complexity.type, degree: r.complexity.degree ?? null } : null,
  checker: r.checker ?? null,
  attack: a ? {
    prefix: "",
    leads: a.pumps.map((p) => p.prefix),
    pumps: a.pumps.map((p) => p.pump),
    suffix: a.suffix,
  } : null,
}));
