<script lang="ts">
  import type { DfaView } from "./api";
  import { visible } from "./engines";

  let { view, witness }: { view: DfaView; witness: string } = $props();
  let showDead = $state(false);

  // Layered layout: breadth-first distance from the start gives the column.
  const layout = $derived.by(() => {
    const n = view.accepting.length;
    const hidden = (q: number) => view.dead[q] && !showDead;
    const depth = new Array(n).fill(-1);
    depth[view.start] = 0;
    const queue = [view.start];
    while (queue.length) {
      const q = queue.shift()!;
      for (const e of view.edges) if (e.from === q && depth[e.to] < 0) { depth[e.to] = depth[q] + 1; queue.push(e.to); }
    }
    const cols = new Map<number, number[]>();
    for (let q = 0; q < n; q++) if (depth[q] >= 0 && !hidden(q)) cols.set(depth[q], [...(cols.get(depth[q]) ?? []), q]);
    const pos = new Map<number, { x: number; y: number }>();
    for (const [d, qs] of cols) qs.forEach((q, i) => pos.set(q, { x: 70 + d * 170, y: 60 + i * 110 }));
    const width = 140 + Math.max(0, ...cols.keys()) * 170;
    const height = 120 + (Math.max(1, ...[...cols.values()].map((c) => c.length)) - 1) * 110;
    const lit = new Set<string>();
    for (let i = 0; i + 1 < view.path.length; i++) lit.add(`${view.path[i]}-${view.path[i + 1]}`);
    const edges = view.edges.filter((e) => pos.has(e.from) && pos.has(e.to));
    return { pos, width, height, lit, edges };
  });

  const short = (s: string) => (s.length > 22 ? s.slice(0, 21) + "…" : s);
  const end = $derived(view.path[view.path.length - 1]);
</script>

<div class="lens">
  <p>
    Minimal DFA ({view.accepting.length} states). Witness <code class="witness">{visible(witness)}</code>
    {#if witness !== "" || view.path.length}
      ends in a {end !== undefined && view.accepting[end] ? "accepting" : "rejecting"} state.
    {/if}
    <label><input type="checkbox" bind:checked={showDead} /> show the dead state</label>
  </p>
  <svg viewBox={`0 0 ${layout.width} ${layout.height}`} role="img" aria-label="Minimal DFA with the witness path highlighted">
    <defs>
      <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0,0 L10,5 L0,10 z" fill="currentColor" />
      </marker>
    </defs>
    {#each layout.edges as e (`${e.from}-${e.to}`)}
      {@const a = layout.pos.get(e.from)!}
      {@const b = layout.pos.get(e.to)!}
      {@const on = layout.lit.has(`${e.from}-${e.to}`)}
      {#if e.from === e.to}
        <path d={`M${a.x - 10},${a.y - 22} C${a.x - 30},${a.y - 70} ${a.x + 30},${a.y - 70} ${a.x + 10},${a.y - 22}`}
              class="edge" class:lit={on} marker-end="url(#arrow)" />
        <text x={a.x} y={a.y - 62} class="elabel">{short(e.label)}</text>
      {:else}
        {@const mx = (a.x + b.x) / 2}
        {@const my = (a.y + b.y) / 2 - (b.x <= a.x ? 40 : 14)}
        <path d={`M${a.x + 22},${a.y} Q${mx},${my} ${b.x - 24},${b.y}`} class="edge" class:lit={on} marker-end="url(#arrow)" />
        <text x={mx} y={my - 4} class="elabel">{short(e.label)}</text>
      {/if}
    {/each}
    {#each [...layout.pos] as [q, p] (q)}
      <g class="state" class:accept={view.accepting[q]} class:onpath={view.path.includes(q)}>
        {#if q === view.start}<path d={`M${p.x - 55},${p.y} L${p.x - 24},${p.y}`} class="edge" marker-end="url(#arrow)" />{/if}
        <circle cx={p.x} cy={p.y} r="20" />
        {#if view.accepting[q]}<circle cx={p.x} cy={p.y} r="15" />{/if}
        <text x={p.x} y={p.y + 4}>{q}</text>
      </g>
    {/each}
  </svg>
</div>
